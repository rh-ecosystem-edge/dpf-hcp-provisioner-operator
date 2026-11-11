//! Port of internal/controller/dpuservicetemplate/dpuservicetemplate.go

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::go::Json;
use apis::openshift::config::v1 as configv1;
use common::OperatorConfig;
use ctrlrt::k8s_openapi::api::apps::v1::DaemonSet;
use ctrlrt::k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use ctrlrt::{
    Client, Ctx, ListOptions, Logger, Object, ObjectKey, OperationResult, Result, create_or_update,
    kv, wrapf,
};
use dpfdefaults::DpuServiceTemplateValues;
use gocompat::semver::blang as semver;
use registry::authn::Keychain;
use serde_json::{Value, json};

use crate::dpuservicetemplatevalues::apply_overrides_from_config_map;
use crate::release_image::{OVN_KUBERNETES_NAME, ReleaseImageReader, split_image};

pub(crate) const CLUSTER_VERSION_NAME: &str = "version";

pub(crate) const CLUSTER_PULL_SECRET_NAME: &str = "pull-secret";
pub(crate) const CLUSTER_PULL_SECRET_NAMESPACE: &str = "openshift-config";

pub(crate) const OVN_DPU_DAEMON_SET_NAME: &str = "ovnkube-node-dpu-host";
pub(crate) const OVN_DPU_DAEMON_SET_NAMESPACE: &str = "openshift-ovn-kubernetes";
pub(crate) const OVN_CONTROLLER_NAME: &str = "ovnkube-controller";

// TODO: This is an OVN-K template, not OVN, some day we should probably
// change "ovn" to "ovn-k", but then we also have to touch the
// DPUDeployment which refers to it, which resides in the openshift-dpf
// automation repo...
pub(crate) const TEMPLATE_NAME_OVNK: &str = "ovn";
pub(crate) const TEMPLATE_NAME_DTS: &str = "doca-telemetry-service";
pub(crate) const TEMPLATE_NAME_HBN: &str = "hbn";

/// AnnotationSourceOVNImage records the x86 OVN image from the DaemonSet that was used
/// to derive the current aarch64 OVN image. When this differs from the DaemonSet's
/// current image, the aarch64 image is re-resolved from the release payload.
/// On multi-arch release payloads, this is actually a manifest-list reference, but
/// we still refer to it as the "x86 OVN image" for clarity that it's what's running
/// on the x86 management cluster.
pub const ANNOTATION_SOURCE_OVN_IMAGE: &str = "dpfhcpprovisioner.dpu.hcp.io/source-x86-ovn-image";

/// DPUServiceTemplateManager handles DPUServiceTemplate resource creation and deletion.
pub struct DPUServiceTemplateManager {
    client: Client,
    api_reader: Client,
    pub release_image_reader: Arc<dyn ReleaseImageReader>,
    pub operator_namespace: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct OvnTemplateInfo {
    repo: String,
    tag: String,
    source_image: String,
    daemonset_version: String,
}

impl DPUServiceTemplateManager {
    /// NewDPUServiceTemplateManager creates a new DPUServiceTemplate manager
    pub fn new(
        c: Client,
        api_reader: Client,
        reader: Arc<dyn ReleaseImageReader>,
        operator_namespace: impl Into<String>,
    ) -> Self {
        Self {
            client: c,
            api_reader,
            release_image_reader: reader,
            operator_namespace: operator_namespace.into(),
        }
    }

    /// EnsureTemplates creates or updates the three DPUServiceTemplate resources
    /// (OVN, DTS, HBN) in the given namespace.
    pub async fn ensure_templates(
        &self,
        ctx: &Ctx,
        namespace: &str,
        operator_config: &OperatorConfig,
    ) -> Result<()> {
        let log = ctx.log.with_values(kv!["namespace" => namespace]);

        log.info("Ensuring DPUServiceTemplates", kv![]);

        let mut dpu_service_template_values = match self
            .get_dpu_service_template_values_for_current_dpf(ctx)
            .await
        {
            Ok(v) => v,
            Err(err) => {
                return Err(wrapf!(
                    err,
                    "determining DPUServiceTemplate values: {}",
                    err
                ));
            }
        };

        dpu_service_template_values.image_pull_secret =
            operator_config.dpu_services_image_pull_secret.clone();

        if let Err(err) = self
            .ensure_ovn_template(ctx, namespace, &dpu_service_template_values)
            .await
        {
            return Err(wrapf!(err, "ensuring OVN template: {}", err));
        }

        if let Err(err) = self
            .ensure_dts_template(ctx, namespace, &dpu_service_template_values)
            .await
        {
            return Err(wrapf!(err, "ensuring DTS template: {}", err));
        }

        if let Err(err) = self
            .ensure_hbn_template(ctx, namespace, &dpu_service_template_values)
            .await
        {
            return Err(wrapf!(err, "ensuring HBN template: {}", err));
        }

        log.info("DPUServiceTemplate configuration complete", kv![]);
        Ok(())
    }

