//! Port of internal/controller/dpuservicetemplate/dpuservicetemplate_test.go
//! (package `dpuservicetemplate_test`: "DPUServiceTemplate Manager" and
//! "DPUServiceTemplate Reconciler" suites). The Ginkgo bootstrap (suite_test.go) has no
//! Rust counterpart: each spec is a plain `#[tokio::test]`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::dpf::operator::v1alpha1 as operatorv1alpha1;
use apis::openshift::config::v1 as configv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use async_trait::async_trait;
use common::OperatorConfig;
use ctrl_dpuservicetemplate::*;
use ctrlrt::k8s_openapi::ByteString;
use ctrlrt::k8s_openapi::api::apps::v1::{DaemonSet, DaemonSetSpec, DaemonSetStatus};
use ctrlrt::k8s_openapi::api::core::v1::{ConfigMap, Container, PodSpec, PodTemplateSpec, Secret};
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use ctrlrt::{Client, Ctx, ObjectKey, Reconciler};
use ctrlrt_fake::{FakeClientBuilder, InitObject, Scheme, obj};
use registry::authn::Keychain;
use serde_json::{Map, Value};

const TEST_OPERATOR_NAMESPACE: &str = "dpf-hcp-provisioner-system";

#[derive(Default)]
struct FakeReleaseImageReader {
    image: Mutex<String>,
    err: Option<String>,
    call_count: AtomicUsize,
}

impl FakeReleaseImageReader {
    fn new(image: &str) -> Arc<Self> {
        Arc::new(Self {
            image: Mutex::new(image.to_string()),
            ..Default::default()
        })
    }
    fn with_err(err: &str) -> Arc<Self> {
        Arc::new(Self {
            err: Some(err.to_string()),
            ..Default::default()
        })
    }
    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
    fn set_image(&self, image: &str) {
        *self.image.lock().unwrap() = image.to_string();
    }
}

#[async_trait]
impl ReleaseImageReader for FakeReleaseImageReader {
    async fn get_component_image(
        &self,
        _ctx: &Ctx,
        _release_image_ref: &str,
        _component_name: &str,
        _keychain: Arc<dyn Keychain>,
    ) -> ctrlrt::Result<String> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        match &self.err {
            Some(e) => Err(anyhow::anyhow!(e.clone())),
            None => Ok(self.image.lock().unwrap().clone()),
        }
    }
}

fn meta(name: &str, namespace: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.into()),
        namespace: (!namespace.is_empty()).then(|| namespace.to_string()),
        ..Default::default()
    }
}

fn new_dpf_operator_config(version: &str) -> operatorv1alpha1::DPFOperatorConfig {
    let mut c = operatorv1alpha1::DPFOperatorConfig {
        metadata: meta("dpfoperatorconfig", "dpf-operator-system"),
        ..Default::default()
    };
    c.status.version = Some(version.into());
    c
}

fn new_cluster_version(image: &str) -> configv1::ClusterVersion {
    let mut cv = configv1::ClusterVersion {
        metadata: meta("version", ""),
        ..Default::default()
    };
    cv.spec.cluster_id = "test-cluster".into();
    cv.status.desired.image = image.into();
    cv.status.desired.version = "4.19.0-ec.5".into();
    cv
}

