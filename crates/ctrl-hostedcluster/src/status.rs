//! Port of internal/controller/hostedcluster/status.go

use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::meta;
use ctrlrt::{Client, Ctx, Object, ObjectKey, ReconcileResult, kv};

/// Map of HostedCluster condition types to DPFHCPProvisioner condition types
/// (key: HostedCluster condition type, value: DPFHCPProvisioner condition type).
/// Only the 7 conditions specified in the DPFHCPProvisioner API are mirrored.
///
/// Go iterates a `map[string]string` (random order); the conditions are mirrored here in
/// this fixed order instead.
pub const CONDITION_MAPPINGS: [(&str, &str); 7] = [
    (
        hyperv1::HOSTED_CLUSTER_AVAILABLE,
        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
    ),
    (
        hyperv1::HOSTED_CLUSTER_PROGRESSING,
        provisioningv1alpha1::HOSTED_CLUSTER_PROGRESSING,
    ),
    (
        hyperv1::HOSTED_CLUSTER_DEGRADED,
        provisioningv1alpha1::HOSTED_CLUSTER_DEGRADED,
    ),
    (
        hyperv1::VALID_RELEASE_INFO,
        provisioningv1alpha1::VALID_RELEASE_INFO,
    ),
    (
        hyperv1::VALID_RELEASE_IMAGE,
        provisioningv1alpha1::VALID_RELEASE_IMAGE,
    ),
    (
        hyperv1::IGNITION_ENDPOINT_AVAILABLE,
        provisioningv1alpha1::IGNITION_ENDPOINT_AVAILABLE,
    ),
    (
        hyperv1::IGNITION_SERVER_VALID_RELEASE_INFO,
        provisioningv1alpha1::IGNITION_SERVER_VALID_RELEASE_INFO,
    ),
];

/// StatusSyncer manages status synchronization from HostedCluster to DPFHCPProvisioner
#[derive(Clone)]
pub struct StatusSyncer {
    client: Client,
}

impl StatusSyncer {
    /// NewStatusSyncer creates a new StatusSyncer
    pub fn new(c: Client) -> Self {
        Self { client: c }
    }

    /// The underlying client (Go embeds `client.Client`).
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// SyncStatusFromHostedCluster mirrors HostedCluster status conditions to DPFHCPProvisioner status
    /// This function:
    /// - Only syncs status when hostedClusterRef is set in DPFHCPProvisioner status
    /// - Handles missing HostedCluster gracefully (may be creating or deleted)
    ///
    /// Returns ctrl.Result and error for reconciliation flow
    pub async fn sync_status_from_hosted_cluster(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;

        // Only sync status if hostedClusterRef is set
        let Some(hc_ref) = &cr.status.hosted_cluster_ref else {
            log.v(1)
                .info("Skipping status sync - hostedClusterRef not set yet", kv![]);
            return Ok(ReconcileResult::default());
        };

        // Get HostedCluster
        let hc_key = ObjectKey::new(&hc_ref.namespace, &hc_ref.name);

        let hc: hyperv1::HostedCluster = match self.client.get(&hc_key).await {
            Ok(hc) => hc,
            Err(err) => {
                if ctrlrt::error::is_not_found(&err) {
                    // HostedCluster not found - may be creating or deleted
                    // Don't fail, just log and skip sync
                    log.v(1).info(
                        "HostedCluster not found, skipping status sync",
                        kv!["hostedCluster" => hc_key.to_string()],
                    );
                    return Ok(ReconcileResult::default());
                }

                // Handle "no matches for kind" error (CRD not installed) gracefully
                if ctrlrt::error::is_no_match(&err) {
                    log.v(1).info(
                        "HostedCluster CRD not installed, skipping status sync",
                        kv!["hostedCluster" => hc_key.to_string()],
                    );
                    return Ok(ReconcileResult::default());
                }

                // Other errors (network, RBAC, API server issues) should be retried
                log.error(
                    &err,
                    "Failed to get HostedCluster for status sync",
                    kv!["hostedCluster" => hc_key.to_string()],
                );
                return Err(err);
            }
        };

        // Check if HostedCluster status is populated yet
        if hc.status.conditions.is_empty() {
            log.v(1).info(
                "HostedCluster status not yet populated, skipping sync",
                kv!["hostedCluster" => hc_key.to_string()],
            );
            // Don't requeue - the HostedCluster watch will trigger reconciliation when status changes.
            return Ok(ReconcileResult::default());
        }

        log.v(1).info(
            "Syncing status from HostedCluster",
            kv![
                "hostedCluster" => hc_key.to_string(),
                "conditions" => hc.status.conditions.len()
            ],
        );

        // Mirror conditions from HostedCluster to DPFHCPProvisioner
        self.mirror_conditions(ctx, cr, &hc);

        log.v(1).info(
            "Status sync completed successfully",
            kv!["hostedCluster" => hc_key.to_string()],
        );

        Ok(ReconcileResult::default())
    }

    /// mirrorConditions mirrors the 7 specific HostedCluster conditions to DPFHCPProvisioner
    /// This simply copies the condition status, reason, and message from HostedCluster to DPFHCPProvisioner
    /// No additional logic - just mirroring
    fn mirror_conditions(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        hc: &hyperv1::HostedCluster,
    ) {
        let log = &ctx.log;
        let generation = cr.generation();

        // Mirror each HostedCluster condition to DPFHCPProvisioner
        for (hc_cond_type, dpf_cond_type) in CONDITION_MAPPINGS {
            if let Some(hc_cond) = meta::find_status_condition(&hc.status.conditions, hc_cond_type)
            {
                // Found the condition, mirror it
                meta::set_status_condition(
                    &mut cr.status.conditions,
                    meta::condition_gen(
                        dpf_cond_type,
                        hc_cond.status.clone(),
                        hc_cond.reason.clone(),
                        hc_cond.message.clone(),
                        generation,
                    ),
                );
                log.v(2).info(
                    "Mirrored condition from HostedCluster",
                    kv![
                        "conditionType" => dpf_cond_type,
                        "status" => hc_cond.status,
                        "reason" => hc_cond.reason
                    ],
                );
            }
        }
    }
}
