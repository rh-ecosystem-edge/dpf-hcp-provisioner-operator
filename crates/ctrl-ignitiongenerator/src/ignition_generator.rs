//! Port of internal/controller/ignitiongenerator/ignition_generator.go

use std::collections::BTreeMap;
use std::time::Duration;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::dpf::operator::v1alpha1 as operatorv1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::error::{find_in_chain, is_not_found};
use ctrlrt::k8s_openapi::api::core::v1::{ConfigMap, Node, Secret, Service};
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use ctrlrt::meta;
use ctrlrt::recorder::{EVENT_TYPE_NORMAL, EVENT_TYPE_WARNING};
use ctrlrt::{
    Client, Ctx, ListOptions, Object, ObjectKey, ReconcileResult, Recorder, RecorderExt, kv, wrapf,
};
use ignition::types::{Config, Unit};

use crate::http;
use crate::mode::{dpu_mode_description, is_zero_trust_mode};

pub(crate) const HTTP_CLIENT_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const IGNITION_SECRET_NAME: &str = "ignition-server-ca-cert";
pub(crate) const IGNITION_TOKEN_PREFIX: &str = "token-";
pub(crate) const IGNITION_TOKEN_KEY: &str = "token";
pub(crate) const TOKEN_PULL_SECRET_HASH_KEY: &str = "pull-secret-hash";
pub(crate) const NODE_POOL_CURRENT_CONFIG_VERSION_ANNOTATION: &str =
    "hypershift.openshift.io/nodePoolCurrentConfigVersion";
pub(crate) const IGNITION_SYNC_REQUEUE_AFTER: Duration = Duration::from_secs(10);
pub(crate) const CONFIG_MAP_KEY_NAME: &str = "BF_CFG_TEMPLATE";
pub(crate) const CONFIG_MAP_NAME_PREFIX: &str = "bfcfg";
pub(crate) const IGNITION_VERSION: &str = "3.4.0";

// DPF provisioning label and annotation constants.
// Copied from github.com/nvidia/doca-platform/internal/provisioning/controllers/util
// (cannot be imported because it is internal)

/// `dpuProvisioningPrefix`
#[allow(dead_code)] // documents the prefix of the constants below (checked in tests)
pub(crate) const DPU_PROVISIONING_PREFIX: &str = "provisioning.dpu.nvidia.com/";
/// BfcfgTemplateLabel is the label on a bf.cfg template ConfigMap.
pub const BFCFG_TEMPLATE_LABEL: &str = "provisioning.dpu.nvidia.com/bfcfg-template";
/// BfcfgTemplateClusterNameAnnotation is the annotation specifying the target DPUCluster name.
pub const BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION: &str =
    "provisioning.dpu.nvidia.com/bfcfg-template-cluster-name";
/// BfcfgTemplateClusterNamespaceAnnotation is the annotation specifying the target DPUCluster namespace.
pub const BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION: &str =
    "provisioning.dpu.nvidia.com/bfcfg-template-cluster-namespace";

/// BfcfgTemplateBFBNameAnnotation is the annotation specifying the target BFB name.
pub const BFCFG_TEMPLATE_BFB_NAME_ANNOTATION: &str =
    "provisioning.dpu.nvidia.com/bfcfg-template-bfb-name";
/// BfcfgTemplateBFBNamespaceAnnotation is the annotation specifying the target BFB namespace.
pub const BFCFG_TEMPLATE_BFB_NAMESPACE_ANNOTATION: &str =
    "provisioning.dpu.nvidia.com/bfcfg-template-bfb-namespace";
/// BfcfgTemplateDPUFlavorNameAnnotation is the annotation specifying the target DPUFlavor name.
pub const BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION: &str =
    "provisioning.dpu.nvidia.com/bfcfg-template-dpuflavor-name";
/// BfcfgTemplateDPUFlavorNamespaceAnnotation is the annotation specifying the target DPUFlavor namespace.
pub const BFCFG_TEMPLATE_DPU_FLAVOR_NAMESPACE_ANNOTATION: &str =
    "provisioning.dpu.nvidia.com/bfcfg-template-dpuflavor-namespace";
/// bfcfgTemplateMachineOSURLAnnotation is the annotation specifying the machine OS image URL used in the ignition.
/// (`operatorcommon.AnnotationPrefix + "bfcfg-template-machine-os-url"`)
pub(crate) const BFCFG_TEMPLATE_MACHINE_OS_URL_ANNOTATION: &str =
    "dpfhcpprovisioner.dpu.hcp.io/bfcfg-template-machine-os-url";

/// BfcfgTemplateOCPReleaseImageAnnotation is the annotation specifying the OCP release image used for this ignition.
/// (`operatorcommon.AnnotationPrefix + "bfcfg-template-ocp-release-image"`)
pub const BFCFG_TEMPLATE_OCP_RELEASE_IMAGE_ANNOTATION: &str =
    "dpfhcpprovisioner.dpu.hcp.io/bfcfg-template-ocp-release-image";

/// `corev1.DockerConfigJsonKey`
const DOCKER_CONFIG_JSON_KEY: &str = ".dockerconfigjson";

/// `errIgnitionSyncPending` (`errors.New("HyperShift ignition credential sync is pending")`);
/// detected with `errors.Is` → [`find_in_chain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IgnitionSyncPending;

impl std::fmt::Display for IgnitionSyncPending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HyperShift ignition credential sync is pending")
    }
}
impl std::error::Error for IgnitionSyncPending {}

/// `fmt.Errorf("%w: <detail>", errIgnitionSyncPending)`
fn sync_pending(detail: impl std::fmt::Display) -> anyhow::Error {
    wrapf!(IgnitionSyncPending, "{}: {}", IgnitionSyncPending, detail)
}