fn new_pull_secret() -> Secret {
    Secret {
        metadata: meta("pull-secret", "openshift-config"),
        data: Some(BTreeMap::from([(
            ".dockerconfigjson".to_string(),
            ByteString(br#"{"auths":{}}"#.to_vec()),
        )])),
        type_: Some("kubernetes.io/dockerconfigjson".into()),
        ..Default::default()
    }
}

fn new_ovn_daemon_set(image: &str) -> DaemonSet {
    let labels = BTreeMap::from([("app".to_string(), "ovnkube".to_string())]);
    DaemonSet {
        metadata: meta("ovnkube-node-dpu-host", "openshift-ovn-kubernetes"),
        spec: Some(DaemonSetSpec {
            selector: LabelSelector {
                match_labels: Some(labels.clone()),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![Container {
                        name: "ovnkube-controller".into(),
                        image: Some(image.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn set_ds_rollout(ds: &mut DaemonSet, generation: i64, desired: i32, updated: i32, available: i32) {
    ds.metadata.generation = Some(generation);
    ds.status = Some(DaemonSetStatus {
        observed_generation: Some(generation),
        desired_number_scheduled: desired,
        updated_number_scheduled: Some(updated),
        number_available: Some(available),
        ..Default::default()
    });
}

fn new_test_scheme() -> Scheme {
    Scheme::new()
        .register::<provisioningv1alpha1::DPFHCPProvisioner>()
        .register::<provisioningv1alpha1::DPFHCPProvisionerConfig>()
        .register::<dpuservicev1alpha1::DPUServiceTemplate>()
        .register::<dpuservicev1alpha1::DPUDeployment>()
        .register::<operatorv1alpha1::DPFOperatorConfig>()
        .register::<configv1::ClusterVersion>()
        .register::<DaemonSet>()
        .register::<Secret>()
        .register::<ConfigMap>()
}

/// allPrereqs returns the standard set of objects needed for EnsureTemplates
/// with a single-arch release image (triggers aarch64 resolution from release payload).
fn all_prereqs(ovn_image: &str) -> Vec<InitObject> {
    all_prereqs_with_release(
        ovn_image,
        "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5",
    )
}

fn all_prereqs_with_release(ovn_image: &str, release_image: &str) -> Vec<InitObject> {
    vec![
        obj(&new_dpf_operator_config("26.4.0-f314aa17")),
        obj(&new_cluster_version(release_image)),
        obj(&new_pull_secret()),
        obj(&new_ovn_daemon_set(ovn_image)),
    ]
}

fn new_operator_config_cr() -> provisioningv1alpha1::DPFHCPProvisionerConfig {
    let mut c = provisioningv1alpha1::DPFHCPProvisionerConfig {
        metadata: meta(provisioningv1alpha1::DEFAULT_CONFIG_NAME, ""),
        ..Default::default()
    };
    c.spec.manage_dpu_service_templates = true;
    c
}

fn new_test_provisioner(
    name: &str,
    dpu_cluster_ns: &str,
    dpu_deployment_name: &str,
    dpu_deployment_ns: &str,
) -> provisioningv1alpha1::DPFHCPProvisioner {
    let mut p = provisioningv1alpha1::DPFHCPProvisioner {
        metadata: meta(name, "default"),
        ..Default::default()
    };
    p.spec.dpu_cluster_ref = provisioningv1alpha1::DPUClusterReference {
        name: "cluster".into(),
        namespace: dpu_cluster_ns.into(),
        ..Default::default()
    };
    p.spec.dpu_deployment_ref = Some(provisioningv1alpha1::DPUDeploymentReference {
        name: dpu_deployment_name.into(),
        namespace: dpu_deployment_ns.into(),
        ..Default::default()
    });
    p
}

fn new_test_dpu_deployment(name: &str, namespace: &str) -> dpuservicev1alpha1::DPUDeployment {
    let mut d = dpuservicev1alpha1::DPUDeployment {
        metadata: meta(name, namespace),
        ..Default::default()
    };
    d.spec.dpus.bfb = Some("bfb".into());
    d.spec.dpus.flavor = Some("flavor".into());
    d
}

fn new_deleting_dpu_deployment(name: &str, namespace: &str) -> dpuservicev1alpha1::DPUDeployment {
    let mut dd = new_test_dpu_deployment(name, namespace);
    dd.metadata.deletion_timestamp = Some(ctrlrt::meta::now());
    dd.metadata.finalizers = Some(vec![dpuservicev1alpha1::DPU_DEPLOYMENT_FINALIZER.into()]);
    dd
}

fn new_template(
    name: &str,
    namespace: &str,
    labels: Option<BTreeMap<String, String>>,
    repo_url: &str,
    version: &str,
) -> dpuservicev1alpha1::DPUServiceTemplate {
    let mut t = dpuservicev1alpha1::DPUServiceTemplate {
        metadata: meta(name, namespace),
        ..Default::default()
    };
    t.metadata.labels = labels;
    t.spec.deployment_service_name = name.into();
    t.spec.helm_chart.source.repo_url = repo_url.into();
    t.spec.helm_chart.source.version = version.into();
    t
}

fn managed_labels() -> Option<BTreeMap<String, String>> {
    Some(BTreeMap::from([(
        "dpfhcpprovisioner.dpu.hcp.io/managed".to_string(),
        "true".to_string(),
    )]))
}

fn new_managed_template(name: &str, namespace: &str) -> dpuservicev1alpha1::DPUServiceTemplate {
    new_template(
        name,
        namespace,
        Some(BTreeMap::from([(
            common::LABEL_MANAGED_BY.to_string(),
            "true".to_string(),
        )])),
        "https://example.com",
        "1",
    )
}

const TARGET_NAMESPACE: &str = "dpf-operator-system";
const X86_OVN_IMAGE: &str = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:aaa111";
const ARM64_OVN_IMAGE: &str = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:bbb222";
const DEPLOYMENT_NAME: &str = "hcp-dpu-deployment";

/// `fake.NewClientBuilder().WithScheme(scheme).WithObjects(objs...)[.WithStatusSubresource(&DPFOperatorConfig{})].Build()`
fn build(objs: Vec<InitObject>, status_subresource: bool) -> Client {
    let mut b = FakeClientBuilder::new()
        .with_scheme(new_test_scheme())
        .with_objects(objs);
    if status_subresource {
        b = b.with_status_subresource::<operatorv1alpha1::DPFOperatorConfig>();
    }
    b.build()
}

fn new_manager(c: &Client, reader: Arc<FakeReleaseImageReader>) -> DPUServiceTemplateManager {
    DPUServiceTemplateManager::new(c.clone(), c.clone(), reader, TEST_OPERATOR_NAMESPACE)
}

async fn get_template(
    c: &Client,
    name: &str,
) -> ctrlrt::Result<dpuservicev1alpha1::DPUServiceTemplate> {
    c.get(&ObjectKey::new(TARGET_NAMESPACE, name)).await
}

/// `json.Unmarshal(template.Spec.HelmChart.Values.Raw, &values)`
fn helm_values(t: &dpuservicev1alpha1::DPUServiceTemplate) -> Map<String, Value> {
    let v = t
        .spec
        .helm_chart
        .values
        .as_ref()
        .expect("values")
        .to_value();
    match v {
        Value::Object(m) => m,
        other => panic!("values not an object: {other}"),
    }
}

fn ovn_image_of(values: &Map<String, Value>) -> (Value, Value) {
    let image = &values["dpuManifests"]["image"];
    (image["repository"].clone(), image["tag"].clone())
}

fn ctx() -> Ctx {
    Ctx::background()
}

// =============================================================================================
// Describe("DPUServiceTemplate Manager") / Describe("EnsureTemplates")
// =============================================================================================

#[tokio::test]
async fn manager_ensure_templates_when_all_prerequisites_are_available_should_create_all_three_dpu_service_templates_with_the_source_annotation()
 {
    let fake_client = build(all_prereqs(X86_OVN_IMAGE), true);
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader.clone());

    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");

    // Verify OVN template
    let ovn_template = get_template(&fake_client, "ovn").await.expect("ovn");
    assert_eq!(ovn_template.spec.deployment_service_name, "ovn");
    assert_eq!(
        ovn_template.spec.helm_chart.source.chart,
        "ovn-kubernetes-chart"
    );

    // Verify the source annotation records the x86 DaemonSet image
    assert_eq!(
        ovn_template
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(ANNOTATION_SOURCE_OVN_IMAGE))
            .map(String::as_str),
        Some(X86_OVN_IMAGE)
    );

    // Verify OVN values contain the aarch64 image
    let ovn_values = helm_values(&ovn_template);
    let (repo, tag) = ovn_image_of(&ovn_values);
    assert_eq!(
        repo,
        "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256"
    );
    assert_eq!(tag, "bbb222");

    // Verify DTS template
    let dts_template = get_template(&fake_client, "doca-telemetry-service")
        .await
        .expect("dts");
    assert_eq!(
        dts_template.spec.deployment_service_name,
        "doca-telemetry-service"
    );
    assert_eq!(dts_template.spec.helm_chart.source.chart, "doca-telemetry");
    assert!(!dts_template.spec.resource_requirements.is_empty());

    // Verify HBN template
    let hbn_template = get_template(&fake_client, "hbn").await.expect("hbn");
    assert_eq!(hbn_template.spec.deployment_service_name, "hbn");
    assert_eq!(hbn_template.spec.helm_chart.source.chart, "doca-hbn");

    assert_eq!(reader.call_count(), 1);
}

#[tokio::test]
async fn manager_ensure_templates_when_ovn_daemon_set_image_has_not_changed_should_skip_the_release_payload_pull()
 {
    let fake_client = build(all_prereqs(X86_OVN_IMAGE), true);
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader.clone());

    // First call resolves from release payload
    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");
    assert_eq!(reader.call_count(), 1);

    // Second call should skip the pull (annotation matches DaemonSet image)
    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");
    assert_eq!(reader.call_count(), 1);
}

#[tokio::test]
async fn manager_ensure_templates_when_ovn_daemon_set_image_has_not_changed_should_not_advance_ovn_daemonset_version_even_when_cvo_desired_version_changes()
 {
    let fake_client = build(all_prereqs(X86_OVN_IMAGE), true);
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader.clone());

    // First call creates templates with OCP 4.19.0-ec.5 → daemonset version 1.3.0
    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");

    let ovn_template = get_template(&fake_client, "ovn").await.expect("ovn");
    assert_eq!(helm_values(&ovn_template)["ovnDaemonsetVersion"], "1.3.0");

    // Simulate CVO desired version jumping to 4.22.1 (daemonset version 1.3.0),
    // but the DaemonSet image hasn't changed yet (CNO hasn't rolled out)
    let mut cv = new_cluster_version("quay.io/openshift-release-dev/ocp-release:4.22.1");
    cv.status.desired.version = "4.22.1".into();
    fake_client
        .delete(&new_cluster_version(
            "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5",
        ))
        .await
        .expect("delete");
    fake_client.create(&mut cv).await.expect("create");

    // Reconcile again — image unchanged, daemonset version should NOT advance
    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");

    let ovn_template = get_template(&fake_client, "ovn").await.expect("ovn");
    assert_eq!(helm_values(&ovn_template)["ovnDaemonsetVersion"], "1.3.0");
}

#[tokio::test]
async fn manager_ensure_templates_on_fresh_install_when_daemon_set_pods_are_pending_bootstrap_deadlock_should_create_the_ovn_template_even_though_rollout_is_not_complete()
 {
    // Simulate: DaemonSet exists (created by CNO) but pods are Pending because
    // SR-IOV VF resources aren't advertised yet (no DPU CRs exist yet).
    // With the old unconditional gate this would loop forever and never create
    // the template, blocking DPUDeployment prereq check → DPUSet → DPU CRs.
    let mut ds = new_ovn_daemon_set(X86_OVN_IMAGE);
    set_ds_rollout(&mut ds, 1, 3, 0, 0);

    let fake_client = build(
        vec![
            obj(&new_dpf_operator_config("26.4.0-f314aa17")),
            obj(&new_cluster_version(
                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5",
            )),
            obj(&new_pull_secret()),
            obj(&ds),
        ],
        true,
    );
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader);

    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");

    let ovn_template = get_template(&fake_client, "ovn").await.expect("ovn");
    assert_eq!(ovn_template.spec.deployment_service_name, "ovn");
}

#[tokio::test]
async fn manager_ensure_templates_when_ovn_daemon_set_image_changed_but_rollout_is_not_complete_should_keep_current_template_values_and_not_pull_release_payload()
 {
    let fake_client = build(all_prereqs(X86_OVN_IMAGE), true);
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader.clone());

    // First call creates templates
    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");
    assert_eq!(reader.call_count(), 1);

    // Simulate an OCP upgrade: DaemonSet spec changes to new image,
    // but rollout is not complete (UpdatedNumberScheduled < DesiredNumberScheduled)
    let new_x86_image = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:ccc333";
    let mut ds = new_ovn_daemon_set(new_x86_image);
    set_ds_rollout(&mut ds, 2, 3, 1, 2);
    fake_client
        .delete(&new_ovn_daemon_set(X86_OVN_IMAGE))
        .await
        .expect("delete");
    fake_client.create(&mut ds).await.expect("create");

    // Second call should detect image change but skip pull due to incomplete rollout
    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");
    assert_eq!(reader.call_count(), 1);

    // Template should still have the original aarch64 image, annotation, and daemonset version
    let ovn_template = get_template(&fake_client, "ovn").await.expect("ovn");
    assert_eq!(
        ovn_template.metadata.annotations.as_ref().unwrap()[ANNOTATION_SOURCE_OVN_IMAGE],
        X86_OVN_IMAGE
    );
    assert_eq!(helm_values(&ovn_template)["ovnDaemonsetVersion"], "1.3.0");
}

#[tokio::test]
async fn manager_ensure_templates_when_ovn_daemon_set_image_changed_and_rollout_is_complete_should_resolve_the_new_aarch64_image_from_release_payload()
 {
    let fake_client = build(all_prereqs(X86_OVN_IMAGE), true);
    let new_arm64_image = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:ddd444";
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader.clone());

    // First call creates templates
    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");
    assert_eq!(reader.call_count(), 1);

    // Simulate completed OCP upgrade: DaemonSet has new image and rollout is done
    let new_x86_image = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:ccc333";
    let mut ds = new_ovn_daemon_set(new_x86_image);
    set_ds_rollout(&mut ds, 2, 3, 3, 3);
    fake_client
        .delete(&new_ovn_daemon_set(X86_OVN_IMAGE))
        .await
        .expect("delete");
    fake_client.create(&mut ds).await.expect("create");

    // Update reader to return new arm64 image
    reader.set_image(new_arm64_image);

    // Second call should resolve the new aarch64 image
    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");
    assert_eq!(reader.call_count(), 2);

    // Template should have the new annotation
    let ovn_template = get_template(&fake_client, "ovn").await.expect("ovn");
    assert_eq!(
        ovn_template.metadata.annotations.as_ref().unwrap()[ANNOTATION_SOURCE_OVN_IMAGE],
        new_x86_image
    );

    // Verify the new aarch64 image is in the template values
    let (repo, tag) = ovn_image_of(&helm_values(&ovn_template));
    assert_eq!(
        repo,
        "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256"
    );
    assert_eq!(tag, "ddd444");
}

#[tokio::test]
async fn manager_ensure_templates_when_release_image_is_multi_arch_should_use_the_management_cluster_ovn_image_directly_and_skip_release_payload_resolution()
 {
    let fake_client = build(
        all_prereqs_with_release(
            X86_OVN_IMAGE,
            "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
        ),
        true,
    );
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader.clone());

    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");

    // Release payload should never be consulted
    assert_eq!(reader.call_count(), 0);

    // OVN template should use the x86 image directly (it's a manifest list)
    let ovn_template = get_template(&fake_client, "ovn").await.expect("ovn");
    let (repo, tag) = ovn_image_of(&helm_values(&ovn_template));
    assert_eq!(
        repo,
        "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256"
    );
    assert_eq!(tag, "aaa111");
}

#[tokio::test]
async fn manager_ensure_templates_when_dpf_operator_config_is_missing_should_return_error() {
    let fake_client = build(
        vec![
            obj(&new_cluster_version(
                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
            )),
            obj(&new_pull_secret()),
            obj(&new_ovn_daemon_set(X86_OVN_IMAGE)),
        ],
        false,
    );
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader);

    let res = manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn manager_ensure_templates_when_dpf_version_has_no_dedicated_embedded_row_should_use_embedded_default_values()
 {
    let fake_client = build(
        vec![
            obj(&new_dpf_operator_config("99.0.0-unknown")),
            obj(&new_cluster_version(
                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
            )),
            obj(&new_pull_secret()),
            obj(&new_ovn_daemon_set(X86_OVN_IMAGE)),
        ],
        true,
    );
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader);

    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");
}

