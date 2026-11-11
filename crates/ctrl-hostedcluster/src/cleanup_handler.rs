//! Port of internal/controller/hostedcluster/cleanup_handler.go

use std::time::Duration;

use apis::go::Duration as GoDuration;
use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::k8s_openapi::api::core::v1::Secret;
use ctrlrt::k8s_openapi::jiff::Timestamp;
use ctrlrt::meta;
use ctrlrt::{Client, Ctx, Object, ObjectKey, ReconcileResult, Recorder, RecorderExt, kv, wrapf};

/// DeletionTimeout is the maximum time to wait for HostedCluster deletion (30 minutes)
pub const DELETION_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// DeletionRequeueInterval is the interval between deletion status checks (10 seconds)
pub const DELETION_REQUEUE_INTERVAL: Duration = Duration::from_secs(10);

/// CleanupHandler handles cleanup of HostedCluster, NodePool, and related secrets
/// when a DPFHCPProvisioner CR is deleted.
///
/// This handler is responsible for:
/// 1. Deleting HostedCluster CR and waiting for full deletion
/// 2. Deleting NodePool CR and waiting for full deletion
/// 3. Deleting copied/generated secrets (pull-secret, ssh-key, etcd-encryption-key)
#[derive(Clone)]
pub struct CleanupHandler {
    client: Client,
    recorder: Recorder,
}

impl CleanupHandler {
    /// NewCleanupHandler creates a new HostedCluster cleanup handler
    pub fn new(c: Client, recorder: Recorder) -> Self {
        Self {
            client: c,
            recorder,
        }
    }

