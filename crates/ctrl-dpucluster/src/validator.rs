//! Port of internal/controller/dpucluster/validator.go

use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use ctrlrt::meta;
use ctrlrt::recorder::{EVENT_TYPE_NORMAL, EVENT_TYPE_WARNING};
use ctrlrt::{Client, Ctx, Logger, Object, ObjectKey, ReconcileResult, Recorder, RecorderExt, kv};

// Event reasons
pub const REASON_DPU_CLUSTER_FOUND: &str = "DPUClusterFound";
pub const REASON_DPU_CLUSTER_NOT_FOUND: &str = "DPUClusterNotFound";
pub const REASON_DPU_CLUSTER_DELETED: &str = "DPUClusterDeleted";
pub const REASON_DPU_CLUSTER_ACCESS_DENIED: &str = "DPUClusterAccessDenied";
pub const REASON_CLUSTER_TYPE_UNSUPPORTED: &str = "ClusterTypeUnsupported";
pub const REASON_CLUSTER_TYPE_VALID: &str = "ClusterTypeValid";
pub const REASON_DPU_CLUSTER_IN_USE: &str = "DPUClusterInUse";
pub const REASON_DPU_CLUSTER_AVAILABLE: &str = "DPUClusterAvailable";

const FEATURE_VALIDATION: &str = "dpucluster-validation";
const FEATURE_EXCLUSIVITY: &str = "dpucluster-exclusivity";

/// Validator validates DPUCluster references and updates status accordingly
pub struct Validator {
    client: Client,
    recorder: Recorder,
}

/// `metav1.Condition{..., LastTransitionTime: metav1.Now(), ObservedGeneration: cr.Generation}`
fn condition(type_: &str, status: &str, reason: &str, message: &str, generation: i64) -> Condition {
    let mut c = meta::condition_gen(type_, status, reason, message, generation);
    c.last_transition_time = meta::now();
    c
}

fn feature_log(ctx: &Ctx, feature: &str) -> Logger {
    ctx.log.with_values(kv!["feature" => feature])
}

/// Parameters of one "set condition, emit event if changed, update status" step.
struct Outcome<'a> {
    feature: &'a str,
    condition_type: &'a str,
    status: &'a str,
    event_type: &'a str,
    reason: &'a str,
    message: String,
}

impl Validator {
    /// NewValidator creates a new DPUCluster validator
    pub fn new(c: Client, recorder: Recorder) -> Self {
        Self {
            client: c,
            recorder,
        }
    }

    /// ValidateDPUCluster validates that the referenced DPUCluster exists
    ///
    /// Transient errors return Go's `ctrl.Result{Requeue: true}, err` via `ctrlrt::with_result`.
    pub async fn validate_dpu_cluster(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = feature_log(ctx, FEATURE_VALIDATION);

        // Get reference to DPUCluster
        let dpu_cluster_ref = cr.spec.dpu_cluster_ref.clone();
        log.v(1).info(
            "Validating DPUCluster reference",
            kv![
                "dpuClusterName" => dpu_cluster_ref.name,
                "dpuClusterNamespace" => dpu_cluster_ref.namespace
            ],
        );

        // Attempt to fetch the DPUCluster
        let dpu_cluster: dpuprovisioningv1alpha1::DPUCluster = match self
            .client
            .get(&ObjectKey::new(
                &dpu_cluster_ref.namespace,
                &dpu_cluster_ref.name,
            ))
            .await
        {
            Ok(d) => d,
            Err(err) => {
                if ctrlrt::error::is_not_found(&err) {
                    // DPUCluster not found - set DPUClusterMissing=True
                    return self
                        .handle_dpu_cluster_missing(ctx, cr, &dpu_cluster_ref)
                        .await;
                }

                if ctrlrt::error::is_forbidden(&err) {
                    // RBAC permission denied - permanent error
                    return self
                        .handle_dpu_cluster_access_denied(ctx, cr, &dpu_cluster_ref, &err)
                        .await;
                }

                // Other transient errors (network timeout, API server error)
                log.v(1).info(
                    "Transient error fetching DPUCluster, will retry",
                    kv!["error" => err.to_string()],
                );
                return Err(ctrlrt::with_result(ReconcileResult::requeue(), err));
            }
        };

        // DPUCluster exists - validate cluster type is not kamaji
        let result = self.validate_cluster_type(ctx, cr, &dpu_cluster).await?;
        if !result.requeue_after.is_zero() {
            return Ok(result);
        }

        // DPUCluster exists and type is valid - validate exclusivity (not in use by another DPFHCPProvisioner)
        let result = self
            .validate_dpu_cluster_exclusivity(ctx, cr, &dpu_cluster)
            .await?;
        if !result.requeue_after.is_zero() {
            return Ok(result);
        }

        // DPUCluster exists, type is valid, and not in use - set DPUClusterMissing=False
        self.handle_dpu_cluster_found(ctx, cr, &dpu_cluster).await
    }

