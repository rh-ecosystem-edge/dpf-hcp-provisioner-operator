//! End-to-end smoke test of the `manager` binary: run the real executable against an envtest
//! control plane (etcd + kube-apiserver with this repo's CRDs), drive a DPFHCPProvisioner
//! through creation and deletion, and check health probes and graceful shutdown.

use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn apply(
    client: &kube::Client,
    group_version: &str,
    plural: &str,
    ns: Option<&str>,
    obj: Value,
) {
    let base = if group_version == "v1" {
        "/api/v1".to_string()
    } else {
        format!("/apis/{group_version}")
    };
    let path = match ns {
        Some(ns) => format!("{base}/namespaces/{ns}/{plural}"),
        None => format!("{base}/{plural}"),
    };
    let req = http::Request::post(path)
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&obj).unwrap())
        .unwrap();
    let res: Result<Value, _> = client.request(req).await;
    if let Err(e) = res {
        let s = e.to_string();
        assert!(
            s.contains("AlreadyExists") || s.contains("already exists"),
            "create {plural}: {s}"
        );
    }
}

async fn get(client: &kube::Client, path: &str) -> Option<Value> {
    let req = http::Request::get(path.to_string())
        .body(Vec::new())
        .unwrap();
    client.request::<Value>(req).await.ok()
}

async fn delete(client: &kube::Client, path: &str) {
    let req = http::Request::delete(path.to_string())
        .body(Vec::new())
        .unwrap();
    let _ = client.request::<Value>(req).await;
}

async fn eventually<F, Fut>(what: &str, timeout: Duration, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out waiting for: {what}");
}

fn condition<'a>(obj: &'a Value, type_: &str) -> Option<&'a Value> {
    obj.pointer("/status/conditions")?
        .as_array()?
        .iter()
        .find(|c| c["type"] == type_)
}

