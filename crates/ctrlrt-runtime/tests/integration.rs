//! Integration tests against a real kube-apiserver (envtest): direct client, RESTMapper,
//! cache, controllers, leader election and health probes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctrlrt::{
    Builder, Client, Ctx, ManagerHandle, Object, ObjectKey, ReconcileResult, Reconciler, Request,
};
use ctrlrt_envtest::Environment;
use ctrlrt_runtime::{
    ByObject, CacheConfig, CacheOptions, ControllerOptions, Manager, ManagerOptions, RestConfig,
    ALL_NAMESPACES,
};
use k8s_openapi::api::coordination::v1::Lease;
use k8s_openapi::api::core::v1::{ConfigMap, Namespace};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct Foo {
    metadata: ObjectMeta,
}
ctrlrt::impl_resource!(Foo, "example.com", "v1", "Foo", "foos", Namespaced);

async fn env() -> Arc<Environment> {
    ctrlrt_envtest::shared(Environment::builder)
        .await
        .expect("envtest environment")
}

fn rest(env: &Environment) -> RestConfig {
    RestConfig::new(env.kube_config())
}

fn cm(ns: &str, name: &str, labels: &[(&str, &str)]) -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            namespace: Some(ns.into()),
            name: Some(name.into()),
            labels: if labels.is_empty() {
                None
            } else {
                Some(
                    labels
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                )
            },
            ..Default::default()
        },
        data: Some(BTreeMap::from([("k".to_string(), "v".to_string())])),
        ..Default::default()
    }
}