    /// DeleteTemplates removes the three DPUServiceTemplate resources from the given namespace.
    pub async fn delete_templates(&self, ctx: &Ctx, namespace: &str) -> Result<()> {
        let log = ctx.log.with_values(kv!["namespace" => namespace]);

        log.info("Deleting DPUServiceTemplates", kv![]);

        let list: Vec<dpuservicev1alpha1::DPUServiceTemplate> = match self
            .client
            .list(
                &ListOptions::new()
                    .in_namespace(namespace)
                    .matching_labels([(common::LABEL_MANAGED_BY, "true")]),
            )
            .await
        {
            Ok(l) => l,
            Err(err) => {
                return Err(wrapf!(
                    err,
                    "listing managed DPUServiceTemplates in {}: {}",
                    namespace,
                    err
                ));
            }
        };

        for item in &list {
            log.info("Deleting DPUServiceTemplate", kv!["name" => item.name()]);
            if let Err(err) = self.client.delete(item).await
                && !ctrlrt::error::is_not_found(&err)
            {
                return Err(wrapf!(
                    err,
                    "deleting DPUServiceTemplate {}/{}: {}",
                    namespace,
                    item.name(),
                    err
                ));
            }
        }

        log.info("DPUServiceTemplate cleanup complete", kv![]);
        Ok(())
    }

    /// getDPUServiceTemplateValuesForCurrentDPF determines the running DPF major.minor version and
    /// returns the matching template values with any user overrides applied.
    async fn get_dpu_service_template_values_for_current_dpf(
        &self,
        ctx: &Ctx,
    ) -> Result<DpuServiceTemplateValues> {
        let dpf_version = match common::get_dpf_version(&self.client).await {
            Ok(v) => v,
            Err(err) => return Err(wrapf!(err, "determining DPF version: {}", err)),
        };
        let version = common::dpf_major_minor_from_version(&dpf_version)?;
        self.get_dpu_service_template_values(ctx, &version).await
    }

    async fn get_dpu_service_template_values(
        &self,
        ctx: &Ctx,
        version: &str,
    ) -> Result<DpuServiceTemplateValues> {
        let mut values = dpfdefaults::template_values(ctx, version)?;

        if let Err(err) = apply_overrides_from_config_map(
            ctx,
            &self.api_reader,
            &self.operator_namespace,
            version,
            &mut values,
        )
        .await
        {
            return Err(wrapf!(err, "applying overrides from configmap: {}", err));
        }
        Ok(values)
    }

    /// existingOVNKTemplateValues extracts the OVN image repo, tag, and daemonset
    /// version from an existing template's values.
    fn existing_ovnk_template_values(
        &self,
        dpu_service_template: &dpuservicev1alpha1::DPUServiceTemplate,
        source_image: &str,
    ) -> Result<OvnTemplateInfo> {
        let raw = match &dpu_service_template.spec.helm_chart.values {
            None | Some(Json::Null) => {
                return Err(anyhow::anyhow!("existing OVN template has no values"));
            }
            Some(v) => serde_json::to_vec(v)?,
        };
        let helm_values = match gocompat::json::unmarshal_object(&raw) {
            Ok(m) => m.unwrap_or_default(),
            Err(e) => {
                let e = anyhow::Error::from(e);
                return Err(wrapf!(e, "parsing existing OVN template values: {}", e));
            }
        };
        let Some(Value::Object(dpu_manifests)) = helm_values.get("dpuManifests") else {
            return Err(anyhow::anyhow!(
                "existing OVN template missing dpuManifests"
            ));
        };
        let Some(Value::Object(dpu_manifests_image)) = dpu_manifests.get("image") else {
            return Err(anyhow::anyhow!(
                "existing OVN template missing dpuManifests.image"
            ));
        };
        let as_str = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_string();
        let ovnk_image_repo = as_str(dpu_manifests_image.get("repository"));
        let ovnk_image_tag = as_str(dpu_manifests_image.get("tag"));
        if ovnk_image_repo.is_empty() || ovnk_image_tag.is_empty() {
            return Err(anyhow::anyhow!(
                "existing OVN template has empty image repository or tag"
            ));
        }
        let daemonset_version = as_str(helm_values.get("ovnDaemonsetVersion"));
        Ok(OvnTemplateInfo {
            repo: ovnk_image_repo,
            tag: ovnk_image_tag,
            source_image: source_image.to_string(),
            daemonset_version,
        })
    }