/// Stand-in for the Go runtime panic `invalid memory address or nil pointer dereference`
/// raised where the Go code dereferences a nil pointer (e.g. `cr.Status.HostedClusterRef`
/// unset, `DPFOperatorConfig.spec.networking` / `.controlPlaneMTU` unset,
/// `spec.provisioningController` unset in zero-trust mode, `spec.dpuDeploymentRef` unset).
///
/// Instead of panicking, the condition is returned as this error; [`IgnitionGenerator::generate_ignition`]
/// propagates it untouched (no status update, no event) as
/// `panic: runtime error: invalid memory address or nil pointer dereference [recovered]` —
/// the error controller-runtime's panic recovery returns from `Reconcile` in Go.
/// Callers should return it immediately (use [`is_nil_pointer_dereference`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NilPointerDereference;

impl std::fmt::Display for NilPointerDereference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "panic: runtime error: invalid memory address or nil pointer dereference [recovered]",
        )
    }
}
impl std::error::Error for NilPointerDereference {}

/// Whether `err` stands for a Go nil-pointer panic (see [`NilPointerDereference`]).
pub fn is_nil_pointer_dereference(err: &anyhow::Error) -> bool {
    find_in_chain::<NilPointerDereference>(err).is_some()
}

fn nil_deref() -> anyhow::Error {
    anyhow::Error::new(NilPointerDereference)
}