async fn ensure_ns(c: &Client, name: &str) {
    let mut ns = Namespace {
        metadata: ObjectMeta {
            name: Some(name.into()),
            ..Default::default()
        },
        ..Default::default()
    };
    match c.create(&mut ns).await {
        Ok(()) => {}
        Err(e) if ctrlrt::error::is_already_exists(&e) => {}
        Err(e) => panic!("create namespace: {e}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_client_crud_errors_and_rest_mapper() {
    let env = env().await;
    let (c, _kc) = ctrlrt_runtime::client_from_rest_config(&rest(&env)).unwrap();

    // The very first lookup of an unknown kind goes through aggregated discovery and fails
    // with ErrResourceDiscoveryFailed (Go: apiutil lazy mapper).
    let err = c.get::<Foo>(&ObjectKey::new("default", "x")).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "unable to retrieve the complete list of server APIs: example.com/v1: no matches for example.com/v1, Resource="
    );

    ensure_ns(&c, "direct").await;
    let mut obj = cm("direct", "one", &[]);
    c.create(&mut obj).await.unwrap();
    assert!(!obj.resource_version().is_empty());

    // later misses re-discover the group version and fail with NoKindMatchError
    let err = c.get::<Foo>(&ObjectKey::new("default", "x")).await.unwrap_err();
    assert!(ctrlrt::error::is_no_match(&err));
    assert_eq!(
        err.to_string(),
        r#"no matches for kind "Foo" in version "example.com/v1""#
    );

    // direct reads come back without TypeMeta (WithoutVersionDecoder)
    let raw = c
        .backend()
        .get(&ConfigMap::resource_info(), "direct", "one")
        .await
        .unwrap();
    assert!(raw.get("kind").is_none() && raw.get("apiVersion").is_none());

    // NotFound keeps the server message/reason/code/details
    let err = c.get::<ConfigMap>(&ObjectKey::new("direct", "missing")).await.unwrap_err();
    assert!(ctrlrt::error::is_not_found(&err));
    assert_eq!(err.to_string(), r#"configmaps "missing" not found"#);
    let api = ctrlrt::error::as_api_error(&err).unwrap();
    assert_eq!(api.code, 404);
    assert_eq!(api.details.as_ref().unwrap().name, "missing");

    // AlreadyExists
    let mut dup = cm("direct", "one", &[]);
    let err = c.create(&mut dup).await.unwrap_err();
    assert!(ctrlrt::error::is_already_exists(&err));

    // Conflict on stale resourceVersion
    let mut stale = obj.clone();
    obj.data = Some(BTreeMap::from([("k".to_string(), "v2".to_string())]));
    c.update(&mut obj).await.unwrap();
    stale.data = Some(BTreeMap::from([("k".to_string(), "v3".to_string())]));
    let err = c.update(&mut stale).await.unwrap_err();
    assert!(ctrlrt::error::is_conflict(&err));

    // merge patch
    let mut p = obj.clone();
    c.patch(
        &mut p,
        &ctrlrt::Patch::Merge(serde_json::json!({"metadata": {"labels": {"a": "b"}}})),
    )
    .await
    .unwrap();
    assert_eq!(p.labels().get("a").map(String::as_str), Some("b"));

    // list with label selector
    let l: Vec<ConfigMap> = c
        .list(&ctrlrt::ListOptions::new().in_namespace("direct").matching_labels([("a", "b")]))
        .await
        .unwrap();
    assert_eq!(l.len(), 1);

    // delete with preconditions
    let err = c
        .delete_with(
            &p,
            &ctrlrt::DeleteOptions {
                precondition_uid: Some("wrong".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(ctrlrt::error::is_conflict(&err), "{err}");
    c.delete_with(
        &p,
        &ctrlrt::DeleteOptions {
            propagation_policy: Some(ctrlrt::PropagationPolicy::Background),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let err = c.delete(&p).await.unwrap_err();
    assert!(ctrlrt::error::is_not_found(&err));

    // empty name validation
    let err = c.get::<ConfigMap>(&ObjectKey::new("direct", "")).await.unwrap_err();
    assert_eq!(err.to_string(), "resource name may not be empty");

    // server version
    let kc = rest(&env).build_client().unwrap();
    let v = ctrlrt_runtime::server_version(&kc).await.unwrap();
    assert_eq!(v.major, "1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kubeconfig_bytes_client() {
    let env = env().await;
    let (c, kc) = ctrlrt_runtime::client_from_kubeconfig_bytes(
        env.kubeconfig_yaml().as_bytes(),
        &Default::default(),
    )
    .await
    .unwrap();
    let _: Vec<Namespace> = c.list_all().await.unwrap();
    ctrlrt_runtime::server_version(&kc).await.unwrap();
}

#[derive(Default)]
struct Recorded {
    reqs: Mutex<Vec<Request>>,
}

struct CmReconciler {
    client: Client,
    seen: Arc<Recorded>,
}

#[async_trait::async_trait]
impl Reconciler for CmReconciler {
    async fn reconcile(&self, ctx: &Ctx, req: Request) -> ctrlrt::Result<ReconcileResult> {
        ctx.log.info("reconciling", ctrlrt::kv![]);
        // cached read of the object being reconciled
        match self.client.get::<ConfigMap>(&req).await {
            Ok(_) => {}
            Err(e) if ctrlrt::error::is_not_found(&e) => {}
            Err(e) => return Err(e),
        }
        self.seen.reqs.lock().unwrap().push(req);
        Ok(ReconcileResult::done())
    }
}

async fn http_get(addr: std::net::SocketAddr, path: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", addr.port()))
        .await
        .unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    let status: u16 = buf[9..12].parse().unwrap();
    let body = buf.split_once("\r\n\r\n").map(|x| x.1.to_string()).unwrap_or_default();
    (status, body)
}

async fn wait_until<F: Fn() -> bool>(what: &str, f: F) {
    for _ in 0..300 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manager_end_to_end() {
    let env = env().await;
    let (direct, _) = ctrlrt_runtime::client_from_rest_config(&rest(&env)).unwrap();
    let op_ns = "e2e-operator";
    let other_ns = "e2e-other";
    ensure_ns(&direct, op_ns).await;
    ensure_ns(&direct, other_ns).await;
    for o in [
        cm(op_ns, "op-unlabeled", &[]),
        cm(other_ns, "other-labeled", &[("bfcfg", "true")]),
        cm(other_ns, "other-unlabeled", &[]),
    ] {
        let mut o = o;
        let _ = direct.create(&mut o).await;
    }

    let opts = ManagerOptions {
        health_probe_bind_address: "127.0.0.1:0".into(),
        leader_election: true,
        leader_election_id: "4ebdb3db.dpu.hcp.io".into(),
        leader_election_namespace: op_ns.into(),
        cache: CacheOptions {
            by_object: vec![
                ByObject::new::<ConfigMap>()
                    .namespace(ALL_NAMESPACES, CacheConfig::with_labels([("bfcfg", "true")]))
                    .namespace(op_ns, CacheConfig::default()),
            ],
            ..Default::default()
        },
        controller: ControllerOptions {
            skip_name_validation: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mgr = Manager::new(rest(&env), opts).await.unwrap();
    mgr.add_healthz_check("healthz", ctrlrt_runtime::ping()).unwrap();
    mgr.add_readyz_check("readyz", ctrlrt_runtime::ping()).unwrap();

    let seen = Arc::new(Recorded::default());
    let spec = Builder::new()
        .for_type::<ConfigMap>(vec![])
        .build(Arc::new(CmReconciler {
            client: mgr.client(),
            seen: seen.clone(),
        }))
        .unwrap();
    assert_eq!(spec.name, "configmap");
    mgr.add_controller(spec).unwrap();

    // reads before the cache started fail like Go
    let err = mgr
        .client()
        .get::<ConfigMap>(&ObjectKey::new(op_ns, "op-unlabeled"))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "the cache is not started, can not read objects");

    let stop = CancellationToken::new();
    let m2 = mgr.clone();
    let s2 = stop.clone();
    let run = tokio::spawn(async move { m2.start(s2).await });

    // health probes
    let addr = mgr.health_probe_addr().unwrap();
    let mut ok = false;
    for _ in 0..50 {
        if let Ok(Ok(_)) = tokio::time::timeout(
            Duration::from_millis(200),
            tokio::net::TcpStream::connect(("127.0.0.1", addr.port())),
        )
        .await
        {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ok);
    assert_eq!(http_get(addr, "/healthz").await, (200, "ok".to_string()));
    assert_eq!(
        http_get(addr, "/readyz?verbose").await,
        (200, "[+]readyz ok\nhealthz check passed\n".to_string())
    );
    assert_eq!(http_get(addr, "/readyz/readyz").await, (200, "ok".to_string()));
    assert_eq!(http_get(addr, "/readyz/nope").await.0, 404);

    // leader election acquires the lease
    tokio::time::timeout(Duration::from_secs(30), mgr.elected())
        .await
        .expect("elected");
    let lease: Lease = direct
        .get(&ObjectKey::new(op_ns, "4ebdb3db.dpu.hcp.io"))
        .await
        .unwrap();
    let holder = lease.spec.as_ref().unwrap().holder_identity.clone().unwrap();
    assert!(holder.contains('_'), "{holder}");
    assert_eq!(lease.spec.as_ref().unwrap().lease_duration_seconds, Some(15));

    // the controller sees what the restricted cache sees: operator namespace (all) +
    // labeled ConfigMaps elsewhere
    let s = seen.clone();
    wait_until("initial reconciles", || {
        let r = s.reqs.lock().unwrap();
        r.contains(&ObjectKey::new(op_ns, "op-unlabeled"))
            && r.contains(&ObjectKey::new(other_ns, "other-labeled"))
    })
    .await;
    assert!(
        !seen
            .reqs
            .lock()
            .unwrap()
            .contains(&ObjectKey::new(other_ns, "other-unlabeled"))
    );

    // cached reads: GVK set, restricted like the multi-namespace cache
    let c = mgr.client();
    let raw = c
        .backend()
        .get(&ConfigMap::resource_info(), op_ns, "op-unlabeled")
        .await
        .unwrap();
    assert_eq!(raw["kind"], "ConfigMap");
    assert_eq!(raw["apiVersion"], "v1");
    let err = c
        .get::<ConfigMap>(&ObjectKey::new(other_ns, "other-unlabeled"))
        .await
        .unwrap_err();
    assert!(ctrlrt::error::is_not_found(&err));
    assert_eq!(err.to_string(), r#"ConfigMap "other-unlabeled" not found"#);
    let all: Vec<ConfigMap> = c.list_all().await.unwrap();
    let names: BTreeSet<String> = all.iter().map(|o| o.name().to_string()).collect();
    assert!(names.contains("op-unlabeled") && names.contains("other-labeled"));
    assert!(!names.contains("other-unlabeled"));
    // API reader is uncached
    let _: ConfigMap = mgr
        .api_reader()
        .get(&ObjectKey::new(other_ns, "other-unlabeled"))
        .await
        .unwrap();

    // watch events: new object, update, delete
    let mut created = cm(op_ns, "late", &[]);
    direct.create(&mut created).await.unwrap();
    let s = seen.clone();
    wait_until("reconcile of new object", || {
        s.reqs.lock().unwrap().contains(&ObjectKey::new(op_ns, "late"))
    })
    .await;
    let before = seen.reqs.lock().unwrap().len();
    direct.delete(&created).await.unwrap();
    let s = seen.clone();
    wait_until("reconcile after delete", || s.reqs.lock().unwrap().len() > before).await;

    // a cached read of a CRD-less kind fails with NoKindMatchError
    let err = c.get::<Foo>(&ObjectKey::new("default", "x")).await.unwrap_err();
    assert!(ctrlrt::error::is_no_match(&err), "{err}");

    // metrics
    let text = ctrlrt_runtime::metrics::gather_text();
    assert!(text.contains(r#"controller_runtime_reconcile_total{controller="configmap",result="success"}"#));
    assert!(text.contains(r#"leader_election_master_status{name="4ebdb3db.dpu.hcp.io"} 1"#));
    assert!(text.contains("rest_client_requests_total"));
    assert!(text.contains(r#"workqueue_adds_total{controller="configmap",name="configmap"}"#));

    // graceful shutdown
    stop.cancel();
    let res = tokio::time::timeout(Duration::from_secs(40), run)
        .await
        .expect("manager stopped")
        .unwrap();
    assert!(res.is_ok(), "{res:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leader_election_lost_stops_manager_and_standby_is_not_elected() {
    let env = env().await;
    let (direct, _) = ctrlrt_runtime::client_from_rest_config(&rest(&env)).unwrap();
    let ns = "e2e-le";
    ensure_ns(&direct, ns).await;
    let opts = || ManagerOptions {
        leader_election: true,
        leader_election_id: "le-lost-test".into(),
        leader_election_namespace: ns.into(),
        lease_duration: Duration::from_secs(4),
        renew_deadline: Duration::from_secs(3),
        retry_period: Duration::from_secs(1),
        ..Default::default()
    };
    let leader = Manager::new(rest(&env), opts()).await.unwrap();
    let stop = CancellationToken::new();
    let l2 = leader.clone();
    let s2 = stop.clone();
    let run = tokio::spawn(async move { l2.start(s2).await });
    tokio::time::timeout(Duration::from_secs(20), leader.elected())
        .await
        .expect("elected");

    // a second manager stays on standby while the lease is held
    let standby = Manager::new(rest(&env), opts()).await.unwrap();
    let sb_stop = CancellationToken::new();
    let sb2 = standby.clone();
    let sbs = sb_stop.clone();
    let sb_run = tokio::spawn(async move { sb2.start(sbs).await });
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!standby.is_elected());
    sb_stop.cancel();
    let r = tokio::time::timeout(Duration::from_secs(10), sb_run).await.unwrap().unwrap();
    assert!(r.is_ok(), "{r:?}");

    // steal the lease: the leader fails to renew and the manager returns "leader election lost"
    // (retry on conflict: the leader renews the lease concurrently every RetryPeriod)
    let mut attempts = 0;
    loop {
        let mut lease: Lease = direct.get(&ObjectKey::new(ns, "le-lost-test")).await.unwrap();
        let spec = lease.spec.as_mut().unwrap();
        spec.holder_identity = Some("thief".into());
        spec.lease_duration_seconds = Some(3600);
        match direct.update(&mut lease).await {
            Ok(()) => break,
            Err(e) if ctrlrt::error::is_conflict(&e) && attempts < 10 => attempts += 1,
            Err(e) => panic!("stealing lease: {e}"),
        }
    }
    let res = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("manager returned")
        .unwrap();
    assert_eq!(res.unwrap_err().to_string(), "leader election lost");
    stop.cancel();
}

#[test]
fn discovery_failure_is_no_match() {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    let mut failures = BTreeMap::new();
    failures.insert(
        "metallb.io/v1beta1".to_string(),
        Arc::new(anyhow::Error::new(ctrlrt_runtime::client::NoResourceMatchError {
            group: "metallb.io".into(),
            version: "v1beta1".into(),
            resource: String::new(),
        })),
    );
    let err: anyhow::Error = ctrlrt_runtime::client::ErrResourceDiscoveryFailed { failures }.into();
    // Go: meta.IsNoMatchError(&ErrResourceDiscoveryFailed{...NotFound...}) == true
    assert!(ctrlrt::error::is_no_match(&err));
}
