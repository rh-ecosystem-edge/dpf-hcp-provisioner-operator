//! Port of internal/controller/dpucluster/validator_test.go ("DPUCluster Validator" suite).

use std::sync::Arc;

use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use ctrlrt::meta;
use ctrlrt::{ApiError, Client, Ctx, FakeRecorder, Object, Recorder};
use ctrlrt_fake::{FakeClientBuilder, InitObject, Reaction, Scheme, obj, obj_mut};

use crate::*;

/// BeforeEach: ctx, recorder (buffer 100) and a scheme with DPFHCPProvisioner + DPUCluster.
struct Env {
    ctx: Ctx,
    recorder: Arc<FakeRecorder>,
    scheme: Scheme,
}

fn before_each() -> Env {
    Env {
        ctx: Ctx::background(),
        recorder: Arc::new(FakeRecorder::new(100)),
        scheme: Scheme::new()
            .register::<provisioningv1alpha1::DPFHCPProvisioner>()
            .register::<dpuprovisioningv1alpha1::DPUCluster>(),
    }
}

impl Env {
    fn validator(&self, c: &Client) -> Validator {
        Validator::new(c.clone(), self.recorder.clone() as Recorder)
    }
    fn build(&self, objs: Vec<InitObject>) -> Client {
        FakeClientBuilder::new()
            .with_scheme(self.scheme.clone())
            .with_objects(objs)
            .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
            .build()
    }
}

fn dpu_cluster(type_: &str) -> dpuprovisioningv1alpha1::DPUCluster {
    let mut d = dpuprovisioningv1alpha1::DPUCluster::default();
    d.metadata.name = Some("test-dpu".into());
    d.metadata.namespace = Some("dpu-system".into());
    d.spec.type_ = type_.into();
    d
}

fn provisioner(
    name: &str,
    namespace: &str,
    generation: i64,
    dpu_name: &str,
) -> provisioningv1alpha1::DPFHCPProvisioner {
    let mut p = provisioningv1alpha1::DPFHCPProvisioner::default();
    p.metadata.name = Some(name.into());
    p.metadata.namespace = Some(namespace.into());
    if generation != 0 {
        p.metadata.generation = Some(generation);
    }
    p.spec.dpu_cluster_ref = provisioningv1alpha1::DPUClusterReference {
        name: dpu_name.into(),
        namespace: "dpu-system".into(),
        ..Default::default()
    };
    p
}

/// `Eventually(recorder.Events).Should(Receive(ContainSubstring(s)))`: receive events until
/// one matches.
fn receive_containing(recorder: &FakeRecorder, s: &str) -> bool {
    while let Some(e) = recorder.try_recv() {
        if e.contains(s) {
            return true;
        }
    }
    false
}

async fn get_updated(
    c: &Client,
    p: &provisioningv1alpha1::DPFHCPProvisioner,
) -> provisioningv1alpha1::DPFHCPProvisioner {
    c.get(&p.key()).await.expect("get updated provisioner")
}

fn find(p: &provisioningv1alpha1::DPFHCPProvisioner, t: &str) -> Option<Condition> {
    meta::find_status_condition(&p.status.conditions, t).cloned()
}

fn cond(type_: &str, status: &str, reason: &str, message: &str, now: bool, gen_: i64) -> Condition {
    let mut c = meta::condition_gen(type_, status, reason, message, gen_);
    if now {
        c.last_transition_time = meta::now();
    }
    c
}

// Describe("ValidateDPUCluster") / Context("when DPUCluster exists")

#[tokio::test]
async fn validate_dpu_cluster_when_dpu_cluster_exists_should_set_dpu_cluster_missing_false() {
    let env = before_each();
    let d = dpu_cluster("");
    let mut p = provisioner("test-provisioner", "default", 1, "test-dpu");
    let c = env.build(vec![obj(&d), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    let result = validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();
    assert!(result.requeue_after.is_zero());

    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_MISSING).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_FALSE);
    assert_eq!(condition.reason, REASON_DPU_CLUSTER_FOUND);
    assert!(condition.message.contains("test-dpu"));
    assert!(condition.message.contains("dpu-system"));
    assert_eq!(condition.observed_generation, Some(1));

    assert!(receive_containing(&env.recorder, "DPUClusterFound"));
}

