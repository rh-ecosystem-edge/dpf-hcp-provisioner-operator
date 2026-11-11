//! Event recorder tests: correlator golden comparison against client-go
//! (fixtures from hack/golden/ctrlrt-cmd-events) and end-to-end recorder behaviour against a
//! fake API backend.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ctrlrt::client::{ClientBackend, DeleteOptions, ListOptions, Patch};
use ctrlrt::error::ApiError;
use ctrlrt::log::{CaptureSink, Logger};
use ctrlrt::object::ResourceInfo;
use ctrlrt_cmd::events::{EventCorrelator, RecorderProvider, generate_event_name};
use k8s_openapi::api::core::v1::{Event, EventSource, ObjectReference};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};
use k8s_openapi::jiff::Timestamp;
use serde_json::{Value, json};

fn base() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2024-03-05T07:08:09.123456789Z")
        .unwrap()
        .with_timezone(&Utc)
}

/// Drop nulls, empty strings and empty objects (Go serializes non-omitempty zero values).
fn normalize(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, v) in m {
                let n = normalize(v);
                let empty = n.is_null()
                    || n.as_str() == Some("")
                    || n.as_object().is_some_and(|o| o.is_empty());
                if !empty {
                    out.insert(k.clone(), n);
                }
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(normalize).collect()),
        o => o.clone(),
    }
}

fn make_event(step: &Value, t: DateTime<Utc>) -> Event {
    let ns = step["namespace"].as_str().unwrap();
    let name = step["name"].as_str().unwrap();
    let r = ObjectReference {
        kind: Some(step["kind"].as_str().unwrap().into()),
        namespace: Some(ns.to_string()).filter(|s| !s.is_empty()),
        name: Some(name.into()),
        uid: Some(step["uid"].as_str().unwrap().into()),
        api_version: Some("provisioning.dpu.hcp.io/v1alpha1".into()),
        resource_version: Some("7".into()),
        field_path: None,
    };
    let mt = Time(Timestamp::from_second(t.timestamp()).unwrap());
    let component = step["component"].as_str().unwrap().to_string();
    Event {
        metadata: ObjectMeta {
            name: Some(generate_event_name(name, t.timestamp_nanos_opt().unwrap())),
            namespace: Some(if ns.is_empty() { "default".into() } else { ns.into() }),
            ..Default::default()
        },
        involved_object: r,
        reason: Some(step["reason"].as_str().unwrap().into()),
        message: Some(step["message"].as_str().unwrap().into()),
        first_timestamp: Some(mt.clone()),
        last_timestamp: Some(mt),
        count: Some(1),
        type_: Some(step["type"].as_str().unwrap().into()),
        source: Some(EventSource {
            component: Some(component.clone()),
            host: None,
        }),
        reporting_component: Some(component),
        reporting_instance: Some(String::new()),
        ..Default::default()
    }
}

#[test]
fn correlator_matches_client_go() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/events/correlator.json");
    let results: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let now = Arc::new(Mutex::new(base()));
    let n2 = now.clone();
    let c = EventCorrelator::new(Arc::new(move || *n2.lock().unwrap()));
    let mut rv = 100;
    for (i, r) in results.iter().enumerate() {
        let step = &r["step"];
        let at = step["at"].as_f64().unwrap();
        let t = base() + chrono::Duration::nanoseconds((at * 1e9) as i64);
        *now.lock().unwrap() = t;
        let ev = make_event(step, t);
        let res = c.correlate(&ev);
        assert_eq!(res.skip, r["skip"].as_bool().unwrap(), "step {i}: {step}");
        if res.skip {
            continue;
        }
        let got = normalize(&serde_json::to_value(&res.event).unwrap());
        let mut want = normalize(&r["event"]);
        // Go's ReportingController field is named reportingComponent in JSON.
        want.as_object_mut().unwrap().remove("eventTime");
        let mut got = got;
        got.as_object_mut().unwrap().remove("apiVersion");
        got.as_object_mut().unwrap().remove("kind");
        assert_eq!(got, want, "step {i}");
        let want_patch = r["patch"].as_str().unwrap_or("");
        match &res.patch {
            None => assert_eq!(want_patch, "", "step {i}"),
            Some(p) => assert_eq!(p.to_string(), want_patch, "step {i}"),
        }
        let action = if res.event.count.unwrap_or(0) > 1 { "patch" } else { "create" };
        assert_eq!(action, r["action"].as_str().unwrap(), "step {i}");
        let mut new_event = res.event.clone();
        rv += 1;
        new_event.metadata.resource_version = Some(rv.to_string());
        c.update_state(&new_event);
    }
}

#[derive(Default)]
struct FakeApi {
    calls: Mutex<Vec<(String, Value)>>,
    /// Errors to return for the next calls (in order).
    errors: Mutex<Vec<ctrlrt::Error>>,
    rv: AtomicI64,
}

