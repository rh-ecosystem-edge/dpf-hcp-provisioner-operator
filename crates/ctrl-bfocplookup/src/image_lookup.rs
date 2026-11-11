//! Port of internal/controller/bfocplookup/image_lookup.go

use std::sync::{Arc, RwLock};

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use async_trait::async_trait;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use ctrlrt::meta;
use ctrlrt::recorder::{EVENT_TYPE_NORMAL, EVENT_TYPE_WARNING};
use ctrlrt::{Client, Ctx, ReconcileResult, Recorder, RecorderExt, Result, kv, wrapf};
use registry::authn::Keychain;
use registry::name::{self, Reference};
use registry::remote;

use crate::errors::*;

/// OCPReleaseLabel is the OCI image label that contains the OCP version string on an OCP release image
pub const OCP_RELEASE_LABEL: &str = "io.openshift.release";

// Reason codes
pub const REASON_IMAGE_FOUND: &str = "ImageFound";
pub const REASON_REGISTRY_ACCESS_ERROR: &str = "RegistryAccessError";
pub const REASON_REGISTRY_AUTH_ERROR: &str = "RegistryAuthError";
pub const REASON_INVALID_IMAGE_FORMAT: &str = "InvalidImageFormat";
pub const REASON_VERSION_NOT_FOUND: &str = "VersionNotFound";
pub const REASON_INVALID_BLUE_FIELD_OCP_LAYER_URL: &str = "InvalidBlueFieldOCPLayerImageURL";

/// ImageChecker abstracts checking whether a specific image tag exists in a registry
#[async_trait]
pub trait ImageChecker: Send + Sync {
    async fn check_tag(&self, ctx: &Ctx, r: &Reference, keychain: Arc<dyn Keychain>) -> Result<()>;
}

/// RemoteImageChecker is the production implementation that checks a real container registry
#[derive(Clone, Copy, Debug, Default)]
pub struct RemoteImageChecker;

#[async_trait]
impl ImageChecker for RemoteImageChecker {
    /// CheckTag checks if a specific tag exists by making a HEAD request to the registry.
    /// Returns nil if the tag exists, or an error if not found or inaccessible.
    async fn check_tag(
        &self,
        _ctx: &Ctx,
        r: &Reference,
        keychain: Arc<dyn Keychain>,
    ) -> Result<()> {
        remote::head(r, &[remote::with_auth_from_keychain(keychain)]).await?;
        Ok(())
    }
}

/// ImageLookup handles BlueField OCP layer image lookup by querying a container registry
pub struct ImageLookup {
    pub client: Client,
    pub recorder: Recorder,
    pub image_checker: Arc<dyn ImageChecker>,
    /// Go: `Repository string` (set by the main controller before every lookup).
    repository: RwLock<String>,
}

/// `metav1.Condition{..., LastTransitionTime: metav1.Now(), ObservedGeneration: cr.Generation}`
fn condition(status: &str, reason: &str, message: &str, generation: i64) -> Condition {
    let mut c = meta::condition_gen(
        provisioningv1alpha1::BLUE_FIELD_OCP_LAYER_IMAGE_FOUND,
        status,
        reason,
        message,
        generation,
    );
    c.last_transition_time = meta::now();
    c
}

fn generation(cr: &provisioningv1alpha1::DPFHCPProvisioner) -> i64 {
    cr.metadata.generation.unwrap_or(0)
}

impl ImageLookup {
    /// NewImageLookup creates a new ImageLookup
    pub fn new(c: Client, recorder: Recorder) -> Self {
        Self {
            client: c,
            recorder,
            image_checker: Arc::new(RemoteImageChecker),
            repository: RwLock::new(String::new()),
        }
    }

    /// `r.ImageLookup.Repository = repo`
    pub fn set_repository(&self, repository: impl Into<String>) {
        *self.repository.write().unwrap_or_else(|e| e.into_inner()) = repository.into();
    }