#[tokio::test]
async fn manager_ensure_templates_when_ovn_daemon_set_is_missing_should_return_error() {
    let fake_client = build(
        vec![
            obj(&new_dpf_operator_config("26.4.0-f314aa17")),
            obj(&new_cluster_version(
                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
            )),
            obj(&new_pull_secret()),
        ],
        true,
    );
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader);

    let res = manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn manager_ensure_templates_when_release_image_reader_returns_an_error_should_return_error() {
    let fake_client = build(all_prereqs(X86_OVN_IMAGE), true);
    let reader = FakeReleaseImageReader::with_err("unable to pull release image");
    let manager = new_manager(&fake_client, reader);

    let err = manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("resolving OVN kubernetes image"),
        "{err}"
    );
}

#[tokio::test]
async fn manager_ensure_templates_when_a_managed_dpu_service_template_is_being_deleted_should_not_recreate_or_update_the_terminating_template()
 {
    let mut deleting_template = new_template(
        "ovn",
        TARGET_NAMESPACE,
        managed_labels(),
        "https://stale",
        "stale",
    );
    deleting_template.metadata.deletion_timestamp = Some(ctrlrt::meta::now());
    deleting_template.metadata.finalizers = Some(vec![
        dpuservicev1alpha1::DPU_SERVICE_TEMPLATE_FINALIZER.into(),
    ]);

    let mut objects = all_prereqs(X86_OVN_IMAGE);
    objects.push(obj(&deleting_template));
    let fake_client = build(objects, true);
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader);

    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");

    let ovn_template = get_template(&fake_client, "ovn").await.expect("ovn");
    assert!(ovn_template.metadata.deletion_timestamp.is_some());
    assert_eq!(ovn_template.spec.helm_chart.source.version, "stale");
}

