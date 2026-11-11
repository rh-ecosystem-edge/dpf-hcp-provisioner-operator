//! Port of internal/controller/secrets/validator.go

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::k8s_openapi::api::core::v1::Secret;
use ctrlrt::meta;
use ctrlrt::recorder::{EVENT_TYPE_NORMAL, EVENT_TYPE_WARNING};
use ctrlrt::{Client, Ctx, ObjectKey, ReconcileResult, Recorder, RecorderExt, kv};

// Event reasons
pub const REASON_SECRETS_VALID: &str = "SecretsValid";
pub const REASON_SSH_KEY_SECRET_MISSING: &str = "SSHKeySecretMissing";
pub const REASON_SSH_KEY_SECRET_INVALID: &str = "SSHKeySecretInvalid";
pub const REASON_PULL_SECRET_MISSING: &str = "PullSecretMissing";
pub const REASON_PULL_SECRET_INVALID: &str = "PullSecretInvalid";
pub const REASON_SECRETS_ACCESS_DENIED: &str = "SecretsAccessDenied";
pub const REASON_SECRETS_RECOVERED: &str = "SecretsRecovered";

// Secret keys
pub const SSH_PUBLIC_KEY_SECRET_KEY: &str = "id_rsa.pub";
pub const PULL_SECRET_KEY: &str = ".dockerconfigjson";

/// Validator validates secret references and updates status accordingly
pub struct Validator {
    client: Client,
    recorder: Recorder,
}

fn has_key(secret: &Secret, key: &str) -> bool {
    secret.data.as_ref().is_some_and(|d| d.contains_key(key))
}

/// `metav1.Condition{..., LastTransitionTime: metav1.Now(), ObservedGeneration: cr.Generation}`
fn condition(
    type_: &str,
    status: &str,
    reason: &str,
    message: &str,
    generation: i64,
) -> ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition {
    let mut c = meta::condition_gen(type_, status, reason, message, generation);
    c.last_transition_time = meta::now();
    c
}

impl Validator {
    /// NewValidator creates a new secrets validator
    pub fn new(c: Client, recorder: Recorder) -> Self {
        Self {
            client: c,
            recorder,
        }
    }

    /// ValidateSecrets validates that both SSH key and pull secret exist and have required keys
    ///
    /// Transient errors return Go's `ctrl.Result{Requeue: true}, err` via `ctrlrt::with_result`.
    pub async fn validate_secrets(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = ctx.log.with_values(kv!["feature" => "secrets-validation"]);
        let namespace = ctrlrt::Object::namespace(cr).to_string();

        log.v(1).info(
            "Validating secrets",
            kv![
                "sshKeySecretRef" => cr.spec.ssh_key_secret_ref.name,
                "pullSecretRef" => cr.spec.pull_secret_ref.name,
                "namespace" => namespace
            ],
        );

        // Validate SSH key secret
        let ssh_secret: Secret = match self
            .client
            .get(&ObjectKey::new(
                &namespace,
                &cr.spec.ssh_key_secret_ref.name,
            ))
            .await
        {
            Ok(s) => s,
            Err(err) => {
                if ctrlrt::error::is_not_found(&err) {
                    return self.handle_ssh_key_secret_missing(ctx, cr).await;
                }
                if ctrlrt::error::is_forbidden(&err) {
                    return self
                        .handle_secrets_access_denied(ctx, cr, "SSH key secret", &err)
                        .await;
                }
                // Transient error - retry
                log.v(1).info(
                    "Transient error fetching SSH key secret, will retry",
                    kv!["error" => err.to_string()],
                );
                return Err(ctrlrt::with_result(ReconcileResult::requeue(), err));
            }
        };

        // Validate SSH secret has required key
        if !has_key(&ssh_secret, SSH_PUBLIC_KEY_SECRET_KEY) {
            return self.handle_ssh_key_secret_invalid(ctx, cr).await;
        }

        // Validate pull secret
        let pull_secret: Secret = match self
            .client
            .get(&ObjectKey::new(&namespace, &cr.spec.pull_secret_ref.name))
            .await
        {
            Ok(s) => s,
            Err(err) => {
                if ctrlrt::error::is_not_found(&err) {
                    return self.handle_pull_secret_missing(ctx, cr).await;
                }
                if ctrlrt::error::is_forbidden(&err) {
                    return self
                        .handle_secrets_access_denied(ctx, cr, "pull secret", &err)
                        .await;
                }
                // Transient error - retry
                log.v(1).info(
                    "Transient error fetching pull secret, will retry",
                    kv!["error" => err.to_string()],
                );
                return Err(ctrlrt::with_result(ReconcileResult::requeue(), err));
            }
        };

        // Validate pull secret has required key
        if !has_key(&pull_secret, PULL_SECRET_KEY) {
            return self.handle_pull_secret_invalid(ctx, cr).await;
        }

        // Both secrets exist and are valid
        self.handle_secrets_valid(ctx, cr).await
    }

    /// Shared tail of every handler: `meta.SetStatusCondition` (+ event and log when changed)
    /// followed by `Status().Update`.
    #[allow(clippy::too_many_arguments)]
    async fn set_condition_and_update(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        status: &str,
        event_type: &str,
        reason: &str,
        message: String,
        on_changed: impl FnOnce(&ctrlrt::Logger, &provisioningv1alpha1::DPFHCPProvisioner),
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = ctx.log.with_values(kv!["feature" => "secrets-validation"]);

        // Set condition and check if it changed
        let cond = condition(
            provisioningv1alpha1::SECRETS_VALID,
            status,
            reason,
            &message,
            ctrlrt::Object::generation(cr),
        );

        // Emit event only if condition changed
        if meta::set_status_condition(&mut cr.status.conditions, cond) {
            self.recorder.event(cr, event_type, reason, &message);
            on_changed(&log, cr);
        }

        // Update status
        if let Err(err) = self.client.update_status(cr).await {
            log.error(&err, "Failed to update status", kv![]);
            return Err(err);
        }

        Ok(ReconcileResult::default())
    }

