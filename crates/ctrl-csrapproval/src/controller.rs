//! Port of `internal/controller/csrapproval/controller.go`.

use std::sync::Arc;

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use async_trait::async_trait;
use ctrlrt::error::{Result, is_not_found};
use ctrlrt::{
    Builder, Client, Ctx, ManagerHandle, Object, ReconcileResult, Reconciler, Request, kv,
};

use crate::csrapproval::{CSR_POLLING_INTERVAL, CSRApprover};

/// The controller name (`Named("csrapproval")`).
pub const CONTROLLER_NAME: &str = "csrapproval";

/// CSRApprovalReconciler is a dedicated controller for CSR auto-approval.
/// This controller runs independently from the main DPFHCPProvisioner controller and
/// continuously polls the hosted cluster for pending CSRs.
///
/// (Go's `Scheme` field is unused by the reconciler and has no Rust counterpart.)
pub struct CSRApprovalReconciler {
    pub client: Client,
    pub approver: Option<Arc<CSRApprover>>,
}

// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpprovisioners,verbs=get;list;watch
// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpprovisioners/status,verbs=get;update;patch
// +kubebuilder:rbac:groups="",resources=events,verbs=create;patch
// +kubebuilder:rbac:groups="",resources=secrets,verbs=get;list;watch
// +kubebuilder:rbac:groups=provisioning.dpu.nvidia.com,resources=dpus,verbs=get;list;watch

#[async_trait]
impl Reconciler for CSRApprovalReconciler {
    /// Reconcile handles CSR auto-approval for a DPFHCPProvisioner. This controller is solely
    /// responsible for:
    /// 1. Connecting to the hosted cluster
    /// 2. Polling for pending CSRs
    /// 3. Validating and approving CSRs for DPU worker nodes
    /// 4. Updating the CSRAutoApprovalActive condition
    async fn reconcile(&self, ctx: &Ctx, req: Request) -> Result<ReconcileResult> {
        let log = &ctx.log;
        log.info(
            "CSR Approval Controller reconciling",
            kv!["namespace" => req.namespace, "name" => req.name],
        );

        // Fetch the DPFHCPProvisioner CR
        let mut cr: provisioningv1alpha1::DPFHCPProvisioner = match self.client.get(&req).await {
            Ok(cr) => cr,
            // CR not found - likely deleted
            Err(err) if is_not_found(&err) => return Ok(ReconcileResult::done()),
            Err(err) => return Err(err),
        };

        // Handle deletion - stop CSR watch
        if cr.is_being_deleted() {
            log.info(
                "DPFHCPProvisioner is being deleted, stopping CSR watch",
                kv![],
            );
            if let Some(approver) = &self.approver {
                approver.stop_csr_watch(ctx, &cr);
            }
            return Ok(ReconcileResult::done());
        }

        // Only process if kubeconfig is available.
        // CSR approval requires kubeconfig to connect to hosted cluster.
        if cr.status.kube_config_secret_ref.is_none() {
            log.v(1)
                .info("Kubeconfig not available yet, skipping CSR approval", kv![]);
            return Ok(ReconcileResult::done());
        }

        // Process CSRs - this is the ONLY thing this controller does. The CSRApprover will:
        // 1. Connect to hosted cluster using kubeconfig
        // 2. List and filter pending CSRs
        // 3. Validate CSRs against DPU objects
        // 4. Approve valid CSRs
        // 5. Update CSRAutoApprovalActive condition
        let Some(approver) = &self.approver else {
            log.v(1)
                .info("Approver not initialized, skipping CSR approval", kv![]);
            return Ok(ReconcileResult::done());
        };
        let result = match approver.process_csrs(ctx, &mut cr).await {
            Ok(r) => r,
            Err(err) => {
                log.error(&err, "Failed to process CSRs", kv![]);
                // Continue polling even on error - CSR approval is best-effort.
                // The next reconciliation will retry.
                return Ok(ReconcileResult::requeue_after(CSR_POLLING_INTERVAL));
            }
        };

        log.v(1).info(
            "CSR approval reconciliation complete",
            kv!["requeueAfter" => ctrlrt_cmd::godur::format(result.requeue_after)],
        );
        Ok(result)
    }
}

impl CSRApprovalReconciler {
    /// SetupWithManager sets up the controller with the Manager.
    pub fn setup_with_manager(self: Arc<Self>, mgr: &dyn ManagerHandle) -> Result<()> {
        let spec = Builder::new()
            .for_type::<provisioningv1alpha1::DPFHCPProvisioner>(vec![])
            .named(CONTROLLER_NAME)
            .build(self)?;
        mgr.add_controller(spec)
    }
}