#[tokio::test]
async fn manager_ensure_templates_when_a_dpu_service_template_already_exists_with_different_spec_should_update_it_to_the_desired_state()
 {
    let existing_template = new_template(
        "ovn",
        TARGET_NAMESPACE,
        managed_labels(),
        "oci://example.com",
        "1.0.0",
    );

    let mut objects = all_prereqs(X86_OVN_IMAGE);
    objects.push(obj(&existing_template));
    let fake_client = build(objects, true);
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader);

    manager
        .ensure_templates(&ctx(), TARGET_NAMESPACE, &OperatorConfig::default())
        .await
        .expect("no error");

    let ovn_template = get_template(&fake_client, "ovn").await.expect("ovn");
    assert_eq!(
        ovn_template.spec.helm_chart.source.repo_url,
        "oci://ghcr.io/mellanox/charts"
    );
}

// =============================================================================================
// Describe("DPUServiceTemplate Manager") / Describe("DeleteTemplates")
// =============================================================================================

#[tokio::test]
async fn manager_delete_templates_when_all_three_templates_exist_should_delete_all_of_them() {
    let templates = vec![
        obj(&new_template(
            "ovn",
            TARGET_NAMESPACE,
            managed_labels(),
            "oci://x",
            "1",
        )),
        obj(&new_template(
            "doca-telemetry-service",
            TARGET_NAMESPACE,
            managed_labels(),
            "https://x",
            "1",
        )),
        obj(&new_template(
            "hbn",
            TARGET_NAMESPACE,
            managed_labels(),
            "https://x",
            "1",
        )),
    ];
    let fake_client = build(templates, false);
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader);

    manager
        .delete_templates(&ctx(), TARGET_NAMESPACE)
        .await
        .expect("no error");

    for name in ["ovn", "doca-telemetry-service", "hbn"] {
        let err = get_template(&fake_client, name).await.unwrap_err();
        assert!(
            ctrlrt::error::is_not_found(&err),
            "expected template {name} to be deleted"
        );
    }
}

