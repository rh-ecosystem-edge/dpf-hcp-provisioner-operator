//! Port of internal/controller/finalizer/handler.go

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::{Ctx, ReconcileResult};

/// CleanupHandler defines the interface for cleanup handlers that are executed
/// during finalizer cleanup when a DPFHCPProvisioner CR is deleted.
///
/// Handlers are executed in the order they are registered with the Manager.
/// Each handler should clean up resources it created and return an error if
/// cleanup should be retried.
#[async_trait::async_trait]
pub trait CleanupHandler: Send + Sync + 'static {
    /// Name returns the handler name for logging and identification purposes.
    /// Should be a short, descriptive name like "hostedcluster" or "kubeconfig-injection".
    fn name(&self) -> String;

    /// Cleanup performs the cleanup logic for this handler.
    /// It should clean up all resources created by the corresponding feature.
    ///
    /// Returns:
    /// - `Ok(ReconcileResult::default())` if cleanup succeeded or resources are already gone
    /// - `Ok(ReconcileResult::requeue_after(d))` if cleanup is in progress and should be retried after d
    /// - `Err(err)` if cleanup failed unexpectedly and should be retried with backoff
    ///
    /// The handler should be idempotent - calling Cleanup multiple times should
    /// be safe and result in the same final state.
    async fn cleanup(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult>;
}