    /// resolveARM64OVNImageForCurrentDaemonSet resolves the aarch64 OVN image from
    /// the release payload for the current DaemonSet image.
    async fn resolve_arm64_ovn_image_for_current_daemon_set(
        &self,
        ctx: &Ctx,
        current_image: &str,
        release_image: &str,
        ocp_version: &str,
    ) -> Result<OvnTemplateInfo> {
        let log = &ctx.log;
        log.info(
            "Resolving aarch64 OVN image from release payload",
            kv!["sourceImage" => current_image],
        );
        let (repo, tag) = self
            .resolve_arm64_ovn_image(ctx, release_image, ocp_version)
            .await?;
        Ok(OvnTemplateInfo {
            repo,
            tag,
            source_image: current_image.to_string(),
            daemonset_version: String::new(),
        })
    }

    /// getWorkerOVNKDaemonSet reads the ovnkube-node-dpu-host DaemonSet.
    async fn get_worker_ovnk_daemon_set(&self) -> Result<DaemonSet> {
        match self
            .client
            .get::<DaemonSet>(&ObjectKey::new(
                OVN_DPU_DAEMON_SET_NAMESPACE,
                OVN_DPU_DAEMON_SET_NAME,
            ))
            .await
        {
            Ok(ds) => Ok(ds),
            Err(err) => Err(wrapf!(
                err,
                "getting DaemonSet {}/{}: {}",
                OVN_DPU_DAEMON_SET_NAMESPACE,
                OVN_DPU_DAEMON_SET_NAME,
                err
            )),
        }
    }

    /// getOCPVersionFromClusterVersion reads the OCP version and release image from ClusterVersion.
    async fn get_ocp_version_from_cluster_version(&self) -> Result<(String, String)> {
        let cv: configv1::ClusterVersion = match self
            .client
            .get(&ObjectKey::cluster(CLUSTER_VERSION_NAME))
            .await
        {
            Ok(cv) => cv,
            Err(err) => return Err(wrapf!(err, "getting ClusterVersion: {}", err)),
        };
        if cv.status.desired.version.is_empty() {
            return Err(anyhow::anyhow!(
                "ClusterVersion status.desired.version is empty"
            ));
        }
        if cv.status.desired.image.is_empty() {
            return Err(anyhow::anyhow!(
                "ClusterVersion status.desired.image is empty"
            ));
        }
        Ok((cv.status.desired.version, cv.status.desired.image))
    }

    /// resolveARM64OVNImage extracts the aarch64 ovn-kubernetes image from the release payload.
    async fn resolve_arm64_ovn_image(
        &self,
        ctx: &Ctx,
        release_image: &str,
        ocp_version: &str,
    ) -> Result<(String, String)> {
        let aarch64_ref = common::aarch64_release_image(release_image, ocp_version);

        let keychain = match self.get_cluster_pull_secret_keychain().await {
            Ok(k) => k,
            Err(err) => return Err(wrapf!(err, "getting cluster pull secret: {}", err)),
        };

        let ovn_image = match self
            .release_image_reader
            .get_component_image(ctx, &aarch64_ref, OVN_KUBERNETES_NAME, keychain)
            .await
        {
            Ok(i) => i,
            Err(err) => {
                return Err(wrapf!(
                    err,
                    "resolving aarch64 OVN image from release {}: {}",
                    gocompat::strconv::quote(&aarch64_ref),
                    err
                ));
            }
        };

        split_image(&ovn_image)
    }

