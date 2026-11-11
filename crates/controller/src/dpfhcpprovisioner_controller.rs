//! Port of `internal/controller/dpfhcpprovisioner_controller.go`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::go::Duration as GoDuration;
use apis::hypershift::v1beta1 as hyperv1;
use apis::k8s::core::v1::ObjectReference;
use apis::openshift::config::v1 as configv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use common::OperatorConfig;
use ctrlrt::meta::{self, CONDITION_FALSE, CONDITION_TRUE};
use ctrlrt::reconcile::Predicate;
use ctrlrt::{
    Builder, Client, ControllerSpec, Ctx, Funcs, GenerationChangedPredicate, ManagerHandle, MapFn,
    Object, ObjectKey, ReconcileResult, Reconciler, Recorder, RecorderExt, Request, kv, map_fn,
    wrapf,
};
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;

use crate::ignition::{self, IgnitionGenerator};
use crate::nil::{nil_client, nil_deref, nil_recorder};

/// The hugepages reservation manager (`*ovshugepages.Manager`).
pub use ctrl_ovshugepages::Manager as HugepagesManager;

/// FinalizerName is the finalizer added to DPFHCPProvisioner resources
pub const FINALIZER_NAME: &str = "dpfhcpprovisioner.provisioning.dpu.hcp.io/finalizer";
/// OperatorNamespace is the namespace where the operator is deployed
pub const OPERATOR_NAMESPACE: &str = "dpf-hcp-provisioner-system";

const EVENT_TYPE_NORMAL: &str = "Normal";
const EVENT_TYPE_WARNING: &str = "Warning";

/// DPFHCPProvisionerReconciler reconciles a DPFHCPProvisioner object
///
/// Every dependency is optional like the Go pointer fields: tests (and the Go envtest suite)
/// wire only a subset. Calling into an unset dependency behaves like Go's nil pointer
/// dereference (a panic, recovered by the controller like controller-runtime's
/// `RecoverPanic`); `Option::None` is never used by the operator binary.
pub struct DPFHCPProvisionerReconciler {
    /// `client.Client` (embedded in Go)
    pub client: Client,
    /// `record.EventRecorder`
    pub recorder: Recorder,
    pub image_lookup: Option<Arc<ctrl_bfocplookup::ImageLookup>>,
    pub dpu_cluster_validator: Option<Arc<ctrl_dpucluster::Validator>>,
    pub secrets_validator: Option<Arc<ctrl_secrets::Validator>>,
    pub secret_manager: Option<Arc<ctrl_hostedcluster::SecretManager>>,
    pub metal_lb_manager: Option<Arc<ctrl_metallb::MetalLBManager>>,
    pub hosted_cluster_manager: Option<Arc<ctrl_hostedcluster::HostedClusterManager>>,
    pub node_pool_manager: Option<Arc<ctrl_hostedcluster::NodePoolManager>>,
    pub finalizer_manager: Option<Arc<ctrl_finalizer::Manager>>,
    pub status_syncer: Option<Arc<ctrl_hostedcluster::StatusSyncer>>,
    pub kubeconfig_injector: Option<Arc<ctrl_kubeconfiginjection::KubeconfigInjector>>,
    pub ignition_generator: Option<Arc<IgnitionGenerator>>,
    pub hugepages_manager: Option<Arc<HugepagesManager>>,

    /// Unexported reconciler state (Go: `ignitionReconciled bool`); always
    /// `ReconcilerState::default()` when constructing a reconciler.
    pub state: ReconcilerState,
}

/// The unexported state of [`DPFHCPProvisionerReconciler`].
#[derive(Debug, Default)]
pub struct ReconcilerState {
    /// Go: `ignitionReconciled bool` (set once the first ignition reconcile ran after startup).
    pub(crate) ignition_reconciled: AtomicBool,
}

impl Default for DPFHCPProvisionerReconciler {
    /// `&DPFHCPProvisionerReconciler{}`: nil client/recorder and no dependencies.
    fn default() -> Self {
        Self {
            client: nil_client(),
            recorder: nil_recorder(),
            image_lookup: None,
            dpu_cluster_validator: None,
            secrets_validator: None,
            secret_manager: None,
            metal_lb_manager: None,
            hosted_cluster_manager: None,
            node_pool_manager: None,
            finalizer_manager: None,
            status_syncer: None,
            kubeconfig_injector: None,
            ignition_generator: None,
            hugepages_manager: None,
            state: ReconcilerState::default(),
        }
    }
}

impl std::fmt::Debug for DPFHCPProvisionerReconciler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DPFHCPProvisionerReconciler")
            .finish_non_exhaustive()
    }
}

/// Dereference an optional dependency (Go: calling a method through a nil pointer).
fn dep<T>(d: &Option<Arc<T>>) -> &T {
    match d {
        Some(v) => v,
        None => nil_deref(),
    }
}

fn generation(cr: &provisioningv1alpha1::DPFHCPProvisioner) -> i64 {
    cr.metadata.generation.unwrap_or(0)
}

fn find<'a>(cr: &'a provisioningv1alpha1::DPFHCPProvisioner, t: &str) -> Option<&'a Condition> {
    meta::find_status_condition(&cr.status.conditions, t)
}

fn set_condition(cr: &mut provisioningv1alpha1::DPFHCPProvisioner, c: Condition) -> bool {
    meta::set_status_condition(&mut cr.status.conditions, c)
}

/// `metav1.Condition{Type, Status, Reason, Message, ObservedGeneration: cr.Generation}`
fn cond_gen(
    cr: &provisioningv1alpha1::DPFHCPProvisioner,
    type_: &str,
    status: &str,
    reason: &str,
    message: impl Into<String>,
) -> Condition {
    meta::condition_gen(type_, status, reason, message, generation(cr))
}

fn hc_key(r: &ObjectReference) -> ObjectKey {
    ObjectKey::new(r.namespace.clone(), r.name.clone())
}

fn go_duration(d: Duration) -> String {
    GoDuration(d.as_nanos().min(i64::MAX as u128) as i64).to_string()
}

/// `ptr.Deref(p, "")`
fn deref(p: &Option<String>) -> String {
    p.clone().unwrap_or_default()
}

#[async_trait::async_trait]
impl Reconciler for DPFHCPProvisionerReconciler {
    /// Reconcile is part of the main kubernetes reconciliation loop which aims to
    /// move the current state of the cluster closer to the desired state.
    async fn reconcile(&self, ctx: &Ctx, req: Request) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;
        log.info(
            "Reconciling DPFHCPProvisioner",
            kv!["namespace" => req.namespace, "name" => req.name],
        );

        // Fetch the DPFHCPProvisioner CR
        let mut cr: provisioningv1alpha1::DPFHCPProvisioner = match self.client.get(&req).await {
            Ok(cr) => cr,
            // CR not found - likely deleted
            Err(err) if ctrlrt::error::is_not_found(&err) => return Ok(ReconcileResult::default()),
            Err(err) => return Err(err),
        };

        // Load operator configuration from Config CR.
        // If the Config CR does not exist, the operator does nothing.
        let operator_config = match common::load_operator_config_from_cr(ctx, &self.client).await {
            Ok(c) => c,
            Err(err) => {
                log.error(&err, "Failed to load operator configuration", kv![]);
                return Err(err);
            }
        };
        let Some(operator_config) = operator_config else {
            log.info(
                "DPFHCPProvisionerConfig CR not found, operator is idle until config CR is created",
                kv![],
            );
            return Ok(ReconcileResult::default());
        };

        // Compute phase from conditions at the start
        // This ensures phase reflects the current state (including Deleting phase)
        self.update_phase_from_conditions(ctx, &mut cr).await;

        // Handle deletion - run finalizer cleanup
        if cr.metadata.deletion_timestamp.is_some() {
            return self.handle_deletion(ctx, &mut cr).await;
        }

        // Add finalizer if not present (Phase 1: Foundation)
        if !meta::contains_finalizer(&cr, FINALIZER_NAME) {
            log.info(
                "Adding finalizer to DPFHCPProvisioner",
                kv!["finalizer" => FINALIZER_NAME],
            );
            meta::add_finalizer(&mut cr, FINALIZER_NAME);
            if let Err(err) = self.client.update(&mut cr).await {
                log.error(&err, "Failed to add finalizer", kv![]);
                return Err(err);
            }
            // Return and requeue to continue with the updated CR
            return Ok(ReconcileResult::requeue());
        }

        // Feature: DPUCluster Validation
        log.v(1)
            .info("Running DPUCluster validation feature", kv![]);
        match dep(&self.dpu_cluster_validator)
            .validate_dpu_cluster(ctx, &mut cr)
            .await
        {
            Err(err) => {
                log.error(&err, "DPUCluster validation failed", kv![]);
                return Err(err);
            }
            Ok(result) if !result.requeue_after.is_zero() => return Ok(result),
            Ok(_) => {}
        }

        // Feature: Secrets Validation
        log.v(1).info("Running secrets validation feature", kv![]);
        match dep(&self.secrets_validator)
            .validate_secrets(ctx, &mut cr)
            .await
        {
            Err(err) => {
                log.error(&err, "Secrets validation failed", kv![]);
                return Err(err);
            }
            Ok(result) if !result.requeue_after.is_zero() => return Ok(result),
            Ok(_) => {}
        }

        // Feature: BlueField OCP Layer Image Lookup
        let result = self
            .lookup_blue_field_ocp_layer_image(ctx, &mut cr, &operator_config)
            .await?;
        if !result.requeue_after.is_zero() {
            return Ok(result);
        }

        // Recompute phase after validations to ensure HostedCluster creation only proceeds if all validations pass
        self.update_phase_from_conditions(ctx, &mut cr).await;