/// hashPullSecret matches HyperShift's support/util.HashSimple for []byte inputs.
/// HashSimple formats its input with %v before hashing, so preserve that exact
/// representation when comparing against the pull-secret hash in token secrets.
pub(crate) fn hash_pull_secret(data: &[u8]) -> String {
    // fmt.Fprintf(hash, "%v", data) of a []byte: "[1 2 3]"
    let formatted = format!(
        "[{}]",
        data.iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    );
    // FNV-1a, 32 bit
    let mut hash: u32 = 0x811c_9dc5;
    for b in formatted.as_bytes() {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

/// ConfigMapName returns the expected ignition ConfigMap name for a given DPUCluster.
pub fn config_map_name(dpu_cluster_name: &str) -> String {
    format!("{CONFIG_MAP_NAME_PREFIX}-{dpu_cluster_name}.cfg")
}

/// IgnitionGenerator handles ignition configuration generation for DPF provisioning
#[derive(Clone)]
pub struct IgnitionGenerator {
    pub client: Client,
    pub recorder: Recorder,
}

impl IgnitionGenerator {
    /// NewIgnitionGenerator creates a new IgnitionGenerator (the Go `*runtime.Scheme` parameter
    /// is not needed: owner references take the GVK from the owner's type).
    pub fn new(client: Client, recorder: Recorder) -> Self {
        Self { client, recorder }
    }

    /// GenerateIgnition performs the complete ignition generation workflow
    pub async fn generate_ignition(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = ctx.log.with_values(kv!["feature" => "ignition-generation"]);

        // Execute ignition generation workflow
        if let Err(err) = self.generate_ignition_inner(ctx, cr).await {
            if is_nil_pointer_dereference(&err) {
                // Go: the runtime panic unwinds out of GenerateIgnition.
                return Err(nil_deref());
            }
            if find_in_chain::<IgnitionSyncPending>(&err).is_some() {
                log.info(
                    "Waiting for HyperShift to sync the current ignition configuration",
                    kv!["reason" => err.to_string()],
                );
                let generation = cr.generation();
                meta::set_status_condition(
                    &mut cr.status.conditions,
                    meta::condition_gen(
                        provisioningv1alpha1::IGNITION_CONFIGURED,
                        meta::CONDITION_FALSE,
                        provisioningv1alpha1::REASON_IGNITION_CREDENTIAL_SYNC_PENDING,
                        err.to_string(),
                        generation,
                    ),
                );
                meta::set_status_condition(
                    &mut cr.status.conditions,
                    meta::condition_gen(
                        provisioningv1alpha1::READY,
                        meta::CONDITION_FALSE,
                        "IgnitionNotConfigured",
                        "Waiting for HyperShift to sync the current ignition configuration",
                        generation,
                    ),
                );
                if let Err(update_err) = self.client.update_status(cr).await {
                    return Err(wrapf!(
                        update_err,
                        "failed to update status while waiting for ignition sync: {}",
                        update_err
                    ));
                }
                return Ok(ReconcileResult::requeue_after(IGNITION_SYNC_REQUEUE_AFTER));
            }

            log.error(&err, "Ignition generation failed", kv![]);

            // Set error condition and persist immediately — this is an early-return error path
            let generation = cr.generation();
            meta::set_status_condition(
                &mut cr.status.conditions,
                meta::condition_gen(
                    provisioningv1alpha1::IGNITION_CONFIGURED,
                    meta::CONDITION_FALSE,
                    provisioningv1alpha1::REASON_IGNITION_GENERATION_FAILED,
                    format!("Failed to generate ignition: {err}"),
                    generation,
                ),
            );
            if let Err(update_err) = self.client.update_status(cr).await {
                return Err(wrapf!(
                    update_err,
                    "failed to update status after ignition error: {}",
                    update_err
                ));
            }

            self.recorder.as_ref().event(
                &*cr,
                EVENT_TYPE_WARNING,
                "IgnitionGenerationFailed",
                &err.to_string(),
            );
            // Use RequeueAfter instead of returning error to avoid tight retry loop
            // (returning error + status update watch would both trigger immediate reconciles)
            return Ok(ReconcileResult::requeue_after(Duration::from_secs(30)));
        }

        // Success - extract version for status message
        let cm_name = config_map_name(&cr.spec.dpu_cluster_ref.name);

        let generation = cr.generation();
        meta::set_status_condition(
            &mut cr.status.conditions,
            meta::condition_gen(
                provisioningv1alpha1::IGNITION_CONFIGURED,
                meta::CONDITION_TRUE,
                provisioningv1alpha1::REASON_IGNITION_GENERATED,
                format!("Ignition ConfigMap {cm_name} created successfully"),
                generation,
            ),
        );

        // Persist status changes
        if let Err(err) = self.client.update_status(cr).await {
            log.error(
                &err,
                "Failed to update status after successful ignition generation",
                kv![],
            );
            return Err(err);
        }

        self.recorder.as_ref().event(
            &*cr,
            EVENT_TYPE_NORMAL,
            "IgnitionConfigured",
            &format!("Ignition ConfigMap {cm_name} configured successfully"),
        );

        log.info("Ignition generation completed successfully", kv![]);
        Ok(ReconcileResult::default())
    }

    /// generateIgnition executes the complete ignition generation workflow
    pub(crate) async fn generate_ignition_inner(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> anyhow::Result<()> {
        let log = &ctx.log;

        // Download HCP ignition
        log.v(1).info("Downloading HCP ignition", kv![]);
        let hcp_ignition_bytes = self
            .download_hcp_ignition(ctx, cr)
            .await
            .map_err(|e| wrapf!(e, "failed to download HCP ignition: {}", e))?;

        let dpf_operator_config = common::config::get_singleton_dpf_operator_config(&self.client)
            .await
            .map_err(|e| wrapf!(e, "failed to get DPFOperatorConfig: {}", e))?;

        // Retrieve DPU Flavor configuration (needed for flavor content and 26.4 dpuMode fallback)
        log.v(1).info("Retrieving DPU Flavor configuration", kv![]);
        let dpu_flavor = self
            .retrieve_dpu_flavor(ctx, cr)
            .await
            .map_err(|e| wrapf!(e, "failed to retrieve DPU Flavor: {}", e))?;

        let (zero_trust, dpu_mode) =
            match is_zero_trust_mode(Some(&dpf_operator_config), Some(&dpu_flavor)) {
                Ok(v) => v,
                Err(err) => {
                    log.error(&err, "Failed to detect DPU mode for ignition", kv![]);
                    return Err(wrapf!(err, "failed to detect DPU mode: {}", err));
                }
            };
        log.info(
            "DPU mode detected for ignition",
            kv![
                "mode" => dpu_mode,
                "deploymentMode" => dpf_operator_config.spec.deployment_mode,
                "dpuFlavorDpuMode" => dpu_flavor.spec.dpu_mode,
                "description" => dpu_mode_description(&dpu_mode),
                "zeroTrust" => zero_trust,
            ],
        );

        // Build target ignition (HCP + DPF modifications)
        log.v(1).info("Building target ignition", kv![]);

        // uint16(*dpfOperatorConfig.Spec.Networking.ControlPlaneMTU)
        let mtu = match dpf_operator_config
            .spec
            .networking
            .as_ref()
            .and_then(|n| n.control_plane_mtu)
        {
            Some(v) => v as u16,
            None => return Err(nil_deref()),
        };

        // Use spec.machineOSURL if set, otherwise fall back to status.blueFieldOCPLayerImage (from OCP layer lookup)
        let mut machine_os_url = cr.spec.machine_osurl.clone();
        if machine_os_url.is_empty() {
            machine_os_url = cr.status.blue_field_ocp_layer_image.clone();
        }
        if machine_os_url.is_empty() {
            let generation = cr.generation();
            meta::set_status_condition(
                &mut cr.status.conditions,
                meta::condition_gen(
                    provisioningv1alpha1::IGNITION_CONFIGURED,
                    meta::CONDITION_FALSE,
                    provisioningv1alpha1::REASON_MACHINE_OSURL_MISSING,
                    "No machine OS URL available: status.cachedMachineOSURL, spec.machineOSURL, and status.blueFieldOCPLayerImage are all empty",
                    generation,
                ),
            );
            return Err(anyhow::anyhow!(
                "no machine OS URL available: spec.machineOSURL is empty and status.blueFieldOCPLayerImage is not set"
            ));
        }

        let mut bfb_registry_url = String::new();
        if zero_trust {
            bfb_registry_url = self
                .resolve_bfb_registry_url(ctx, &dpf_operator_config)
                .await
                .map_err(|e| {
                    wrapf!(
                        e,
                        "failed to resolve bfb-registry URL for zero-trust mode: {}",
                        e
                    )
                })?;
            log.info(
                "Resolved bfb-registry URL for zero-trust mode",
                kv!["url" => bfb_registry_url],
            );
        }

        let mut target_ignition = self
            .build_target_ignition(
                &hcp_ignition_bytes,
                &dpu_flavor,
                &machine_os_url,
                mtu,
                zero_trust,
                &bfb_registry_url,
            )
            .map_err(|e| wrapf!(e, "failed to build target ignition: {}", e))?;

        // Validate target ignition before compression
        log.v(1).info("Validating target ignition", kv![]);
        ignition::validate_config(Some(&target_ignition))
            .map_err(|e| wrapf!(e, "target ignition validation failed: {}", e))?;

        // Gzip-compress all uncompressed files in target ignition
        ignition::gzip_ignition_files(&mut target_ignition)
            .map_err(|e| wrapf!(e, "failed to gzip target ignition files: {}", e))?;

        // Build live ignition (embed target)
        log.v(1).info("Building live ignition", kv![]);
        // We are adding DPU Flavor in JSON format for early setup tasks, such as nvconfig parameters setup.
        // JSON is easier to parse with the tooling we have in Live RHCOS install media.
        let live_ignition = self
            .build_live_ignition(
                &target_ignition,
                &hcp_ignition_bytes,
                &dpu_flavor,
                zero_trust,
            )
            .map_err(|e| wrapf!(e, "failed to build live ignition: {}", e))?;

        // NOTE: Live ignition is intentionally NOT validated here. It contains Go template
        // placeholders (e.g. {{.DPUHostName}}) in data URIs that are substituted later by the
        // DPF provisioning controller. These placeholders use curly braces which are invalid
        // in RFC 2397 data URIs, so the ignition validator would reject them. The target
        // ignition (validated above) is the structurally important config — it's what actually
        // boots the DPU. The live ignition is just a delivery wrapper with templates.

        // Create/Update ConfigMap
        log.v(1).info("Creating/Updating ConfigMap", kv![]);
        self.create_or_update_config_map(ctx, cr, &live_ignition, &machine_os_url)
            .await
            .map_err(|e| wrapf!(e, "failed to create/update ConfigMap: {}", e))?;

        Ok(())
    }

    /// controlPlaneNamespace returns the HyperShift control plane namespace for this CR.
    /// HyperShift creates it as "<hostedcluster-namespace>-<hostedcluster-name>".
    /// (An unset `status.hostedClusterRef` is a Go nil-pointer panic.)
    pub(crate) fn control_plane_namespace(
        &self,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> anyhow::Result<String> {
        let r = hosted_cluster_ref(cr)?;
        Ok(format!("{}-{}", r.namespace, r.name))
    }

    /// getIgnitionCACert reads the ignition server's TLS CA certificate from the control plane namespace.
    pub(crate) async fn get_ignition_ca_cert(
        &self,
        _ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> anyhow::Result<Vec<u8>> {
        let key = ObjectKey::new(self.control_plane_namespace(cr)?, IGNITION_SECRET_NAME);
        let ca_secret: Secret = self
            .client
            .get(&key)
            .await
            .map_err(|e| wrapf!(e, "failed to get ignition CA cert secret: {}", e))?;

        match ca_secret.data.as_ref().and_then(|d| d.get("tls.crt")) {
            Some(ca_cert) => Ok(ca_cert.0.clone()),
            None => Err(anyhow::anyhow!(
                "tls.crt not found in secret {}",
                IGNITION_SECRET_NAME
            )),
        }
    }

    /// getIgnitionTokens returns token candidates for legacy objects that do not have credential
    /// references. Valid DPFHCPProvisioners use getSynchronizedIgnitionToken to select the token for
    /// the NodePool's current config and verify its pull-secret hash.
    pub(crate) async fn get_ignition_tokens(
        &self,
        _ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> anyhow::Result<Vec<String>> {
        let ns = self.control_plane_namespace(cr)?;
        let prefix = format!("{}{}-", IGNITION_TOKEN_PREFIX, hosted_cluster_ref(cr)?.name);

        let secret_list: Vec<Secret> = self
            .client
            .list(&ListOptions::new().in_namespace(ns.clone()))
            .await
            .map_err(|e| wrapf!(e, "failed to list secrets in {}: {}", ns, e))?;

        let mut candidates: Vec<Secret> = secret_list
            .into_iter()
            .filter(|s| s.name().starts_with(&prefix))
            .collect();
        if candidates.is_empty() {
            return Err(anyhow::anyhow!(
                "no token secret with prefix {} found in namespace {}",
                gocompat::strconv::quote(&prefix),
                ns
            ));
        }

        // Newest first — usually the right answer after a forward rotation.
        // (Go's sort.Slice is an insertion sort — i.e. stable — for up to 12 elements.)
        candidates.sort_by_key(|s| std::cmp::Reverse(creation_timestamp(s)));

        let mut tokens = Vec::new();
        for c in &candidates {
            let Some(token_bytes) = c.data.as_ref().and_then(|d| d.get(IGNITION_TOKEN_KEY)) else {
                continue;
            };
            tokens.push(gocompat::base64::encode_to_string(&token_bytes.0));
        }
        if tokens.is_empty() {
            return Err(anyhow::anyhow!(
                "no usable token data in secrets with prefix {} in namespace {}",
                gocompat::strconv::quote(&prefix),
                ns
            ));
        }
        Ok(tokens)
    }

    /// getSynchronizedIgnitionToken returns the token for the NodePool's current config only after
    /// HyperShift has created it with the pull-secret hash that matches the current HostedCluster
    /// pull secret. It also returns the current SSH public key so the downloaded ignition can be
    /// checked for propagation of SSHKey changes, which do not affect the pull-secret hash.
    pub(crate) async fn get_synchronized_ignition_token(
        &self,
        _ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> anyhow::Result<(String, Vec<u8>)> {
        let Some(hc_ref) = cr.status.hosted_cluster_ref.as_ref() else {
            return Err(sync_pending("HostedCluster reference is not available"));
        };
        if cr.spec.pull_secret_ref.name.is_empty() || cr.spec.ssh_key_secret_ref.name.is_empty() {
            return Err(sync_pending(
                "pull-secret and SSH-key references must both be set",
            ));
        }

        let hc_key = ObjectKey::new(hc_ref.namespace.clone(), hc_ref.name.clone());
        let hc: hyperv1::HostedCluster = match self.client.get(&hc_key).await {
            Ok(hc) => hc,
            Err(err) => {
                if is_not_found(&err) {
                    return Err(sync_pending(format!(
                        "HostedCluster {hc_key} is not available yet"
                    )));
                }
                return Err(wrapf!(
                    err,
                    "failed to get HostedCluster {}: {}",
                    hc_key,
                    err
                ));
            }
        };

        let expected_pull_secret_name = ctrl_hostedcluster::pull_secret_copy_name(cr);
        if hc.spec.pull_secret.name != expected_pull_secret_name {
            return Err(sync_pending(format!(
                "HostedCluster pull-secret reference is {}, waiting for {}",
                gocompat::strconv::quote(&hc.spec.pull_secret.name),
                gocompat::strconv::quote(&expected_pull_secret_name)
            )));
        }
        let expected_ssh_key_name = ctrl_hostedcluster::ssh_key_copy_name(cr);
        if hc.spec.ssh_key.name != expected_ssh_key_name {
            return Err(sync_pending(format!(
                "HostedCluster SSH-key reference is {}, waiting for {}",
                gocompat::strconv::quote(&hc.spec.ssh_key.name),
                gocompat::strconv::quote(&expected_ssh_key_name)
            )));
        }

        let pull_secret_key = ObjectKey::new(hc.namespace(), hc.spec.pull_secret.name.clone());
        let pull_secret: Secret = match self.client.get(&pull_secret_key).await {
            Ok(s) => s,
            Err(err) => {
                if is_not_found(&err) {
                    return Err(sync_pending(format!(
                        "pull secret {pull_secret_key} is not available yet"
                    )));
                }
                return Err(wrapf!(
                    err,
                    "failed to get pull secret {}: {}",
                    pull_secret_key,
                    err
                ));
            }
        };
        let Some(pull_secret_data) = pull_secret
            .data
            .as_ref()
            .and_then(|d| d.get(DOCKER_CONFIG_JSON_KEY))
        else {
            return Err(anyhow::anyhow!(
                "pull secret {} is missing {}",
                pull_secret_key,
                gocompat::strconv::quote(DOCKER_CONFIG_JSON_KEY)
            ));
        };
        let expected_pull_secret_hash = hash_pull_secret(&pull_secret_data.0);

        let node_pool_key = ObjectKey::new(hc.namespace(), hc.name());
        let node_pool: hyperv1::NodePool = match self.client.get(&node_pool_key).await {
            Ok(np) => np,
            Err(err) => {
                if is_not_found(&err) {
                    return Err(sync_pending(format!(
                        "NodePool {node_pool_key} is not available yet"
                    )));
                }
                return Err(wrapf!(
                    err,
                    "failed to get NodePool {}: {}",
                    node_pool_key,
                    err
                ));
            }
        };

        let current_config_version = node_pool
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(NODE_POOL_CURRENT_CONFIG_VERSION_ANNOTATION))
            .cloned()
            .unwrap_or_default();
        if current_config_version.is_empty() {
            return Err(sync_pending(format!(
                "NodePool {node_pool_key} has not published its current config version"
            )));
        }

        let token_secret_key = ObjectKey::new(
            self.control_plane_namespace(cr)?,
            format!(
                "{}{}-{}",
                IGNITION_TOKEN_PREFIX,
                node_pool.name(),
                current_config_version
            ),
        );
        let token_secret: Secret = match self.client.get(&token_secret_key).await {
            Ok(s) => s,
            Err(err) => {
                if is_not_found(&err) {
                    return Err(sync_pending(format!(
                        "current token secret {token_secret_key} is not available yet"
                    )));
                }
                return Err(wrapf!(
                    err,
                    "failed to get current token secret {}: {}",
                    token_secret_key,
                    err
                ));
            }
        };
        let token_data = token_secret.data.clone().unwrap_or_default();
        let Some(token) = token_data.get(IGNITION_TOKEN_KEY) else {
            return Err(sync_pending(format!(
                "current token secret {token_secret_key} has no token yet"
            )));
        };
        let token_pull_secret_hash = token_data
            .get(TOKEN_PULL_SECRET_HASH_KEY)
            .map(|b| b.0.as_slice())
            .unwrap_or_default();
        if token_pull_secret_hash != expected_pull_secret_hash.as_bytes() {
            return Err(sync_pending(
                "current token secret pull-secret hash does not match HostedCluster pull secret",
            ));
        }

        let ssh_key_secret_key = ObjectKey::new(hc.namespace(), hc.spec.ssh_key.name.clone());
        let ssh_key_secret: Secret = match self.client.get(&ssh_key_secret_key).await {
            Ok(s) => s,
            Err(err) => {
                if is_not_found(&err) {
                    return Err(sync_pending(format!(
                        "SSH-key secret {ssh_key_secret_key} is not available yet"
                    )));
                }
                return Err(wrapf!(
                    err,
                    "failed to get SSH-key secret {}: {}",
                    ssh_key_secret_key,
                    err
                ));
            }
        };
        let ssh_key = ssh_key_secret
            .data
            .as_ref()
            .and_then(|d| d.get("id_rsa.pub"))
            .map(|b| b.0.clone());
        let ssh_key = match ssh_key {
            Some(k) if !gocompat::unicode::trim_space(&k).is_empty() => k,
            _ => {
                return Err(anyhow::anyhow!(
                    "SSH-key secret {} is missing a non-empty id_rsa.pub key",
                    ssh_key_secret_key
                ));
            }
        };

        Ok((gocompat::base64::encode_to_string(&token.0), ssh_key))
    }

    /// getIgnitionEndpoint reads the ignition endpoint URL from the HostedCluster status.
    pub(crate) async fn get_ignition_endpoint(
        &self,
        _ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> anyhow::Result<String> {
        let r = hosted_cluster_ref(cr)?;
        let key = ObjectKey::new(r.namespace.clone(), r.name.clone());
        let hc: hyperv1::HostedCluster = self
            .client
            .get(&key)
            .await
            .map_err(|e| wrapf!(e, "failed to get HostedCluster: {}", e))?;
        if hc.status.ignition_endpoint.is_empty() {
            return Err(anyhow::anyhow!(
                "ignition endpoint not available in HostedCluster status"
            ));
        }
        Ok(hc.status.ignition_endpoint)
    }

    /// downloadHCPIgnition downloads ignition from the HostedCluster endpoint. For valid provisioners,
    /// it uses only the token for the NodePool's current config and verifies that the response contains
    /// the current SSH key before allowing the caller to publish the ConfigMap.
    pub(crate) async fn download_hcp_ignition(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> anyhow::Result<Vec<u8>> {
        let log = &ctx.log;
        let strict_credential_sync =
            !cr.spec.pull_secret_ref.name.is_empty() || !cr.spec.ssh_key_secret_ref.name.is_empty();

        let ca_cert = self.get_ignition_ca_cert(ctx, cr).await?;

        let tokens: Vec<String>;
        let mut expected_ssh_key: Vec<u8> = Vec::new();
        if strict_credential_sync {
            let (current_token, ssh_key) = self.get_synchronized_ignition_token(ctx, cr).await?;
            tokens = vec![current_token];
            expected_ssh_key = ssh_key;
        } else {
            // Compatibility fallback for incomplete legacy objects (and test fixtures) with both
            // required refs empty. This probes tokens by HTTP acceptance but cannot prove the
            // ignition matches the current refs; valid DPFHCPProvisioners must use the strict path.
            tokens = self.get_ignition_tokens(ctx, cr).await?;
        }

        let endpoint = self.get_ignition_endpoint(ctx, cr).await?;

        // Build HTTPS client with the ignition server's CA
        let http_client = http::new_client(&ca_cert, HTTP_CLIENT_TIMEOUT)?;

        let ignition_url = format!("https://{endpoint}/ignition");
        log.info(
            "Downloading ignition from HCP",
            kv!["url" => ignition_url, "token_candidates" => tokens.len()],
        );

        // Try each token; the correct one for the current HC spec returns 200.
        // Wrong tokens (stale pull-secret-hash) return a non-200 status — skip them.
        for (i, token) in tokens.iter().enumerate() {
            let resp = match http_client.get(ctx, &ignition_url, token).await {
                Ok(resp) => resp,
                Err(http::GetError::NewRequest(err)) => {
                    return Err(wrapf!(err, "failed to create HTTP request: {}", err));
                }
                Err(http::GetError::Do(err)) => {
                    if strict_credential_sync {
                        return Err(sync_pending(format!(
                            "ignition endpoint is not ready for the current token: {err}"
                        )));
                    }
                    return Err(wrapf!(err, "HTTP request failed: {}", err));
                }
            };

            if resp.status().as_u16() == 200 {
                // fall through to read body below
                let body = match http_client.read_body(ctx, resp).await {
                    Ok(b) => b,
                    Err(read_err) => {
                        return Err(wrapf!(
                            read_err,
                            "failed to read ignition response body: {}",
                            read_err
                        ));
                    }
                };
                if strict_credential_sync {
                    let contains_ssh_key = ignition_contains_ssh_key(&body, &expected_ssh_key)?;
                    if !contains_ssh_key {
                        return Err(sync_pending(
                            "ignition endpoint has not propagated the current SSH key",
                        ));
                    }
                }
                log.info("Ignition downloaded successfully", kv!["token_index" => i]);
                return Ok(body);
            }

            let status = resp.status().as_u16();
            drop(resp);
            if strict_credential_sync {
                return Err(sync_pending(format!(
                    "current ignition token was rejected with HTTP status {status}"
                )));
            }
            log.v(1).info(
                "Ignition token rejected, trying next",
                kv![
                    "token_index" => i,
                    "status" => status,
                    "remaining" => tokens.len() - i - 1,
                ],
            );
        }

        Err(anyhow::anyhow!(
            "all {} token candidate(s) rejected by ignition server at {}",
            tokens.len(),
            ignition_url
        ))
    }

    /// getDPUDeployment fetches the DPUDeployment CR referenced by the provisioner
    pub(crate) async fn get_dpu_deployment(
        &self,
        _ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> anyhow::Result<dpuservicev1alpha1::DPUDeployment> {
        let Some(r) = cr.spec.dpu_deployment_ref.as_ref() else {
            return Err(nil_deref());
        };
        let deployment_key = ObjectKey::new(r.namespace.clone(), r.name.clone());
        self.client
            .get(&deployment_key)
            .await
            .map_err(|e| wrapf!(e, "failed to get DPUDeployment: {}", e))
    }

    /// retrieveDPUFlavor retrieves the DPU Flavor configuration from DPUDeployment
    pub(crate) async fn retrieve_dpu_flavor(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> anyhow::Result<dpuprovisioningv1alpha1::DPUFlavor> {
        let dpu_deployment = self.get_dpu_deployment(ctx, cr).await?;

        let flavor_name = dpu_deployment.spec.dpus.flavor.clone().unwrap_or_default();
        if flavor_name.is_empty() {
            return Err(anyhow::anyhow!("DPUDeployment.Spec.DPUs.Flavor is empty"));
        }

        // Get DPUFlavor from DPUDeployment's namespace
        let Some(r) = cr.spec.dpu_deployment_ref.as_ref() else {
            return Err(nil_deref());
        };
        let flavor_key = ObjectKey::new(r.namespace.clone(), flavor_name.clone());
        self.client
            .get(&flavor_key)
            .await
            .map_err(|e| wrapf!(e, "failed to get DPUFlavor {}: {}", flavor_name, e))
    }

    /// buildTargetIgnition builds the target ignition with HCP ignition + DPF modifications
    pub(crate) fn build_target_ignition(
        &self,
        hcp_ignition_bytes: &[u8],
        dpu_flavor: &dpuprovisioningv1alpha1::DPUFlavor,
        machine_os_url: &str,
        mtu: u16,
        zero_trust: bool,
        bfb_registry_url: &str,
    ) -> anyhow::Result<Config> {
        // Parse HCP ignition
        let mut target_ignition = ignition::unmarshal_config(hcp_ignition_bytes)
            .map_err(|e| wrapf!(e, "failed to parse HCP ignition: {}", e))?;

        // Replace machine OS URL
        ignition::replace_machine_os_url(&mut target_ignition, machine_os_url)
            .map_err(|e| wrapf!(e, "failed to replace machine OS URL: {}", e))?;

        // Add target content files and systemd units
        let target_provider = ignition::resources::target::new_provider(zero_trust);
        ignition::content::add_content(&mut target_ignition, &target_provider)
            .map_err(|e| wrapf!(e, "failed to add target content: {}", e))?;

        // Add templated dpu-agent.service unit (ZT mode adds extra flags)
        let (unit_name, unit_contents) =
            ignition::resources::target::render_dpu_agent_service_unit(zero_trust)
                .map_err(|e| wrapf!(e, "failed to render dpu-agent.service: {}", e))?;
        let mut unit = Unit::new(unit_name);
        unit.enabled = Some(true);
        unit.contents = Some(unit_contents);
        ignition::units_mut(&mut target_ignition).push(unit);

        // Add common content files and systemd units
        let common_provider = ignition::resources::common::new_provider(zero_trust);
        ignition::content::add_content(&mut target_ignition, &common_provider)
            .map_err(|e| wrapf!(e, "failed to add common content: {}", e))?;

        // Add flavor OVS script
        ignition::add_flavor_ovs_script(&mut target_ignition, &dpu_flavor.spec);

        // Merge DPUFlavor config files into the target ignition.
        ignition::merge_flavor_config_files(&mut target_ignition, &dpu_flavor.spec)
            .map_err(|e| wrapf!(e, "failed to merge flavor config files: {}", e))?;

        // Add DPU flavor YAML
        ignition::add_dpu_flavor_yaml(&mut target_ignition, dpu_flavor)
            .map_err(|e| wrapf!(e, "failed to add DPU flavor YAML: {}", e))?;

        if mtu != 1500 {
            ignition::enable_mtu(&mut target_ignition, mtu);
        }

        if !bfb_registry_url.is_empty() {
            let source = format!("data:,BFBRegistryURL={bfb_registry_url}%0A");
            ignition::files_mut(&mut target_ignition).push(ignition::new_file(
                "/etc/dpf/bfb-registry",
                0o644,
                source,
            ));
        }

        Ok(target_ignition)
    }

    /// buildLiveIgnition builds the live ignition with embedded target ignition
    pub(crate) fn build_live_ignition(
        &self,
        target_ignition: &Config,
        hcp_ignition_bytes: &[u8],
        dpu_flavor: &dpuprovisioningv1alpha1::DPUFlavor,
        zero_trust: bool,
    ) -> anyhow::Result<Config> {
        // Parse HCP to extract passwd
        let hcp_ignition = ignition::unmarshal_config(hcp_ignition_bytes)
            .map_err(|e| wrapf!(e, "failed to parse HCP ignition for passwd: {}", e))?;

        // Create empty live ignition
        let mut live_ignition = ignition::new_empty_ignition(IGNITION_VERSION);

        // Copy passwd from HCP ignition
        let has_groups = hcp_ignition
            .passwd
            .as_ref()
            .and_then(|p| p.groups.as_ref())
            .is_some_and(|g| !g.is_empty());
        if !ignition::users(&hcp_ignition).is_empty() || has_groups {
            live_ignition.passwd = hcp_ignition.passwd.clone();
        }

        // Add live content files and systemd units
        let live_provider = ignition::resources::live::new_provider(zero_trust);
        ignition::content::add_content(&mut live_ignition, &live_provider)
            .map_err(|e| wrapf!(e, "failed to add live content: {}", e))?;

        // Add common content files and systemd units
        let common_provider = ignition::resources::common::new_provider(zero_trust);
        ignition::content::add_content(&mut live_ignition, &common_provider)
            .map_err(|e| wrapf!(e, "failed to add common content: {}", e))?;

        // Add DPU flavor JSON
        ignition::add_dpu_flavor_json(&mut live_ignition, dpu_flavor)
            .map_err(|e| wrapf!(e, "failed to add DPU flavor JSON: {}", e))?;

        // Encode target ignition (gzip + base64)
        let encoded_target = ignition::encode_ignition(target_ignition)
            .map_err(|e| wrapf!(e, "failed to encode target ignition: {}", e))?;

        // Embed encoded target as file at /var/target.ign
        let source = format!("data:;base64,{encoded_target}");
        let mut target_file = ignition::new_file("/var/target.ign", 0o644, source);
        if let Some(c) = target_file.contents.as_mut() {
            c.compression = Some("gzip".to_string());
        }
        ignition::files_mut(&mut live_ignition).push(target_file);

        Ok(live_ignition)
    }

    /// createOrUpdateConfigMap creates or updates the ignition ConfigMap in DPUCluster namespace
    pub(crate) async fn create_or_update_config_map(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
        live_ignition: &Config,
        machine_os_url: &str,
    ) -> anyhow::Result<()> {
        let log = &ctx.log;

        // Marshal live ignition to JSON
        let ignition_json = ignition::marshal_json(live_ignition)
            .map_err(|e| wrapf!(e, "failed to marshal live ignition: {}", e))?;

        let cm_name = config_map_name(&cr.spec.dpu_cluster_ref.name);
        let cm_namespace = cr.spec.dpu_cluster_ref.namespace.clone();

        // Build labels and annotations required by the DPF provisioning controller
        let labels = BTreeMap::from([(BFCFG_TEMPLATE_LABEL.to_string(), "true".to_string())]);
        let mut annotations = BTreeMap::from([
            (
                BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION.to_string(),
                cr.spec.dpu_cluster_ref.name.clone(),
            ),
            (
                BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION.to_string(),
                cr.spec.dpu_cluster_ref.namespace.clone(),
            ),
            (
                BFCFG_TEMPLATE_MACHINE_OS_URL_ANNOTATION.to_string(),
                machine_os_url.to_string(),
            ),
            (
                BFCFG_TEMPLATE_OCP_RELEASE_IMAGE_ANNOTATION.to_string(),
                cr.spec.ocp_release_image.clone(),
            ),
        ]);
        if let Some(dpu_deployment_ref) = cr.spec.dpu_deployment_ref.as_ref() {
            let dpu_deployment = self
                .get_dpu_deployment(ctx, cr)
                .await
                .map_err(|e| wrapf!(e, "failed to get DPUDeployment for BFB name: {}", e))?;
            let bfb_name = dpu_deployment.spec.dpus.bfb.clone().unwrap_or_default();
            if !bfb_name.is_empty() {
                annotations.insert(BFCFG_TEMPLATE_BFB_NAME_ANNOTATION.to_string(), bfb_name);
            }
            annotations.insert(
                BFCFG_TEMPLATE_BFB_NAMESPACE_ANNOTATION.to_string(),
                dpu_deployment_ref.namespace.clone(),
            );
            let flavor_name = dpu_deployment.spec.dpus.flavor.clone().unwrap_or_default();
            if !flavor_name.is_empty() {
                annotations.insert(
                    BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION.to_string(),
                    flavor_name,
                );
            }
            annotations.insert(
                BFCFG_TEMPLATE_DPU_FLAVOR_NAMESPACE_ANNOTATION.to_string(),
                dpu_deployment_ref.namespace.clone(),
            );
        }

        let mut data = String::from_utf8_lossy(&ignition_json).into_owned();
        data.push('\n');
        let mut cm = ConfigMap {
            metadata: ObjectMeta {
                name: Some(cm_name.clone()),
                namespace: Some(cm_namespace.clone()),
                labels: Some(labels),
                annotations: Some(annotations),
                ..Default::default()
            },
            data: Some(BTreeMap::from([(CONFIG_MAP_KEY_NAME.to_string(), data)])),
            ..Default::default()
        };

        // Set owner reference if same namespace (cross-namespace owner refs not supported)
        if cr.namespace() == cm_namespace
            && let Err(err) = meta::set_controller_reference(cr, &mut cm)
        {
            log.v(1).info(
                "Cannot set owner reference",
                kv!["error" => err.to_string()],
            );
        }

        // Create or update
        let cm_key = ObjectKey::new(cm_namespace.clone(), cm_name.clone());
        let mut existing_cm: ConfigMap = match self.client.get(&cm_key).await {
            Ok(existing) => existing,
            Err(err) => {
                if is_not_found(&err) {
                    // Create new ConfigMap
                    if let Err(err) = self.client.create(&mut cm).await {
                        return Err(wrapf!(err, "failed to create ConfigMap: {}", err));
                    }
                    log.info(
                        "Created ignition ConfigMap",
                        kv!["name" => cm_name, "namespace" => cm_namespace],
                    );
                    return Ok(());
                }
                return Err(wrapf!(err, "failed to get ConfigMap: {}", err));
            }
        };

        // Update existing ConfigMap
        existing_cm.metadata.labels = cm.metadata.labels;
        existing_cm.metadata.annotations = cm.metadata.annotations;
        existing_cm.data = cm.data;
        if let Err(err) = self.client.update(&mut existing_cm).await {
            return Err(wrapf!(err, "failed to update ConfigMap: {}", err));
        }

        log.info(
            "Updated ignition ConfigMap",
            kv!["name" => cm_name, "namespace" => cm_namespace],
        );
        Ok(())
    }

    /// resolveBFBRegistryURL resolves the bfb-registry URL for zero-trust mode.
    /// It checks the DPFOperatorConfig registry configuration first, then falls back
    /// to discovering the bfb-registry NodePort service.
    pub(crate) async fn resolve_bfb_registry_url(
        &self,
        ctx: &Ctx,
        dpf_operator_config: &operatorv1alpha1::DPFOperatorConfig,
    ) -> anyhow::Result<String> {
        let log = &ctx.log;

        let Some(provisioning_controller) =
            dpf_operator_config.spec.provisioning_controller.as_ref()
        else {
            return Err(nil_deref());
        };
        if let Some(reg) = provisioning_controller.registry.as_ref() {
            if let Some(addr) = reg.load_balancer_address.as_deref()
                && !addr.is_empty()
            {
                log.v(1).info(
                    "Using LoadBalancerAddress from DPFOperatorConfig",
                    kv!["address" => addr],
                );
                return Ok(addr.trim_end_matches('/').to_string());
            }
            if let Some(addr) = reg.address.as_deref()
                && !addr.is_empty()
            {
                log.v(1).info(
                    "Using Address from DPFOperatorConfig",
                    kv!["address" => addr],
                );
                return Ok(addr.trim_end_matches('/').to_string());
            }
        }

        log.v(1).info(
            "No registry address configured, discovering bfb-registry NodePort service",
            kv![],
        );

        let svc_key = ObjectKey::new(
            dpf_operator_config.namespace(),
            operatorv1alpha1::BFB_REGISTRY_NAME,
        );
        let svc: Service = self
            .client
            .get(&svc_key)
            .await
            .map_err(|e| wrapf!(e, "failed to get bfb-registry service {}: {}", svc_key, e))?;

        let node_port = svc
            .spec
            .as_ref()
            .and_then(|s| s.ports.as_ref())
            .and_then(|ports| {
                ports
                    .iter()
                    .map(|p| p.node_port.unwrap_or(0))
                    .find(|np| *np != 0)
            })
            .unwrap_or(0);
        if node_port == 0 {
            return Err(anyhow::anyhow!(
                "bfb-registry service {} has no NodePort",
                svc_key
            ));
        }

        let node_list: Vec<Node> = self
            .client
            .list_all()
            .await
            .map_err(|e| wrapf!(e, "failed to list nodes: {}", e))?;

        let mut node_ip = String::new();
        for node in &node_list {
            let addresses = node
                .status
                .as_ref()
                .and_then(|s| s.addresses.as_deref())
                .unwrap_or_default();
            for addr in addresses {
                if addr.type_ == "InternalIP" {
                    node_ip = addr.address.clone();
                    break;
                }
            }
            if !node_ip.is_empty() {
                break;
            }
        }
        if node_ip.is_empty() {
            return Err(anyhow::anyhow!("no node with InternalIP found"));
        }

        let url = format!("http://{node_ip}:{node_port}");
        log.info(
            "Discovered bfb-registry via NodePort",
            kv!["url" => url, "service" => svc_key],
        );
        Ok(url)
    }
}

/// `cr.Status.HostedClusterRef` (a nil pointer dereference in Go when unset).
fn hosted_cluster_ref(
    cr: &provisioningv1alpha1::DPFHCPProvisioner,
) -> anyhow::Result<&apis::k8s::core::v1::ObjectReference> {
    cr.status.hosted_cluster_ref.as_ref().ok_or_else(nil_deref)
}

/// `CreationTimestamp` as an orderable key (unset = Go's zero time, before everything).
fn creation_timestamp(s: &Secret) -> Option<ctrlrt::k8s_openapi::jiff::Timestamp> {
    s.metadata.creation_timestamp.as_ref().map(|t| t.0)
}

/// ignitionContainsSSHKey reports whether an ignition payload includes the expected public key.
pub(crate) fn ignition_contains_ssh_key(
    ignition_bytes: &[u8],
    expected_ssh_key: &[u8],
) -> anyhow::Result<bool> {
    let config = ignition::unmarshal_config(ignition_bytes).map_err(|e| {
        wrapf!(
            e,
            "failed to decode ignition while checking SSH-key sync: {}",
            e
        )
    })?;

    let expected = gocompat::unicode::trim_space(expected_ssh_key);
    for user in ignition::users(&config) {
        for authorized_key in user.ssh_authorized_keys.as_deref().unwrap_or_default() {
            if gocompat::unicode::trim_space(authorized_key.as_bytes()) == expected {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