    /// getClusterPullSecretKeychain reads the global cluster pull secret and returns a keychain.
    async fn get_cluster_pull_secret_keychain(&self) -> Result<Arc<dyn Keychain>> {
        common::keychain_from_pull_secret(
            &self.client,
            CLUSTER_PULL_SECRET_NAME,
            CLUSTER_PULL_SECRET_NAMESPACE,
        )
        .await
    }

    async fn ensure_ovn_template(
        &self,
        ctx: &Ctx,
        namespace: &str,
        defaults: &DpuServiceTemplateValues,
    ) -> Result<()> {
        let log = &ctx.log;

        let management_worker_ovnk_daemon_set = match self.get_worker_ovnk_daemon_set().await {
            Ok(ds) => ds,
            Err(err) => return Err(wrapf!(err, "reading OVN DaemonSet: {}", err)),
        };

        let current_ovnk_image =
            match ovnk_image_from_daemon_set(&management_worker_ovnk_daemon_set) {
                Ok(i) => i,
                Err(err) => return Err(wrapf!(err, "reading current OVN image: {}", err)),
            };

        // When the OVN image hasn't changed since the template was last updated,
        // keep the existing OVN image and daemonset version, as the daemonset
        // version is derived from the desired CVO version, which may update long
        // before the OVN DaemonSet image does. We only want to update the OVN
        // image and the ovnDaemonsetVersion identifier in lockstep when the OVN
        // image actually changes.
        // We don't return early because we still want to reconcile other changes,
        // such as user overrides.
        let mut existing = dpuservicev1alpha1::DPUServiceTemplate::default();
        let mut template_exists = false;
        let mut cno_ovnk_daemon_set_image_changed = true;
        match self
            .client
            .get::<dpuservicev1alpha1::DPUServiceTemplate>(&ObjectKey::new(
                namespace,
                TEMPLATE_NAME_OVNK,
            ))
            .await
        {
            Ok(t) => {
                existing = t;
                template_exists = true;
                let last_updated_ovnk_image = existing
                    .annotations()
                    .get(ANNOTATION_SOURCE_OVN_IMAGE)
                    .cloned()
                    .unwrap_or_default();
                cno_ovnk_daemon_set_image_changed = last_updated_ovnk_image != current_ovnk_image;
            }
            Err(err) if !ctrlrt::error::is_not_found(&err) => {
                return Err(wrapf!(err, "getting existing OVN template: {}", err));
            }
            Err(_) => {}
        }

        // Gate DaemonSet rollout check only for updates to existing templates to avoid races.
        // On first creation, proceed immediately to break the bootstrap deadlock where
        // DPU CRs can't be created without the template, and the template can't be created
        // because DPU pods (which provide SR-IOV VFs) haven't scheduled yet.
        if template_exists && !daemon_set_rollout_complete(&management_worker_ovnk_daemon_set) {
            log.info(
                "OVN DaemonSet rollout in progress, skipping template update",
                kv![],
            );
            return Ok(());
        }

        let (ocp_version, release_image) = match self.get_ocp_version_from_cluster_version().await {
            Ok(v) => v,
            Err(err) => return Err(wrapf!(err, "determining OCP version: {}", err)),
        };

        let keychain = match self.get_cluster_pull_secret_keychain().await {
            Ok(k) => k,
            Err(err) => return Err(wrapf!(err, "getting cluster pull secret: {}", err)),
        };

        let mut ovnk_template_info;
        if cno_ovnk_daemon_set_image_changed {
            if common::is_multi_arch_release_image(ctx, &release_image, keychain).await {
                log.info(
                    "Multi-arch release image detected, using management cluster OVN image directly",
                    kv!["image" => current_ovnk_image],
                );
                let (repo, tag) = match split_image(&current_ovnk_image) {
                    Ok(v) => v,
                    Err(err) => {
                        return Err(wrapf!(
                            err,
                            "splitting management cluster OVN image: {}",
                            err
                        ));
                    }
                };
                ovnk_template_info = OvnTemplateInfo {
                    repo,
                    tag,
                    source_image: current_ovnk_image.clone(),
                    daemonset_version: String::new(),
                };
            } else {
                ovnk_template_info = match self
                    .resolve_arm64_ovn_image_for_current_daemon_set(
                        ctx,
                        &current_ovnk_image,
                        &release_image,
                        &ocp_version,
                    )
                    .await
                {
                    Ok(i) => i,
                    Err(err) => {
                        return Err(wrapf!(err, "resolving OVN kubernetes image: {}", err));
                    }
                };
            }
            ovnk_template_info.daemonset_version = ovnk_daemonset_version_for_ocp(&ocp_version);
        } else {
            // The OVN image hasn't changed, so we keep the existing template
            // values for the OVN image and daemonset version, only changing the
            // other values that may have been overridden by the user.
            ovnk_template_info =
                match self.existing_ovnk_template_values(&existing, &current_ovnk_image) {
                    Ok(i) => i,
                    Err(err) => {
                        return Err(wrapf!(err, "reading existing OVN template values: {}", err));
                    }
                };
            log.v(1).info(
                "OVN DaemonSet image unchanged, keeping existing template OVN values",
                kv![],
            );
        }
        log.info(
            "Setting OVN daemonset version",
            kv!["ovnDaemonsetVersion" => ovnk_template_info.daemonset_version],
        );

        let mut dpu_manifests = json!({
            "image": {
                "repository": ovnk_template_info.repo,
                "tag": ovnk_template_info.tag,
            },
            "enabled": true,
            "cniBinDir": "/var/lib/cni/bin/",
            "cniConfDir": "/run/multus/cni/net.d",
        });
        let mut values = json!({
            "commonManifests": {"enabled": true},
            "ovnDaemonsetVersion": ovnk_template_info.daemonset_version,
            "leaseNamespace": "openshift-ovn-kubernetes",
            "gatewayOpts": "--gateway-interface=br-dpu",
        });

        static MIN_OVN_FEATURES: LazyLock<semver::Version> =
            LazyLock::new(|| semver::must_parse("4.22.2"));
        if let Ok(mut v) = semver::parse_tolerant(&ocp_version) {
            v.pre = Vec::new();
            if v.gte(&MIN_OVN_FEATURES) {
                values["global"] = json!({
                    "enableEgressIP": "true",
                    "egressIpHealthCheckPort": "0",
                    "enableEgressFirewall": "true",
                    "enableEgressQoS": "true",
                    "enableEgressService": "true",
                    "enableMultiNetwork": "false",
                    "enableMultiNetworkPolicy": "true",
                    "enableAdminNetworkPolicy": "true",
                    "enableNetworkSegmentation": "true",
                });
                dpu_manifests["ovnMultiNetworkEnable"] = json!("true");
                log.info("OCP >= 4.22.2: enabling OVN feature flags", kv![]);
            }
        }
        values["dpuManifests"] = dpu_manifests;

        let annotations = BTreeMap::from([(
            ANNOTATION_SOURCE_OVN_IMAGE.to_string(),
            ovnk_template_info.source_image.clone(),
        )]);

        self.ensure_template(
            namespace,
            TemplateSpec {
                name: TEMPLATE_NAME_OVNK,
                deployment_service_name: TEMPLATE_NAME_OVNK,
                repo_url: &defaults.ovn.chart_repo_url,
                chart_name: &defaults.ovn.chart_name,
                chart_version: &defaults.ovn.chart_version,
                values,
                resource_reqs: BTreeMap::new(),
                annotations,
            },
            log,
        )
        .await
    }