#[tokio::test]
async fn manager_delete_templates_when_no_templates_exist_should_succeed_immediately_idempotent() {
    let fake_client = build(vec![], false);
    let reader = FakeReleaseImageReader::new(ARM64_OVN_IMAGE);
    let manager = new_manager(&fake_client, reader);

    manager
        .delete_templates(&ctx(), TARGET_NAMESPACE)
        .await
        .expect("no error");
}

// =============================================================================================
// Describe("OVNDaemonsetVersionForOCP") — DescribeTable("should return the correct daemonset version")
// =============================================================================================

fn check_ovn_daemonset_version(ocp_version: &str, expected: &str) {
    assert_eq!(
        ovnk_daemonset_version_for_ocp(ocp_version),
        expected,
        "{ocp_version}"
    );
}

#[test]
fn manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_old_ocp() {
    check_ovn_daemonset_version("4.18.0", "1.3.0");
}

#[test]
fn manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_4_21_x() {
    check_ovn_daemonset_version("4.21.15", "1.3.0");
}

#[test]
fn manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_just_before_4_22_0()
 {
    check_ovn_daemonset_version("4.21.99", "1.3.0");
}

#[test]
fn manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_4_22_0() {
    check_ovn_daemonset_version("4.22.0", "1.2.0");
}

