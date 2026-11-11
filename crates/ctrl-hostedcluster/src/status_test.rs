//! Port of internal/controller/hostedcluster/status_test.go ("Status Syncer").

use apis::hypershift::v1beta1 as hyperv1;
use apis::k8s::core::v1::ObjectReference;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::Ctx;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use ctrlrt::meta;
use ctrlrt_fake::{FakeClientBuilder, InitObject, Scheme, obj};

use crate::*;

/// BeforeEach state. The Go test keeps a `*fake.ClientBuilder` holding pointers to `cr`/`hc`
/// and builds it inside each `It`, after the test mutated them; [`Env::builder`] reproduces
/// that by snapshotting the objects at build time.
struct Env {
    ctx: Ctx,
    scheme: Scheme,
    cr: provisioningv1alpha1::DPFHCPProvisioner,
    hc: hyperv1::HostedCluster,
}

fn hc_ref(name: &str) -> ObjectReference {
    ObjectReference {
        name: name.into(),
        namespace: "default".into(),
        kind: "HostedCluster".into(),
        api_version: "hypershift.openshift.io/v1beta1".into(),
        ..Default::default()
    }
}

fn hc_cond(type_: &str, status: &str, reason: &str) -> Condition {
    meta::condition(type_, status, reason, "")
}

fn before_each() -> Env {
    // Create test DPFHCPProvisioner CR
    let mut cr = provisioningv1alpha1::DPFHCPProvisioner::default();
    cr.metadata.name = Some("test-provisioner".into());
    cr.metadata.namespace = Some("default".into());
    cr.metadata.generation = Some(1);
    cr.spec.ocp_release_image = "quay.io/openshift-release-dev/ocp-release:4.19.0-multi".into();
    cr.status.hosted_cluster_ref = Some(hc_ref("test-provisioner"));

    // Create test HostedCluster with status
    let mut hc = hyperv1::HostedCluster::default();
    hc.metadata.name = Some("test-provisioner".into());
    hc.metadata.namespace = Some("default".into());
    hc.status.conditions = vec![
        hc_cond(
            hyperv1::HOSTED_CLUSTER_AVAILABLE,
            meta::CONDITION_TRUE,
            "HostedClusterAsExpected",
        ),
        hc_cond(
            hyperv1::HOSTED_CLUSTER_PROGRESSING,
            meta::CONDITION_FALSE,
            "HostedClusterAsExpected",
        ),
        hc_cond(
            hyperv1::INFRASTRUCTURE_READY,
            meta::CONDITION_TRUE,
            "InfrastructureReady",
        ),
    ];

    Env {
        ctx: Ctx::background(),
        scheme: Scheme::new()
            .register::<provisioningv1alpha1::DPFHCPProvisioner>()
            .register::<hyperv1::HostedCluster>()
            .register::<hyperv1::NodePool>(),
        cr,
        hc,
    }
}

impl Env {
    /// `fake.NewClientBuilder().WithScheme(scheme).WithRuntimeObjects(cr, hc)` (+ extra
    /// `WithRuntimeObjects(...)`), then `Build()`.
    fn build(&self, extra: Vec<InitObject>) -> ctrlrt::Client {
        let mut objs = vec![obj(&self.cr), obj(&self.hc)];
        objs.extend(extra);
        FakeClientBuilder::new()
            .with_scheme(self.scheme.clone())
            .with_runtime_objects(objs)
            .build()
    }
}

// Context("SyncStatusFromHostedCluster")

#[tokio::test]
async fn status_syncer_sync_status_from_hosted_cluster_should_skip_sync_when_hosted_cluster_ref_is_not_set()
 {
    let mut env = before_each();
    env.cr.status.hosted_cluster_ref = None;
    let c = env.build(vec![]);
    let syncer = StatusSyncer::new(c);

    let result = syncer
        .sync_status_from_hosted_cluster(&env.ctx, &mut env.cr)
        .await
        .unwrap();
    assert!(result.requeue_after.is_zero());
}

#[tokio::test]
async fn status_syncer_sync_status_from_hosted_cluster_should_handle_missing_hosted_cluster_gracefully()
 {
    let env = before_each();
    // Create CR without corresponding HostedCluster
    let mut cr_no_hc = provisioningv1alpha1::DPFHCPProvisioner::default();
    cr_no_hc.metadata.name = Some("test-provisioner-no-hc".into());
    cr_no_hc.metadata.namespace = Some("default".into());
    cr_no_hc.metadata.generation = Some(1);
    cr_no_hc.status.hosted_cluster_ref = Some(hc_ref("missing-hc"));

    let c = env.build(vec![obj(&cr_no_hc)]);
    let syncer = StatusSyncer::new(c);

    // Should not error, just skip sync
    syncer
        .sync_status_from_hosted_cluster(&env.ctx, &mut cr_no_hc)
        .await
        .unwrap();
}

