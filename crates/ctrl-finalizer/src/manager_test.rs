//! Rust-only tests: the Go package `internal/controller/finalizer` has no tests. These cover
//! the documented behaviour of `Manager.HandleFinalizerCleanup` (ordering, error event,
//! requeue short-circuit, success event).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::{Ctx, FakeRecorder, ReconcileResult, Recorder};
use ctrlrt_fake::FakeClientBuilder;

use crate::{CleanupHandler, Manager};

struct TestHandler {
    name: &'static str,
    calls: Arc<Mutex<Vec<String>>>,
    outcome: fn() -> ctrlrt::Result<ReconcileResult>,
}

#[async_trait::async_trait]
impl CleanupHandler for TestHandler {
    fn name(&self) -> String {
        self.name.to_string()
    }
    async fn cleanup(
        &self,
        _ctx: &Ctx,
        _cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        self.calls.lock().unwrap().push(self.name.to_string());
        (self.outcome)()
    }
}

fn setup() -> (Manager, Arc<FakeRecorder>, Arc<Mutex<Vec<String>>>) {
    let recorder = Arc::new(FakeRecorder::new(100));
    let c = FakeClientBuilder::new().build();
    let m = Manager::new(c, recorder.clone() as Recorder);
    (m, recorder, Arc::new(Mutex::new(Vec::new())))
}

fn cr() -> provisioningv1alpha1::DPFHCPProvisioner {
    let mut cr = provisioningv1alpha1::DPFHCPProvisioner::default();
    cr.metadata.name = Some("test".into());
    cr.metadata.namespace = Some("default".into());
    cr
}

#[tokio::test]
async fn handlers_run_in_registration_order_and_emit_cleanup_succeeded() {
    let (mut m, recorder, calls) = setup();
    for name in ["kubeconfig-injection", "hostedcluster", "metallb"] {
        m.register_handler(TestHandler {
            name,
            calls: calls.clone(),
            outcome: || Ok(ReconcileResult::default()),
        });
    }
    let res = m
        .handle_finalizer_cleanup(&Ctx::background(), &cr())
        .await
        .unwrap();
    assert!(res.is_zero());
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["kubeconfig-injection", "hostedcluster", "metallb"]
    );
    assert_eq!(
        recorder.drain(),
        vec!["Normal CleanupSucceeded All resources cleaned up successfully"]
    );
}

#[tokio::test]
async fn handler_error_stops_cleanup_and_emits_warning() {
    let (mut m, recorder, calls) = setup();
    m.register_handler(TestHandler {
        name: "first",
        calls: calls.clone(),
        outcome: || Err(anyhow::anyhow!("boom")),
    });
    m.register_handler(TestHandler {
        name: "second",
        calls: calls.clone(),
        outcome: || Ok(ReconcileResult::default()),
    });
    let err = m
        .handle_finalizer_cleanup(&Ctx::background(), &cr())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "boom");
    assert_eq!(*calls.lock().unwrap(), vec!["first"]);
    assert_eq!(
        recorder.drain(),
        vec!["Warning CleanupHandlerFailed Cleanup handler 'first' failed: boom"]
    );
}

#[tokio::test]
async fn handler_requeue_after_stops_cleanup_without_event() {
    let (mut m, recorder, calls) = setup();
    m.register_handler(TestHandler {
        name: "first",
        calls: calls.clone(),
        outcome: || Ok(ReconcileResult::requeue_after(Duration::from_secs(10))),
    });
    m.register_handler(TestHandler {
        name: "second",
        calls: calls.clone(),
        outcome: || Ok(ReconcileResult::default()),
    });
    let res = m
        .handle_finalizer_cleanup(&Ctx::background(), &cr())
        .await
        .unwrap();
    assert_eq!(res.requeue_after, Duration::from_secs(10));
    assert_eq!(*calls.lock().unwrap(), vec!["first"]);
    assert!(recorder.is_empty());
}

#[tokio::test]
async fn no_handlers_completes_immediately() {
    let (m, recorder, _) = setup();
    assert_eq!(m.handler_count(), 0);
    let res = m
        .handle_finalizer_cleanup(&Ctx::background(), &cr())
        .await
        .unwrap();
    assert!(res.is_zero());
    assert_eq!(recorder.len(), 1);
}