        // Feature: MetalLB Configuration
        // Configure MetalLB resources (IPAddressPool and L2Advertisement) when LoadBalancer exposure is needed
        log.v(1)
            .info("Running MetalLB configuration feature", kv![]);
        match self
            .configure_metal_lb(ctx, &mut cr, &operator_config)
            .await
        {
            Err(err) => {
                log.error(&err, "MetalLB configuration failed", kv![]);
                return Err(err);
            }
            Ok(result) if !result.requeue_after.is_zero() => return Ok(result),
            Ok(_) => {}
        }

        // Feature: Copy Secrets to clusters namespace
        // Runs whenever secrets have been validated successfully, not just during Pending phase.
        // This ensures that when a user rotates credentials (creates a new secret and updates the
        // CR's PullSecretRef/SSHKeySecretRef), the new copies are created and stale copies removed
        // in all phases. The copy names encode the source reference, so changing the reference
        // also changes HostedCluster.Spec.PullSecret.Name, causing HyperShift to create a new
        // ignition-server token with the correct hash.
        let secrets_valid = find(&cr, provisioningv1alpha1::SECRETS_VALID);
        if secrets_valid.is_some_and(|c| c.status == CONDITION_TRUE) {
            log.v(1)
                .info("Copying secrets to clusters namespace", kv![]);
            match dep(&self.secret_manager).copy_secrets(ctx, &cr).await {
                Err(err) => {
                    log.error(&err, "Secret copying failed", kv![]);
                    return Err(err);
                }
                Ok(result) if !result.requeue_after.is_zero() => return Ok(result),
                Ok(_) => {}
            }
        }

        // Generate ETCD encryption key (only once during initial provisioning)
        if cr.status.phase == provisioningv1alpha1::PHASE_PENDING {
            log.v(1).info("Generating ETCD encryption key", kv![]);
            match dep(&self.secret_manager)
                .generate_etcd_encryption_key(ctx, &cr)
                .await
            {
                Err(err) => {
                    log.error(&err, "ETCD key generation failed", kv![]);
                    return Err(err);
                }
                Ok(result) if !result.requeue_after.is_zero() => return Ok(result),
                Ok(_) => {}
            }
        }

        // This MUST run BEFORE reconcileHostedClusterAndNodePool so conditions are set
        // before the HC/NP images are updated.
        let result = self.handle_upgrade(ctx, &mut cr).await?;
        if !result.requeue_after.is_zero() {
            return Ok(result);
        }

        // Feature: HostedCluster & NodePool Creation and Upgrade
        let result = self
            .reconcile_hosted_cluster_and_node_pool(ctx, &cr)
            .await?;
        if !result.requeue_after.is_zero() {
            return Ok(result);
        }

        self.set_hosted_cluster_ref_if_owned(ctx, &mut cr).await;

        // Feature: HostedCluster Status Mirroring
        // Sync status from HostedCluster to DPFHCPProvisioner
        // This runs in all phases (Pending, Provisioning, Ready) to keep status up-to-date
        // Only syncs if hostedClusterRef is set (after HostedCluster creation)
        log.v(1).info("Syncing status from HostedCluster", kv![]);
        match dep(&self.status_syncer)
            .sync_status_from_hosted_cluster(ctx, &mut cr)
            .await
        {
            Err(err) => {
                log.error(&err, "Status sync failed", kv![]);
                return Err(err);
            }
            Ok(result) if !result.requeue_after.is_zero() => return Ok(result),
            Ok(_) => {}
        }

        // Feature: Kubeconfig Injection
        // Inject HostedCluster kubeconfig into DPUCluster namespace and update DPUCluster CR
        // Only runs after HostedCluster creation (hostedClusterRef is set)
        if cr.status.hosted_cluster_ref.is_some() {
            log.v(1).info("Running kubeconfig injection feature", kv![]);
            match dep(&self.kubeconfig_injector)
                .inject_kubeconfig(ctx, &mut cr)
                .await
            {
                Err(err) => {
                    log.error(&err, "Kubeconfig injection failed", kv![]);
                    return Err(err);
                }
                Ok(result) if !result.requeue_after.is_zero() => return Ok(result),
                Ok(_) => {}
            }
        } else {
            log.v(1).info(
                "Skipping kubeconfig injection - HostedCluster not created yet",
                kv![],
            );
        }

        // Detect DPUDeployment/DPUFlavor changes or deletions and invalidate ignition if needed.
        let (changed, result) = self.handle_dependency_changes(ctx, &mut cr).await?;
        if changed {
            return Ok(result);
        }

        // Feature: Ignition lifecycle (verify + generate)
        let result = self.reconcile_ignition(ctx, &mut cr).await?;
        if !result.requeue_after.is_zero() {
            return Ok(result);
        }

        // Compute Ready condition based on all operational requirements
        // This must run AFTER all features have updated their conditions
        // (HostedClusterAvailable, KubeConfigInjected, etc.)
        self.compute_ready_condition(ctx, &mut cr).await;

        // Compute final phase from all conditions after features have updated them
        // This must run AFTER computeReadyCondition since it checks the Ready condition
        self.update_phase_from_conditions(ctx, &mut cr).await;

        // Persist status with computed phase
        if let Err(err) = self.client.update_status(&mut cr).await {
            log.error(&err, "Failed to update status with computed phase", kv![]);
            return Err(err);
        }

        // Feature: Hugepages reservation DaemonSet
        // Create a DaemonSet inside the hosted cluster that runs one idle pod per node
        // (each node is a DPU), reserving hugepages so they are not consumed by other
        // workloads. Runs as the last step of reconciliation. Non-fatal: the hosted
        // cluster API may not be reachable yet (e.g. control plane still coming up),
        // so we don't fail the whole reconcile. Instead we return a bounded retry
        // delay so the reconcile is retried even if no other event re-triggers it.
        let mut result = ReconcileResult::default();
        if cr.status.hosted_cluster_ref.is_some() {
            log.v(1)
                .info("Reconciling hugepages reservation DaemonSet", kv![]);
            if let Err(err) = dep(&self.hugepages_manager)
                .reconcile_hugepages_daemon_set(
                    ctx,
                    &cr,
                    &operator_config.ovs_hugepages_size,
                    operator_config.ovs_hugepages_amount,
                )
                .await
            {
                log.v(1).info(
                    "Hugepages reservation DaemonSet reconciliation deferred, will retry",
                    kv!["error" => err.to_string()],
                );
                result.requeue_after = Duration::from_secs(30);
            }
        } else {
            log.v(1).info(
                "Skipping hugepages reservation - HostedCluster not created yet",
                kv![],
            );
        }

        log.info(
            "Reconciliation complete",
            kv![
                "namespace" => cr.namespace(),
                "name" => cr.name(),
                "phase" => cr.status.phase
            ],
        );
        Ok(result)
    }
}

impl DPFHCPProvisionerReconciler {
    /// `r.MetalLBManager.ConfigureMetalLB(ctx, &cr, operatorConfig)`. With a nil manager Go
    /// still runs the method up to its first receiver dereference: the
    /// `ShouldExposeThroughLoadBalancer` early return works, anything else panics.
    pub(crate) async fn configure_metal_lb(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        operator_config: &OperatorConfig,
    ) -> ctrlrt::Result<ReconcileResult> {
        match &self.metal_lb_manager {
            Some(m) => m.configure_metal_lb(ctx, cr, operator_config).await,
            None => {
                if !cr.should_expose_through_load_balancer() {
                    ctx.log
                        .with_values(kv!["provisioner" => cr.key()])
                        .v(1)
                        .info(
                            "ShouldExposeThroughLoadBalancer is false, skipping MetalLB configuration",
                            kv![],
                        );
                    return Ok(ReconcileResult::default());
                }
                nil_deref()
            }
        }
    }

    /// setHostedClusterRefIfOwned ensures the HostedCluster reference is set whenever
    /// an owned HostedCluster exists, regardless of the provisioner's phase.
    pub(crate) async fn set_hosted_cluster_ref_if_owned(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) {
        if cr.status.hosted_cluster_ref.is_some() {
            return;
        }

        let hc_key = ObjectKey::new(cr.namespace(), cr.name());
        match self.client.get::<hyperv1::HostedCluster>(&hc_key).await {
            Ok(hc) if meta::is_controlled_by(&hc, cr) => {}
            _ => return,
        }

        ctx.log
            .v(1)
            .info("Setting hostedClusterRef for existing HostedCluster", kv![]);
        cr.status.hosted_cluster_ref = Some(ObjectReference {
            name: cr.name().to_string(),
            namespace: cr.namespace().to_string(),
            kind: "HostedCluster".into(),
            api_version: "hypershift.openshift.io/v1beta1".into(),
            ..Default::default()
        });
    }

    /// SetupWithManager sets up the controller with the Manager.
    pub fn setup_with_manager(self: &Arc<Self>, mgr: &dyn ManagerHandle) -> ctrlrt::Result<()> {
        mgr.add_controller(self.controller_spec()?)
    }

