//! Port of internal/controller/dpuservicetemplate/controller.go

use std::collections::HashSet;
use std::sync::Arc;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use async_trait::async_trait;
use ctrlrt::k8s_openapi::api::apps::v1::DaemonSet;
use ctrlrt::k8s_openapi::api::core::v1::ConfigMap;
use ctrlrt::reconcile::{WatchSpec, enqueue_from_map_fn};
use ctrlrt::{
    Client, ControllerSpec, Ctx, Funcs, GenerationChangedPredicate, ManagerHandle, MapFn, Object,
    ObjectKey, Predicate, ReconcileResult, Reconciler, Request, Result, kv, map_fn,
};

use crate::dpuservicetemplate::{
    DPUServiceTemplateManager, OVN_DPU_DAEMON_SET_NAME, OVN_DPU_DAEMON_SET_NAMESPACE,
};
use crate::dpuservicetemplatevalues::OVERRIDES_CONFIG_MAP_NAME;

/// The controller name (`Named("dpuservicetemplate")`).
pub const CONTROLLER_NAME: &str = "dpuservicetemplate";

/// DPUServiceTemplateReconciler manages DPUServiceTemplate resources based on
/// DPFHCPProvisioner and DPUDeployment presence. Templates are created when an
/// active provisioner references a non-deleting DPUDeployment in the DPUCluster
/// namespace, and deleted when no provisioner or DPUDeployment remains.
pub struct DPUServiceTemplateReconciler {
    pub client: Client,
    pub manager: Arc<DPUServiceTemplateManager>,
}

#[async_trait]
impl Reconciler for DPUServiceTemplateReconciler {
    /// Reconcile ensures DPUServiceTemplates are in sync for a given DPUCluster namespace.
    async fn reconcile(&self, ctx: &Ctx, req: Request) -> Result<ReconcileResult> {
        // The reconcile request's Name is the DPUCluster namespace (not a real object key).
        // See SetupWithManager mappings for more info
        let dpu_cluster_ns = req.name;
        let log = ctx
            .log
            .with_values(kv!["dpuClusterNamespace" => dpu_cluster_ns]);

        // TODO: Remove this when we remove the flag which turns off DPUServiceTemplate management.
        let operator_config = match common::load_operator_config_from_cr(ctx, &self.client).await {
            Ok(c) => c,
            Err(err) => {
                log.error(&err, "Failed to load operator configuration", kv![]);
                return Err(err);
            }
        };
        let Some(operator_config) = operator_config.filter(|c| c.manage_dpu_service_templates)
        else {
            log.v(1)
                .info("DPUServiceTemplate management is disabled", kv![]);
            return Ok(ReconcileResult::default());
        };

        let all_provisioners: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            self.client.list_all().await?;

        let deployments = self
            .active_dpu_deployments(&all_provisioners, &dpu_cluster_ns)
            .await?;
        if deployments.is_empty() {
            if let Err(err) = self.manager.delete_templates(ctx, &dpu_cluster_ns).await {
                log.error(&err, "Failed to delete DPUServiceTemplates", kv![]);
                return Err(err);
            }
            return Ok(ReconcileResult::default());
        }

        if let Err(err) = self
            .manager
            .ensure_templates(ctx, &dpu_cluster_ns, &operator_config)
            .await
        {
            log.error(&err, "Failed to ensure DPUServiceTemplates", kv![]);
            return Err(err);
        }
        Ok(ReconcileResult::default())
    }
}

impl DPUServiceTemplateReconciler {
    pub fn new(client: Client, manager: Arc<DPUServiceTemplateManager>) -> Self {
        Self { client, manager }
    }

    /// activeDPUDeployments returns non-deleting DPUDeployments referenced by
    /// active provisioners for this DPUCluster namespace.
    async fn active_dpu_deployments(
        &self,
        all_provisioners: &[provisioningv1alpha1::DPFHCPProvisioner],
        dpu_cluster_namespace: &str,
    ) -> Result<Vec<dpuservicev1alpha1::DPUDeployment>> {
        let mut seen: HashSet<ObjectKey> = HashSet::new();
        let mut deployments = Vec::new();

        for p in all_provisioners {
            if p.is_being_deleted() || p.spec.dpu_cluster_ref.namespace != dpu_cluster_namespace {
                continue;
            }
            let Some(dpu_deployment_ref) = &p.spec.dpu_deployment_ref else {
                continue;
            };

            let key = ObjectKey::new(
                dpu_deployment_ref.namespace.clone(),
                dpu_deployment_ref.name.clone(),
            );
            if seen.contains(&key) {
                continue;
            }

            let dpu_deployment = match self
                .client
                .get::<dpuservicev1alpha1::DPUDeployment>(&key)
                .await
            {
                Ok(d) => d,
                Err(err) if ctrlrt::error::is_not_found(&err) => continue,
                Err(err) => return Err(err),
            };
            if dpu_deployment.is_being_deleted() {
                continue;
            }

            seen.insert(key);
            deployments.push(dpu_deployment);
        }

        Ok(deployments)
    }