    /// handleSSHKeySecretMissing handles the case when SSH key secret is not found
    async fn handle_ssh_key_secret_missing(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let message = format!(
            "SSH key secret '{}' not found in namespace '{}'",
            cr.spec.ssh_key_secret_ref.name,
            ctrlrt::Object::namespace(cr)
        );
        // Do NOT requeue - permanent error requiring user to create secret
        self.set_condition_and_update(
            ctx,
            cr,
            meta::CONDITION_FALSE,
            EVENT_TYPE_WARNING,
            REASON_SSH_KEY_SECRET_MISSING,
            message,
            |log, cr| {
                log.info(
                    "SSH key secret not found",
                    kv![
                        "secretName" => cr.spec.ssh_key_secret_ref.name,
                        "namespace" => ctrlrt::Object::namespace(cr)
                    ],
                )
            },
        )
        .await
    }

    /// handleSSHKeySecretInvalid handles the case when SSH key secret is missing required key
    async fn handle_ssh_key_secret_invalid(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let message = format!(
            "SSH key secret '{}' is missing required key '{}'",
            cr.spec.ssh_key_secret_ref.name, SSH_PUBLIC_KEY_SECRET_KEY
        );
        // Do NOT requeue - permanent error requiring user to fix secret
        self.set_condition_and_update(
            ctx,
            cr,
            meta::CONDITION_FALSE,
            EVENT_TYPE_WARNING,
            REASON_SSH_KEY_SECRET_INVALID,
            message,
            |log, cr| {
                log.info(
                    "SSH key secret is invalid",
                    kv![
                        "secretName" => cr.spec.ssh_key_secret_ref.name,
                        "missingKey" => SSH_PUBLIC_KEY_SECRET_KEY
                    ],
                )
            },
        )
        .await
    }

    /// handlePullSecretMissing handles the case when pull secret is not found
    async fn handle_pull_secret_missing(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let message = format!(
            "Pull secret '{}' not found in namespace '{}'",
            cr.spec.pull_secret_ref.name,
            ctrlrt::Object::namespace(cr)
        );
        // Do NOT requeue - permanent error requiring user to create secret
        self.set_condition_and_update(
            ctx,
            cr,
            meta::CONDITION_FALSE,
            EVENT_TYPE_WARNING,
            REASON_PULL_SECRET_MISSING,
            message,
            |log, cr| {
                log.info(
                    "Pull secret not found",
                    kv![
                        "secretName" => cr.spec.pull_secret_ref.name,
                        "namespace" => ctrlrt::Object::namespace(cr)
                    ],
                )
            },
        )
        .await
    }

    /// handlePullSecretInvalid handles the case when pull secret is missing required key
    async fn handle_pull_secret_invalid(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let message = format!(
            "Pull secret '{}' is missing required key '{}'",
            cr.spec.pull_secret_ref.name, PULL_SECRET_KEY
        );
        // Do NOT requeue - permanent error requiring user to fix secret
        self.set_condition_and_update(
            ctx,
            cr,
            meta::CONDITION_FALSE,
            EVENT_TYPE_WARNING,
            REASON_PULL_SECRET_INVALID,
            message,
            |log, cr| {
                log.info(
                    "Pull secret is invalid",
                    kv![
                        "secretName" => cr.spec.pull_secret_ref.name,
                        "missingKey" => PULL_SECRET_KEY
                    ],
                )
            },
        )
        .await
    }

    /// handleSecretsAccessDenied handles RBAC permission errors
    async fn handle_secrets_access_denied(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        secret_type: &str,
        err: &ctrlrt::Error,
    ) -> ctrlrt::Result<ReconcileResult> {
        let message = format!(
            "Operator lacks RBAC permissions to access {} in namespace '{}': {}",
            secret_type,
            ctrlrt::Object::namespace(cr),
            err
        );
        // Do NOT requeue - permanent error requiring admin to fix RBAC
        self.set_condition_and_update(
            ctx,
            cr,
            meta::CONDITION_FALSE,
            EVENT_TYPE_WARNING,
            REASON_SECRETS_ACCESS_DENIED,
            message,
            |log, cr| {
                log.error(
                    err,
                    "RBAC permission denied for secret",
                    kv![
                        "secretType" => secret_type,
                        "namespace" => ctrlrt::Object::namespace(cr)
                    ],
                )
            },
        )
        .await
    }

    /// handleSecretsValid handles the case when both secrets exist and are valid
    async fn handle_secrets_valid(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let message = format!(
            "Both SSH key secret '{}' and pull secret '{}' are valid",
            cr.spec.ssh_key_secret_ref.name, cr.spec.pull_secret_ref.name
        );
        // Emit event only if condition changed (e.g., recovered from invalid state)
        // Success - continue with reconciliation
        self.set_condition_and_update(
            ctx,
            cr,
            meta::CONDITION_TRUE,
            EVENT_TYPE_NORMAL,
            REASON_SECRETS_VALID,
            message,
            |log, cr| {
                log.info(
                    "Secrets validated",
                    kv![
                        "sshKeySecret" => cr.spec.ssh_key_secret_ref.name,
                        "pullSecret" => cr.spec.pull_secret_ref.name
                    ],
                )
            },
        )
        .await
    }
}