    /// The controller description registered by [`Self::setup_with_manager`]
    /// (`ctrl.NewControllerManagedBy(mgr)...Complete(r)`).
    pub fn controller_spec(self: &Arc<Self>) -> ctrlrt::Result<ControllerSpec> {
        let r = self.clone();
        let dpu_cluster_map: MapFn<dpuprovisioningv1alpha1::DPUCluster> = map_fn(
            move |ctx: Ctx, obj: Arc<dpuprovisioningv1alpha1::DPUCluster>| {
                let r = r.clone();
                async move { r.dpu_cluster_to_requests(&ctx, &obj).await }
            },
        );
        let r = self.clone();
        let secret_map: MapFn<Secret> = map_fn(move |ctx: Ctx, obj: Arc<Secret>| {
            let r = r.clone();
            async move { r.secret_to_requests(&ctx, &obj).await }
        });
        let r = self.clone();
        let config_map_fn: MapFn<provisioningv1alpha1::DPFHCPProvisionerConfig> = map_fn(
            move |ctx: Ctx, obj: Arc<provisioningv1alpha1::DPFHCPProvisionerConfig>| {
                let r = r.clone();
                async move { r.config_to_requests(&ctx, &obj).await }
            },
        );
        let r = self.clone();
        let ignition_cm_map: MapFn<ConfigMap> = map_fn(move |ctx: Ctx, obj: Arc<ConfigMap>| {
            let r = r.clone();
            async move { r.ignition_config_map_to_requests(&ctx, &obj).await }
        });
        let r = self.clone();
        let dpu_deployment_map: MapFn<dpuservicev1alpha1::DPUDeployment> = map_fn(
            move |ctx: Ctx, obj: Arc<dpuservicev1alpha1::DPUDeployment>| {
                let r = r.clone();
                async move { r.dpu_deployment_to_requests(&ctx, &obj).await }
            },
        );
        let r = self.clone();
        let dpu_flavor_map: MapFn<dpuprovisioningv1alpha1::DPUFlavor> = map_fn(
            move |ctx: Ctx, obj: Arc<dpuprovisioningv1alpha1::DPUFlavor>| {
                let r = r.clone();
                async move { r.dpu_flavor_to_requests(&ctx, &obj).await }
            },
        );

        Builder::new()
            .for_type::<provisioningv1alpha1::DPFHCPProvisioner>(vec![])
            .watches(dpu_cluster_map, vec![Arc::new(dpu_cluster_predicate())])
            .watches(secret_map, vec![Arc::new(secret_predicate())])
            .watches_owner::<hyperv1::HostedCluster, provisioningv1alpha1::DPFHCPProvisioner>(
                true,
                vec![Arc::new(hosted_cluster_predicate())],
            )
            .watches::<Secret>(
                ctrl_kubeconfiginjection::find_provisioner_for_kubeconfig_secret_map_fn(
                    self.client.clone(),
                ),
                vec![Arc::new(
                    ctrl_kubeconfiginjection::is_hosted_cluster_kubeconfig_secret_predicate::<
                        Secret,
                    >(),
                )],
            )
            .watches(config_map_fn, vec![Arc::new(config_predicate())])
            .watches(ignition_cm_map, vec![Arc::new(ignition_config_map_predicate())])
            .watches(
                dpu_deployment_map,
                vec![Arc::new(GenerationChangedPredicate)
                    as Arc<dyn Predicate<dpuservicev1alpha1::DPUDeployment>>],
            )
            .watches(
                dpu_flavor_map,
                vec![Arc::new(GenerationChangedPredicate)
                    as Arc<dyn Predicate<dpuprovisioningv1alpha1::DPUFlavor>>],
            )
            .named("dpfhcpprovisioner")
            .build(self.clone())
    }

    /// dpuClusterToRequests maps DPUCluster events to reconcile requests for DPFHCPProvisioner CRs
    /// that reference the affected DPUCluster.
    /// Note: the relationship is 1:1 (one DPFHCPProvisioner per DPUCluster), but we still
    /// iterate to find the matching CR since we don't know the CR name from the DPUCluster.
    pub(crate) async fn dpu_cluster_to_requests(
        &self,
        ctx: &Ctx,
        dpu_cluster: &dpuprovisioningv1alpha1::DPUCluster,
    ) -> Vec<Request> {
        let log = &ctx.log;

        // List all DPFHCPProvisioner CRs cluster-wide
        let provisioner_list: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            match self.client.list_all().await {
                Ok(l) => l,
                Err(err) => {
                    log.error(
                        &err,
                        "Failed to list DPFHCPProvisioner CRs for DPUCluster watch",
                        kv![],
                    );
                    return Vec::new();
                }
            };

        // Find the DPFHCPProvisioner CR that references this DPUCluster (should be at most one per 1:1 relationship)
        let mut requests = Vec::with_capacity(1);
        for provisioner in &provisioner_list {
            if provisioner.spec.dpu_cluster_ref.name == dpu_cluster.name()
                && provisioner.spec.dpu_cluster_ref.namespace == dpu_cluster.namespace()
            {
                requests.push(ObjectKey::new(provisioner.namespace(), provisioner.name()));

                // there should only be one DPFHCPProvisioner per DPUCluster
                // If we find multiple, log a warning but still reconcile all of them
                if requests.len() > 1 {
                    log.info(
                        "WARNING: Multiple DPFHCPProvisioner CRs reference the same DPUCluster (violates 1:1 relationship)",
                        kv![
                            "dpuCluster" => dpu_cluster.name(),
                            "dpuClusterNamespace" => dpu_cluster.namespace(),
                            "count" => requests.len()
                        ],
                    );
                }
            }
        }

        if !requests.is_empty() {
            log.info(
                "DPUCluster changed, reconciling DPFHCPProvisioner CR",
                kv![
                    "dpuCluster" => dpu_cluster.name(),
                    "dpuClusterNamespace" => dpu_cluster.namespace(),
                    "affectedCRs" => requests.len()
                ],
            );
        }

        requests
    }

    /// secretToRequests maps Secret events to reconcile requests for DPFHCPProvisioner CRs
    /// that reference the secret via sshKeySecretRef or pullSecretRef
    pub(crate) async fn secret_to_requests(&self, ctx: &Ctx, secret: &Secret) -> Vec<Request> {
        let log = &ctx.log;

        // List all DPFHCPProvisioner CRs cluster-wide
        let provisioner_list: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            match self.client.list_all().await {
                Ok(l) => l,
                Err(err) => {
                    log.error(
                        &err,
                        "Failed to list DPFHCPProvisioner CRs for Secret watch",
                        kv![],
                    );
                    return Vec::new();
                }
            };

        // Find all DPFHCPProvisioner CRs that reference this secret
        let mut requests = Vec::new();
        for provisioner in &provisioner_list {
            // Check if this secret is referenced by sshKeySecretRef or pullSecretRef
            // Note: Secrets are namespace-scoped, so we need to check both name and namespace
            let is_ssh_key_secret = provisioner.spec.ssh_key_secret_ref.name == secret.name()
                && provisioner.namespace() == secret.namespace();
            let is_pull_secret = provisioner.spec.pull_secret_ref.name == secret.name()
                && provisioner.namespace() == secret.namespace();

            if is_ssh_key_secret || is_pull_secret {
                requests.push(ObjectKey::new(provisioner.namespace(), provisioner.name()));

                log.v(1).info(
                    "Secret referenced by DPFHCPProvisioner CR",
                    kv![
                        "secret" => secret.name(),
                        "secretNamespace" => secret.namespace(),
                        "provisioner" => provisioner.name(),
                        "provisionerNamespace" => provisioner.namespace(),
                        "isSSHKey" => is_ssh_key_secret,
                        "isPullSecret" => is_pull_secret
                    ],
                );
            }
        }

        if !requests.is_empty() {
            log.info(
                "Secret changed, reconciling DPFHCPProvisioner CRs",
                kv![
                    "secret" => secret.name(),
                    "secretNamespace" => secret.namespace(),
                    "affectedCRs" => requests.len()
                ],
            );
        }

        requests
    }

    /// configToRequests enqueues all DPFHCPProvisioner CRs when the Config CR changes
    pub(crate) async fn config_to_requests(
        &self,
        ctx: &Ctx,
        _obj: &provisioningv1alpha1::DPFHCPProvisionerConfig,
    ) -> Vec<Request> {
        let log = &ctx.log;

        let provisioner_list: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            match self.client.list_all().await {
                Ok(l) => l,
                Err(err) => {
                    log.error(
                        &err,
                        "Failed to list DPFHCPProvisioner CRs for Config watch",
                        kv![],
                    );
                    return Vec::new();
                }
            };

        let requests: Vec<Request> = provisioner_list
            .iter()
            .map(|p| ObjectKey::new(p.namespace(), p.name()))
            .collect();

        if !requests.is_empty() {
            log.info(
                "DPFHCPProvisionerConfig changed, reconciling all DPFHCPProvisioner CRs",
                kv!["affectedCRs" => requests.len()],
            );
        }

        requests
    }

    /// ignitionConfigMapToRequests maps ignition ConfigMap delete events to reconcile requests
    /// for the DPFHCPProvisioner CR that owns the ConfigMap.
    pub(crate) async fn ignition_config_map_to_requests(
        &self,
        ctx: &Ctx,
        obj: &ConfigMap,
    ) -> Vec<Request> {
        let log = &ctx.log;

        let Some(annotations) = obj.metadata.annotations.as_ref() else {
            return Vec::new();
        };

        let dpu_cluster_name = annotations
            .get(ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION)
            .cloned()
            .unwrap_or_default();
        let dpu_cluster_namespace = annotations
            .get(ignition::BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION)
            .cloned()
            .unwrap_or_default();
        if dpu_cluster_name.is_empty() || dpu_cluster_namespace.is_empty() {
            return Vec::new();
        }

        let provisioner_list: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            match self.client.list_all().await {
                Ok(l) => l,
                Err(err) => {
                    log.error(
                        &err,
                        "Failed to list DPFHCPProvisioner CRs for ignition ConfigMap watch",
                        kv![],
                    );
                    return Vec::new();
                }
            };

        for provisioner in &provisioner_list {
            if provisioner.spec.dpu_cluster_ref.name == dpu_cluster_name
                && provisioner.spec.dpu_cluster_ref.namespace == dpu_cluster_namespace
            {
                log.info(
                    "Ignition ConfigMap deleted, reconciling DPFHCPProvisioner",
                    kv![
                        "configmap" => obj.name(),
                        "configmapNamespace" => obj.namespace(),
                        "provisioner" => provisioner.name(),
                        "provisionerNamespace" => provisioner.namespace()
                    ],
                );
                return vec![ObjectKey::new(provisioner.namespace(), provisioner.name())];
            }
        }

        Vec::new()
    }

