//! Port of internal/controller/finalizer/manager.go

use std::sync::Arc;

use apis::go::Duration as GoDuration;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::{Client, Ctx, ReconcileResult, Recorder, RecorderExt, kv};

use crate::handler::CleanupHandler;

/// Manager manages the finalizer cleanup process for DPFHCPProvisioner resources.
/// It maintains a list of cleanup handlers that are executed in order during
/// the finalizer cleanup phase.
///
/// The Manager uses a single finalizer (dpfhcpprovisioner.provisioning.dpu.hcp.io/finalizer)
/// and executes all registered handlers sequentially. This ensures:
/// - Explicit cleanup ordering (handlers execute in registration order)
/// - Atomicity (all cleanup succeeds or all cleanup is retried)
/// - Single blocking point (simpler debugging than multiple finalizers)
pub struct Manager {
    #[allow(dead_code)]
    client: Client,
    recorder: Recorder,
    handlers: Vec<Arc<dyn CleanupHandler>>,
}

impl Manager {
    /// NewManager creates a new finalizer Manager with no handlers registered.
    /// Handlers must be registered using RegisterHandler before the Manager can perform cleanup.
    pub fn new(c: Client, recorder: Recorder) -> Self {
        Self {
            client: c,
            recorder,
            handlers: Vec::new(),
        }
    }

    /// RegisterHandler adds a cleanup handler to the manager.
    /// Handlers are executed in the order they are registered.
    ///
    /// IMPORTANT: Order matters! Register dependent resources before their dependencies.
    /// Example: Register kubeconfig cleanup before HostedCluster cleanup, since kubeconfig
    /// is a dependent resource.
    pub fn register_handler<H: CleanupHandler>(&mut self, handler: H) {
        self.handlers.push(Arc::new(handler));
    }

    /// [`Self::register_handler`] for an already shared handler.
    pub fn register_handler_arc(&mut self, handler: Arc<dyn CleanupHandler>) {
        self.handlers.push(handler);
    }

    /// Number of registered handlers (`len(m.handlers)`).
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }

    /// HandleFinalizerCleanup executes all registered cleanup handlers in order.
    /// It is called when a DPFHCPProvisioner CR is deleted and the finalizer needs to run.
    ///
    /// Returns:
    /// - `ReconcileResult::default()`: Cleanup completed, finalizer can be removed
    /// - `ReconcileResult{requeue_after: d}`: Cleanup in progress, retry needed
    /// - error: Cleanup failed, will be retried with exponential backoff
    pub async fn handle_finalizer_cleanup(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;
        log.info(
            "Starting finalizer cleanup with registered handlers",
            kv!["handlerCount" => self.handlers.len()],
        );

        // Execute all handlers in registration order
        for (i, handler) in self.handlers.iter().enumerate() {
            let handler_log = log.with_values(kv![
                "handler" => handler.name(),
                "index" => i,
                "total" => self.handlers.len()
            ]);
            handler_log.info("Executing cleanup handler", kv![]);

            // Execute handler cleanup
            let result = match handler.cleanup(ctx, cr).await {
                Ok(r) => r,
                Err(err) => {
                    handler_log.error(&err, "Cleanup handler failed", kv![]);
                    self.recorder.event(
                        cr,
                        "Warning",
                        "CleanupHandlerFailed",
                        &format!("Cleanup handler '{}' failed: {}", handler.name(), err),
                    );

                    // Return error to trigger requeue with exponential backoff
                    return Err(err);
                }
            };

            // Handler signalled it needs more time (e.g. waiting for a resource to be deleted)
            if !result.requeue_after.is_zero() {
                handler_log.info(
                    "Cleanup handler still in progress, will requeue",
                    kv!["requeueAfter" => go_duration(result.requeue_after)],
                );
                return Ok(result);
            }

            handler_log.info("Cleanup handler completed successfully", kv![]);
        }

        // All handlers succeeded
        log.info("All cleanup handlers completed successfully", kv![]);
        self.recorder.event(
            cr,
            "Normal",
            "CleanupSucceeded",
            "All resources cleaned up successfully",
        );

        Ok(ReconcileResult::default())
    }
}

/// `time.Duration` as rendered by zap's development encoder (`Duration.String()`).
fn go_duration(d: std::time::Duration) -> String {
    GoDuration(d.as_nanos().min(i64::MAX as u128) as i64).to_string()
}