    /// SetupWithManager registers this controller with the manager.
    ///
    /// The Go controller has no `For()` type, only `Watches()`; the resulting
    /// [`ControllerSpec`] therefore carries an empty `for_resource`.
    pub fn setup_with_manager(self: &Arc<Self>, mgr: &dyn ManagerHandle) -> Result<()> {
        mgr.add_controller(self.controller_spec())
    }

    /// The controller description built by [`Self::setup_with_manager`].
    pub fn controller_spec(self: &Arc<Self>) -> ControllerSpec {
        let operator_namespace = self.manager.operator_namespace.clone();

        let r = self.clone();
        let provisioner_map: MapFn<provisioningv1alpha1::DPFHCPProvisioner> = map_fn(
            move |_ctx, obj: Arc<provisioningv1alpha1::DPFHCPProvisioner>| {
                let reqs = r.provisioner_to_namespace(&obj);
                async move { reqs }
            },
        );

        let r = self.clone();
        let dpu_deployment_map: MapFn<dpuservicev1alpha1::DPUDeployment> =
            map_fn(move |ctx, obj: Arc<dpuservicev1alpha1::DPUDeployment>| {
                let r = r.clone();
                async move { r.dpu_deployment_to_namespace(&ctx, &obj).await }
            });

        let r = self.clone();
        let daemon_set_map: MapFn<DaemonSet> = map_fn(move |ctx, _obj: Arc<DaemonSet>| {
            let r = r.clone();
            async move { r.all_provisioner_dpu_cluster_namespaces(&ctx).await }
        });

        let r = self.clone();
        let config_map_fn: MapFn<provisioningv1alpha1::DPFHCPProvisionerConfig> = map_fn(
            move |ctx, _obj: Arc<provisioningv1alpha1::DPFHCPProvisionerConfig>| {
                let r = r.clone();
                async move { r.all_provisioner_dpu_cluster_namespaces(&ctx).await }
            },
        );

        let r = self.clone();
        let overrides_map: MapFn<ConfigMap> = map_fn(move |ctx, _obj: Arc<ConfigMap>| {
            let r = r.clone();
            async move { r.all_provisioner_dpu_cluster_namespaces(&ctx).await }
        });

        let template_map: MapFn<dpuservicev1alpha1::DPUServiceTemplate> =
            map_fn(|_ctx, obj: Arc<dpuservicev1alpha1::DPUServiceTemplate>| {
                let reqs = template_to_namespace(&obj);
                async move { reqs }
            });

        let watches = vec![
            watch::<provisioningv1alpha1::DPFHCPProvisioner>(provisioner_map, vec![]),
            watch::<dpuservicev1alpha1::DPUDeployment>(dpu_deployment_map, vec![]),
            // We only care about the OVN DaemonSet
            watch::<DaemonSet>(daemon_set_map, vec![Arc::new(ovn_daemon_set_predicate())]),
            // TODO: Remove this when we remove the flag which turns off DPUServiceTemplate management. Currently We need to watch the config to trigger reconciles when the flag is toggled.
            // Avoids status update reconciliations
            watch::<provisioningv1alpha1::DPFHCPProvisionerConfig>(
                config_map_fn,
                vec![Arc::new(GenerationChangedPredicate)],
            ),
            watch::<ConfigMap>(
                overrides_map,
                vec![Arc::new(overrides_config_map_predicate(operator_namespace))],
            ),
            watch::<dpuservicev1alpha1::DPUServiceTemplate>(template_map, vec![]),
        ];

        ControllerSpec {
            name: CONTROLLER_NAME.to_string(),
            // No For(): Go builds this controller with Named() + Watches() only.
            for_resource: None,
            watches,
            max_concurrent_reconciles: 1,
            reconciler: self.clone() as Arc<dyn Reconciler>,
        }
    }

