//! Port of `internal/controller/kubeconfiginjection/cleanup_handler.go`.

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::error::is_not_found;
use ctrlrt::{
    Client, Ctx, ListOptions, Object, ReconcileResult, Recorder, RecorderExt, Result, kv, wrapf,
};
use k8s_openapi::api::core::v1::Secret;

/// CleanupHandler handles cleanup of kubeconfig secrets created in DPUCluster namespace
/// when a DPFHCPProvisioner CR is deleted.
///
/// This handler is responsible for:
/// 1. Finding kubeconfig secrets by labels (owned by this DPFHCPProvisioner)
/// 2. Deleting all found kubeconfig secrets
///
/// Implements Go's `finalizer.CleanupHandler` interface ([`ctrl_finalizer::CleanupHandler`]);
/// the same operations are also available as inherent methods.
#[derive(Clone)]
pub struct CleanupHandler {
    client: Client,
    recorder: Recorder,
}

impl CleanupHandler {
    /// NewCleanupHandler creates a new kubeconfig cleanup handler
    pub fn new(client: Client, recorder: Recorder) -> Self {
        Self { client, recorder }
    }

    /// Name returns the handler name for logging
    pub fn name(&self) -> &'static str {
        "kubeconfig-injection"
    }

    /// Cleanup deletes the kubeconfig secret created in DPUCluster namespace.
    /// This is called during finalizer cleanup when the DPFHCPProvisioner is deleted.
    ///
    /// The cleanup process:
    /// 1. List all secrets with labels matching this DPFHCPProvisioner
    /// 2. Delete each found secret
    ///
    /// Labels used for finding secrets (defined in internal/common/constants.go):
    /// - common.LabelDPFHCPProvisionerName: <provisioner-name>
    /// - common.LabelDPFHCPProvisionerNamespace: <provisioner-namespace>
    ///
    /// Returns:
    /// - Ok if cleanup succeeded or secrets are already gone
    /// - error if cleanup failed and should be retried
    pub async fn cleanup(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<ReconcileResult> {
        let log = ctx.log.with_values(kv![
            "handler" => self.name(),
            common::DPFHCP_PROVISIONER_NAME => format!("{}/{}", cr.namespace(), cr.name())
        ]);

        log.info("Cleaning up kubeconfig secrets", kv![]);

        // Find kubeconfig secret by labels
        let secret_list = match self
            .client
            .list::<Secret>(&ListOptions::new().matching_labels([
                (common::LABEL_DPFHCP_PROVISIONER_NAME, cr.name()),
                (common::LABEL_DPFHCP_PROVISIONER_NAMESPACE, cr.namespace()),
            ]))
            .await
        {
            Ok(v) => v,
            Err(err) => {
                log.error(&err, "Failed to list kubeconfig secrets", kv![]);
                return Err(wrapf!(err, "failed to list kubeconfig secrets: {}", err));
            }
        };

        if secret_list.is_empty() {
            log.info("No kubeconfig secrets found, nothing to clean up", kv![]);
            return Ok(ReconcileResult::default());
        }

        // Delete found secrets (ownership verified via label selector above)
        let mut deleted_count = 0;
        for secret in &secret_list {
            log.info(
                "Deleting kubeconfig secret",
                kv!["secretName" => secret.name(), "namespace" => secret.namespace()],
            );

            if let Err(err) = self.client.delete(secret).await {
                if is_not_found(&err) {
                    // Already deleted (race condition)
                    log.v(1).info(
                        "Kubeconfig secret already deleted",
                        kv!["secretName" => secret.name(), "namespace" => secret.namespace()],
                    );
                    continue;
                }
                log.error(
                    &err,
                    "Failed to delete kubeconfig secret",
                    kv!["secretName" => secret.name(), "namespace" => secret.namespace()],
                );
                return Err(wrapf!(
                    err,
                    "failed to delete kubeconfig secret {}/{}: {}",
                    secret.namespace(),
                    secret.name(),
                    err
                ));
            }

            deleted_count += 1;
            log.info(
                "Kubeconfig secret deleted successfully",
                kv!["secretName" => secret.name(), "namespace" => secret.namespace()],
            );
        }

        log.info(
            "Kubeconfig cleanup completed successfully",
            kv!["deletedCount" => deleted_count],
        );
        self.recorder.event(
            cr,
            "Normal",
            "KubeconfigCleanupSucceeded",
            &format!("Deleted {deleted_count} kubeconfig secret(s)"),
        );

        Ok(ReconcileResult::default())
    }
}

#[async_trait::async_trait]
impl ctrl_finalizer::CleanupHandler for CleanupHandler {
    fn name(&self) -> String {
        CleanupHandler::name(self).to_string()
    }

    async fn cleanup(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<ReconcileResult> {
        CleanupHandler::cleanup(self, ctx, cr).await
    }
}
