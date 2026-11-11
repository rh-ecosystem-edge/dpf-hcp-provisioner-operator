//! Port of internal/controller/ovs-hugepages/ovs-hugepages_test.go (Ginkgo suite
//! `TestHugepages` / "Hugepages Suite" in suite_test.go; each `It` is one `#[tokio::test]`
//! named after its spec path).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use apis::provisioning::v1alpha1::{DPFHCPProvisioner, DPFHCPProvisionerSpec};
use apis::resource::canonical;
use async_trait::async_trait;
use ctrl_dpuservicetemplate::ReleaseImageReader;
use ctrlrt::error::is_not_found;
use ctrlrt::k8s_openapi::ByteString;
use ctrlrt::k8s_openapi::api::apps::v1::DaemonSet;
use ctrlrt::k8s_openapi::api::core::v1::{Namespace, Secret};
use ctrlrt::k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use ctrlrt::{Ctx, Result};
use ctrlrt_fake::{FakeClientBuilder, FakeClientsetBuilder, Scheme, new_simple_clientset};
use hostedclient::{ClientManager, HostedClient};
use registry::authn::Keychain;

use crate::ovs_hugepages::*;

/// testPauseImage is an arbitrary resolved pause image used to drive the builder and
/// reconcile helpers; the runtime resolution from the release payload is not unit-tested.
const TEST_PAUSE_IMAGE: &str = "registry.example.com/openshift4/ose-pod@sha256:0123456789abcdef";

/// `req.Equal(want)` for quantities: equal canonical values.
fn qty_equal(q: Option<&Quantity>, want: &str) -> bool {
    let got = q.map(|q| q.0.as_str()).unwrap_or("0");
    canonical(got).ok() == canonical(want).ok()
}

// --- Describe("buildDaemonSet") ----------------------------------------------------------

#[test]
fn build_daemon_set_reserves_the_configured_hugepages_amount_on_every_node_with_no_node_selector() {
    let ds = build_daemon_set(
        TEST_PAUSE_IMAGE,
        DEFAULT_HUGEPAGES_SIZE,
        DEFAULT_HUGEPAGES_AMOUNT,
    )
    .unwrap();

    assert_eq!(ds.metadata.name.as_deref(), Some(DAEMON_SET_NAME));
    assert_eq!(
        ds.metadata.namespace.as_deref(),
        Some("openshift-doca-hugepages-holder")
    );

    let pod_spec = ds.spec.unwrap().template.spec.unwrap();
    // No nodeSelector: every node of the hosted cluster is a DPU.
    assert!(pod_spec.node_selector.unwrap_or_default().is_empty());
    // Tolerate all taints so the reservation lands on every node.
    let tolerations = pod_spec.tolerations.unwrap_or_default();
    assert_eq!(tolerations.len(), 1);
    assert_eq!(tolerations[0].operator.as_deref(), Some("Exists"));

    assert_eq!(pod_spec.containers.len(), 1);
    let c = &pod_spec.containers[0];
    assert_eq!(c.name, CONTAINER_NAME);
    assert_eq!(c.image.as_deref(), Some(TEST_PAUSE_IMAGE));
    // The pause image idles on its own entrypoint — no command/shell needed.
    assert!(c.command.clone().unwrap_or_default().is_empty());

    let resource_name = format!("hugepages-{DEFAULT_HUGEPAGES_SIZE}");
    let res = c.resources.clone().unwrap_or_default();
    let requests = res.requests.unwrap_or_default();
    let limits = res.limits.unwrap_or_default();
    // 250 pages x 2Mi = 500Mi. Hugepages requests must equal limits.
    assert!(
        qty_equal(requests.get(&resource_name), "500Mi"),
        "request should be 500Mi"
    );
    assert!(
        qty_equal(limits.get(&resource_name), "500Mi"),
        "limit should be 500Mi"
    );
}

#[test]
fn build_daemon_set_computes_amount_x_size_for_a_custom_size_and_page_count() {
    // 4 pages x 1Gi = 4Gi.
    let ds = build_daemon_set(TEST_PAUSE_IMAGE, "1Gi", 4).unwrap();
    let c = &ds.spec.unwrap().template.spec.unwrap().containers[0];

    let requests = c
        .resources
        .clone()
        .unwrap_or_default()
        .requests
        .unwrap_or_default();
    assert!(
        qty_equal(requests.get("hugepages-1Gi"), "4Gi"),
        "request should be 4Gi of hugepages-1Gi"
    );
}

// --- Describe("ensureNamespace") ---------------------------------------------------------