    /// provisionerToNamespace maps a DPFHCPProvisioner event to a reconcile request
    /// keyed by the DPUCluster namespace.
    pub fn provisioner_to_namespace(
        &self,
        provisioner: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Vec<Request> {
        vec![ObjectKey::new(
            "",
            provisioner.spec.dpu_cluster_ref.namespace.clone(),
        )]
    }

    /// allProvisionerDPUClusterNamespaces finds all namespaces managed by all the
    /// DPFHCPProvisioners and creates a reconcile.Request for each unique
    /// namespace, then returns the list of the requests.
    pub async fn all_provisioner_dpu_cluster_namespaces(&self, ctx: &Ctx) -> Vec<Request> {
        let log = &ctx.log;

        let provisioners: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            match self.client.list_all().await {
                Ok(l) => l,
                Err(err) => {
                    log.error(&err, "Failed to list DPFHCPProvisioners", kv![]);
                    return vec![];
                }
            };

        // Multiple provisioners might reference the same DPUCluster namespace, so
        // we need to deduplicate the requests with this ad-hoc set.
        let mut seen_namespaces_set: HashSet<String> = HashSet::new();

        let mut requests = Vec::new();
        for provisioner in &provisioners {
            // The namespace of the provisioner itself is not relevant, we care
            // about the namespace of the DPUCluster it references.
            let provisioner_managed_namespace = &provisioner.spec.dpu_cluster_ref.namespace;
            if seen_namespaces_set.contains(provisioner_managed_namespace) {
                continue;
            }

            // Mark set as seen so we don't generate duplicate requests for the same namespace.
            seen_namespaces_set.insert(provisioner_managed_namespace.clone());

            // Finally generate a request so we reconcile that namespace.
            requests.push(ObjectKey::new("", provisioner_managed_namespace.clone()));
        }

        requests
    }

    /// dpuDeploymentToNamespace maps a DPUDeployment event to reconcile requests for
    /// DPUCluster namespaces of provisioners that reference the DPUDeployment.
    pub async fn dpu_deployment_to_namespace(
        &self,
        ctx: &Ctx,
        dpu_deployment: &dpuservicev1alpha1::DPUDeployment,
    ) -> Vec<Request> {
        let provisioners: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            match self.client.list_all().await {
                Ok(l) => l,
                Err(err) => {
                    ctx.log.error(
                        &err,
                        "Failed to list DPFHCPProvisioners for DPUDeployment watch",
                        kv![],
                    );
                    return vec![];
                }
            };

        let mut seen_namespaces_set: HashSet<String> = HashSet::new();
        let mut requests = Vec::new();
        for p in &provisioners {
            let Some(r) = &p.spec.dpu_deployment_ref else {
                continue;
            };
            if r.name != dpu_deployment.name() || r.namespace != dpu_deployment.namespace() {
                continue;
            }

            let ns = &p.spec.dpu_cluster_ref.namespace;
            if seen_namespaces_set.contains(ns) {
                continue;
            }
            seen_namespaces_set.insert(ns.clone());
            requests.push(ObjectKey::new("", ns.clone()));
        }
        requests
    }
}

/// templateToNamespace maps a DPUServiceTemplate event to a reconcile request.
/// This controller reconciles all templates in a namespace at once, so the reconcile
/// key is the namespace name (not an individual template name).
pub fn template_to_namespace(obj: &dpuservicev1alpha1::DPUServiceTemplate) -> Vec<Request> {
    vec![ObjectKey::new("", obj.namespace())]
}

fn watch<K: Object>(map: MapFn<K>, preds: Vec<Arc<dyn Predicate<K>>>) -> WatchSpec {
    WatchSpec {
        resource: K::resource_info(),
        handler: enqueue_from_map_fn::<K>(map, preds),
    }
}

/// `overridesConfigMapPredicate(operatorNamespace)`
pub fn overrides_config_map_predicate(operator_namespace: String) -> Funcs<ConfigMap> {
    let ns = Arc::new(operator_namespace);
    let is_overrides_config_map = move |obj: &ConfigMap| {
        obj.name() == OVERRIDES_CONFIG_MAP_NAME && obj.namespace() == ns.as_str()
    };
    let (c, u, d) = (
        is_overrides_config_map.clone(),
        is_overrides_config_map.clone(),
        is_overrides_config_map,
    );
    Funcs::new()
        .on_create(move |o| c(o))
        .on_update(move |_old, new| u(new))
        .on_delete(move |o| d(o))
}

/// `ovnDaemonSetPredicate()`
pub fn ovn_daemon_set_predicate() -> Funcs<DaemonSet> {
    fn is_ovn_daemon_set(obj: &DaemonSet) -> bool {
        obj.name() == OVN_DPU_DAEMON_SET_NAME && obj.namespace() == OVN_DPU_DAEMON_SET_NAMESPACE
    }
    Funcs::new()
        .on_create(is_ovn_daemon_set)
        .on_update(|_old, new| is_ovn_daemon_set(new))
        .on_delete(|_| false)
}