#[test]
fn manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_4_22_1_boundary()
 {
    check_ovn_daemonset_version("4.22.1", "1.3.0");
}

#[test]
fn manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_4_22_z_after()
{
    check_ovn_daemonset_version("4.22.5", "1.3.0");
}

#[test]
fn manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_future_4_23() {
    check_ovn_daemonset_version("4.23.0", "1.3.0");
}

#[test]
fn manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_pre_release() {
    check_ovn_daemonset_version("4.22.1-rc.1", "1.3.0");
}

#[test]
fn manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_garbage() {
    check_ovn_daemonset_version("not-a-version", "1.3.0");
}

// =============================================================================================
// Describe("GetDefaultsForVersion")
// =============================================================================================

#[test]
fn manager_get_defaults_for_version_should_return_defaults_for_supported_version_26_4() {
    let defaults = dpfdefaults::template_values(&Ctx::background(), "26.4").expect("no error");
    assert_eq!(defaults.ovn.chart_version, "v26.4.1-ocp-release-v4.22");
    assert_eq!(defaults.dts.chart_version, "1.25.5");
    assert_eq!(defaults.hbn.chart_version, "3.4.0");
    assert_eq!(defaults.hbn.image_tag, "3.4.0-doca3.4.0");
}

#[test]
fn manager_get_defaults_for_version_should_return_embedded_default_for_unknown_dpf_minors() {
    let defaults264 = dpfdefaults::template_values(&Ctx::background(), "26.4").expect("no error");
    let defaults = dpfdefaults::template_values(&Ctx::background(), "26.12").expect("no error");
    assert_eq!(defaults, defaults264);
}