#[tokio::test]
async fn ensure_namespace_creates_the_reservation_namespace_when_missing_and_is_idempotent() {
    let ctx = Ctx::background();
    let cs = HostedClient::fake(FakeClientsetBuilder::new().build());

    ensure_namespace(&ctx, &cs).await.unwrap();

    let ns = cs
        .resource::<Namespace>("")
        .get(&ctx, OVS_HUGEPAGES_NAMESPACE)
        .await
        .unwrap();
    assert_eq!(
        ns.metadata.name.as_deref(),
        Some("openshift-doca-hugepages-holder")
    );

    ensure_namespace(&ctx, &cs).await.unwrap();
}

// --- Describe("ensureDaemonSet") ---------------------------------------------------------

/// BeforeEach: a clientset seeded with the reservation namespace.
fn ensure_daemon_set_clientset() -> HostedClient {
    HostedClient::fake(new_simple_clientset([&Namespace {
        metadata: ObjectMeta {
            name: Some(OVS_HUGEPAGES_NAMESPACE.into()),
            ..Default::default()
        },
        ..Default::default()
    }]))
}

#[tokio::test]
async fn ensure_daemon_set_creates_the_daemon_set_on_first_call() {
    let ctx = Ctx::background();
    let cs = ensure_daemon_set_clientset();

    ensure_daemon_set(
        &ctx,
        &cs,
        TEST_PAUSE_IMAGE,
        DEFAULT_HUGEPAGES_SIZE,
        DEFAULT_HUGEPAGES_AMOUNT,
    )
    .await
    .unwrap();

    let ds = cs
        .resource::<DaemonSet>(OVS_HUGEPAGES_NAMESPACE)
        .get(&ctx, DAEMON_SET_NAME)
        .await
        .unwrap();
    assert_eq!(ds.metadata.name.as_deref(), Some(DAEMON_SET_NAME));
}

#[tokio::test]
async fn ensure_daemon_set_does_not_modify_the_daemon_set_on_an_unchanged_subsequent_call() {
    let ctx = Ctx::background();
    let cs = ensure_daemon_set_clientset();
    let dss = cs.resource::<DaemonSet>(OVS_HUGEPAGES_NAMESPACE);

    ensure_daemon_set(
        &ctx,
        &cs,
        TEST_PAUSE_IMAGE,
        DEFAULT_HUGEPAGES_SIZE,
        DEFAULT_HUGEPAGES_AMOUNT,
    )
    .await
    .unwrap();

    let before = dss.get(&ctx, DAEMON_SET_NAME).await.unwrap();

    ensure_daemon_set(
        &ctx,
        &cs,
        TEST_PAUSE_IMAGE,
        DEFAULT_HUGEPAGES_SIZE,
        DEFAULT_HUGEPAGES_AMOUNT,
    )
    .await
    .unwrap();

    let after = dss.get(&ctx, DAEMON_SET_NAME).await.unwrap();
    // No write happened, so the resourceVersion is unchanged.
    assert_eq!(
        after.metadata.resource_version,
        before.metadata.resource_version
    );

    // Still exactly one DaemonSet.
    let list = dss.list(&ctx).await.unwrap();
    assert_eq!(list.len(), 1);
}

#[tokio::test]
async fn ensure_daemon_set_updates_the_daemon_set_when_the_reservation_amount_changes() {
    let ctx = Ctx::background();
    let cs = ensure_daemon_set_clientset();

    ensure_daemon_set(
        &ctx,
        &cs,
        TEST_PAUSE_IMAGE,
        DEFAULT_HUGEPAGES_SIZE,
        DEFAULT_HUGEPAGES_AMOUNT,
    )
    .await
    .unwrap();
    ensure_daemon_set(
        &ctx,
        &cs,
        TEST_PAUSE_IMAGE,
        DEFAULT_HUGEPAGES_SIZE,
        DEFAULT_HUGEPAGES_AMOUNT * 2,
    )
    .await
    .unwrap();

    let ds = cs
        .resource::<DaemonSet>(OVS_HUGEPAGES_NAMESPACE)
        .get(&ctx, DAEMON_SET_NAME)
        .await
        .unwrap();
    let resource_name = format!("hugepages-{DEFAULT_HUGEPAGES_SIZE}");
    // 500 pages x 2Mi = 1000Mi.
    let requests = ds.spec.unwrap().template.spec.unwrap().containers[0]
        .resources
        .clone()
        .unwrap_or_default()
        .requests
        .unwrap_or_default();
    assert!(
        qty_equal(requests.get(&resource_name), "1000Mi"),
        "request should reflect the doubled amount"
    );
}

#[tokio::test]
async fn ensure_daemon_set_deletes_the_daemon_set_when_amount_is_zero() {
    let ctx = Ctx::background();
    let cs = ensure_daemon_set_clientset();

    ensure_daemon_set(
        &ctx,
        &cs,
        TEST_PAUSE_IMAGE,
        DEFAULT_HUGEPAGES_SIZE,
        DEFAULT_HUGEPAGES_AMOUNT,
    )
    .await
    .unwrap();

    ensure_daemon_set(&ctx, &cs, TEST_PAUSE_IMAGE, DEFAULT_HUGEPAGES_SIZE, 0)
        .await
        .unwrap();

    let err = cs
        .resource::<DaemonSet>(OVS_HUGEPAGES_NAMESPACE)
        .get(&ctx, DAEMON_SET_NAME)
        .await
        .unwrap_err();
    assert!(is_not_found(&err));
}