#[tokio::test]
async fn validate_dpu_cluster_when_dpu_cluster_exists_should_emit_event_only_on_first_success_not_on_subsequent_reconciliations()
 {
    let env = before_each();
    let d = dpu_cluster("");
    let mut p = provisioner("test-provisioner", "default", 1, "test-dpu");
    let c = env.build(vec![obj(&d), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    // First reconciliation - should emit event
    let _ = validator.validate_dpu_cluster(&env.ctx, &mut p).await;
    assert!(receive_containing(&env.recorder, "DPUClusterFound"));

    let mut updated = get_updated(&c, &p).await;

    // Second reconciliation - should NOT emit event (condition unchanged)
    let _ = validator.validate_dpu_cluster(&env.ctx, &mut updated).await;
    assert!(env.recorder.try_recv().is_none());
}

// Context("when DPUCluster does not exist")

#[tokio::test]
async fn validate_dpu_cluster_when_dpu_cluster_does_not_exist_should_set_dpu_cluster_missing_true_and_not_requeue()
 {
    let env = before_each();
    let mut p = provisioner("test-provisioner", "default", 1, "missing-dpu");
    let c = env.build(vec![obj_mut(&mut p)]);
    let validator = env.validator(&c);

    let result = validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();
    assert!(result.requeue_after.is_zero()); // Permanent error - don't requeue

    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_MISSING).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_TRUE);
    assert_eq!(condition.reason, REASON_DPU_CLUSTER_NOT_FOUND);
    assert!(condition.message.contains("missing-dpu"));
    assert!(condition.message.contains("not found"));

    assert!(receive_containing(&env.recorder, "DPUClusterNotFound"));
}

#[tokio::test]
async fn validate_dpu_cluster_when_dpu_cluster_does_not_exist_should_use_deleted_reason_when_dpu_cluster_was_previously_found()
 {
    let env = before_each();
    let mut p = provisioner("test-provisioner", "default", 1, "deleted-dpu");
    // Simulate previous successful validation
    p.status.conditions = vec![cond(
        provisioningv1alpha1::DPU_CLUSTER_MISSING,
        meta::CONDITION_FALSE,
        REASON_DPU_CLUSTER_FOUND,
        "",
        true,
        0,
    )];
    let c = env.build(vec![obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();

    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_MISSING).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_TRUE);
    assert_eq!(condition.reason, REASON_DPU_CLUSTER_DELETED);
    assert!(condition.message.contains("has been deleted"));
    assert!(
        condition
            .message
            .contains("Please delete this DPFHCPProvisioner to clean up")
    );
}

// Context("when RBAC permission is denied")

#[tokio::test]
async fn validate_dpu_cluster_when_rbac_permission_is_denied_should_set_dpu_cluster_missing_true_with_access_denied_reason_and_not_requeue()
 {
    let env = before_each();
    let mut p = provisioner("test-provisioner", "default", 1, "forbidden-dpu");
    // forbiddenClient: Forbidden for DPUCluster Get operations
    let c = FakeClientBuilder::new()
        .with_scheme(env.scheme.clone())
        .with_objects([obj_mut(&mut p)])
        .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
        .with_reactor("get", "dpuclusters", |a| {
            Reaction::error(ApiError::forbidden(
                "provisioning.dpu.nvidia.com",
                "dpuclusters",
                &a.name,
                "<nil>",
            ))
        })
        .build();
    let validator = env.validator(&c);

    let res = validator.validate_dpu_cluster(&env.ctx, &mut p).await;
    assert!(res.is_ok(), "{:?}", res.err());

    // Extra (beyond the Go test): the condition carries the Forbidden error.
    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_MISSING).expect("condition");
    assert_eq!(condition.reason, REASON_DPU_CLUSTER_ACCESS_DENIED);
    assert_eq!(
        condition.message,
        "Operator lacks RBAC permissions to access DPUCluster 'forbidden-dpu' in namespace 'dpu-system': dpuclusters.provisioning.dpu.nvidia.com \"forbidden-dpu\" is forbidden: <nil>"
    );
}

// Context("when validation runs in different phases")