#[tokio::test(flavor = "multi_thread")]
async fn manager_binary_reconciles_and_shuts_down() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let env = ctrlrt_envtest::Environment::builder()
        .crd_directory_paths(["config/crd/bases", "config/crd/external"])
        .error_if_crd_path_missing(true)
        .start()
        .await
        .expect("start envtest");
    let client = env.kube_client();

    let dir = tempfile::tempdir().unwrap();
    let kubeconfig = dir.path().join("kubeconfig");
    std::fs::write(&kubeconfig, env.kubeconfig_yaml()).unwrap();

    for ns in [
        "clusters",
        "dpf-operator-system",
        "dpf-hcp-provisioner-system",
        "smoke",
    ] {
        apply(
            &client,
            "v1",
            "namespaces",
            None,
            json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":ns}}),
        )
        .await;
    }

    // Start the real binary.
    let probe_port = free_port();
    // SMOKE_MANAGER_BIN lets the same scenario run against another build (e.g. the Go
    // reference binary) for differential comparison; SMOKE_DUMP writes the observed state.
    let bin = std::env::var("SMOKE_MANAGER_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_manager").to_string());
    let mut child = tokio::process::Command::new(bin)
        .arg(format!("--kubeconfig={}", kubeconfig.display()))
        .arg(format!(
            "--health-probe-bind-address=127.0.0.1:{probe_port}"
        ))
        .arg("--metrics-bind-address=0")
        .arg("--zap-log-level=info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn manager");
    let stderr = child.stderr.take().unwrap();
    let logs = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let logs2 = logs.clone();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            logs2.lock().unwrap().push_str(&l);
            logs2.lock().unwrap().push('\n');
        }
    });

    // Health probes (controller-runtime healthz handler).
    let http = reqwest::Client::new();
    eventually("healthz/readyz ok", Duration::from_secs(30), || {
        let http = http.clone();
        async move {
            let h = http
                .get(format!("http://127.0.0.1:{probe_port}/healthz"))
                .send()
                .await;
            let r = http
                .get(format!("http://127.0.0.1:{probe_port}/readyz"))
                .send()
                .await;
            matches!((h, r), (Ok(h), Ok(r)) if h.status() == 200 && r.status() == 200)
        }
    })
    .await;
    let body = http
        .get(format!("http://127.0.0.1:{probe_port}/healthz"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "ok");

    // Operator config + inputs.
    apply(&client, "provisioning.dpu.hcp.io/v1alpha1", "dpfhcpprovisionerconfigs", None,
        json!({"apiVersion":"provisioning.dpu.hcp.io/v1alpha1","kind":"DPFHCPProvisionerConfig","metadata":{"name":"default"},"spec":{}})).await;
    apply(&client, "v1", "secrets", Some("smoke"),
        json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":"ssh"},"stringData":{"id_rsa.pub":"ssh-rsa AAAA test"}})).await;
    apply(&client, "v1", "secrets", Some("smoke"),
        json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":"pull"},"type":"kubernetes.io/dockerconfigjson",
               "stringData":{".dockerconfigjson":"{\"auths\":{}}"}})).await;
    apply(&client, "provisioning.dpu.nvidia.com/v1alpha1", "dpuclusters", Some("dpf-operator-system"),
        json!({"apiVersion":"provisioning.dpu.nvidia.com/v1alpha1","kind":"DPUCluster","metadata":{"name":"dpu-cluster"},
               "spec":{"type":"static","maxNodes":10}})).await;
    apply(&client, "provisioning.dpu.hcp.io/v1alpha1", "dpfhcpprovisioners", Some("smoke"),
        json!({"apiVersion":"provisioning.dpu.hcp.io/v1alpha1","kind":"DPFHCPProvisioner","metadata":{"name":"prov"},
               "spec":{
                   "dpuClusterRef":{"name":"dpu-cluster","namespace":"dpf-operator-system"},
                   "baseDomain":"example.com",
                   "ocpReleaseImage":"registry.invalid/ocp-release:4.19.0-multi",
                   "sshKeySecretRef":{"name":"ssh"},
                   "pullSecretRef":{"name":"pull"},
                   "controlPlaneAvailabilityPolicy":"SingleReplica",
                   "dpuDeploymentRef":{"name":"dd","namespace":"dpf-operator-system"}
               }})).await;

    let prov_path =
        "/apis/provisioning.dpu.hcp.io/v1alpha1/namespaces/smoke/dpfhcpprovisioners/prov";
    eventually(
        "finalizer added and secrets validated",
        Duration::from_secs(60),
        || {
            let client = client.clone();
            async move {
                let Some(p) = get(&client, prov_path).await else {
                    return false;
                };
                let fin = p
                    .pointer("/metadata/finalizers")
                    .and_then(Value::as_array)
                    .is_some_and(|f| {
                        f.iter()
                            .any(|x| x == "dpfhcpprovisioner.provisioning.dpu.hcp.io/finalizer")
                    });
                let secrets_ok =
                    condition(&p, "SecretsValid").is_some_and(|c| c["status"] == "True");
                let cluster_ok =
                    condition(&p, "DPUClusterMissing").is_some_and(|c| c["status"] == "False");
                fin && secrets_ok && cluster_ok && p.pointer("/status/phase").is_some()
            }
        },
    )
    .await;

    // The image lookup fails against registry.invalid; the controller must report it on the
    // CR rather than crash.
    eventually(
        "BlueField image lookup condition reported",
        Duration::from_secs(60),
        || {
            let client = client.clone();
            async move {
                get(&client, prov_path)
                    .await
                    .is_some_and(|p| condition(&p, "BlueFieldOCPLayerImageFound").is_some())
            }
        },
    )
    .await;

    if let Ok(dump) = std::env::var("SMOKE_DUMP") {
        // Let reconciliation settle, then record status (without timestamps) and the
        // objects the operator created.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mut p = get(&client, prov_path).await.unwrap();
        if let Some(conds) = p
            .pointer_mut("/status/conditions")
            .and_then(Value::as_array_mut)
        {
            for c in conds.iter_mut() {
                c.as_object_mut().unwrap().remove("lastTransitionTime");
            }
            conds.sort_by(|a, b| a["type"].as_str().cmp(&b["type"].as_str()));
        }
        let secrets = get(&client, "/api/v1/namespaces/clusters/secrets")
            .await
            .unwrap();
        let mut names: Vec<String> = secrets["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["metadata"]["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        let out = json!({
            "status": p["status"],
            "finalizers": p["metadata"]["finalizers"],
            "clustersSecrets": names,
        });
        std::fs::write(dump, serde_json::to_string_pretty(&out).unwrap()).unwrap();
    }

    // Deletion runs the finalizer cleanup handlers and removes the CR.
    delete(&client, prov_path).await;
    eventually(
        "provisioner deleted after finalizer cleanup",
        Duration::from_secs(60),
        || {
            let client = client.clone();
            async move { get(&client, prov_path).await.is_none() }
        },
    )
    .await;

    // Graceful shutdown on SIGTERM.
    let pid = child.id().unwrap() as i32;
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid).unwrap(),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(40), child.wait())
        .await
        .expect("manager exits after SIGTERM")
        .unwrap();
    let logs = logs.lock().unwrap().clone();
    assert!(status.success(), "manager exit status {status:?}\n{logs}");
    let go_binary = std::env::var("SMOKE_MANAGER_BIN").is_ok();
    for needle in if go_binary {
        vec![]
    } else {
        vec![
            "starting manager",
            "Starting EventSource",
            "Starting Controller",
            "Starting workers",
            "Reconciling DPFHCPProvisioner",
            "Stopping and waiting for non leader election runnables",
        ]
    } {
        assert!(logs.contains(needle), "missing log line {needle:?}\n{logs}");
    }
    assert!(!logs.contains("panic"), "unexpected panic in logs\n{logs}");
    env.stop().await.unwrap();
}
