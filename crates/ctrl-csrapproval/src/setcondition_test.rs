//! Port of `setcondition_test.go` (Describe "setCondition").

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::meta::{CONDITION_TRUE, find_status_condition};
use ctrlrt::{Client, Ctx, FakeRecorder, ObjectKey};
use ctrlrt_fake::{FakeClient, FakeClientBuilder, Reaction, Scheme, obj};
use hostedclient::ClientManager;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, ObjectMeta};

use crate::csrapproval::CSRApprover;

/// BeforeEach state.
struct Suite {
    ctx: Ctx,
    cr: provisioningv1alpha1::DPFHCPProvisioner,
    test_scheme: Scheme,
}

fn before_each() -> Suite {
    Suite {
        ctx: Ctx::background(),
        test_scheme: Scheme::new().register::<provisioningv1alpha1::DPFHCPProvisioner>(),
        cr: provisioningv1alpha1::DPFHCPProvisioner {
            metadata: ObjectMeta {
                name: Some("doca".into()),
                namespace: Some("clusters".into()),
                ..Default::default()
            },
            ..Default::default()
        },
    }
}

impl Suite {
    fn fake(&self) -> FakeClient {
        FakeClientBuilder::new()
            .with_scheme(self.test_scheme.clone())
            .with_objects([obj(&self.cr)])
            .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
            .build_fake()
    }
}

/// `newApprover(c)`: approver with a fake recorder (the client manager is unused by
/// setCondition; Go leaves it nil).
fn new_approver(c: Client, recorder: Arc<FakeRecorder>) -> CSRApprover {
    CSRApprover {
        mgmt_client: c.clone(),
        recorder,
        client_manager: Arc::new(ClientManager::new(c)),
    }
}

mod set_condition {
    use super::*;

    #[tokio::test]
    async fn succeeds_when_there_is_no_conflict() {
        let mut s = before_each();
        let c = s.fake().client();
        let approver = new_approver(c, Arc::new(FakeRecorder::new(10)));

        approver
            .set_condition(
                &s.ctx,
                &mut s.cr,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_CSR_APPROVAL_ACTIVE,
                "active",
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn retries_and_succeeds_after_a_single_conflict() {
        // The fake client enforces resourceVersion optimistic concurrency — if you hold a
        // stale copy of the object and another writer updates it first, the fake client's own
        // tracker returns the conflict error as a real API server.
        //
        // Race reproduced here:
        //   1. CSR controller fetches the CR (stale copy, resourceVersion N)
        //   2. Main controller updates the CR status (resourceVersion becomes N+1)
        //   3. CSR controller tries Status().Update with resourceVersion N → conflict
        //   4. RetryOnConflict re-fetches (gets N+1) and retries → success
        let s = before_each();
        let c = s.fake().client();

        // Step 1: CSR controller fetches the CR — this is the stale copy it will use
        let mut stale_cr: provisioningv1alpha1::DPFHCPProvisioner =
            c.get(&ObjectKey::new("clusters", "doca")).await.unwrap();

        // Step 2: main controller updates the CR concurrently, incrementing the
        // resourceVersion
        let mut concurrent_cr = stale_cr.clone();
        concurrent_cr.status.conditions.push(Condition {
            type_: "SomeOtherCondition".into(),
            status: CONDITION_TRUE.into(),
            reason: "UpdatedByMainController".into(),
            last_transition_time: ctrlrt::meta::now(),
            message: String::new(),
            observed_generation: None,
        });
        c.update_status(&mut concurrent_cr).await.unwrap();

        // Step 3+4: setCondition is called with the stale copy
        let approver = new_approver(c, Arc::new(FakeRecorder::new(10)));
        approver
            .set_condition(
                &s.ctx,
                &mut stale_cr,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_CSR_APPROVAL_ACTIVE,
                "active",
            )
            .await
            .unwrap();
    }
}

/// Rust-only: the conflict path is exercised with injected conflicts, plus the
/// condition/event side effects.
mod set_condition_extra {
    use super::*;

    fn conflict_reactor(fake: &FakeClient, times: usize) -> Arc<AtomicUsize> {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        fake.prepend_reactor("update", "dpfhcpprovisioners/status", move |_| {
            if counter.fetch_add(1, Ordering::SeqCst) < times {
                Reaction::error(ctrlrt::error::ApiError::conflict(
                    "provisioning.dpu.hcp.io",
                    "dpfhcpprovisioners",
                    "doca",
                    "the object has been modified; please apply your changes to the latest version and try again",
                ))
            } else {
                Reaction::NotHandled
            }
        });
        calls
    }

    #[tokio::test]
    async fn retries_injected_conflicts_and_records_event() {
        let mut s = before_each();
        let fake = s.fake();
        let calls = conflict_reactor(&fake, 2);
        let recorder = Arc::new(FakeRecorder::new(10));
        let approver = new_approver(fake.client(), recorder.clone());

        approver
            .set_condition(
                &s.ctx,
                &mut s.cr,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_CSR_APPROVAL_ACTIVE,
                "active",
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(recorder.drain(), vec!["Normal Active active".to_string()]);

        let stored: provisioningv1alpha1::DPFHCPProvisioner = fake
            .client()
            .get(&ObjectKey::new("clusters", "doca"))
            .await
            .unwrap();
        let cond = find_status_condition(
            &stored.status.conditions,
            provisioningv1alpha1::CSR_AUTO_APPROVAL_ACTIVE,
        )
        .unwrap();
        assert_eq!(cond.status, "True");
        assert_eq!(cond.reason, "Active");
        assert_eq!(cond.message, "active");

        // Unchanged condition: no update, no event.
        approver
            .set_condition(
                &s.ctx,
                &mut s.cr,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_CSR_APPROVAL_ACTIVE,
                "active",
            )
            .await
            .unwrap();
        assert!(recorder.is_empty());
    }

    #[tokio::test]
    async fn gives_up_after_five_conflicts() {
        let mut s = before_each();
        let fake = s.fake();
        let calls = conflict_reactor(&fake, usize::MAX);
        let approver = new_approver(fake.client(), Arc::new(FakeRecorder::new(10)));

        let err = approver
            .set_condition(
                &s.ctx,
                &mut s.cr,
                "False",
                provisioningv1alpha1::REASON_KUBECONFIG_NOT_AVAILABLE,
                "x",
            )
            .await
            .unwrap_err();
        assert!(ctrlrt::error::is_conflict(&err));
        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn false_status_records_warning_event() {
        let mut s = before_each();
        let recorder = Arc::new(FakeRecorder::new(10));
        let approver = new_approver(s.fake().client(), recorder.clone());
        approver
            .set_condition(
                &s.ctx,
                &mut s.cr,
                "False",
                provisioningv1alpha1::REASON_KUBECONFIG_NOT_AVAILABLE,
                "Kubeconfig secret not available for hosted cluster",
            )
            .await
            .unwrap();
        assert_eq!(
            recorder.drain(),
            vec![
                "Warning KubeconfigNotAvailable Kubeconfig secret not available for hosted cluster"
                    .to_string()
            ]
        );
        // The passed object is refreshed with the server copy.
        assert_eq!(s.cr.status.conditions.len(), 1);
    }
}