    /// Shared tail of every handler: `meta.SetStatusCondition` (+ event and log when changed)
    /// followed by `Status().Update`.
    async fn apply(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        o: Outcome<'_>,
        on_changed: impl FnOnce(&Logger),
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = feature_log(ctx, o.feature);

        // Set condition and check if it changed
        let cond = condition(
            o.condition_type,
            o.status,
            o.reason,
            &o.message,
            cr.generation(),
        );

        // Emit event only if condition changed
        if meta::set_status_condition(&mut cr.status.conditions, cond) {
            self.recorder.event(cr, o.event_type, o.reason, &o.message);
            on_changed(&log);
        }

        // Update status
        if let Err(err) = self.client.update_status(cr).await {
            log.error(&err, "Failed to update status", kv![]);
            return Err(err);
        }

        Ok(ReconcileResult::default())
    }

    /// validateClusterType validates that DPUCluster.Spec.Type is not kamaji
    /// This operator only supports non-Kamaji cluster types
    async fn validate_cluster_type(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        dpu_cluster: &dpuprovisioningv1alpha1::DPUCluster,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = feature_log(ctx, FEATURE_VALIDATION);

        log.v(1).info(
            "Validating cluster type",
            kv!["dpuClusterType" => dpu_cluster.spec.type_],
        );

        if dpu_cluster.spec.type_ == dpuprovisioningv1alpha1::KAMAJI_CLUSTER {
            log.error_nil(
                "Kamaji cluster type is not supported",
                kv!["dpuClusterType" => dpu_cluster.spec.type_],
            );
            return self.handle_cluster_type_invalid(ctx, cr, dpu_cluster).await;
        }

        // Type is valid (not kamaji) - set ClusterTypeValid=True
        self.handle_cluster_type_valid(ctx, cr, dpu_cluster).await
    }

    /// handleClusterTypeInvalid handles the case when DPUCluster type is kamaji (unsupported)
    async fn handle_cluster_type_invalid(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        dpu_cluster: &dpuprovisioningv1alpha1::DPUCluster,
    ) -> ctrlrt::Result<ReconcileResult> {
        // Note: Phase will be computed from conditions by the reconciler
        let message = format!(
            "DPUCluster '{}' has unsupported type '{}'. This operator only supports non-Kamaji cluster types",
            dpu_cluster.name(),
            dpu_cluster.spec.type_
        );
        // Do NOT requeue - permanent error requiring user to reference a different DPUCluster
        self.apply(
            ctx,
            cr,
            Outcome {
                feature: FEATURE_VALIDATION,
                condition_type: provisioningv1alpha1::CLUSTER_TYPE_VALID,
                status: meta::CONDITION_FALSE,
                event_type: EVENT_TYPE_WARNING,
                reason: REASON_CLUSTER_TYPE_UNSUPPORTED,
                message,
            },
            |log| {
                log.info(
                    "Unsupported cluster type detected",
                    kv!["dpuClusterType" => dpu_cluster.spec.type_],
                )
            },
        )
        .await
    }

    /// handleClusterTypeValid handles the case when DPUCluster type is valid (not kamaji)
    async fn handle_cluster_type_valid(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        dpu_cluster: &dpuprovisioningv1alpha1::DPUCluster,
    ) -> ctrlrt::Result<ReconcileResult> {
        // Note: Phase will be computed from conditions by the reconciler
        let message = format!("DPUCluster type '{}' is supported", dpu_cluster.spec.type_);
        // Emit event only if condition changed (e.g., recovered from unsupported type)
        // Success - continue with reconciliation
        self.apply(
            ctx,
            cr,
            Outcome {
                feature: FEATURE_VALIDATION,
                condition_type: provisioningv1alpha1::CLUSTER_TYPE_VALID,
                status: meta::CONDITION_TRUE,
                event_type: EVENT_TYPE_NORMAL,
                reason: REASON_CLUSTER_TYPE_VALID,
                message,
            },
            |log| {
                log.info(
                    "ClusterType validated",
                    kv!["dpuClusterType" => dpu_cluster.spec.type_],
                )
            },
        )
        .await
    }