#[tokio::test]
async fn validate_dpu_cluster_when_validation_runs_in_different_phases_should_validate_in_pending_phase()
 {
    let env = before_each();
    let d = dpu_cluster("");
    let mut p = provisioner("test-provisioner", "default", 0, "test-dpu");
    p.status.phase = provisioningv1alpha1::PHASE_PENDING.into();
    let c = env.build(vec![obj(&d), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();
}

#[tokio::test]
async fn validate_dpu_cluster_when_validation_runs_in_different_phases_should_validate_in_ready_phase_proves_no_phase_gating()
 {
    let env = before_each();
    let d = dpu_cluster("");
    let mut p = provisioner("test-provisioner", "default", 0, "test-dpu");
    p.status.phase = provisioningv1alpha1::PHASE_READY.into();
    let c = env.build(vec![obj(&d), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();
}

#[tokio::test]
async fn validate_dpu_cluster_when_validation_runs_in_different_phases_should_detect_dpu_cluster_deletion_when_provisioner_is_ready()
 {
    let env = before_each();
    let mut p = provisioner("test-provisioner", "default", 0, "deleted-dpu");
    p.status.phase = provisioningv1alpha1::PHASE_READY.into();
    p.status.conditions = vec![cond(
        provisioningv1alpha1::DPU_CLUSTER_MISSING,
        meta::CONDITION_FALSE,
        REASON_DPU_CLUSTER_FOUND,
        "",
        false,
        0,
    )];
    let c = env.build(vec![obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();

    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_MISSING).expect("condition");
    assert_eq!(condition.reason, REASON_DPU_CLUSTER_DELETED);
}

// Context("when clusterType validation is performed")

#[tokio::test]
async fn validate_dpu_cluster_when_cluster_type_validation_is_performed_should_set_cluster_type_valid_true_when_type_is_not_kamaji()
 {
    let env = before_each();
    let d = dpu_cluster("static");
    let mut p = provisioner("test-provisioner", "default", 1, "test-dpu");
    let c = env.build(vec![obj(&d), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();

    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::CLUSTER_TYPE_VALID).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_TRUE);
    assert_eq!(condition.reason, REASON_CLUSTER_TYPE_VALID);
    assert!(condition.message.contains("static"));
    assert!(condition.message.contains("supported"));
}

#[tokio::test]
async fn validate_dpu_cluster_when_cluster_type_validation_is_performed_should_set_cluster_type_valid_false_when_type_is_kamaji()
 {
    let env = before_each();
    let d = dpu_cluster(dpuprovisioningv1alpha1::KAMAJI_CLUSTER);
    let mut p = provisioner("test-provisioner", "default", 1, "test-dpu");
    let c = env.build(vec![obj(&d), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();

    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::CLUSTER_TYPE_VALID).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_FALSE);
    assert_eq!(condition.reason, REASON_CLUSTER_TYPE_UNSUPPORTED);
    assert!(condition.message.contains("kamaji"));
    assert!(condition.message.contains("unsupported"));
}

#[tokio::test]
async fn validate_dpu_cluster_when_cluster_type_validation_is_performed_should_recover_when_dpu_cluster_type_changes_from_kamaji_to_supported_type()
 {
    let env = before_each();
    let d = dpu_cluster("static");
    let mut p = provisioner("test-provisioner", "default", 1, "test-dpu");
    p.status.phase = provisioningv1alpha1::PHASE_FAILED.into();
    p.status.conditions = vec![cond(
        provisioningv1alpha1::CLUSTER_TYPE_VALID,
        meta::CONDITION_FALSE, // Was False before
        REASON_CLUSTER_TYPE_UNSUPPORTED,
        "ClusterType unsupported",
        true,
        1,
    )];
    let c = env.build(vec![obj(&d), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();

    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::CLUSTER_TYPE_VALID).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_TRUE);
    assert_eq!(condition.reason, REASON_CLUSTER_TYPE_VALID);
}

// Context("when DPUCluster exclusivity validation is performed")

#[tokio::test]
async fn validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_set_dpu_cluster_in_use_false_when_dpu_cluster_is_available()
 {
    let env = before_each();
    let d = dpu_cluster("static");
    let mut p = provisioner("test-provisioner", "default", 1, "test-dpu");
    let c = env.build(vec![obj(&d), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();

    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_IN_USE).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_FALSE);
    assert_eq!(condition.reason, REASON_DPU_CLUSTER_AVAILABLE);
    assert!(condition.message.contains("available"));
    assert!(condition.message.contains("not in use"));

    assert!(receive_containing(&env.recorder, "DPUClusterAvailable"));
}

#[tokio::test]
async fn validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_set_dpu_cluster_in_use_true_when_dpu_cluster_is_already_in_use_by_another_provisioner()
 {
    let env = before_each();
    let d = dpu_cluster("static");
    let mut first = provisioner("first-provisioner", "default", 1, "test-dpu");
    let mut second = provisioner("second-provisioner", "default", 1, "test-dpu");
    let c = env.build(vec![obj(&d), obj_mut(&mut first), obj_mut(&mut second)]);
    let validator = env.validator(&c);

    // Validate second provisioner - should fail because first provisioner already uses the DPUCluster
    validator
        .validate_dpu_cluster(&env.ctx, &mut second)
        .await
        .unwrap();

    let updated = get_updated(&c, &second).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_IN_USE).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_TRUE);
    assert_eq!(condition.reason, REASON_DPU_CLUSTER_IN_USE);
    assert!(condition.message.contains("already in use"));
    assert!(condition.message.contains("first-provisioner"));
    assert!(
        condition
            .message
            .contains("Each DPUCluster can only be referenced by one DPFHCPProvisioner")
    );

    assert!(receive_containing(&env.recorder, "DPUClusterInUse"));
}

#[tokio::test]
async fn validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_allow_same_provisioner_to_validate_multiple_times_skip_self()
 {
    let env = before_each();
    let d = dpu_cluster("static");
    let mut p = provisioner("test-provisioner", "default", 1, "test-dpu");
    let c = env.build(vec![obj(&d), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    // First validation
    validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();

    let mut updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_IN_USE).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_FALSE);

    // Second validation of same provisioner - should still succeed (skips itself)
    validator
        .validate_dpu_cluster(&env.ctx, &mut updated)
        .await
        .unwrap();

    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_IN_USE).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_FALSE);
}

#[tokio::test]
async fn validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_recover_when_conflicting_provisioner_is_deleted()
 {
    let env = before_each();
    let d = dpu_cluster("static");
    let mut p = provisioner("test-provisioner", "default", 1, "test-dpu");
    p.status.phase = provisioningv1alpha1::PHASE_FAILED.into();
    p.status.conditions = vec![cond(
        provisioningv1alpha1::DPU_CLUSTER_IN_USE,
        meta::CONDITION_TRUE, // Was True before
        REASON_DPU_CLUSTER_IN_USE,
        "DPUCluster in use by another provisioner",
        true,
        1,
    )];
    // Note: No conflicting provisioner in the client - it was deleted
    let c = env.build(vec![obj(&d), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator
        .validate_dpu_cluster(&env.ctx, &mut p)
        .await
        .unwrap();

    let updated = get_updated(&c, &p).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_IN_USE).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_FALSE);
    assert_eq!(condition.reason, REASON_DPU_CLUSTER_AVAILABLE);
}

#[tokio::test]
async fn validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_handle_provisioners_in_different_namespaces_referencing_same_dpu_cluster()
 {
    let env = before_each();
    let d = dpu_cluster("static");
    let mut first = provisioner("provisioner", "ns1", 1, "test-dpu");
    let mut second = provisioner("provisioner", "ns2", 1, "test-dpu");
    let c = env.build(vec![obj(&d), obj_mut(&mut first), obj_mut(&mut second)]);
    let validator = env.validator(&c);

    validator
        .validate_dpu_cluster(&env.ctx, &mut second)
        .await
        .unwrap();

    let updated = get_updated(&c, &second).await;
    let condition = find(&updated, provisioningv1alpha1::DPU_CLUSTER_IN_USE).expect("condition");
    assert_eq!(condition.status, meta::CONDITION_TRUE);
    assert!(condition.message.contains("ns1/provisioner"));
}

#[tokio::test]
async fn validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_not_emit_event_on_subsequent_validations_with_same_result()
 {
    let env = before_each();
    let d = dpu_cluster("static");
    let mut first = provisioner("first-provisioner", "default", 1, "test-dpu");
    let mut second = provisioner("second-provisioner", "default", 1, "test-dpu");
    let c = env.build(vec![obj(&d), obj_mut(&mut first), obj_mut(&mut second)]);
    let validator = env.validator(&c);

    // First validation - should emit multiple events
    let _ = validator.validate_dpu_cluster(&env.ctx, &mut second).await;

    // Drain all events from first validation (DPUClusterFound, ClusterTypeValid, DPUClusterInUse)
    let events = env.recorder.drain();
    assert!(events.iter().any(|e| e
        == "Warning DPUClusterInUse DPUCluster 'dpu-system/test-dpu' is already in use by DPFHCPProvisioner 'default/first-provisioner'. Each DPUCluster can only be referenced by one DPFHCPProvisioner"));

    let mut updated = get_updated(&c, &second).await;

    // Second validation - should NOT emit event (condition unchanged)
    let _ = validator.validate_dpu_cluster(&env.ctx, &mut updated).await;
    assert!(env.recorder.try_recv().is_none());
}