    /// `r.ImageLookup.Repository`
    pub fn repository(&self) -> String {
        self.repository
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// LookupBlueFieldOCPLayerImage is the main function for BlueField OCP layer image lookup.
    /// It extracts the OCP version from the ocpReleaseImage, queries the container registry
    /// to find a matching tag, and updates the CR status with the found image reference.
    pub async fn lookup_blue_field_ocp_layer_image(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<ReconcileResult> {
        let logger = ctx
            .log
            .with_values(kv!["feature" => "bluefield-ocp-layer-lookup"]);

        // Read ocpReleaseImage from spec
        let ocp_release_image = cr.spec.ocp_release_image.clone();
        if ocp_release_image.is_empty() {
            let err = anyhow::anyhow!("ocpReleaseImage is required but empty");
            logger.error(&err, "Missing required field", kv![]);
            return self
                .handle_validation_error(
                    ctx,
                    cr,
                    InvalidImageFormatError {
                        message: "ocpReleaseImage is required but empty".into(),
                        url: String::new(),
                    }
                    .into(),
                )
                .await;
        }

        // Use the repository from operator config
        let repository = self.repository();

        // Get authentication keychain from the CR's pullSecretRef
        let keychain = match self.get_keychain(cr).await {
            Ok(k) => k,
            Err(err) => {
                logger.error(&err, "Failed to get registry credentials", kv![]);
                return self
                    .handle_permanent_error(
                        ctx,
                        cr,
                        RegistryAuthError { repository, err }.into(),
                        "",
                    )
                    .await;
            }
        };

        // Extract OCP version (tag parsing first, then registry label for digest images)
        logger.v(1).info(
            "Extracting OCP version",
            kv!["ocpReleaseImage" => ocp_release_image],
        );
        let version = match extract_ocp_version(ctx, &ocp_release_image, keychain.clone()).await {
            Ok(v) => v,
            Err(err) => {
                logger.error(
                    &err,
                    "Failed to extract OCP version",
                    kv!["ocpReleaseImage" => ocp_release_image],
                );
                if is_auth_error(&err) {
                    return self
                        .handle_permanent_error(
                            ctx,
                            cr,
                            RegistryAuthError {
                                repository: ocp_release_image,
                                err,
                            }
                            .into(),
                            "",
                        )
                        .await;
                }
                // Transient registry errors — return error to trigger controller-runtime backoff requeue
                let msg = err.to_string();
                if msg.contains("failed to fetch") || msg.contains("failed to get") {
                    return self.handle_transient_error(ctx, cr, err).await;
                }
                return self
                    .handle_validation_error(
                        ctx,
                        cr,
                        InvalidImageFormatError {
                            message: msg,
                            url: ocp_release_image,
                        }
                        .into(),
                    )
                    .await;
            }
        };
        logger
            .v(1)
            .info("Extracted OCP version", kv!["version" => version]);

        // Query registry for matching tag
        logger.v(1).info(
            "Querying registry for BlueField OCP layer image",
            kv!["repository" => repository, "version" => version],
        );
        let blue_field_ocp_layer_image = match self
            .validate_tag_exists(ctx, &repository, &version, keychain)
            .await
        {
            Ok(image) => image,
            Err(err) => {
                if err.is::<VersionNotFoundError>() || err.is::<RegistryAuthError>() {
                    return self.handle_permanent_error(ctx, cr, err, &version).await;
                }
                // Registry access errors are transient
                return self.handle_transient_error(ctx, cr, err).await;
            }
        };

        // Validate the found image URL
        logger.v(1).info(
            "Validating BlueField OCP layer image URL",
            kv!["blueFieldOCPLayerImage" => blue_field_ocp_layer_image],
        );
        if let Err(err) =
            validate_blue_field_ocp_layer_image_url(&blue_field_ocp_layer_image, &version)
        {
            logger.error(
                &err,
                "BlueField OCP layer image URL validation failed",
                kv!["blueFieldOCPLayerImage" => blue_field_ocp_layer_image],
            );
            return self.handle_permanent_error(ctx, cr, err, &version).await;
        }

        // Update status on success
        logger.info(
            "BlueField OCP layer image found successfully",
            kv![
                "version" => version,
                "blueFieldOCPLayerImage" => blue_field_ocp_layer_image,
            ],
        );
        self.update_status_on_success(ctx, cr, &blue_field_ocp_layer_image, &version)
            .await
    }

    /// getKeychain returns an authn.Keychain for authenticating to the container registry.
    /// It reuses the CR's pullSecretRef which typically contains credentials for multiple registries.
    async fn get_keychain(
        &self,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<Arc<dyn Keychain>> {
        common::keychain_from_pull_secret(
            &self.client,
            &cr.spec.pull_secret_ref.name,
            cr.metadata.namespace.as_deref().unwrap_or(""),
        )
        .await
    }

    /// validateTagExists checks if a specific tag exists in the container registry via a HEAD request.
    /// Returns the full image reference (repository:tag) if found.
    pub(crate) async fn validate_tag_exists(
        &self,
        ctx: &Ctx,
        repository: &str,
        version: &str,
        keychain: Arc<dyn Keychain>,
    ) -> Result<String> {
        let (image_ref, r) = match build_image_reference(repository, version) {
            Ok(v) => v,
            Err(err) => {
                return Err(RegistryAccessError {
                    repository: repository.to_string(),
                    err,
                }
                .into());
            }
        };

        if let Err(err) = self.image_checker.check_tag(ctx, &r, keychain).await {
            if is_auth_error(&err) {
                return Err(RegistryAuthError {
                    repository: repository.to_string(),
                    err,
                }
                .into());
            }
            if is_not_found_error(&err) {
                return Err(VersionNotFoundError {
                    version: version.to_string(),
                    repository: repository.to_string(),
                }
                .into());
            }
            return Err(RegistryAccessError {
                repository: repository.to_string(),
                err,
            }
            .into());
        }

        Ok(image_ref)
    }

    /// updateStatusOnSuccess updates the CR status when BlueField OCP layer image lookup succeeds
    async fn update_status_on_success(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        blue_field_ocp_layer_image: &str,
        version: &str,
    ) -> Result<ReconcileResult> {
        let logger = &ctx.log;

        // Update status field
        cr.status.blue_field_ocp_layer_image = blue_field_ocp_layer_image.to_string();

        // Update condition
        let cond = condition(
            meta::CONDITION_TRUE,
            REASON_IMAGE_FOUND,
            &format!("BlueField OCP layer image found: {blue_field_ocp_layer_image}"),
            generation(cr),
        );

        // Emit event only if condition status/reason changed
        if meta::set_status_condition(&mut cr.status.conditions, cond) {
            self.recorder.event(
                &*cr,
                EVENT_TYPE_NORMAL,
                REASON_IMAGE_FOUND,
                &format!(
                    "BlueField OCP layer image found for OCP version {version}: {blue_field_ocp_layer_image}"
                ),
            );
        }

        // Persist status
        if let Err(err) = self.client.update_status(cr).await {
            if ctrlrt::error::is_conflict(&err) {
                logger
                    .v(1)
                    .info("Status update conflict, will retry", kv![]);
                return Err(err);
            }
            logger.error(&err, "Failed to update status", kv![]);
            return Err(err);
        }

        logger.info("Status updated successfully", kv![]);
        Ok(ReconcileResult::default())
    }

    /// handleValidationError handles permanent validation errors
    async fn handle_validation_error(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        err: anyhow::Error,
    ) -> Result<ReconcileResult> {
        let logger = &ctx.log;
        logger.v(1).info(
            "Validation error - check CR spec",
            kv!["error" => err.to_string()],
        );

        // Clear status field
        cr.status.blue_field_ocp_layer_image = String::new();

        // Both the InvalidImageFormatError case and the default case use the same reason and
        // err.Error() as message.
        let reason = REASON_INVALID_IMAGE_FORMAT;
        let message = err.to_string();

        let cond = condition(meta::CONDITION_FALSE, reason, &message, generation(cr));

        if meta::set_status_condition(&mut cr.status.conditions, cond) {
            self.recorder
                .event(&*cr, EVENT_TYPE_WARNING, reason, &message);
        }

        if let Err(update_err) = self.client.update_status(cr).await {
            logger.error(
                &update_err,
                "Failed to update status after validation error",
                kv![],
            );
        }

        // Do NOT requeue - permanent error
        Ok(ReconcileResult::default())
    }

    /// handlePermanentError handles permanent errors (version not found, auth denied, invalid URL)
    async fn handle_permanent_error(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        err: anyhow::Error,
        version: &str,
    ) -> Result<ReconcileResult> {
        let logger = &ctx.log;
        logger.v(1).info(
            "Permanent error - user action required",
            kv!["version" => version, "error" => err.to_string()],
        );

        // Clear status field
        cr.status.blue_field_ocp_layer_image = String::new();

        let reason = if err.is::<VersionNotFoundError>() {
            REASON_VERSION_NOT_FOUND
        } else if err.is::<RegistryAuthError>() {
            REASON_REGISTRY_AUTH_ERROR
        } else if err.is::<InvalidBlueFieldOCPLayerImageURLError>() {
            REASON_INVALID_BLUE_FIELD_OCP_LAYER_URL
        } else {
            REASON_VERSION_NOT_FOUND
        };
        let message = err.to_string();

        let cond = condition(meta::CONDITION_FALSE, reason, &message, generation(cr));

        if meta::set_status_condition(&mut cr.status.conditions, cond) {
            self.recorder
                .event(&*cr, EVENT_TYPE_WARNING, reason, &message);
        }

        if let Err(update_err) = self.client.update_status(cr).await {
            logger.error(
                &update_err,
                "Failed to update status after permanent error",
                kv![],
            );
        }

        // Do NOT requeue - permanent error
        Ok(ReconcileResult::default())
    }

    /// handleTransientError handles transient errors
    async fn handle_transient_error(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        err: anyhow::Error,
    ) -> Result<ReconcileResult> {
        let logger = &ctx.log;

        let reason = REASON_REGISTRY_ACCESS_ERROR;
        let message = if err.is::<RegistryAccessError>() {
            err.to_string()
        } else {
            format!("Transient error accessing registry: {err}")
        };

        let cond = condition(meta::CONDITION_FALSE, reason, &message, generation(cr));

        if meta::set_status_condition(&mut cr.status.conditions, cond) {
            self.recorder
                .event(&*cr, EVENT_TYPE_WARNING, reason, &message);
        }

        if let Err(update_err) = self.client.update_status(cr).await {
            logger.error(
                &update_err,
                "Failed to update status after transient error",
                kv![],
            );
        }

        logger.v(1).info(
            "Transient error, returning error for native retry with exponential backoff",
            kv!["error" => err.to_string()],
        );

        // Return error to trigger controller-runtime's native exponential backoff
        Err(err)
    }
}

/// Go `strings.LastIndex` (returns -1 when absent).
fn last_index(s: &str, pat: char) -> isize {
    s.rfind(pat).map(|i| i as isize).unwrap_or(-1)
}

/// extractVersionFromTag is an internal helper that extracts OCP version from a tagged image URL.
/// Returns empty string if not parseable (digest images, missing tag, etc.).
pub(crate) fn extract_version_from_tag(image_url: &str) -> String {
    if image_url.contains('@') {
        return String::new();
    }
    let last_slash = last_index(image_url, '/');
    let last_colon = last_index(image_url, ':');
    if last_colon <= last_slash {
        return String::new();
    }
    let tag = &image_url[last_colon as usize + 1..];
    if tag.is_empty() {
        return String::new();
    }
    const SUFFIXES: [&str; 6] = [
        "-multi", "-amd64", "-arm64", "-ppc64le", "-s390x", "-x86_64",
    ];
    let mut version = tag;
    for suffix in SUFFIXES {
        version = version.strip_suffix(suffix).unwrap_or(version);
    }
    version.to_string()
}

/// ExtractOCPVersion resolves the OCP version from a release image.
/// For tagged images, parses the version from the tag (no registry call).
/// For digest images, reads the [`OCP_RELEASE_LABEL`] label from the image config via registry.
pub async fn extract_ocp_version(
    _ctx: &Ctx,
    release_image: &str,
    keychain: Arc<dyn Keychain>,
) -> Result<String> {
    // Try tag-based extraction first (fast, no registry call)
    let version = extract_version_from_tag(release_image);
    if !version.is_empty() {
        return Ok(version);
    }

    // Digest image: fetch the image config from the registry
    let r = match name::parse_reference(release_image, &[]) {
        Ok(r) => r,
        Err(e) => {
            let e = anyhow::Error::from(e);
            return Err(wrapf!(
                e,
                "failed to parse image reference {}: {}",
                release_image,
                e
            ));
        }
    };

    let desc = match remote::get(&r, &[remote::with_auth_from_keychain(keychain)]).await {
        Ok(d) => d,
        Err(e) => {
            return Err(wrapf!(
                e,
                "failed to fetch image descriptor for {}: {}",
                release_image,
                e
            ));
        }
    };

    let img = match desc.image().await {
        Ok(i) => i,
        Err(e) => {
            return Err(wrapf!(
                e,
                "failed to get image from descriptor for {}: {}",
                release_image,
                e
            ));
        }
    };

    let cfg = match img.config_file().await {
        Ok(c) => c,
        Err(e) => {
            return Err(wrapf!(
                e,
                "failed to get config file for {}: {}",
                release_image,
                e
            ));
        }
    };

    match cfg.config.labels.get(OCP_RELEASE_LABEL) {
        Some(v) if !v.is_empty() => Ok(v.clone()),
        _ => Err(anyhow::anyhow!(
            "image {} does not have label {}",
            release_image,
            OCP_RELEASE_LABEL
        )),
    }
}

/// buildImageReference constructs a full image reference string and parses it into a name.Reference.
fn build_image_reference(repository: &str, version: &str) -> Result<(String, Reference)> {
    let image_ref = format!("{repository}:{version}");
    match name::parse_reference(&image_ref, &[]) {
        Ok(r) => Ok((image_ref, r)),
        Err(e) => {
            let e = anyhow::Error::from(e);
            Err(wrapf!(e, "invalid image reference {}: {}", image_ref, e))
        }
    }
}

/// isAuthError checks if an error is an authentication/authorization error
pub(crate) fn is_auth_error(err: &anyhow::Error) -> bool {
    let s = err.to_string();
    s.contains("UNAUTHORIZED")
        || s.contains("unauthorized")
        || s.contains("DENIED")
        || s.contains("denied")
        || s.contains("403")
}

/// isNotFoundError checks if an error indicates the image tag was not found
pub(crate) fn is_not_found_error(err: &anyhow::Error) -> bool {
    let s = err.to_string();
    s.contains("NOT_FOUND")
        || s.contains("not found")
        || s.contains("MANIFEST_UNKNOWN")
        || s.contains("manifest unknown")
        || s.contains("404")
}

/// validateBlueFieldOCPLayerImageURL validates the BlueField OCP layer image URL format.
pub(crate) fn validate_blue_field_ocp_layer_image_url(url: &str, version: &str) -> Result<()> {
    if url.is_empty() {
        return Err(InvalidBlueFieldOCPLayerImageURLError {
            version: version.to_string(),
            url: url.to_string(),
            message: "BlueField OCP layer image URL is empty".into(),
        }
        .into());
    }

    // Basic validation: should contain a colon (registry/repo:tag format)
    if !url.contains(':') {
        return Err(InvalidBlueFieldOCPLayerImageURLError {
            version: version.to_string(),
            url: url.to_string(),
            message: "BlueField OCP layer image URL is malformed (missing tag separator ':')"
                .into(),
        }
        .into());
    }

    Ok(())
}