    async fn ensure_dts_template(
        &self,
        ctx: &Ctx,
        namespace: &str,
        defaults: &DpuServiceTemplateValues,
    ) -> Result<()> {
        let log = &ctx.log;

        let mut values = json!({
            "configMapData": {
                "prometheus": {
                    "port": defaults.dts.prometheus_port,
                },
            },
            "hostVolumePrefix": "/var/lib",
            "imageDTS": defaults.dts.image_dts,
        });
        if !defaults.image_pull_secret.is_empty() {
            values["imagePullSecrets"] = json!([{"name": defaults.image_pull_secret}]);
        }

        let resource_reqs = BTreeMap::from([
            ("cpu".to_string(), Quantity("1".into())),
            ("memory".to_string(), Quantity("1Gi".into())),
            ("storage".to_string(), Quantity("1Gi".into())),
        ]);

        self.ensure_template(
            namespace,
            TemplateSpec {
                name: TEMPLATE_NAME_DTS,
                deployment_service_name: TEMPLATE_NAME_DTS,
                repo_url: &defaults.dts.chart_repo_url,
                chart_name: &defaults.dts.chart_name,
                chart_version: &defaults.dts.chart_version,
                values,
                resource_reqs,
                annotations: BTreeMap::new(),
            },
            log,
        )
        .await
    }