// =============================================================================================
// Describe("DPUServiceTemplate Reconciler")
// =============================================================================================

fn reconciler_for(c: &Client) -> DPUServiceTemplateReconciler {
    DPUServiceTemplateReconciler {
        client: c.clone(),
        manager: Arc::new(new_manager(c, FakeReleaseImageReader::new(ARM64_OVN_IMAGE))),
    }
}

/// `setupReconciler(objects...)` (no status subresource).
fn setup_reconciler(objects: Vec<InitObject>) -> (Client, DPUServiceTemplateReconciler) {
    let c = build(objects, false);
    let r = reconciler_for(&c);
    (c, r)
}

fn request() -> ObjectKey {
    ObjectKey::new("", TARGET_NAMESPACE)
}

#[tokio::test]
async fn reconciler_should_delete_dpu_service_templates_when_the_referenced_dpu_deployment_is_being_deleted()
 {
    let (fake_client, reconciler) = setup_reconciler(vec![
        obj(&new_operator_config_cr()),
        obj(&new_test_provisioner(
            "test-provisioner",
            TARGET_NAMESPACE,
            DEPLOYMENT_NAME,
            TARGET_NAMESPACE,
        )),
        obj(&new_deleting_dpu_deployment(
            DEPLOYMENT_NAME,
            TARGET_NAMESPACE,
        )),
        obj(&new_managed_template("ovn", TARGET_NAMESPACE)),
    ]);

    reconciler
        .reconcile(&ctx(), request())
        .await
        .expect("no error");

    let err = get_template(&fake_client, "ovn").await.unwrap_err();
    assert!(ctrlrt::error::is_not_found(&err));
}