    /// dpuDeploymentToRequests maps DPUDeployment events to reconcile requests for DPFHCPProvisioner CRs
    /// that reference the affected DPUDeployment via spec.dpuDeploymentRef.
    pub(crate) async fn dpu_deployment_to_requests(
        &self,
        ctx: &Ctx,
        dpu_deployment: &dpuservicev1alpha1::DPUDeployment,
    ) -> Vec<Request> {
        let log = &ctx.log;

        let provisioner_list: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            match self.client.list_all().await {
                Ok(l) => l,
                Err(err) => {
                    log.error(
                        &err,
                        "Failed to list DPFHCPProvisioner CRs for DPUDeployment watch",
                        kv![],
                    );
                    return Vec::new();
                }
            };

        let mut requests = Vec::new();
        for provisioner in &provisioner_list {
            if let Some(r) = &provisioner.spec.dpu_deployment_ref
                && r.name == dpu_deployment.name()
                && r.namespace == dpu_deployment.namespace()
            {
                requests.push(ObjectKey::new(provisioner.namespace(), provisioner.name()));
            }
        }

        if !requests.is_empty() {
            log.info(
                "DPUDeployment changed, reconciling DPFHCPProvisioner CRs",
                kv![
                    "dpuDeployment" => dpu_deployment.name(),
                    "dpuDeploymentNamespace" => dpu_deployment.namespace(),
                    "affectedCRs" => requests.len()
                ],
            );
        }

        requests
    }

    /// dpuFlavorToRequests maps DPUFlavor events to reconcile requests for DPFHCPProvisioner CRs
    /// that use the affected DPUFlavor via their DPUDeployment reference.
    pub(crate) async fn dpu_flavor_to_requests(
        &self,
        ctx: &Ctx,
        obj: &dpuprovisioningv1alpha1::DPUFlavor,
    ) -> Vec<Request> {
        let log = &ctx.log;

        let provisioner_list: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            match self.client.list_all().await {
                Ok(l) => l,
                Err(err) => {
                    log.error(
                        &err,
                        "Failed to list DPFHCPProvisioner CRs for DPUFlavor watch",
                        kv![],
                    );
                    return Vec::new();
                }
            };

        let mut requests = Vec::new();
        for provisioner in &provisioner_list {
            let Some(dd_ref) = &provisioner.spec.dpu_deployment_ref else {
                continue;
            };
            if dd_ref.namespace != obj.namespace() {
                continue;
            }

            let key = ObjectKey::new(dd_ref.namespace.clone(), dd_ref.name.clone());
            let Ok(dd) = self
                .client
                .get::<dpuservicev1alpha1::DPUDeployment>(&key)
                .await
            else {
                continue;
            };
            if deref(&dd.spec.dpus.flavor) == obj.name() {
                requests.push(ObjectKey::new(provisioner.namespace(), provisioner.name()));
            }
        }

        if !requests.is_empty() {
            log.info(
                "DPUFlavor changed, reconciling DPFHCPProvisioner CRs",
                kv![
                    "dpuFlavor" => obj.name(),
                    "dpuFlavorNamespace" => obj.namespace(),
                    "affectedCRs" => requests.len()
                ],
            );
        }

        requests
    }