    /// validateDPUClusterExclusivity validates that the DPUCluster is not already in use by another DPFHCPProvisioner
    /// Ensures 1:1 relationship between DPFHCPProvisioner and DPUCluster
    async fn validate_dpu_cluster_exclusivity(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        dpu_cluster: &dpuprovisioningv1alpha1::DPUCluster,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = feature_log(ctx, FEATURE_EXCLUSIVITY);

        log.v(1).info(
            "Validating DPUCluster exclusivity",
            kv![
                "dpuClusterName" => dpu_cluster.name(),
                "dpuClusterNamespace" => dpu_cluster.namespace()
            ],
        );

        // List all DPFHCPProvisioner resources in the cluster
        let provisioner_list: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            match self.client.list_all().await {
                Ok(l) => l,
                Err(err) => {
                    log.error(&err, "Failed to list DPFHCPProvisioner resources", kv![]);
                    return Err(ctrlrt::with_result(ReconcileResult::requeue(), err));
                }
            };

        // Check if any OTHER DPFHCPProvisioner references the same DPUCluster
        for provisioner in &provisioner_list {
            // Skip the current DPFHCPProvisioner (compare by namespace/name)
            if provisioner.namespace() == cr.namespace() && provisioner.name() == cr.name() {
                continue;
            }

            // Check if this provisioner references the same DPUCluster
            if provisioner.spec.dpu_cluster_ref.name == dpu_cluster.name()
                && provisioner.spec.dpu_cluster_ref.namespace == dpu_cluster.namespace()
            {
                // Found another DPFHCPProvisioner using this DPUCluster
                return self
                    .handle_dpu_cluster_in_use(ctx, cr, dpu_cluster, provisioner)
                    .await;
            }
        }

        // No other DPFHCPProvisioner is using this DPUCluster - it's available
        self.handle_dpu_cluster_available(ctx, cr, dpu_cluster)
            .await
    }

    /// handleDPUClusterInUse handles the case when DPUCluster is already in use by another DPFHCPProvisioner
    async fn handle_dpu_cluster_in_use(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        dpu_cluster: &dpuprovisioningv1alpha1::DPUCluster,
        conflicting_provisioner: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let message = format!(
            "DPUCluster '{}/{}' is already in use by DPFHCPProvisioner '{}/{}'. Each DPUCluster can only be referenced by one DPFHCPProvisioner",
            dpu_cluster.namespace(),
            dpu_cluster.name(),
            conflicting_provisioner.namespace(),
            conflicting_provisioner.name()
        );
        // Do NOT requeue - permanent error requiring user to reference a different DPUCluster
        self.apply(
            ctx,
            cr,
            Outcome {
                feature: FEATURE_EXCLUSIVITY,
                condition_type: provisioningv1alpha1::DPU_CLUSTER_IN_USE,
                status: meta::CONDITION_TRUE,
                event_type: EVENT_TYPE_WARNING,
                reason: REASON_DPU_CLUSTER_IN_USE,
                message,
            },
            |log| {
                log.info(
                    "DPUCluster already in use",
                    kv![
                        "dpuClusterName" => dpu_cluster.name(),
                        "dpuClusterNamespace" => dpu_cluster.namespace(),
                        "conflictingProvisioner" => format!(
                            "{}/{}",
                            conflicting_provisioner.namespace(),
                            conflicting_provisioner.name()
                        )
                    ],
                )
            },
        )
        .await
    }

    /// handleDPUClusterAvailable handles the case when DPUCluster is available (not in use)
    async fn handle_dpu_cluster_available(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        dpu_cluster: &dpuprovisioningv1alpha1::DPUCluster,
    ) -> ctrlrt::Result<ReconcileResult> {
        let message = format!(
            "DPUCluster '{}/{}' is available (not in use by another DPFHCPProvisioner)",
            dpu_cluster.namespace(),
            dpu_cluster.name()
        );
        // Emit event only if condition changed (e.g., recovered from in-use state)
        // Success - continue with reconciliation
        self.apply(
            ctx,
            cr,
            Outcome {
                feature: FEATURE_EXCLUSIVITY,
                condition_type: provisioningv1alpha1::DPU_CLUSTER_IN_USE,
                status: meta::CONDITION_FALSE,
                event_type: EVENT_TYPE_NORMAL,
                reason: REASON_DPU_CLUSTER_AVAILABLE,
                message,
            },
            |log| {
                log.info(
                    "DPUCluster is available",
                    kv![
                        "dpuClusterName" => dpu_cluster.name(),
                        "dpuClusterNamespace" => dpu_cluster.namespace()
                    ],
                )
            },
        )
        .await
    }