impl FakeApi {
    fn next_err(&self) -> Option<ctrlrt::Error> {
        let mut e = self.errors.lock().unwrap();
        if e.is_empty() { None } else { Some(e.remove(0)) }
    }
    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ClientBackend for FakeApi {
    async fn get(&self, _: &ResourceInfo, _: &str, _: &str) -> ctrlrt::Result<Value> {
        unimplemented!()
    }
    async fn list(&self, _: &ResourceInfo, _: &ListOptions) -> ctrlrt::Result<Vec<Value>> {
        unimplemented!()
    }
    async fn create(&self, res: &ResourceInfo, mut obj: Value) -> ctrlrt::Result<Value> {
        assert_eq!(res.kind, "Event");
        self.calls.lock().unwrap().push(("create".into(), obj.clone()));
        if let Some(e) = self.next_err() {
            return Err(e);
        }
        obj["metadata"]["resourceVersion"] = json!(self.rv.fetch_add(1, Ordering::SeqCst).to_string());
        Ok(obj)
    }
    async fn update(&self, _: &ResourceInfo, _: Value) -> ctrlrt::Result<Value> {
        unimplemented!()
    }
    async fn update_subresource(&self, _: &ResourceInfo, _: &str, _: Value) -> ctrlrt::Result<Value> {
        unimplemented!()
    }
    async fn patch(&self, res: &ResourceInfo, ns: &str, name: &str, patch: &Patch) -> ctrlrt::Result<Value> {
        assert_eq!(res.kind, "Event");
        let Patch::Strategic(p) = patch else { panic!("expected strategic merge patch") };
        self.calls
            .lock()
            .unwrap()
            .push(("patch".into(), json!({"namespace": ns, "name": name, "patch": p})));
        if let Some(e) = self.next_err() {
            return Err(e);
        }
        Ok(json!({
            "apiVersion": "v1", "kind": "Event",
            "metadata": {"name": name, "namespace": ns, "resourceVersion": self.rv.fetch_add(1, Ordering::SeqCst).to_string()},
            "involvedObject": {"kind": "DPFHCPProvisioner", "name": "p1", "namespace": "ns"},
            "count": p["count"], "message": p["message"], "reason": "Created", "type": "Normal",
            "source": {"component": "comp"}
        }))
    }
    async fn patch_subresource(&self, _: &ResourceInfo, _: &str, _: &str, _: &str, _: &Patch) -> ctrlrt::Result<Value> {
        unimplemented!()
    }
    async fn delete(&self, _: &ResourceInfo, _: &str, _: &str, _: &DeleteOptions) -> ctrlrt::Result<()> {
        unimplemented!()
    }
}

fn obj_ref(ns: &str, name: &str) -> ObjectReference {
    ObjectReference {
        api_version: Some("provisioning.dpu.hcp.io/v1alpha1".into()),
        kind: Some("DPFHCPProvisioner".into()),
        name: Some(name.into()),
        namespace: Some(ns.to_string()).filter(|s| !s.is_empty()),
        uid: Some("uid-1".into()),
        resource_version: Some("5".into()),
        field_path: None,
    }
}

struct Env {
    api: Arc<FakeApi>,
    capture: Arc<CaptureSink>,
    provider: RecorderProvider,
}

fn env() -> Env {
    let api = Arc::new(FakeApi::default());
    let capture = Arc::new(CaptureSink::default());
    let clock_ns = Arc::new(AtomicI64::new(base().timestamp_nanos_opt().unwrap()));
    let clock: ctrlrt_cmd::events::Clock = Arc::new(move || {
        let n = clock_ns.fetch_add(1_000_000_000, Ordering::SeqCst);
        DateTime::from_timestamp_nanos(n)
    });
    let provider = RecorderProvider::with_options(
        ctrlrt::Client::new(api.clone()),
        Logger::with_sink(capture.clone()).with_name("events"),
        clock,
        Duration::from_millis(10),
    );
    Env { api, capture, provider }
}

async fn wait_calls(api: &FakeApi, n: usize) -> Vec<(String, Value)> {
    for _ in 0..500 {
        let c = api.calls();
        if c.len() >= n {
            return c;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("expected {n} calls, got {:?}", api.calls());
}

#[tokio::test]
async fn records_creates_and_patches_events() {
    let e = env();
    let rec = e.provider.get_event_recorder_for("dpfhcpprovisioner-controller");
    rec.event_ref(&obj_ref("ns", "p1"), "Normal", "Created", "created <thing>");
    let calls = wait_calls(&e.api, 1).await;
    let (verb, body) = &calls[0];
    assert_eq!(verb, "create");
    let want_name = generate_event_name("p1", base().timestamp_nanos_opt().unwrap());
    assert_eq!(
        body,
        &json!({
            "apiVersion": "v1",
            "kind": "Event",
            "metadata": {"name": want_name, "namespace": "ns"},
            "involvedObject": {
                "apiVersion": "provisioning.dpu.hcp.io/v1alpha1", "kind": "DPFHCPProvisioner",
                "name": "p1", "namespace": "ns", "uid": "uid-1", "resourceVersion": "5"
            },
            "reason": "Created",
            "message": "created <thing>",
            "source": {"component": "dpfhcpprovisioner-controller"},
            "firstTimestamp": "2024-03-05T07:08:09Z",
            "lastTimestamp": "2024-03-05T07:08:09Z",
            "count": 1,
            "type": "Normal",
            "reportingComponent": "dpfhcpprovisioner-controller",
            "reportingInstance": ""
        })
    );

    // Same event again -> PATCH of the existing event (count 2).
    rec.event_ref(&obj_ref("ns", "p1"), "Normal", "Created", "created <thing>");
    let calls = wait_calls(&e.api, 2).await;
    assert_eq!(calls[1].0, "patch");
    assert_eq!(
        calls[1].1,
        json!({"namespace": "ns", "name": want_name, "patch": {
            // the test clock advances 1s per read (makeEvent, aggregator, spam filter)
            "count": 2, "lastTimestamp": "2024-03-05T07:08:12Z", "message": "created <thing>"}})
    );

    // Logged at V(1) on the events logger: message, type, object, reason.
    for _ in 0..100 {
        if e.capture.records.lock().unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let recs = e.capture.records.lock().unwrap().clone();
    assert_eq!(recs[0].1, "events: created <thing>");
    let kvs: Vec<&str> = recs[0].2.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kvs, ["type", "object", "reason"]);
    assert_eq!(recs[0].2[0].1, json!("Normal"));
    assert_eq!(recs[0].2[1].1["name"], json!("p1"));
    assert_eq!(recs[0].2[2].1, json!("Created"));
}

#[tokio::test]
async fn cluster_scoped_objects_use_default_namespace_and_invalid_types_are_dropped() {
    let e = env();
    let rec = e.provider.get_event_recorder_for("csr-approval-controller");
    rec.event_ref(&obj_ref("", "node-1"), "Bogus", "X", "dropped");
    rec.event_ref(&obj_ref("", "node-1"), "Warning", "Bad", "bad node");
    let calls = wait_calls(&e.api, 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(e.api.calls().len(), 1);
    assert_eq!(calls[0].1["metadata"]["namespace"], "default");
    assert_eq!(calls[0].1["type"], "Warning");
}

#[tokio::test]
async fn patch_not_found_falls_back_to_create() {
    let e = env();
    let rec = e.provider.get_event_recorder_for("c");
    rec.event_ref(&obj_ref("ns", "p1"), "Normal", "R", "m");
    wait_calls(&e.api, 1).await;
    e.api
        .errors
        .lock()
        .unwrap()
        .push(ApiError::not_found("", "events", "p1.x").into());
    rec.event_ref(&obj_ref("ns", "p1"), "Normal", "R", "m");
    let calls = wait_calls(&e.api, 3).await;
    assert_eq!(calls[1].0, "patch");
    assert_eq!(calls[2].0, "create");
    assert_eq!(calls[2].1["count"], 2);
    assert!(calls[2].1["metadata"].get("resourceVersion").is_none());
}

#[tokio::test]
async fn retries_transport_errors_but_not_api_errors() {
    let e = env();
    let rec = e.provider.get_event_recorder_for("c");
    e.api.errors.lock().unwrap().push(anyhow::anyhow!("connection refused"));
    rec.event_ref(&obj_ref("ns", "p1"), "Normal", "R", "m");
    let calls = wait_calls(&e.api, 2).await;
    assert_eq!(calls[0].0, "create");
    assert_eq!(calls[1].0, "create");
    assert_eq!(calls[0].1, calls[1].1);

    e.api
        .errors
        .lock()
        .unwrap()
        .push(ApiError::forbidden("", "events", "", "denied").into());
    rec.event_ref(&obj_ref("ns", "p2"), "Normal", "R", "m");
    wait_calls(&e.api, 3).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(e.api.calls().len(), 3);
}

#[tokio::test]
async fn stop_drops_later_events() {
    let e = env();
    let rec = e.provider.get_event_recorder_for("c");
    rec.event_ref(&obj_ref("ns", "p1"), "Normal", "R", "m");
    wait_calls(&e.api, 1).await;
    e.provider.stop(Duration::from_secs(5)).await;
    rec.event_ref(&obj_ref("ns", "p1"), "Normal", "R2", "m2");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(e.api.calls().len(), 1);
}
