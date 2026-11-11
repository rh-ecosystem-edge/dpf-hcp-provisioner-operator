//! Port of `internal/controller/metallb/cleanup_handler.go`.

use std::time::Duration;

use apis::metallb::v1beta1 as metallbv1beta1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::error::{is_no_match, is_not_found};
use ctrlrt::{
    Client, Ctx, Object, ObjectKey, ReconcileResult, Recorder, RecorderExt, Result, kv, wrapf,
};

/// CleanupHandler handles cleanup of MetalLB resources (IPAddressPool and L2Advertisement)
/// when a DPFHCPProvisioner CR is deleted.
///
/// This handler is responsible for:
/// 1. Deleting IPAddressPool created for this DPFHCPProvisioner
/// 2. Deleting L2Advertisement created for this DPFHCPProvisioner
/// 3. Waiting for resources to be fully deleted before completing
///
/// Implements Go's `finalizer.CleanupHandler` interface ([`ctrl_finalizer::CleanupHandler`]);
/// the same operations are also available as inherent methods.
#[derive(Clone)]
pub struct CleanupHandler {
    client: Client,
    recorder: Recorder,
}

impl CleanupHandler {
    /// NewCleanupHandler creates a new MetalLB cleanup handler
    pub fn new(client: Client, recorder: Recorder) -> Self {
        Self { client, recorder }
    }

    /// Name returns the handler name for logging
    pub fn name(&self) -> &'static str {
        "metallb"
    }

    /// Cleanup deletes MetalLB resources (IPAddressPool and L2Advertisement) created for this DPFHCPProvisioner.
    /// This is called during finalizer cleanup when the DPFHCPProvisioner is deleted.
    ///
    /// The cleanup process:
    /// 1. Delete IPAddressPool if it exists
    /// 2. Wait for IPAddressPool to be fully deleted
    /// 3. Delete L2Advertisement if it exists
    /// 4. Wait for L2Advertisement to be fully deleted
    pub async fn cleanup(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<ReconcileResult> {
        let log = ctx.log.with_values(kv![
            "handler" => self.name(),
            common::DPFHCP_PROVISIONER_NAME => format!("{}/{}", cr.namespace(), cr.name())
        ]);

        log.info("Cleaning up MetalLB resources", kv![]);

        // Step 1: Delete IPAddressPool
        match self
            .client
            .get::<metallbv1beta1::IPAddressPool>(&ObjectKey::new(
                common::OPENSHIFT_OPERATORS_NAMESPACE,
                cr.name(),
            ))
            .await
        {
            Err(err) => {
                if is_not_found(&err) {
                    log.v(1)
                        .info("IPAddressPool already deleted or never existed", kv![]);
                } else if is_no_match(&err) {
                    log.v(1).info(
                        "IPAddressPool CRD not installed, treating as deleted",
                        kv![],
                    );
                } else {
                    log.error(&err, "Failed to get IPAddressPool", kv![]);
                    return Err(wrapf!(err, "getting IPAddressPool: {}", err));
                }
            }
            Ok(pool) => {
                // Verify ownership via labels before deleting
                if !common::is_owned_by_provisioner(
                    pool.metadata.labels.as_ref(),
                    cr.name(),
                    cr.namespace(),
                ) {
                    log.info(
                        "Skipping IPAddressPool deletion - not owned by this DPFHCPProvisioner",
                        kv!["name" => pool.name(), "namespace" => pool.namespace()],
                    );
                } else {
                    // Resource still exists and is owned by us, initiate deletion
                    log.info(
                        "Deleting IPAddressPool",
                        kv!["name" => pool.name(), "namespace" => pool.namespace()],
                    );

                    if let Err(err) = self.client.delete(&pool).await
                        && !is_not_found(&err)
                    {
                        log.error(&err, "Failed to delete IPAddressPool", kv![]);
                        return Err(wrapf!(err, "deleting IPAddressPool: {}", err));
                    }

                    log.v(1)
                        .info("Waiting for IPAddressPool deletion to complete", kv![]);
                    return Ok(ReconcileResult::requeue_after(Duration::from_secs(10)));
                }
            }
        }

        // Step 2: Delete L2Advertisement
        match self
            .client
            .get::<metallbv1beta1::L2Advertisement>(&ObjectKey::new(
                common::OPENSHIFT_OPERATORS_NAMESPACE,
                format!("advertise-{}", cr.name()),
            ))
            .await
        {
            Err(err) => {
                if is_not_found(&err) {
                    log.v(1)
                        .info("L2Advertisement already deleted or never existed", kv![]);
                } else if is_no_match(&err) {
                    log.v(1).info(
                        "L2Advertisement CRD not installed, treating as deleted",
                        kv![],
                    );
                } else {
                    log.error(&err, "Failed to get L2Advertisement", kv![]);
                    return Err(wrapf!(err, "getting L2Advertisement: {}", err));
                }
            }
            Ok(advert) => {
                // Verify ownership via labels before deleting
                if !common::is_owned_by_provisioner(
                    advert.metadata.labels.as_ref(),
                    cr.name(),
                    cr.namespace(),
                ) {
                    log.info(
                        "Skipping L2Advertisement deletion - not owned by this DPFHCPProvisioner",
                        kv!["name" => advert.name(), "namespace" => advert.namespace()],
                    );
                } else {
                    log.info(
                        "Deleting L2Advertisement",
                        kv!["name" => advert.name(), "namespace" => advert.namespace()],
                    );

                    if let Err(err) = self.client.delete(&advert).await
                        && !is_not_found(&err)
                    {
                        log.error(&err, "Failed to delete L2Advertisement", kv![]);
                        return Err(wrapf!(err, "deleting L2Advertisement: {}", err));
                    }

                    log.v(1)
                        .info("Waiting for L2Advertisement deletion to complete", kv![]);
                    return Ok(ReconcileResult::requeue_after(Duration::from_secs(10)));
                }
            }
        }

        // All resources cleaned up successfully
        log.info("MetalLB cleanup completed successfully", kv![]);
        self.recorder.event(
            cr,
            "Normal",
            "MetalLBCleanupComplete",
            "MetalLB resources cleaned up successfully",
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