    /// handleDPUClusterMissing handles the case when DPUCluster is not found
    async fn handle_dpu_cluster_missing(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        dpu_cluster_ref: &provisioningv1alpha1::DPUClusterReference,
    ) -> ctrlrt::Result<ReconcileResult> {
        // Get previous condition to determine if this is a new error or deletion
        let previous_condition = meta::find_status_condition(
            &cr.status.conditions,
            provisioningv1alpha1::DPU_CLUSTER_MISSING,
        );

        // Determine message based on whether cluster was previously found
        let (message, reason) = if previous_condition
            .is_some_and(|c| c.status == meta::CONDITION_FALSE)
        {
            // DPUCluster was previously found but now deleted
            (
                format!(
                    "Referenced DPUCluster '{}' in namespace '{}' has been deleted. Please delete this DPFHCPProvisioner to clean up",
                    dpu_cluster_ref.name, dpu_cluster_ref.namespace
                ),
                REASON_DPU_CLUSTER_DELETED,
            )
        } else {
            // DPUCluster never existed or still missing
            (
                format!(
                    "Referenced DPUCluster '{}' not found in namespace '{}'",
                    dpu_cluster_ref.name, dpu_cluster_ref.namespace
                ),
                REASON_DPU_CLUSTER_NOT_FOUND,
            )
        };

        // Do NOT requeue - this is a permanent error requiring manual intervention
        // User must either create the DPUCluster or delete the DPFHCPProvisioner
        self.apply(
            ctx,
            cr,
            Outcome {
                feature: FEATURE_VALIDATION,
                condition_type: provisioningv1alpha1::DPU_CLUSTER_MISSING,
                status: meta::CONDITION_TRUE,
                event_type: EVENT_TYPE_WARNING,
                reason,
                message,
            },
            |log| {
                log.info(
                    "DPUCluster not found",
                    kv![
                        "dpuClusterName" => dpu_cluster_ref.name,
                        "dpuClusterNamespace" => dpu_cluster_ref.namespace,
                        "reason" => reason
                    ],
                )
            },
        )
        .await
    }

    /// handleDPUClusterAccessDenied handles RBAC permission errors
    async fn handle_dpu_cluster_access_denied(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        dpu_cluster_ref: &provisioningv1alpha1::DPUClusterReference,
        err: &ctrlrt::Error,
    ) -> ctrlrt::Result<ReconcileResult> {
        let message = format!(
            "Operator lacks RBAC permissions to access DPUCluster '{}' in namespace '{}': {}",
            dpu_cluster_ref.name, dpu_cluster_ref.namespace, err
        );
        // Do NOT requeue - permanent error requiring admin to fix RBAC
        self.apply(
            ctx,
            cr,
            Outcome {
                feature: FEATURE_VALIDATION,
                condition_type: provisioningv1alpha1::DPU_CLUSTER_MISSING,
                status: meta::CONDITION_TRUE,
                event_type: EVENT_TYPE_WARNING,
                reason: REASON_DPU_CLUSTER_ACCESS_DENIED,
                message,
            },
            |log| {
                log.error(
                    err,
                    "RBAC permission denied for DPUCluster",
                    kv![
                        "dpuClusterName" => dpu_cluster_ref.name,
                        "dpuClusterNamespace" => dpu_cluster_ref.namespace
                    ],
                )
            },
        )
        .await
    }

    /// handleDPUClusterFound handles the case when DPUCluster is found
    async fn handle_dpu_cluster_found(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        dpu_cluster: &dpuprovisioningv1alpha1::DPUCluster,
    ) -> ctrlrt::Result<ReconcileResult> {
        let message = format!(
            "DPUCluster '{}' found in namespace '{}'",
            dpu_cluster.name(),
            dpu_cluster.namespace()
        );
        // Emit event only if condition changed (e.g., recovered from missing state)
        // Success - continue with reconciliation
        self.apply(
            ctx,
            cr,
            Outcome {
                feature: FEATURE_VALIDATION,
                condition_type: provisioningv1alpha1::DPU_CLUSTER_MISSING,
                status: meta::CONDITION_FALSE,
                event_type: EVENT_TYPE_NORMAL,
                reason: REASON_DPU_CLUSTER_FOUND,
                message,
            },
            |log| {
                log.info(
                    "DPUCluster found",
                    kv![
                        "dpuClusterName" => dpu_cluster.name(),
                        "dpuClusterNamespace" => dpu_cluster.namespace()
                    ],
                )
            },
        )
        .await
    }
}