#[tokio::test]
async fn status_syncer_sync_status_from_hosted_cluster_should_skip_sync_when_hosted_cluster_status_is_not_populated()
 {
    let env = before_each();
    // Create HostedCluster without status conditions
    let mut hc_no_status = hyperv1::HostedCluster::default();
    hc_no_status.metadata.name = Some("test-provisioner-no-status".into());
    hc_no_status.metadata.namespace = Some("default".into());
    hc_no_status.status.conditions = Vec::new(); // No conditions yet

    let mut cr_no_status = provisioningv1alpha1::DPFHCPProvisioner::default();
    cr_no_status.metadata.name = Some("test-provisioner-no-status".into());
    cr_no_status.metadata.namespace = Some("default".into());
    cr_no_status.metadata.generation = Some(1);
    cr_no_status.status.hosted_cluster_ref = Some(hc_ref("test-provisioner-no-status"));

    let c = env.build(vec![obj(&cr_no_status), obj(&hc_no_status)]);
    let syncer = StatusSyncer::new(c);

    let result = syncer
        .sync_status_from_hosted_cluster(&env.ctx, &mut cr_no_status)
        .await
        .unwrap();
    // Should not requeue - the HostedCluster watch will trigger reconciliation when status changes
    assert!(result.requeue_after.is_zero());
    // Extra: nothing mirrored
    assert!(cr_no_status.status.conditions.is_empty());
}

#[tokio::test]
async fn status_syncer_sync_status_from_hosted_cluster_should_mirror_all_7_hosted_cluster_conditions_to_dpfhcp_provisioner()
 {
    let mut env = before_each();
    // Add the 7 conditions we mirror from the spec
    env.hc.status.conditions = vec![
        hc_cond(
            hyperv1::HOSTED_CLUSTER_AVAILABLE,
            meta::CONDITION_TRUE,
            "Test",
        ),
        hc_cond(
            hyperv1::HOSTED_CLUSTER_PROGRESSING,
            meta::CONDITION_FALSE,
            "Test",
        ),
        hc_cond(
            hyperv1::HOSTED_CLUSTER_DEGRADED,
            meta::CONDITION_FALSE,
            "Test",
        ),
        hc_cond(hyperv1::VALID_RELEASE_INFO, meta::CONDITION_TRUE, "Test"),
        hc_cond(hyperv1::VALID_RELEASE_IMAGE, meta::CONDITION_TRUE, "Test"),
        hc_cond(
            hyperv1::IGNITION_ENDPOINT_AVAILABLE,
            meta::CONDITION_TRUE,
            "Test",
        ),
        hc_cond(
            hyperv1::IGNITION_SERVER_VALID_RELEASE_INFO,
            meta::CONDITION_TRUE,
            "Test",
        ),
    ];
    let c = env.build(vec![]);
    let syncer = StatusSyncer::new(c);

    syncer
        .sync_status_from_hosted_cluster(&env.ctx, &mut env.cr)
        .await
        .unwrap();

    // Verify all 7 conditions were mirrored
    let expected_conditions = [
        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
        provisioningv1alpha1::HOSTED_CLUSTER_PROGRESSING,
        provisioningv1alpha1::HOSTED_CLUSTER_DEGRADED,
        provisioningv1alpha1::VALID_RELEASE_INFO,
        provisioningv1alpha1::VALID_RELEASE_IMAGE,
        provisioningv1alpha1::IGNITION_ENDPOINT_AVAILABLE,
        provisioningv1alpha1::IGNITION_SERVER_VALID_RELEASE_INFO,
    ];

    for cond_type in expected_conditions {
        assert!(
            meta::find_status_condition(&env.cr.status.conditions, cond_type).is_some(),
            "Condition {cond_type} should be mirrored"
        );
    }
}

#[tokio::test]
async fn status_syncer_sync_status_from_hosted_cluster_should_set_observed_generation_on_mirrored_conditions()
 {
    let mut env = before_each();
    let c = env.build(vec![]);
    let syncer = StatusSyncer::new(c);

    syncer
        .sync_status_from_hosted_cluster(&env.ctx, &mut env.cr)
        .await
        .unwrap();

    let generation = env.cr.metadata.generation;

    // Verify ObservedGeneration is set on mirrored conditions
    let available = meta::find_status_condition(
        &env.cr.status.conditions,
        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
    )
    .expect("available");
    assert_eq!(available.observed_generation, generation);

    let progressing = meta::find_status_condition(
        &env.cr.status.conditions,
        provisioningv1alpha1::HOSTED_CLUSTER_PROGRESSING,
    )
    .expect("progressing");
    assert_eq!(progressing.observed_generation, generation);
}