    async fn ensure_hbn_template(
        &self,
        ctx: &Ctx,
        namespace: &str,
        defaults: &DpuServiceTemplateValues,
    ) -> Result<()> {
        let log = &ctx.log;

        let mut values = json!({
            "image": {
                "repository": defaults.hbn.image_repo,
                "tag": defaults.hbn.image_tag,
            },
            "resources": {
                "memory": "6Gi",
                "nvidia.com/bf_sf": defaults.hbn.bf_sf_count,
            },
        });
        if !defaults.image_pull_secret.is_empty() {
            values["imagePullSecrets"] = json!([{"name": defaults.image_pull_secret}]);
        }

        self.ensure_template(
            namespace,
            TemplateSpec {
                name: TEMPLATE_NAME_HBN,
                deployment_service_name: TEMPLATE_NAME_HBN,
                repo_url: &defaults.hbn.chart_repo_url,
                chart_name: &defaults.hbn.chart_name,
                chart_version: &defaults.hbn.chart_version,
                values,
                resource_reqs: BTreeMap::new(),
                annotations: BTreeMap::new(),
            },
            log,
        )
        .await
    }

    async fn ensure_template(
        &self,
        namespace: &str,
        spec: TemplateSpec<'_>,
        log: &Logger,
    ) -> Result<()> {
        let TemplateSpec {
            name,
            deployment_service_name,
            repo_url,
            chart_name,
            chart_version,
            values,
            resource_reqs,
            annotations,
        } = spec;

        // json.Marshal(map[string]any): keys sorted (serde_json maps are sorted).
        let values_json = Json::from(values);

        let mut template = dpuservicev1alpha1::DPUServiceTemplate {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        // Check if a user-created template with this name already exists
        match self
            .client
            .get::<dpuservicev1alpha1::DPUServiceTemplate>(&ObjectKey::new(namespace, name))
            .await
        {
            Ok(existing) => {
                template = existing;
                if template
                    .labels()
                    .get(common::LABEL_MANAGED_BY)
                    .map(String::as_str)
                    != Some("true")
                {
                    log.info(
                        "Skipping DPUServiceTemplate not managed by operator",
                        kv!["name" => name],
                    );
                    return Ok(());
                }
                if template.is_being_deleted() {
                    log.info(
                        "Skipping DPUServiceTemplate that is being deleted",
                        kv!["name" => name],
                    );
                    return Ok(());
                }
            }
            Err(err) if !ctrlrt::error::is_not_found(&err) => {
                return Err(wrapf!(
                    err,
                    "checking existing DPUServiceTemplate {}: {}",
                    name,
                    err
                ));
            }
            Err(_) => {}
        }

        let op = create_or_update(&self.client, &mut template, |template| {
            template
                .metadata
                .labels
                .get_or_insert_with(BTreeMap::new)
                .insert(common::LABEL_MANAGED_BY.to_string(), "true".to_string());
            if !annotations.is_empty() {
                let a = template
                    .metadata
                    .annotations
                    .get_or_insert_with(BTreeMap::new);
                for (k, v) in &annotations {
                    a.insert(k.clone(), v.clone());
                }
            }
            template.spec = dpuservicev1alpha1::DPUServiceTemplateSpec {
                deployment_service_name: deployment_service_name.to_string(),
                helm_chart: dpuservicev1alpha1::HelmChart {
                    source: dpuservicev1alpha1::ApplicationSource {
                        repo_url: repo_url.to_string(),
                        chart: chart_name.to_string(),
                        version: chart_version.to_string(),
                        ..Default::default()
                    },
                    values: Some(values_json.clone()),
                    ..Default::default()
                },
                resource_requirements: resource_reqs.clone(),
                ..Default::default()
            };
            Ok(())
        })
        .await;

        let op = match op {
            Ok(op) => op,
            Err(err) => {
                return Err(wrapf!(
                    err,
                    "creating or updating DPUServiceTemplate {}: {}",
                    name,
                    err
                ));
            }
        };

        match op {
            OperationResult::Created => log.info(
                "Created DPUServiceTemplate",
                kv!["name" => name, "namespace" => namespace],
            ),
            OperationResult::Updated => log.info(
                "Updated DPUServiceTemplate (drift corrected)",
                kv!["name" => name],
            ),
            OperationResult::None => log.v(1).info(
                "DPUServiceTemplate already matches desired state",
                kv!["name" => name],
            ),
            _ => {}
        }

        Ok(())
    }
}

/// The arguments of Go's `ensureTemplate` (besides ctx/namespace/log).
struct TemplateSpec<'a> {
    name: &'a str,
    deployment_service_name: &'a str,
    repo_url: &'a str,
    chart_name: &'a str,
    chart_version: &'a str,
    values: Value,
    resource_reqs: BTreeMap<String, Quantity>,
    annotations: BTreeMap<String, String>,
}

fn ovnk_image_from_daemon_set(daemon_set: &DaemonSet) -> Result<String> {
    let containers = daemon_set
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .map(|p| p.containers.as_slice())
        .unwrap_or(&[]);
    for container in containers {
        if container.name == OVN_CONTROLLER_NAME {
            return Ok(container.image.clone().unwrap_or_default());
        }
    }
    Err(anyhow::anyhow!(
        "container {} not found in DaemonSet {}/{}",
        gocompat::strconv::quote(OVN_CONTROLLER_NAME),
        OVN_DPU_DAEMON_SET_NAMESPACE,
        OVN_DPU_DAEMON_SET_NAME
    ))
}

fn daemon_set_rollout_complete(ds: &DaemonSet) -> bool {
    let status = ds.status.clone().unwrap_or_default();
    let observed_generation = status.observed_generation.unwrap_or(0);
    let updated_number_scheduled = status.updated_number_scheduled.unwrap_or(0);
    let number_available = status.number_available.unwrap_or(0);
    observed_generation >= ds.generation()
        && updated_number_scheduled == status.desired_number_scheduled
        && number_available == status.desired_number_scheduled
}

/// ovnDaemonsetVersionEntry maps a minimum OCP version to the OVN daemonset version
/// that was introduced at that release. Entries are sorted ascending by version at init.
struct OvnDaemonsetVersionEntry {
    min_version: semver::Version,
    daemonset_version: &'static str,
}

/// ovnDaemonsetVersionTable lists the OCP versions where OVN_DAEMONSET_VERSION changed.
/// Each stream (4.21, 4.22, …) can have independent entries because OCP backports
/// newer OVN builds into older minor releases at arbitrary z-stream versions.
static OVN_DAEMONSET_VERSION_TABLE: LazyLock<Vec<OvnDaemonsetVersionEntry>> = LazyLock::new(|| {
    let raw: [(&str, &'static str); 2] = [
        // We only really support OCP versions starting from 4.22.6 but leaving
        // 4.22.0 and 4.22.1 here for completeness in case someone tries to run
        // on those versions, and to serve as an example of how to add future
        // entries for new OCP versions.
        ("4.22.0", "1.2.0"),
        ("4.22.1", "1.3.0"),
    ];
    let mut table: Vec<OvnDaemonsetVersionEntry> = raw
        .iter()
        .map(|(v, dv)| OvnDaemonsetVersionEntry {
            min_version: semver::must_parse(v),
            daemonset_version: dv,
        })
        .collect();
    table.sort_by(|a, b| a.min_version.compare(&b.min_version).cmp(&0));
    table
});

/// OVNKDaemonsetVersionForOCP returns the OVN_DAEMONSET_VERSION value for a given
/// OCP version string. It walks the table in reverse and returns the daemonset
/// version from the highest entry whose minVersion <= ocpVersion.
pub fn ovnk_daemonset_version_for_ocp(ocp_version: &str) -> String {
    const DEFAULT_VERSION: &str = "1.3.0";

    let Ok(mut v) = semver::parse_tolerant(ocp_version) else {
        return DEFAULT_VERSION.to_string();
    };
    // Strip pre-release so that e.g. "4.22.1-rc.1" matches >= 4.22.1
    v.pre = Vec::new();

    let mut result = DEFAULT_VERSION;
    for e in OVN_DAEMONSET_VERSION_TABLE.iter() {
        if v.gte(&e.min_version) {
            result = e.daemonset_version;
        }
    }
    result.to_string()
}