    /// handleDependencyChanges ensures ignition dependencies exist and detects DPUDeployment or
    /// DPUFlavor changes/deletions. A missing dependency invalidates ignition and stops
    /// reconciliation until DPUDeployment/DPUFlavor watches requeue. Returns `(true, result)`
    /// if reconciliation should stop early; an error always stops it (Go: `changed || err != nil`).
    pub(crate) async fn handle_dependency_changes(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<(bool, ReconcileResult)> {
        let log = &ctx.log;

        let Some(dd_ref) = cr.spec.dpu_deployment_ref.clone() else {
            return Ok((false, ReconcileResult::default()));
        };

        let ign_configured = find(cr, provisioningv1alpha1::IGNITION_CONFIGURED).cloned();
        let ignition_was_configured = ign_configured
            .as_ref()
            .is_some_and(|c| c.status == CONDITION_TRUE);
        if !ignition_was_configured && !self.ignition_prerequisites_met(cr) {
            return Ok((false, ReconcileResult::default()));
        }

        let key = ObjectKey::new(dd_ref.namespace.clone(), dd_ref.name.clone());
        let dpu_deployment: dpuservicev1alpha1::DPUDeployment = match self.client.get(&key).await {
            Ok(dd) => dd,
            Err(err) => {
                if ctrlrt::error::is_not_found(&err) {
                    log.info("DPUDeployment not found, invalidating ignition", kv![]);
                    return self
                        .invalidate_ignition_for_dependency_deletion(
                            ctx,
                            cr,
                            &format!("DPUDeployment {key} not found, removing ignition ConfigMap"),
                        )
                        .await;
                }
                log.v(1).info(
                    "Cannot fetch DPUDeployment for change detection, skipping",
                    kv!["error" => err.to_string()],
                );
                return Ok((false, ReconcileResult::default()));
            }
        };

        let flavor_name = deref(&dpu_deployment.spec.dpus.flavor);
        if flavor_name.is_empty() {
            return self
                .invalidate_ignition_for_dependency_deletion(
                    ctx,
                    cr,
                    &format!(
                        "DPUDeployment {key} has no flavor configured, removing ignition ConfigMap"
                    ),
                )
                .await;
        }

        let flavor_key = ObjectKey::new(dd_ref.namespace.clone(), flavor_name.clone());
        if let Err(err) = self
            .client
            .get::<dpuprovisioningv1alpha1::DPUFlavor>(&flavor_key)
            .await
        {
            if ctrlrt::error::is_not_found(&err) {
                log.info(
                    "DPUFlavor not found, invalidating ignition",
                    kv!["dpuFlavor" => flavor_name],
                );
                return self
                    .invalidate_ignition_for_dependency_deletion(
                        ctx,
                        cr,
                        &format!("DPUFlavor {flavor_key} not found, removing ignition ConfigMap"),
                    )
                    .await;
            }
            if ignition_was_configured {
                log.v(1).info(
                    "Cannot fetch DPUFlavor for change detection, skipping",
                    kv!["error" => err.to_string()],
                );
            } else {
                return Err(err);
            }
        }

        if !ign_configured
            .as_ref()
            .is_some_and(|c| c.status == CONDITION_TRUE)
        {
            return Ok((false, ReconcileResult::default()));
        }

        let Some(mut cm) = self.get_ignition_config_map(cr).await? else {
            return Ok((false, ReconcileResult::default()));
        };

        let annotations = cm.metadata.annotations.get_or_insert_with(Default::default);

        let current_flavor = deref(&dpu_deployment.spec.dpus.flavor);
        let stored_flavor = annotations
            .get(ignition::BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION)
            .cloned()
            .unwrap_or_default();
        let current_bfb = deref(&dpu_deployment.spec.dpus.bfb);
        let stored_bfb = annotations
            .get(ignition::BFCFG_TEMPLATE_BFB_NAME_ANNOTATION)
            .cloned()
            .unwrap_or_default();

        // Flavor changed → delete CM and regenerate ignition (content depends on flavor)
        if current_flavor != stored_flavor {
            log.info(
                "DPUDeployment Flavor changed, deleting stale ignition ConfigMap and regenerating",
                kv!["currentFlavor" => current_flavor, "storedFlavor" => stored_flavor],
            );

            self.delete_ignition_config_map(cr).await?;

            let c = cond_gen(
                cr,
                provisioningv1alpha1::IGNITION_CONFIGURED,
                CONDITION_FALSE,
                provisioningv1alpha1::REASON_DPU_DEPLOYMENT_CHANGED,
                "DPUDeployment Flavor changed, regenerating ignition configuration",
            );
            set_condition(cr, c);
            self.recorder.event(
                cr,
                EVENT_TYPE_NORMAL,
                "DPUDeploymentChanged",
                "DPUDeployment Flavor changed, triggering ignition regeneration",
            );
            self.compute_ready_condition(ctx, cr).await;
            self.update_phase_from_conditions(ctx, cr).await;
            if let Err(err) = self.client.update_status(cr).await {
                log.error(
                    &err,
                    "Failed to update status after DPUDeployment Flavor change",
                    kv![],
                );
                return Err(err);
            }
            return Ok((true, ReconcileResult::requeue_after(Duration::from_secs(1))));
        }

        // BFB changed → update CM annotation only (ignition content doesn't depend on BFB name)
        if current_bfb != stored_bfb {
            log.info(
                "DPUDeployment BFB changed, updating ConfigMap annotation",
                kv!["currentBFB" => current_bfb, "storedBFB" => stored_bfb],
            );

            annotations.insert(
                ignition::BFCFG_TEMPLATE_BFB_NAME_ANNOTATION.to_string(),
                current_bfb.clone(),
            );
            if let Err(err) = self.client.update(&mut cm).await {
                log.error(
                    &err,
                    "Failed to update ignition ConfigMap BFB annotation",
                    kv![],
                );
                return Err(err);
            }
            self.recorder.event(
                cr,
                EVENT_TYPE_NORMAL,
                "BFBAnnotationUpdated",
                &format!("Ignition ConfigMap BFB annotation updated to {current_bfb}"),
            );
            return Ok((true, ReconcileResult::default()));
        }

        Ok((false, ReconcileResult::default()))
    }

    /// invalidateIgnitionForDependencyDeletion deletes the ignition ConfigMap and sets
    /// IgnitionConfigured=False when a required dependency (DPUDeployment or DPUFlavor) is missing.
    /// Returns changed=true with no requeue so generateIgnition does not run; watches requeue
    /// the provisioner when the dependency is recreated.
    async fn invalidate_ignition_for_dependency_deletion(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        message: &str,
    ) -> ctrlrt::Result<(bool, ReconcileResult)> {
        let log = &ctx.log;

        self.delete_ignition_config_map(cr).await?;

        let c = cond_gen(
            cr,
            provisioningv1alpha1::IGNITION_CONFIGURED,
            CONDITION_FALSE,
            provisioningv1alpha1::REASON_DEPENDENCY_DELETED,
            message,
        );
        let cond_changed = set_condition(cr, c);
        if cond_changed {
            self.recorder
                .event(cr, EVENT_TYPE_WARNING, "DependencyDeleted", message);
        }
        self.compute_ready_condition(ctx, cr).await;
        let old_phase = cr.status.phase.clone();
        self.update_phase_from_conditions(ctx, cr).await;
        if (cond_changed || cr.status.phase != old_phase)
            && let Err(err) = self.client.update_status(cr).await
        {
            log.error(
                &err,
                "Failed to update status after dependency deletion",
                kv![],
            );
            return Err(err);
        }
        Ok((true, ReconcileResult::default()))
    }

    /// getIgnitionConfigMap returns the ignition ConfigMap for a DPUCluster by name.
    /// Returns `None` if not found.
    async fn get_ignition_config_map(
        &self,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<Option<ConfigMap>> {
        let cm_name = ignition::config_map_name(&cr.spec.dpu_cluster_ref.name);
        match self
            .client
            .get::<ConfigMap>(&ObjectKey::new(
                cr.spec.dpu_cluster_ref.namespace.clone(),
                cm_name,
            ))
            .await
        {
            Ok(cm) => Ok(Some(cm)),
            Err(err) if ctrlrt::error::is_not_found(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// deleteIgnitionConfigMap deletes the ignition ConfigMap for a DPUCluster.
    /// Returns true if a ConfigMap was deleted.
    async fn delete_ignition_config_map(
        &self,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<bool> {
        let Some(cm) = self.get_ignition_config_map(cr).await? else {
            return Ok(false);
        };
        if let Err(err) = self.client.delete(&cm).await {
            return Err(wrapf!(
                err,
                "failed to delete ignition ConfigMap {}/{}: {}",
                cm.namespace(),
                cm.name(),
                err
            ));
        }
        Ok(true)
    }

    /// invalidateIgnitionState deletes the stale ignition ConfigMap and clears the cached
    /// BlueField OCP layer image. Used when release image changes make existing ignition invalid.
    async fn invalidate_ignition_state(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<()> {
        if self.delete_ignition_config_map(cr).await? {
            ctx.log.info("Deleted stale ignition ConfigMap", kv![]);
        }
        cr.status.blue_field_ocp_layer_image = String::new();
        Ok(())
    }

    /// reconcileIgnition handles the ignition ConfigMap lifecycle: verification and generation.
    async fn reconcile_ignition(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;

        if self.verify_ignition_config_map(ctx, cr).await {
            self.compute_ready_condition(ctx, cr).await;
            self.update_phase_from_conditions(ctx, cr).await;
            if let Err(err) = self.client.update_status(cr).await {
                log.error(
                    &err,
                    "Failed to update status after ignition ConfigMap deletion",
                    kv![],
                );
                return Err(err);
            }
            return Ok(ReconcileResult::requeue_after(Duration::from_secs(1)));
        }

        let result = self.generate_ignition(ctx, cr).await?;
        if !result.requeue_after.is_zero() {
            return Ok(result);
        }

        self.state.ignition_reconciled.store(true, Ordering::SeqCst);
        Ok(ReconcileResult::default())
    }

    /// verifyIgnitionConfigMap checks the ignition ConfigMap when IgnitionConfigured=True.
    /// Clears IgnitionConfigured to trigger regeneration if the ConfigMap was deleted or
    /// if this is the first reconcile after operator startup (ensures bug fixes in embedded
    /// ignition content are applied when the operator image is updated).
    /// Returns true if regeneration was triggered.
    pub(crate) async fn verify_ignition_config_map(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> bool {
        let log = &ctx.log;

        if !find(cr, provisioningv1alpha1::IGNITION_CONFIGURED)
            .is_some_and(|c| c.status == CONDITION_TRUE)
        {
            return false;
        }

        let cm = match self.get_ignition_config_map(cr).await {
            Ok(cm) => cm,
            Err(err) => {
                log.error(&err, "Failed to verify ignition ConfigMap existence", kv![]);
                return false;
            }
        };

        let mut reason = "";
        let mut message = "";
        let mut event_type = EVENT_TYPE_NORMAL;
        let mut event_reason = "";

        if cm.is_none() {
            reason = "ConfigMapDeleted";
            message = "Ignition ConfigMap was deleted, will regenerate";
            event_type = EVENT_TYPE_WARNING;
            event_reason = "IgnitionConfigMapDeleted";
        } else if !self.state.ignition_reconciled.load(Ordering::SeqCst) {
            reason = "OperatorRestarted";
            message = "Operator restarted, regenerating ignition to apply any updates";
            event_reason = "IgnitionRegeneration";
        }

        if !reason.is_empty() {
            log.info("Triggering ignition regeneration", kv!["reason" => reason]);
            let c = cond_gen(
                cr,
                provisioningv1alpha1::IGNITION_CONFIGURED,
                CONDITION_FALSE,
                reason,
                message,
            );
            set_condition(cr, c);
            self.recorder.event(cr, event_type, event_reason, message);
            self.state.ignition_reconciled.store(true, Ordering::SeqCst);
            return true;
        }

        self.state.ignition_reconciled.store(true, Ordering::SeqCst);
        false
    }

    /// lookupBlueFieldOCPLayerImage handles BlueField OCP layer image lookup.
    /// Skips lookup if machineOSURL is provided in the CR spec.
    /// Runs lookup during initial creation/retry and ignition generation phases.
    /// Skips during Provisioning/Ready to avoid false failures when old OCP versions are removed from registry.
    async fn lookup_blue_field_ocp_layer_image(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        operator_config: &OperatorConfig,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;

        // Skip lookup if machineOSURL is provided directly
        if !cr.spec.machine_osurl.is_empty() {
            log.v(1).info(
                "Skipping BlueField OCP layer lookup - machineOSURL provided in spec",
                kv![],
            );
            let condition = cond_gen(
                cr,
                provisioningv1alpha1::BLUE_FIELD_OCP_LAYER_IMAGE_FOUND,
                CONDITION_TRUE,
                "LookupSkipped",
                "BlueField OCP layer lookup skipped - machineOSURL provided in spec",
            );
            if set_condition(cr, condition)
                && let Err(err) = self.client.update_status(cr).await
            {
                log.error(
                    &err,
                    "Failed to update BlueFieldOCPLayerImageFound condition",
                    kv![],
                );
                return Err(err);
            }
            return Ok(ReconcileResult::default());
        }

        // Run lookup (phase-gated)
        let image_lookup = dep(&self.image_lookup);
        image_lookup.set_repository(operator_config.blue_field_ocp_layer_repo.clone());
        if cr.status.phase == provisioningv1alpha1::PHASE_PENDING
            || cr.status.phase == provisioningv1alpha1::PHASE_FAILED
            || cr.status.phase == provisioningv1alpha1::PHASE_GENERATING_IGNITION
        {
            log.v(1)
                .info("Running BlueField OCP layer image lookup", kv![]);
            let result = image_lookup
                .lookup_blue_field_ocp_layer_image(ctx, cr)
                .await?;
            if !result.requeue_after.is_zero() {
                return Ok(result);
            }
        } else {
            log.v(1).info(
                "Skipping BlueField OCP layer lookup - not in applicable phase",
                kv!["phase" => cr.status.phase],
            );
        }

        Ok(ReconcileResult::default())
    }

    /// generateIgnition runs the ignition generation feature if the CR is in the IgnitionGenerating phase,
    /// or if it's in the Failed phase due to a previous ignition generation failure (transient retry).
    pub(crate) async fn generate_ignition(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;

        let Some(hc_ref) = cr.status.hosted_cluster_ref.clone() else {
            log.v(1).info(
                "Skipping ignition generation - no HostedCluster ref",
                kv!["phase" => cr.status.phase],
            );
            return Ok(ReconcileResult::default());
        };

        // Run ignition generation when:
        // 1. Phase is IgnitionGenerating (normal path), OR
        // 2. Phase is Failed AND the failure was caused by ignition generation (retry path), OR
        // 3. IgnitionConfigured was cleared due to operator restart or ConfigMap deletion
        let mut should_generate =
            cr.status.phase == provisioningv1alpha1::PHASE_GENERATING_IGNITION;
        if !should_generate
            && let Some(ign_configured) = find(cr, provisioningv1alpha1::IGNITION_CONFIGURED)
            && ign_configured.status == CONDITION_FALSE
        {
            match ign_configured.reason.as_str() {
                provisioningv1alpha1::REASON_IGNITION_GENERATION_FAILED => {
                    should_generate = true;
                    log.info(
                        "Retrying ignition generation after transient failure",
                        kv![],
                    );
                }
                "OperatorRestarted" | "ConfigMapDeleted" => {
                    should_generate = true;
                    log.info(
                        "Regenerating ignition",
                        kv!["reason" => ign_configured.reason],
                    );
                }
                _ => {}
            }
        }
        if !should_generate {
            log.v(1).info(
                "Skipping ignition generation",
                kv!["phase" => cr.status.phase],
            );
            return Ok(ReconcileResult::default());
        }

        // When ignition was invalidated due to a release image change, wait for the HC
        // to finish rolling out the new version before regenerating — the ignition server
        // serves content for whatever version is currently running.
        if find(cr, provisioningv1alpha1::IGNITION_CONFIGURED)
            .is_some_and(|c| c.reason == provisioningv1alpha1::REASON_RELEASE_IMAGE_UPDATED)
        {
            let hc: hyperv1::HostedCluster = match self.client.get(&hc_key(&hc_ref)).await {
                Ok(hc) => hc,
                Err(err) if ctrlrt::error::is_not_found(&err) => {
                    return Ok(ReconcileResult::default());
                }
                Err(err) => return Err(err),
            };
            if !self.is_control_plane_ready(&hc, &cr.spec.ocp_release_image) {
                log.v(1).info(
                    "Waiting for control plane rollout before regenerating ignition",
                    kv![],
                );
                return Ok(ReconcileResult::requeue_after(Duration::from_secs(30)));
            }
        }

        log.v(1).info("Running ignition generation feature", kv![]);
        let result = match dep(&self.ignition_generator)
            .generate_ignition(ctx, cr)
            .await
        {
            Ok(r) => r,
            Err(err) => {
                // Go: a nil pointer dereference inside GenerateIgnition is a runtime panic
                // that unwinds out of Reconcile (recovered by the controller).
                if ignition::is_nil_pointer_dereference(&err) {
                    nil_deref();
                }
                log.error(&err, "Ignition generation failed", kv![]);
                self.update_phase_from_conditions(ctx, cr).await;
                return Err(err);
            }
        };

        // When ignition generation fails, the IgnitionGenerator sets IgnitionConfigured=False
        // and returns RequeueAfter (without error). In that case, recompute the phase so the
        // CR transitions to Failed instead of staying in IgnitionGenerating indefinitely.
        if !result.requeue_after.is_zero()
            && find(cr, provisioningv1alpha1::IGNITION_CONFIGURED)
                .is_some_and(|c| c.status == CONDITION_FALSE)
        {
            self.update_phase_from_conditions(ctx, cr).await;
            // The IgnitionGenerator already persisted the IgnitionConfigured condition,
            // but we need to persist the updated phase.
            if let Err(status_err) = self.client.update_status(cr).await {
                log.error(
                    &status_err,
                    "Failed to update phase after ignition generation failure",
                    kv![],
                );
                return Err(status_err);
            }
        }

        Ok(result)
    }

    /// ignitionPrerequisitesMet reports whether HostedCluster and kubeconfig injection are ready
    /// for ignition generation.
    fn ignition_prerequisites_met(&self, cr: &provisioningv1alpha1::DPFHCPProvisioner) -> bool {
        if cr.status.hosted_cluster_ref.is_none() {
            return false;
        }

        if !find(cr, provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE)
            .is_some_and(|c| c.status == CONDITION_TRUE)
        {
            return false;
        }

        find(cr, provisioningv1alpha1::KUBE_CONFIG_INJECTED)
            .is_some_and(|c| c.status == CONDITION_TRUE)
    }

    /// computeReadyCondition determines if the DPFHCPProvisioner is fully operational and sets the Ready condition.
    ///
    /// Ready state requires ALL of the following currently implemented features:
    /// 1. HostedCluster is available and healthy (HostedClusterAvailable=True)
    /// 2. Kubeconfig successfully injected into DPUCluster (KubeConfigInjected=True)
    ///
    /// This function should be called AFTER all feature reconciliation completes, so that all
    /// sub-conditions (HostedClusterAvailable, KubeConfigInjected, etc.) are up-to-date.
    pub(crate) async fn compute_ready_condition(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) {
        let log = &ctx.log;

        // MetalLB must be configured (if required)
        // This is only required when exposing services through LoadBalancer
        // Checked first because MetalLB configuration happens before HostedCluster creation
        if cr.should_expose_through_load_balancer()
            && !find(cr, provisioningv1alpha1::METAL_LB_CONFIGURED)
                .is_some_and(|c| c.status == CONDITION_TRUE)
        {
            set_condition(
                cr,
                meta::condition(
                    provisioningv1alpha1::READY,
                    CONDITION_FALSE,
                    "MetalLBNotConfigured",
                    "Waiting for MetalLB configuration to complete",
                ),
            );
            log.v(1).info("Not ready: MetalLB not configured", kv![]);
            return;
        }

        // HostedCluster must be available
        // This is set by the StatusSyncer after mirroring HostedCluster status
        if !find(cr, provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE)
            .is_some_and(|c| c.status == CONDITION_TRUE)
        {
            set_condition(
                cr,
                meta::condition(
                    provisioningv1alpha1::READY,
                    CONDITION_FALSE,
                    provisioningv1alpha1::REASON_HOSTED_CLUSTER_NOT_READY,
                    "Waiting for HostedCluster to become available",
                ),
            );
            log.v(1)
                .info("Not ready: HostedCluster not available", kv![]);
            return;
        }

        // Kubeconfig must be injected
        // This is set by the KubeconfigInjector after successful injection
        if !find(cr, provisioningv1alpha1::KUBE_CONFIG_INJECTED)
            .is_some_and(|c| c.status == CONDITION_TRUE)
        {
            set_condition(
                cr,
                meta::condition(
                    provisioningv1alpha1::READY,
                    CONDITION_FALSE,
                    provisioningv1alpha1::REASON_KUBE_CONFIG_NOT_INJECTED,
                    "Waiting for kubeconfig injection to DPUCluster",
                ),
            );
            log.v(1).info("Not ready: Kubeconfig not injected", kv![]);
            return;
        }

        // Ignition must be configured
        let gen_ = generation(cr);
        if !find(cr, provisioningv1alpha1::IGNITION_CONFIGURED).is_some_and(|c| {
            c.status == CONDITION_TRUE && c.observed_generation.unwrap_or(0) == gen_
        }) {
            set_condition(
                cr,
                meta::condition(
                    provisioningv1alpha1::READY,
                    CONDITION_FALSE,
                    "IgnitionNotConfigured",
                    "Waiting for ignition configuration to complete",
                ),
            );
            log.v(1).info("Not ready: Ignition not configured", kv![]);
            return;
        }

        // Control plane must have finished rolling out the target version
        if let Some(hc_ref) = cr.status.hosted_cluster_ref.clone() {
            let hc: hyperv1::HostedCluster = match self.client.get(&hc_key(&hc_ref)).await {
                Ok(hc) => hc,
                Err(err) => {
                    log.error(&err, "Not ready: failed to read HostedCluster", kv![]);
                    set_condition(
                        cr,
                        meta::condition(
                            provisioningv1alpha1::READY,
                            CONDITION_FALSE,
                            provisioningv1alpha1::REASON_HOSTED_CLUSTER_NOT_READY,
                            "Failed to read HostedCluster to confirm version rollout",
                        ),
                    );
                    return;
                }
            };
            if !self.is_control_plane_ready(&hc, &cr.spec.ocp_release_image) {
                set_condition(
                    cr,
                    meta::condition(
                        provisioningv1alpha1::READY,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_CONTROL_PLANE_PROGRESSING,
                        "Waiting for control plane to finish rolling out target version",
                    ),
                );
                log.v(1)
                    .info("Not ready: control plane version not confirmed", kv![]);
                return;
            }
        }

        // All requirements met - set Ready to True
        set_condition(
            cr,
            meta::condition(
                provisioningv1alpha1::READY,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_ALL_COMPONENTS_OPERATIONAL,
                "All required components are operational",
            ),
        );
        log.info("DPFHCPProvisioner is ready", kv![]);
    }

    /// updatePhaseFromConditions computes the phase based on all conditions
    pub(crate) async fn update_phase_from_conditions(
        &self,
        _ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) {
        // Check for deletion (highest priority)
        if cr.metadata.deletion_timestamp.is_some() {
            cr.status.phase = provisioningv1alpha1::PHASE_DELETING.into();
            return;
        }

        // List of validation conditions that must pass before provisioning
        // Order matters: check critical validations first
        // (condType, negative: true if ConditionTrue = bad, false if ConditionFalse = bad)
        let validation_checks: [(&str, bool); 5] = [
            ("DPUClusterMissing", true),            // True = cluster missing = bad
            ("ClusterTypeValid", false),            // False = type invalid = bad
            ("DPUClusterInUse", true),              // True = cluster already in use = bad
            ("SecretsValid", false),                // False = secrets invalid = bad
            ("BlueFieldOCPLayerImageFound", false), // False = OCP layer image not found = bad
        ];

        // Check all validation conditions
        for (cond_type, negative) in validation_checks {
            let Some(cond) = find(cr, cond_type) else {
                // Condition not set yet - still initializing
                continue;
            };

            // Determine if this condition represents a failure
            // For negative conditions: True = bad (e.g., DPUClusterMissing=True means missing)
            // For positive conditions: False = bad (e.g., ClusterTypeValid=False means invalid)
            let is_failed = (negative && cond.status == CONDITION_TRUE)
                || (!negative && cond.status == CONDITION_FALSE);

            if is_failed {
                cr.status.phase = provisioningv1alpha1::PHASE_FAILED.into();
                return;
            }
        }

        // Check if ignition generation failed.
        // When IgnitionConfigured is explicitly False with a failure reason, transition to Failed
        // so the user can see the error and take action. The controller will retry on requeue.
        // Note: IgnitionConfigured=False with transient reasons (e.g., ReleaseImageUpdated,
        // OperatorRestarted) does NOT cause Failed — those are handled by generateIgnition
        // and the phase will be GeneratingIgnition instead.
        let ign_configured = find(cr, provisioningv1alpha1::IGNITION_CONFIGURED).cloned();
        if let Some(c) = &ign_configured
            && c.status == CONDITION_FALSE
            && (c.reason == provisioningv1alpha1::REASON_IGNITION_GENERATION_FAILED
                || c.reason == provisioningv1alpha1::REASON_MACHINE_OSURL_MISSING)
        {
            cr.status.phase = provisioningv1alpha1::PHASE_FAILED.into();
            return;
        }

        // Check for HostedCluster error states (blocked or degraded).
        // If the HC cannot make progress (Progressing=False/Blocked) or is degraded,
        // show Error so the user knows the cluster needs attention.
        if cr.status.hosted_cluster_ref.is_some() {
            let is_blocked = find(cr, provisioningv1alpha1::HOSTED_CLUSTER_PROGRESSING)
                .is_some_and(|c| {
                    c.status == CONDITION_FALSE && c.reason == hyperv1::BLOCKED_REASON
                });
            let is_degraded = find(cr, provisioningv1alpha1::HOSTED_CLUSTER_DEGRADED)
                .is_some_and(|c| c.status == CONDITION_TRUE);

            if is_blocked || is_degraded {
                cr.status.phase = provisioningv1alpha1::PHASE_ERROR.into();
                return;
            }
        }

        if find(cr, "Ready").is_some_and(|c| c.status == CONDITION_TRUE) {
            cr.status.phase = provisioningv1alpha1::PHASE_READY.into();
            return;
        }

        // Ignition configured but not yet Ready — control plane version still progressing
        let gen_ = generation(cr);
        if let Some(c) = &ign_configured
            && c.status == CONDITION_TRUE
            && c.observed_generation.unwrap_or(0) == gen_
        {
            cr.status.phase = provisioningv1alpha1::PHASE_CLUSTER_VERSION_PROGRESSING.into();
            return;
        }

        let hc_available = find(cr, provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE)
            .is_some_and(|c| c.status == CONDITION_TRUE);
        let kc_injected = find(cr, provisioningv1alpha1::KUBE_CONFIG_INJECTED)
            .is_some_and(|c| c.status == CONDITION_TRUE);

        // HC available, kubeconfig injected, ignition not yet configured → generating ignition
        // During upgrades the control plane may still be rolling out the new version even though
        // Available=True (it stays true for the old version). Show WaitingForControlPlane until
        // the control plane is on the target version.
        if hc_available && kc_injected {
            if is_upgrading(cr)
                && let Some(hc_ref) = cr.status.hosted_cluster_ref.clone()
            {
                let ready = match self
                    .client
                    .get::<hyperv1::HostedCluster>(&hc_key(&hc_ref))
                    .await
                {
                    Ok(hc) => self.is_control_plane_ready(&hc, &cr.spec.ocp_release_image),
                    Err(_) => false,
                };
                if !ready {
                    cr.status.phase = provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE.into();
                    return;
                }
            }
            cr.status.phase = provisioningv1alpha1::PHASE_GENERATING_IGNITION.into();
            return;
        }

        // HostedCluster created but not yet available → waiting for control plane
        if cr.status.hosted_cluster_ref.is_some() {
            cr.status.phase = provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE.into();
            return;
        }

        cr.status.phase = provisioningv1alpha1::PHASE_PENDING.into();
    }

    /// reconcileHostedClusterAndNodePool creates the HostedCluster and NodePool resources.
    /// During Pending phase: creates HC and NP for the first time (initial provisioning).
    /// After provisioning (hostedClusterRef is set): verifies HC and NP exist with correct ownership.
    /// Release image upgrades are handled separately by handleUpgrade.
    async fn reconcile_hosted_cluster_and_node_pool(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;

        let (hc_failed_msg, np_failed_msg) =
            if cr.status.phase == provisioningv1alpha1::PHASE_PENDING {
                log.v(1).info("Creating HostedCluster and NodePool", kv![]);
                ("HostedCluster creation failed", "NodePool creation failed")
            } else if cr.status.hosted_cluster_ref.is_some() {
                log.v(1).info(
                    "Checking HostedCluster and NodePool for release image updates",
                    kv![],
                );
                ("HostedCluster update failed", "NodePool update failed")
            } else {
                log.v(1).info(
                    "Skipping HostedCluster/NodePool operations - not in applicable phase",
                    kv!["phase" => cr.status.phase],
                );
                return Ok(ReconcileResult::default());
            };

        match dep(&self.hosted_cluster_manager)
            .create_or_update_hosted_cluster(ctx, cr)
            .await
        {
            Err(err) => {
                log.error(&err, hc_failed_msg, kv![]);
                return Err(err);
            }
            Ok(result) if !result.requeue_after.is_zero() => return Ok(result),
            Ok(_) => {}
        }

        match dep(&self.node_pool_manager).create_node_pool(ctx, cr).await {
            Err(err) => {
                log.error(&err, np_failed_msg, kv![]);
                return Err(err);
            }
            Ok(result) if !result.requeue_after.is_zero() => return Ok(result),
            Ok(_) => {}
        }

        Ok(ReconcileResult::default())
    }

    /// handleUpgrade detects and manages the HostedCluster upgrade lifecycle.
    /// When the user changes spec.ocpReleaseImage, this function detects the mismatch,
    /// invalidates stale ignition, syncs the release image to the HC/NP, and sets
    /// HostedClusterUpgrading=True (cosmetic — for observability only).
    ///
    /// HostedClusterUpgrading does not gate any flows. Ignition regeneration is blocked
    /// by generateIgnition checking isHostedClusterVersionReady when IgnitionConfigured
    /// was cleared due to a release image change.
    pub(crate) async fn handle_upgrade(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;

        let Some(hc_ref) = cr.status.hosted_cluster_ref.clone() else {
            return Ok(ReconcileResult::default());
        };

        // Skip during initial setup (Pending, Failed).
        if cr.status.phase == provisioningv1alpha1::PHASE_PENDING
            || cr.status.phase == provisioningv1alpha1::PHASE_FAILED
        {
            return Ok(ReconcileResult::default());
        }

        // Fetch current HC to compare release images
        let mut hc: hyperv1::HostedCluster = match self.client.get(&hc_key(&hc_ref)).await {
            Ok(hc) => hc,
            Err(err) if ctrlrt::error::is_not_found(&err) => {
                return Ok(ReconcileResult::default());
            }
            Err(err) => return Err(err),
        };

        let target = cr.spec.ocp_release_image.clone();

        // Case 1: Upgrade in progress — check if the HC has finished upgrading
        // by verifying the version history shows the target version as Completed.
        if is_upgrading(cr) {
            if self.is_hosted_cluster_version_ready(&hc, &target) {
                log.info(
                    "HostedCluster upgrade completed",
                    kv!["releaseImage" => target],
                );
                let c = cond_gen(
                    cr,
                    provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING,
                    CONDITION_FALSE,
                    provisioningv1alpha1::REASON_UPGRADE_COMPLETE,
                    format!("Upgrade to {target} complete"),
                );
                set_condition(cr, c);
                self.recorder.event(
                    cr,
                    EVENT_TYPE_NORMAL,
                    provisioningv1alpha1::REASON_UPGRADE_COMPLETE,
                    &format!("HostedCluster upgrade to {target} completed"),
                );
                self.compute_ready_condition(ctx, cr).await;
                self.update_phase_from_conditions(ctx, cr).await;
                self.client.update_status(cr).await?;
                return Ok(ReconcileResult::default());
            }
            return Ok(ReconcileResult::requeue_after(Duration::from_secs(30)));
        }

        // HC spec already matches CR — if the control plane has finished rolling out, nothing to do.
        // If it hasn't, check for a missed upgrade (operator crashed after updating HC but before
        // persisting HostedClusterUpgrading). All three conditions must hold:
        //   1. Phase is Ready (we previously completed successfully with some version)
        //   2. isControlPlaneReady returns false (target version not confirmed)
        //   3. Version history has a Completed entry for a DIFFERENT image (proves a prior
        //      version was active — eliminates any false positives from initial install)
        if hc.spec.release.image == target {
            if cr.status.phase != provisioningv1alpha1::PHASE_READY
                || self.is_control_plane_ready(&hc, &target)
                || !self.has_completed_different_version(&hc, &target)
            {
                return Ok(ReconcileResult::default());
            }
            log.info(
                "HC spec matches but version not yet ready, entering upgrade flow",
                kv!["releaseImage" => target],
            );
        }

        // Case 2: Upgrade detected — HC release image differs from CR spec
        log.info(
            "Upgrade detected",
            kv![
                "currentImage" => hc.spec.release.image,
                "targetImage" => target
            ],
        );

        // Set HostedClusterUpgrading=True
        let c = cond_gen(
            cr,
            provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING,
            CONDITION_TRUE,
            provisioningv1alpha1::REASON_UPGRADE_IN_PROGRESS,
            format!("Upgrading from {} to {}", hc.spec.release.image, target),
        );
        set_condition(cr, c);

        // Set IgnitionConfigured=False (stale for old version)
        let c = cond_gen(
            cr,
            provisioningv1alpha1::IGNITION_CONFIGURED,
            CONDITION_FALSE,
            provisioningv1alpha1::REASON_RELEASE_IMAGE_UPDATED,
            "Ignition invalidated due to release image upgrade, will regenerate after upgrade completes",
        );
        set_condition(cr, c);

        self.invalidate_ignition_state(ctx, cr).await?;

        // Sync the release image on HC and NP — this is the only place that updates
        // the release image, so the upgrade is always detected before the sync happens.
        // Skip the update if images already match (recovery path where HC spec was already synced).
        let old_image = hc.spec.release.image.clone();
        if hc.spec.release.image != target {
            log.info(
                "Updating HostedCluster release image",
                kv![
                    "hostedCluster" => hc.name(),
                    "oldImage" => hc.spec.release.image,
                    "newImage" => target
                ],
            );
            hc.spec.release.image = target.clone();
            if let Err(err) = self.client.update(&mut hc).await {
                return Err(wrapf!(
                    err,
                    "failed to update HostedCluster release image: {}",
                    err
                ));
            }
        }

        let np_key = ObjectKey::new(cr.namespace(), cr.name());
        match self.client.get::<hyperv1::NodePool>(&np_key).await {
            Err(np_err) if ctrlrt::error::is_not_found(&np_err) => {
                log.info(
                    "NodePool not found, it will be created with the target release image",
                    kv!["nodePool" => np_key.name],
                );
            }
            Err(np_err) => {
                return Err(wrapf!(np_err, "failed to get NodePool: {}", np_err));
            }
            Ok(mut np) if np.spec.release.image != target => {
                log.info(
                    "Updating NodePool release image",
                    kv![
                        "nodePool" => np.name(),
                        "oldImage" => np.spec.release.image,
                        "newImage" => target
                    ],
                );
                np.spec.release.image = target.clone();
                if let Err(err) = self.client.update(&mut np).await {
                    return Err(wrapf!(
                        err,
                        "failed to update NodePool release image: {}",
                        err
                    ));
                }
            }
            Ok(_) => {}
        }

        cr.status.blue_field_ocp_layer_image = String::new();

        // Persist status and requeue
        self.compute_ready_condition(ctx, cr).await;
        self.update_phase_from_conditions(ctx, cr).await;
        self.client.update_status(cr).await?;

        self.recorder.event(
            cr,
            EVENT_TYPE_NORMAL,
            provisioningv1alpha1::REASON_UPGRADE_IN_PROGRESS,
            &format!("Upgrade started: {old_image} → {target}"),
        );

        Ok(ReconcileResult::requeue_after(Duration::from_secs(1)))
    }

    /// isControlPlaneReady checks if the hosted control plane has finished rolling out the target
    /// version. Only checks ControlPlaneVersion history — CVO operator rollout may still be in
    /// progress. Used to gate ignition generation, which only needs the control plane (API server,
    /// ignition server) to be on the target version.
    pub(crate) fn is_control_plane_ready(
        &self,
        hc: &hyperv1::HostedCluster,
        target_release_image: &str,
    ) -> bool {
        let Some(latest) = hc.status.control_plane_version.history.first() else {
            return false;
        };
        if latest.state != configv1::COMPLETED_UPDATE {
            return false;
        }
        version_matches_image(&latest.version, &latest.image, target_release_image)
    }

    /// isHostedClusterVersionReady checks if the HostedCluster has fully completed rolling out the
    /// target release image. Requires both control plane completion AND CVO finishing all operators.
    pub(crate) fn is_hosted_cluster_version_ready(
        &self,
        hc: &hyperv1::HostedCluster,
        target_release_image: &str,
    ) -> bool {
        if !self.is_control_plane_ready(hc, target_release_image) {
            return false;
        }
        let cv_progressing = meta::find_status_condition(
            &hc.status.conditions,
            hyperv1::CLUSTER_VERSION_PROGRESSING,
        );
        !cv_progressing.is_some_and(|c| c.status == CONDITION_TRUE)
    }

    /// hasCompletedDifferentVersion checks if the HostedCluster version history contains at least
    /// one Completed entry for an image different from targetReleaseImage. This definitively proves
    /// a prior version was fully operational — making it safe to conclude an upgrade is in progress.
    fn has_completed_different_version(
        &self,
        hc: &hyperv1::HostedCluster,
        target_release_image: &str,
    ) -> bool {
        hc.status.control_plane_version.history.iter().any(|entry| {
            entry.state == configv1::COMPLETED_UPDATE
                && !version_matches_image(&entry.version, &entry.image, target_release_image)
        })
    }

    /// handleDeletion handles the deletion of a DPFHCPProvisioner CR by running finalizer cleanup
    async fn handle_deletion(
        &self,
        ctx: &Ctx,
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;
        log.info(
            "DPFHCPProvisioner is being deleted",
            kv!["namespace" => cr.namespace(), "name" => cr.name()],
        );

        // Persist the Deleting phase before removing finalizer
        if let Err(err) = self.client.update_status(cr).await {
            log.error(&err, "Failed to update status to Deleting phase", kv![]);
            return Err(err);
        }

        if !meta::contains_finalizer(cr, FINALIZER_NAME) {
            // No finalizer, nothing to clean up
            return Ok(ReconcileResult::default());
        }

        // Run finalizer cleanup
        let result = match dep(&self.finalizer_manager)
            .handle_finalizer_cleanup(ctx, cr)
            .await
        {
            Ok(r) => r,
            Err(err) => {
                log.error(&err, "Finalizer cleanup failed", kv![]);
                return Err(err);
            }
        };

        // If cleanup is still in progress (requeue requested), don't remove finalizer yet
        if !result.requeue_after.is_zero() {
            log.info(
                "Cleanup still in progress, will requeue",
                kv!["requeueAfter" => go_duration(result.requeue_after)],
            );
            return Ok(result);
        }

        // Cleanup fully completed - remove finalizer
        log.info("Removing finalizer after successful cleanup", kv![]);
        meta::remove_finalizer(cr, FINALIZER_NAME);
        if let Err(err) = self.client.update(cr).await {
            log.error(&err, "Failed to remove finalizer", kv![]);
            return Err(err);
        }

        log.info(
            "Finalizer removed, DPFHCPProvisioner will be deleted",
            kv![],
        );
        Ok(ReconcileResult::default())
    }
}

/// isUpgrading returns true if a HostedCluster upgrade is currently in progress.
pub(crate) fn is_upgrading(cr: &provisioningv1alpha1::DPFHCPProvisioner) -> bool {
    find(cr, provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING)
        .is_some_and(|c| c.status == CONDITION_TRUE)
}

fn version_matches_image(version: &str, image: &str, target_release_image: &str) -> bool {
    if target_release_image.contains(':') && !target_release_image.contains("@sha256:") {
        let mut tag = target_release_image.rsplit(':').next().unwrap_or_default();
        for suffix in [
            "-multi", "-amd64", "-arm64", "-ppc64le", "-s390x", "-x86_64",
        ] {
            tag = tag.strip_suffix(suffix).unwrap_or(tag);
        }
        if !tag.is_empty() {
            return version == tag;
        }
    }
    image == target_release_image
}

/// conditionsEqual compares two condition slices for equality
pub(crate) fn conditions_equal(old_conds: &[Condition], new_conds: &[Condition]) -> bool {
    if old_conds.len() != new_conds.len() {
        return false;
    }

    // Create maps for O(n) comparison
    let old_map: std::collections::HashMap<&str, &Condition> =
        old_conds.iter().map(|c| (c.type_.as_str(), c)).collect();

    // Compare each new condition
    for new_cond in new_conds {
        let Some(old_cond) = old_map.get(new_cond.type_.as_str()) else {
            return false;
        };
        if old_cond.status != new_cond.status
            || old_cond.reason != new_cond.reason
            || old_cond.message != new_cond.message
        {
            return false;
        }
    }

    true
}

/// dpuClusterPredicate filters DPUCluster events to watch for deletion and updates
pub(crate) fn dpu_cluster_predicate() -> Funcs<dpuprovisioningv1alpha1::DPUCluster> {
    Funcs::new()
        // Watch creation - reconcile affected DPFHCPProvisioner CR
        .on_create(|_| true)
        // Watch updates - reconcile affected DPFHCPProvisioner CR
        .on_update(|_, _| true)
        // CRITICAL: Watch deletion to alert users
        .on_delete(|_| true)
}

/// secretPredicate filters Secret events to watch for changes to referenced secrets
pub(crate) fn secret_predicate() -> Funcs<Secret> {
    Funcs::new()
        // Watch creation - reconcile affected DPFHCPProvisioner CRs
        .on_create(|_| true)
        // Watch updates - reconcile affected DPFHCPProvisioner CRs
        .on_update(|_, _| true)
        // Watch deletion - reconcile affected DPFHCPProvisioner CRs
        .on_delete(|_| true)
}

/// hostedClusterPredicate filters HostedCluster events to watch for status changes
pub(crate) fn hosted_cluster_predicate() -> Funcs<hyperv1::HostedCluster> {
    Funcs::new()
        // Watch creation - reconcile to set initial status
        .on_create(|_| true)
        .on_update(
            |old_hc: &hyperv1::HostedCluster, new_hc: &hyperv1::HostedCluster| {
                // Only reconcile if status changed (not spec)
                // This prevents unnecessary reconciliations when we update the HostedCluster spec

                // Reconcile on condition changes (status mirroring, readiness) or
                // control plane version changes (upgrade completion detection).
                if !conditions_equal(&old_hc.status.conditions, &new_hc.status.conditions) {
                    return true;
                }
                old_hc.status.control_plane_version != new_hc.status.control_plane_version
            },
        )
        // Watch deletion - reconcile to handle cleanup
        .on_delete(|_| true)
}

/// configPredicate filters DPFHCPProvisionerConfig events to only react to spec changes
pub(crate) fn config_predicate() -> GenerationChangedPredicate {
    GenerationChangedPredicate
}

/// ignitionConfigMapPredicate filters ConfigMap events to only react to ignition bfcfg template ConfigMaps.
/// Only Delete events are processed since the reconciler re-creates missing ConfigMaps.
pub(crate) fn ignition_config_map_predicate() -> Funcs<ConfigMap> {
    fn is_bfcfg_template(cm: &ConfigMap) -> bool {
        cm.metadata
            .labels
            .as_ref()
            .and_then(|l| l.get(ignition::BFCFG_TEMPLATE_LABEL))
            .is_some_and(|v| v == "true")
    }
    Funcs::new()
        .on_create(|_| false)
        .on_update(|_, _| false)
        .on_delete(is_bfcfg_template)
        .on_generic(|_| false)
}
