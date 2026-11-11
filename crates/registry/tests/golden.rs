//! Differential tests against fixtures generated from the Go implementation by
//! `go run -mod=vendor ./hack/golden/registry`.

use std::path::PathBuf;

use registry::authn::AuthConfig;
use registry::gourl::GoUrl;
use registry::mutate::{self, StaticImage, StaticLayer};
use registry::name::{self, NameOption, Reference};
use registry::transport::{self, RequestInfo};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn load(name: &str) -> Value {
    let data = std::fs::read(fixture(name)).expect("fixture");
    serde_json::from_slice(&data).expect("json")
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn opts(name: &str) -> Vec<NameOption> {
    match name {
        "default" => vec![],
        "strict" => vec![NameOption::StrictValidation],
        "insecure" => vec![NameOption::Insecure],
        "defaultRegistry" => vec![NameOption::WithDefaultRegistry("registry.example.com".into())],
        "defaultTag" => vec![NameOption::WithDefaultTag("v1".into())],
        "weak" => vec![NameOption::WeakValidation],
        other => panic!("unknown opts {other}"),
    }
}

fn ref_result(r: &Reference) -> Value {
    serde_json::json!({
        "ok": true,
        "kind": match r { Reference::Tag(_) => "tag", Reference::Digest(_) => "digest" },
        "string": r.to_string(),
        "name": r.name(),
        "registryStr": r.context().registry_str(),
        "repositoryStr": r.context().repository_str(),
        "identifier": r.identifier(),
        "context": r.context().to_string(),
        "scheme": r.context().scheme(),
        "scope": r.scope("pull"),
    })
}

fn err_result(msg: String, bad_name: bool) -> Value {
    let mut v = serde_json::json!({"ok": false, "err": msg});
    if bad_name {
        v["isBadName"] = Value::Bool(true);
    }
    v
}

/// Drop empty-string fields, like Go's `omitempty`.
fn normalize(v: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Value::Object(o) = v {
        for (k, val) in o {
            match val {
                Value::String(s) if s.is_empty() => {}
                Value::Bool(false) if k != "ok" => {}
                other => {
                    out.insert(k.clone(), other.clone());
                }
            }
        }
    }
    Value::Object(out)
}

#[test]
fn name_parsing_matches_go() {
    let cases = load("name_golden.json");
    let mut failures = Vec::new();
    let cases = cases.as_array().expect("array");
    for c in cases {
        let func = s(c, "func");
        let o = opts(s(c, "opts"));
        let input = s(c, "input");
        let got = match func {
            "ParseReference" => match name::parse_reference(input, &o) {
                Ok(r) => ref_result(&r),
                Err(e) => err_result(e.to_string(), true),
            },
            "NewTag" => match name::new_tag(input, &o) {
                Ok(t) => ref_result(&t.into()),
                Err(e) => err_result(e.to_string(), true),
            },
            "NewDigest" => match name::new_digest(input, &o) {
                Ok(d) => ref_result(&d.into()),
                Err(e) => err_result(e.to_string(), e.is_bad_name()),
            },
            "NewRepository" => match name::new_repository(input, &o) {
                Ok(r) => serde_json::json!({
                    "ok": true,
                    "string": r.to_string(),
                    "name": r.name(),
                    "registryStr": r.registry_str(),
                    "repositoryStr": r.repository_str(),
                    "scheme": r.scheme(),
                    "scope": r.scope("pull"),
                }),
                Err(e) => err_result(e.to_string(), true),
            },
            "NewRegistry" => match name::new_registry(input, &o) {
                Ok(r) => serde_json::json!({
                    "ok": true,
                    "string": r.to_string(),
                    "name": r.name(),
                    "registryStr": r.registry_str(),
                    "scheme": r.scheme(),
                    "scope": r.scope("pull"),
                }),
                Err(e) => err_result(e.to_string(), true),
            },
            "Repository.Tag" => {
                let repo = name::new_repository("quay.io/foo/bar", &[]).unwrap();
                ref_result(&repo.tag("v1").into())
            }
            "Repository.Digest" => {
                let repo = name::new_repository("quay.io/foo/bar", &[]).unwrap();
                ref_result(&repo.digest("sha256:abc").into())
            }
            "Registry.Repo" => {
                let reg = name::new_registry("docker.io", &[]).unwrap();
                let parts: Vec<&str> = input.split('|').nth(1).unwrap().split(',').collect();
                let r = reg.repo(&parts);
                serde_json::json!({"ok": true, "string": r.to_string(), "repositoryStr": r.repository_str()})
            }
            other => panic!("unknown func {other}"),
        };
        let want = normalize(&c["result"]);
        let got = normalize(&got);
        if want != got {
            failures.push(format!("{func}[{}]({input:?}):\n  want {want}\n  got  {got}", s(c, "opts")));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

#[test]
fn auth_config_json_matches_go() {
    let cases = load("authn_golden.json");
    let mut failures = Vec::new();
    for c in cases.as_array().unwrap() {
        let input = s(c, "input");
        let res = AuthConfig::from_json(input.as_bytes());
        let ok = c["ok"].as_bool().unwrap();
        match (&res, ok) {
            (Ok(cfg), true) => {
                let got = (
                    cfg.username.as_str(),
                    cfg.password.as_str(),
                    cfg.auth.as_str(),
                    cfg.identity_token.as_str(),
                    cfg.registry_token.as_str(),
                );
                let want = (
                    s(c, "username"),
                    s(c, "password"),
                    s(c, "auth"),
                    s(c, "identitytoken"),
                    s(c, "registrytoken"),
                );
                if got != want {
                    failures.push(format!("{input}: want {want:?} got {got:?}"));
                }
                if cfg.to_json() != s(c, "marshaled") {
                    failures.push(format!("{input}: marshal want {} got {}", s(c, "marshaled"), cfg.to_json()));
                }
                let via_serde = serde_json::to_string(cfg).unwrap();
                let want_marshal: Value = serde_json::from_str(s(c, "marshaled")).unwrap();
                let got_marshal: Value = serde_json::from_str(&via_serde).unwrap();
                if want_marshal != got_marshal {
                    failures.push(format!("{input}: serde marshal want {want_marshal} got {got_marshal}"));
                }
            }
            (Err(e), false) => {
                if e.to_string() != s(c, "err") {
                    failures.push(format!("{input}: err want {:?} got {:?}", s(c, "err"), e.to_string()));
                }
            }
            (r, _) => failures.push(format!("{input}: want ok={ok}, got {r:?}")),
        }
        // The serde Deserialize impl agrees on success/failure.
        let serde_res: Result<AuthConfig, _> = serde_json::from_str(input);
        if serde_res.is_ok() != ok {
            failures.push(format!("{input}: serde Deserialize ok={} want {ok}", serde_res.is_ok()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn www_authenticate_parsing_matches_go() {
    let cases = load("challenge_golden.json");
    let mut failures = Vec::new();
    for c in cases.as_array().unwrap() {
        let headers: Vec<String> = c["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h.as_str().unwrap().to_string())
            .collect();
        let got = transport::parse_www_authenticate(headers.iter().map(String::as_str));
        let got: Vec<Value> = got
            .iter()
            .map(|ch| serde_json::json!({"Scheme": ch.scheme, "Parameters": ch.parameters}))
            .collect();
        let want = c["challenges"].as_array().cloned().unwrap_or_default();
        if Value::Array(got.clone()) != Value::Array(want.clone()) {
            failures.push(format!("{headers:?}: want {want:?} got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn transport_error_strings_match_go() {
    let cases = load("transport_error_golden.json");
    let mut failures = Vec::new();
    for c in cases.as_array().unwrap() {
        let status = c["status"].as_u64().unwrap() as u16;
        if status == 200 {
            continue;
        }
        let err = transport::Error::from_response_parts(
            status,
            Some(RequestInfo {
                method: s(c, "method").to_string(),
                url: GoUrl::parse(s(c, "url")).unwrap(),
            }),
            s(c, "body").as_bytes(),
        );
        if err.to_string() != s(c, "error") {
            failures.push(format!("{} {} {status}:\n  want {:?}\n  got  {:?}", s(c, "method"), s(c, "url"), s(c, "error"), err.to_string()));
        }
        if err.temporary() != c["temporary"].as_bool().unwrap() {
            failures.push(format!("{} {status}: temporary want {}", s(c, "url"), c["temporary"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutate_extract_matches_go() {
    let cases = load("extract_golden.json");
    let mut failures = Vec::new();
    for c in cases.as_array().unwrap() {
        let case = s(c, "name");
        let layers: Vec<StaticLayer> = c["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let data = std::fs::read(fixture(&format!("extract/{}", f.as_str().unwrap()))).unwrap();
                StaticLayer(data.into())
            })
            .collect();
        let img = StaticImage(layers);
        let mut ex = mutate::extract(&img);
        let mut got = Vec::new();
        let mut err = String::new();
        loop {
            match ex.next_entry().await {
                Ok(None) => break,
                Err(e) => {
                    err = format!("next: {e}");
                    break;
                }
                Ok(Some(h)) => {
                    let data = match ex.read_to_end().await {
                        Ok(d) => d,
                        Err(e) => {
                            err = format!("read: {e}");
                            break;
                        }
                    };
                    let mut e = serde_json::json!({
                        "name": h.name,
                        "typeflag": h.typeflag,
                        "mode": h.mode,
                        "size": h.size,
                        "uid": h.uid,
                        "gid": h.gid,
                        "mtime": h.mtime,
                    });
                    if !h.uname.is_empty() {
                        e["uname"] = Value::String(h.uname.clone());
                    }
                    if !h.gname.is_empty() {
                        e["gname"] = Value::String(h.gname.clone());
                    }
                    if !h.linkname.is_empty() {
                        e["linkname"] = Value::String(h.linkname.clone());
                    }
                    if !data.is_empty() {
                        e["sha256"] = Value::String(hex::encode(Sha256::digest(&data)));
                    }
                    got.push(e);
                }
            }
        }
        let want = c["entries"].as_array().cloned().unwrap_or_default();
        if got != want {
            failures.push(format!("{case}: entries differ\n  want {}\n  got  {}", Value::Array(want), Value::Array(got.clone())));
        }
        if err != s(c, "err") {
            failures.push(format!("{case}: err want {:?} got {err:?}", s(c, "err")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
