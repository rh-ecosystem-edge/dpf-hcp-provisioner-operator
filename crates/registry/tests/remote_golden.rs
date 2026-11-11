//! Replays the scenarios of `hack/golden/registry-remote` (recorded from Go's
//! go-containerregistry against scripted httptest registries) against the Rust port
//! with an equivalently scripted fake registry, comparing results, exact error strings
//! and the sequence of requests sent.

mod support;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Response;
use registry::authn::{self, AuthConfig, Authenticator, Keychain, Resource};
use registry::name;
use registry::remote;
use registry::transport::Backoff;
use serde_json::Value;
use support::{AuthMode, FakeRegistry};

struct StaticKeychain(Option<(String, String)>);

impl Keychain for StaticKeychain {
    fn resolve(&self, _: &dyn Resource) -> anyhow::Result<Arc<dyn Authenticator>> {
        Ok(match &self.0 {
            Some((u, p)) => authn::from_config(AuthConfig {
                username: u.clone(),
                password: p.clone(),
                ..Default::default()
            }),
            None => authn::anonymous(),
        })
    }
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

async fn run(sc: &Value) -> (String, String, Vec<String>) {
    let (host, reg) = if sc.get("dead").and_then(Value::as_bool).unwrap_or(false) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let h = l.local_addr().unwrap().to_string();
        drop(l);
        (h, None)
    } else {
        let reg = FakeRegistry::start(AuthMode::None).await;
        (reg.host(), Some(reg))
    };
    if let Some(reg) = &reg {
        let rules = sc["rules"].as_array().cloned().unwrap_or_default();
        let host2 = host.clone();
        reg.hook(move |l, _| {
            for r in &rules {
                let m = s(r, "method");
                if (m.is_empty() || m == l.method) && s(r, "path") == l.path {
                    let mut b = Response::builder().status(r["status"].as_u64().unwrap() as u16);
                    if let Some(Value::Object(h)) = r.get("headers") {
                        for (k, v) in h {
                            b = b.header(k.as_str(), v.as_str().unwrap().replace("HOST", &host2));
                        }
                    }
                    let body = if l.method == "HEAD" {
                        String::new()
                    } else {
                        s(r, "body")
                    };
                    return Some(b.body(Body::from(body)).unwrap());
                }
            }
            Some(Response::builder().status(404).body(Body::empty()).unwrap())
        });
    }
    let user = s(sc, "user");
    let kc: Arc<dyn Keychain> = Arc::new(StaticKeychain(
        (!user.is_empty()).then(|| (user.clone(), s(sc, "pass"))),
    ));
    let opts = [
        remote::with_auth_from_keychain(kc),
        remote::with_retry_backoff(Backoff::fast()),
    ];
    let reference = name::parse_reference(&s(sc, "ref").replace("HOST", &host), &[]).unwrap();
    let mut result = String::new();
    let err = match s(sc, "op").as_str() {
        "head" => match remote::head(&reference, &opts).await {
            Ok(d) => {
                result = format!("{} {} {}", d.media_type, d.digest, d.size);
                None
            }
            Err(e) => Some(e),
        },
        "get" => match remote::get(&reference, &opts).await {
            Ok(d) => {
                result = format!(
                    "{} {} {}",
                    d.descriptor.media_type, d.descriptor.digest, d.descriptor.size
                );
                None
            }
            Err(e) => Some(e),
        },
        "image" | "config" => match remote::image(&reference, &opts).await {
            Ok(img) => {
                result = img.digest().to_string();
                if s(sc, "op") == "config" {
                    match img.config_file().await {
                        Ok(cfg) => {
                            let parts: Vec<String> = cfg
                                .config
                                .labels
                                .iter()
                                .map(|(k, v)| format!("{k}:{v}"))
                                .collect();
                            result = format!("map[{}]", parts.join(" "));
                            None
                        }
                        Err(e) => Some(e),
                    }
                } else {
                    None
                }
            }
            Err(e) => Some(e),
        },
        other => panic!("unknown op {other}"),
    };
    let expected = err
        .map(|e| e.to_string().replace(&host, "HOST"))
        .unwrap_or_default();
    let requests = reg
        .map(|r| {
            r.log()
                .iter()
                .map(|l| {
                    let q = if l.query.is_empty() {
                        String::new()
                    } else {
                        format!("?{}", l.query)
                    };
                    format!("{} {}{}", l.method, l.path, q)
                })
                .collect()
        })
        .unwrap_or_default();
    (expected, result, requests)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_flows_match_go() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/remote_golden.json");
    let scenarios: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut failures = Vec::new();
    assert!(scenarios.as_array().unwrap().len() >= 30);
    for sc in scenarios.as_array().unwrap() {
        let name = s(sc, "name");
        let (expected, result, requests) = run(sc).await;
        if expected != s(sc, "expected") {
            failures.push(format!(
                "{name}: error\n  want {:?}\n  got  {expected:?}",
                s(sc, "expected")
            ));
        }
        if result != s(sc, "result") {
            failures.push(format!(
                "{name}: result\n  want {:?}\n  got  {result:?}",
                s(sc, "result")
            ));
        }
        let want_reqs: Vec<String> = sc
            .get("requests")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        if requests != want_reqs {
            failures.push(format!(
                "{name}: requests\n  want {want_reqs:?}\n  got  {requests:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