    /// Name returns the handler name for logging
    pub fn name(&self) -> &'static str {
        "hostedcluster"
    }

    /// Cleanup performs the cleanup logic for HostedCluster resources.
    /// This includes:
    /// 1. Deleting HostedCluster CR in the same namespace as DPFHCPProvisioner
    /// 2. Waiting for HostedCluster to be fully deleted
    /// 3. Deleting NodePool CR in the same namespace as DPFHCPProvisioner
    /// 4. Waiting for NodePool to be fully deleted
    /// 5. Deleting copied/generated secrets
    ///
    /// Returns:
    /// - nil if cleanup succeeded or resources are already gone
    /// - error if cleanup failed and should be retried
    ///
    /// Note: This handler does NOT enforce timeout. The finalizer manager
    /// is responsible for timeout handling if needed.
    pub async fn cleanup(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = ctx.log.with_values(kv![
            "handler" => self.name(),
            "dpfhcpprovisioner" => format!("{}/{}", cr.namespace(), cr.name())
        ]);

        // Step 1: Delete HostedCluster and wait for it to be fully removed
        log.info("Deleting HostedCluster", kv![]);
        let hc_deleted = match self
            .delete_resource::<hyperv1::HostedCluster>(ctx, cr, "HostedCluster")
            .await
        {
            Ok(d) => d,
            Err(err) => {
                log.error(&err, "Failed to delete HostedCluster", kv![]);
                return Err(err);
            }
        };

        if !hc_deleted {
            log.info("HostedCluster deletion in progress, will retry", kv![]);
            return Ok(ReconcileResult::requeue_after(DELETION_REQUEUE_INTERVAL));
        }

        // Step 2: Delete NodePool and wait for it to be fully removed
        log.info("HostedCluster deleted, deleting NodePool", kv![]);
        let np_deleted = match self
            .delete_resource::<hyperv1::NodePool>(ctx, cr, "NodePool")
            .await
        {
            Ok(d) => d,
            Err(err) => {
                log.error(&err, "Failed to delete NodePool", kv![]);
                return Err(err);
            }
        };

        if !np_deleted {
            log.info("NodePool deletion in progress, will retry", kv![]);
            return Ok(ReconcileResult::requeue_after(DELETION_REQUEUE_INTERVAL));
        }

        // Step 3: Delete secrets
        log.info("NodePool deleted, deleting secrets", kv![]);
        if let Err(err) = self.delete_secrets(ctx, cr).await {
            log.error(&err, "Failed to delete secrets", kv![]);
            return Err(err);
        }

        log.info("HostedCluster cleanup completed successfully", kv![]);
        self.recorder.event(
            cr,
            "Normal",
            "HostedClusterCleanupSucceeded",
            "HostedCluster, NodePool, and secrets deleted successfully",
        );

        Ok(ReconcileResult::default())
    }

    /// deleteResource is a generic function to delete a Kubernetes resource and wait for deletion
    /// Returns true when resource is fully deleted (NotFound), false if still exists
    async fn delete_resource<K: Object>(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
        resource_kind: &str,
    ) -> ctrlrt::Result<bool> {
        let log = &ctx.log;

        // Construct the resource key from CR name (HostedCluster and NodePool use same name as CR)
        let key = ObjectKey::new(cr.namespace(), cr.name());

        let obj: K = match self.client.get(&key).await {
            Ok(o) => o,
            Err(err) => {
                if ctrlrt::error::is_not_found(&err) {
                    // Resource is fully deleted
                    log.info(
                        &format!("{resource_kind} deleted successfully"),
                        kv![resource_kind => key.name, "namespace" => key.namespace],
                    );
                    return Ok(true);
                }
                // Handle "no matches for kind" error (CRD not installed) as if resource doesn't exist
                if ctrlrt::error::is_no_match(&err) {
                    log.v(1).info(
                        &format!("{resource_kind} CRD not installed, treating as deleted"),
                        kv![resource_kind => key.name, "namespace" => key.namespace],
                    );
                    return Ok(true);
                }
                return Err(wrapf!(err, "failed to get {}: {}", resource_kind, err));
            }
        };

        // Verify ownership before deleting
        if !meta::is_controlled_by(&obj, cr) {
            log.info(
                &format!("Skipping {resource_kind} deletion - not owned by this DPFHCPProvisioner"),
                kv![resource_kind => key.name, "namespace" => key.namespace],
            );
            return Ok(true);
        }

        // Resource still exists and is owned by us
        match &obj.object_meta().deletion_timestamp {
            None => {
                // Resource not yet marked for deletion, delete it now
                log.info(
                    &format!("Deleting {resource_kind}"),
                    kv![resource_kind => key.name, "namespace" => key.namespace],
                );

                if let Err(err) = self.client.delete(&obj).await {
                    if ctrlrt::error::is_not_found(&err) {
                        // Already deleted (race condition)
                        return Ok(true);
                    }
                    return Err(wrapf!(err, "failed to delete {}: {}", resource_kind, err));
                }
                log.info(
                    &format!("{resource_kind} deletion initiated"),
                    kv![resource_kind => key.name, "namespace" => key.namespace],
                );
            }
            Some(deletion_timestamp) => {
                // Resource is marked for deletion but still exists (finalizers running)
                let elapsed_deletion = Timestamp::now()
                    .duration_since(deletion_timestamp.0)
                    .as_nanos()
                    .clamp(i64::MIN as i128, i64::MAX as i128)
                    as i64;
                log.v(1).info(
                    &format!("{resource_kind} deletion in progress (finalizers running)"),
                    kv![
                        resource_kind => key.name,
                        "namespace" => key.namespace,
                        "deletionElapsed" => GoDuration(elapsed_deletion).to_string()
                    ],
                );
            }
        }

        // Resource still exists, need to wait
        Ok(false)
    }

    /// deleteSecrets deletes all copied/generated secrets
    async fn delete_secrets(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<()> {
        let log = &ctx.log;

        let secret_names = [
            format!("{}-pull-secret", cr.name()),
            format!("{}-ssh-key", cr.name()),
            format!("{}-etcd-encryption-key", cr.name()),
        ];

        for secret_name in &secret_names {
            if let Err(err) = self.delete_secret(ctx, cr, secret_name).await {
                log.error(
                    &err,
                    "Failed to delete secret",
                    kv!["secret" => secret_name, "namespace" => cr.namespace()],
                );
                return Err(err);
            }
        }

        log.info(
            "All secrets deleted successfully",
            kv!["count" => secret_names.len(), "namespace" => cr.namespace()],
        );

        Ok(())
    }

    /// deleteSecret deletes a single secret after verifying ownership
    async fn delete_secret(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
        secret_name: &str,
    ) -> ctrlrt::Result<()> {
        let log = &ctx.log;

        let secret: Secret = match self
            .client
            .get(&ObjectKey::new(cr.namespace(), secret_name))
            .await
        {
            Ok(s) => s,
            Err(err) => {
                if ctrlrt::error::is_not_found(&err) {
                    // Secret already deleted
                    log.v(1).info(
                        "Secret already deleted",
                        kv!["secret" => secret_name, "namespace" => cr.namespace()],
                    );
                    return Ok(());
                }
                return Err(wrapf!(err, "failed to get secret {}: {}", secret_name, err));
            }
        };

        // Verify ownership before deleting
        if !meta::is_controlled_by(&secret, cr) {
            log.info(
                "Skipping secret deletion - not owned by this DPFHCPProvisioner",
                kv!["secret" => secret_name, "namespace" => cr.namespace()],
            );
            return Ok(());
        }

        // Delete secret
        log.v(1).info(
            "Deleting secret",
            kv!["secret" => secret_name, "namespace" => cr.namespace()],
        );

        if let Err(err) = self.client.delete(&secret).await {
            if ctrlrt::error::is_not_found(&err) {
                // Already deleted (race condition)
                return Ok(());
            }
            return Err(wrapf!(
                err,
                "failed to delete secret {}: {}",
                secret_name,
                err
            ));
        }

        log.info(
            "Secret deleted successfully",
            kv!["secret" => secret_name, "namespace" => cr.namespace()],
        );

        Ok(())
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
    ) -> ctrlrt::Result<ReconcileResult> {
        CleanupHandler::cleanup(self, ctx, cr).await
    }
}