#[tokio::test]
async fn reconciler_should_create_dpu_service_templates_when_the_referenced_dpu_deployment_is_active()
 {
    let mut objects = all_prereqs(X86_OVN_IMAGE);
    objects.extend([
        obj(&new_operator_config_cr()),
        obj(&new_test_provisioner(
            "test-provisioner",
            TARGET_NAMESPACE,
            DEPLOYMENT_NAME,
            TARGET_NAMESPACE,
        )),
        obj(&new_test_dpu_deployment(DEPLOYMENT_NAME, TARGET_NAMESPACE)),
    ]);
    let fake_client = build(objects, true);
    let reconciler = reconciler_for(&fake_client);

    reconciler
        .reconcile(&ctx(), request())
        .await
        .expect("no error");

    get_template(&fake_client, "ovn").await.expect("ovn exists");
}

#[tokio::test]
async fn reconciler_should_delete_dpu_service_templates_when_the_referenced_dpu_deployment_is_not_found()
 {
    let (fake_client, reconciler) = setup_reconciler(vec![
        obj(&new_operator_config_cr()),
        obj(&new_test_provisioner(
            "test-provisioner",
            TARGET_NAMESPACE,
            DEPLOYMENT_NAME,
            TARGET_NAMESPACE,
        )),
        obj(&new_managed_template("ovn", TARGET_NAMESPACE)),
    ]);

    reconciler
        .reconcile(&ctx(), request())
        .await
        .expect("no error");

    let err = get_template(&fake_client, "ovn").await.unwrap_err();
    assert!(ctrlrt::error::is_not_found(&err));
}

#[tokio::test]
async fn reconciler_should_delete_dpu_service_templates_when_no_provisioner_remains() {
    let (fake_client, reconciler) = setup_reconciler(vec![
        obj(&new_operator_config_cr()),
        obj(&new_managed_template("ovn", TARGET_NAMESPACE)),
    ]);

    reconciler
        .reconcile(&ctx(), request())
        .await
        .expect("no error");

    let err = get_template(&fake_client, "ovn").await.unwrap_err();
    assert!(ctrlrt::error::is_not_found(&err));
}

#[tokio::test]
async fn reconciler_should_still_create_dpu_service_templates_when_another_dpu_deployment_remains_active()
 {
    let mut objects = all_prereqs(X86_OVN_IMAGE);
    objects.extend([
        obj(&new_operator_config_cr()),
        obj(&new_test_provisioner(
            "deleting-provisioner",
            TARGET_NAMESPACE,
            "deleting-dd",
            TARGET_NAMESPACE,
        )),
        obj(&new_deleting_dpu_deployment(
            "deleting-dd",
            TARGET_NAMESPACE,
        )),
        obj(&new_test_provisioner(
            "active-provisioner",
            TARGET_NAMESPACE,
            "active-dd",
            TARGET_NAMESPACE,
        )),
        obj(&new_test_dpu_deployment("active-dd", TARGET_NAMESPACE)),
    ]);
    let fake_client = build(objects, true);
    let reconciler = reconciler_for(&fake_client);

    reconciler
        .reconcile(&ctx(), request())
        .await
        .expect("no error");

    get_template(&fake_client, "ovn").await.expect("ovn exists");
}