#[tokio::test]
async fn ensure_daemon_set_is_a_no_op_when_amount_is_zero_and_nothing_exists() {
    let ctx = Ctx::background();
    let cs = ensure_daemon_set_clientset();

    ensure_daemon_set(&ctx, &cs, TEST_PAUSE_IMAGE, DEFAULT_HUGEPAGES_SIZE, 0)
        .await
        .unwrap();
}

// --- resolvePauseImage ---------------------------------------------------------------------

struct FakeReleaseImageReader {
    image: String,
    err: Option<String>,
    call_count: AtomicUsize,
}

impl FakeReleaseImageReader {
    fn new(image: &str, err: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            image: image.into(),
            err: err.map(Into::into),
            call_count: AtomicUsize::new(0),
        })
    }
    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
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
    ) -> Result<String> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        match &self.err {
            Some(e) => Err(anyhow::anyhow!("{}", e)),
            None => Ok(self.image.clone()),
        }
    }
}

fn new_test_manager(reader: Arc<FakeReleaseImageReader>) -> Manager {
    let scheme = Scheme::new()
        .register::<Secret>()
        .register::<DPFHCPProvisioner>();
    let pull_secret = Secret {
        metadata: ObjectMeta {
            name: Some("pull-secret".into()),
            namespace: Some("test-ns".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            ".dockerconfigjson".to_string(),
            ByteString(br#"{"auths":{}}"#.to_vec()),
        )])),
        type_: Some("kubernetes.io/dockerconfigjson".into()),
        ..Default::default()
    };
    let mgmt_client = FakeClientBuilder::new()
        .with_scheme(scheme)
        .with_object(&pull_secret)
        .build();
    // Go leaves clientManager nil; resolvePauseImage never uses it.
    Manager {
        client_manager: Arc::new(ClientManager::new(mgmt_client.clone())),
        mgmt_client,
        release_reader: reader,
        pause_images: Mutex::default(),
    }
}

fn new_test_cr() -> DPFHCPProvisioner {
    DPFHCPProvisioner {
        metadata: ObjectMeta {
            name: Some("test".into()),
            namespace: Some("test-ns".into()),
            ..Default::default()
        },
        spec: DPFHCPProvisionerSpec {
            ocp_release_image: "quay.io/openshift-release-dev/ocp-release:4.17.6-x86_64".into(),
            pull_secret_ref: apis::k8s::core::v1::LocalObjectReference {
                name: "pull-secret".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

const RESOLVED_PAUSE: &str = "quay.io/openshift4/ose-pod@sha256:abc123";

#[tokio::test]
async fn resolve_pause_image_resolves_the_pause_image_from_the_aarch64_release_payload_and_caches_it()
 {
    let reader = FakeReleaseImageReader::new(RESOLVED_PAUSE, None);
    let m = new_test_manager(reader.clone());
    let ctx = Ctx::background();

    let image = m.resolve_pause_image(&ctx, &new_test_cr()).await.unwrap();
    assert_eq!(image, RESOLVED_PAUSE);

    let image2 = m.resolve_pause_image(&ctx, &new_test_cr()).await.unwrap();
    assert_eq!(image2, RESOLVED_PAUSE);
    assert_eq!(reader.call_count(), 1, "second call should hit cache");
}

#[tokio::test]
async fn resolve_pause_image_resolves_the_pause_image_directly_from_a_multi_arch_release() {
    let reader = FakeReleaseImageReader::new(RESOLVED_PAUSE, None);
    let m = new_test_manager(reader.clone());
    let mut cr = new_test_cr();
    cr.spec.ocp_release_image = "quay.io/openshift-release-dev/ocp-release:4.17.6-multi".into();

    let image = m
        .resolve_pause_image(&Ctx::background(), &cr)
        .await
        .unwrap();
    assert_eq!(image, RESOLVED_PAUSE);
    assert_eq!(reader.call_count(), 1);
}

#[tokio::test]
async fn resolve_pause_image_returns_an_error_when_the_aarch64_release_lookup_fails() {
    let reader = FakeReleaseImageReader::new("", Some("registry unreachable"));
    let m = new_test_manager(reader.clone());

    let err = m
        .resolve_pause_image(&Ctx::background(), &new_test_cr())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("registry unreachable"), "{err}");
    assert_eq!(
        reader.call_count(),
        1,
        "must not fall back to a potentially non-aarch64 release"
    );
}
