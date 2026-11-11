//! `remote` / `transport` / `mutate.Extract` against the in-process fake registry.

mod support;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::body::Body;
use axum::http::Response;
use registry::authn::{self, AuthConfig, Authenticator, Keychain, Resource};
use registry::name::{self, Reference};
use registry::remote::{self, RemoteOption};
use registry::transport::Backoff;
use registry::v1::MediaType;
use registry::{mutate, transport};
use support::{AuthMode, FakeRegistry, config_json, gz_layer, sha256};

const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
const DOCKER_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";
const OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";
const GO_ACCEPT: &str = "application/vnd.docker.distribution.manifest.v1+json,application/vnd.docker.distribution.manifest.v1+prettyjws,application/vnd.docker.distribution.manifest.v2+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.list.v2+json,application/vnd.oci.image.index.v1+json";

/// Same resolution rules as the operator's dockerConfigKeychain.
struct MapKeychain(HashMap<String, AuthConfig>);

impl Keychain for MapKeychain {
    fn resolve(&self, target: &dyn Resource) -> anyhow::Result<Arc<dyn Authenticator>> {
        if let Some(c) = self.0.get(&target.string()) {
            return Ok(authn::from_config(c.clone()));
        }
        if let Some(c) = self.0.get(&target.registry_str()) {
            return Ok(authn::from_config(c.clone()));
        }
        Ok(authn::anonymous())
    }
}

fn keychain(host: &str, user: &str, pass: &str) -> Arc<dyn Keychain> {
    let json = format!(r#"{{"username":"{user}","password":"{pass}"}}"#);
    let mut m = HashMap::new();
    m.insert(
        host.to_string(),
        AuthConfig::from_json(json.as_bytes()).unwrap(),
    );
    Arc::new(MapKeychain(m))
}

fn r(s: &str) -> Reference {
    name::parse_reference(s, &[]).unwrap()
}

fn fast() -> RemoteOption {
    remote::with_retry_backoff(Backoff::fast())
}

fn bearer(creds: Option<(&str, &str)>) -> AuthMode {
    AuthMode::Bearer {
        service: "fake-registry".into(),
        creds: creds.map(|(u, p)| (u.to_string(), p.to_string())),
        omit_realm: false,
        token_body: None,
        access_token: false,
    }
}

fn simple_image(reg: &FakeRegistry, repo: &str, tag: &str, mt: &str) -> String {
    let mut labels = BTreeMap::new();
    labels.insert("io.openshift.release", "4.19.0");
    reg.push_image(
        repo,
        Some(tag),
        mt,
        &config_json(&labels),
        &[gz_layer(&[("etc/", b'5', ""), ("etc/hello", b'0', "hi")])],
    )
}

#[tokio::test]
async fn head_returns_descriptor_anonymously() {
    let reg = FakeRegistry::start(AuthMode::None).await;
    let digest = simple_image(&reg, "foo/bar", "v1", OCI_MANIFEST);
    let host = reg.host();
    let d = remote::head(&r(&format!("{host}/foo/bar:v1")), &[])
        .await
        .unwrap();
    assert_eq!(d.media_type, MediaType::new(OCI_MANIFEST));
    assert_eq!(d.digest.to_string(), digest);
    assert!(d.size > 0);
    assert!(!d.media_type.is_index());

    let log = reg.log();
    // Ping first (no ggcr user agent yet), then the HEAD.
    assert_eq!(log[0].path, "/v2/");
    assert_eq!(log[0].user_agent, "Go-http-client/1.1");
    let head = log.iter().find(|l| l.method == "HEAD").unwrap();
    assert_eq!(head.path, "/v2/foo/bar/manifests/v1");
    assert_eq!(head.accept, GO_ACCEPT);
    assert_eq!(head.user_agent, "go-containerregistry/v0.21.2");
    assert_eq!(head.authorization, "");
}

#[tokio::test]
async fn head_index_is_index() {
    let reg = FakeRegistry::start(AuthMode::None).await;
    let child = simple_image(&reg, "foo/bar", "child", OCI_MANIFEST);
    let idx = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_INDEX,
        "manifests": [{"mediaType": OCI_MANIFEST, "size": 10, "digest": child,
                        "platform": {"architecture": "amd64", "os": "linux"}}],
    });
    reg.add_manifest(
        "foo/bar",
        Some("multi"),
        OCI_INDEX,
        &serde_json::to_vec(&idx).unwrap(),
    );
    let d = remote::head(&r(&format!("{}/foo/bar:multi", reg.host())), &[])
        .await
        .unwrap();
    assert!(d.media_type.is_index());
}

#[tokio::test]
async fn token_auth_with_keychain_credentials() {
    let reg = FakeRegistry::start(bearer(Some(("user", "secret")))).await;
    simple_image(&reg, "foo/bar", "v1", OCI_MANIFEST);
    let host = reg.host();
    let kc = keychain(&host, "user", "secret");
    remote::head(
        &r(&format!("{host}/foo/bar:v1")),
        &[remote::with_auth_from_keychain(kc)],
    )
    .await
    .unwrap();
    let tok = reg.requests_to("/token");
    assert_eq!(tok.len(), 1);
    assert_eq!(
        tok[0].query,
        "scope=repository%3Afoo%2Fbar%3Apull&service=fake-registry"
    );
    assert_eq!(tok[0].authorization, "Basic dXNlcjpzZWNyZXQ=");
    assert_eq!(tok[0].user_agent, "go-containerregistry/v0.21.2");
    let head = reg.log().into_iter().find(|l| l.method == "HEAD").unwrap();
    assert_eq!(head.authorization, "Bearer tok-1");
}

#[tokio::test]
async fn repository_scoped_keychain_entry_is_used() {
    let reg = FakeRegistry::start(bearer(Some(("repo", "repo-pass")))).await;
    simple_image(&reg, "foo/bar", "v1", OCI_MANIFEST);
    let host = reg.host();
    let mut m = HashMap::new();
    m.insert(
        host.clone(),
        AuthConfig::from_json(br#"{"username":"generic","password":"x"}"#).unwrap(),
    );
    m.insert(
        format!("{host}/foo/bar"),
        AuthConfig::from_json(br#"{"auth":"cmVwbzpyZXBvLXBhc3M="}"#).unwrap(),
    );
    let kc: Arc<dyn Keychain> = Arc::new(MapKeychain(m));
    remote::head(
        &r(&format!("{host}/foo/bar:v1")),
        &[remote::with_auth_from_keychain(kc)],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn anonymous_token_flow_and_access_token_field() {
    let reg = FakeRegistry::start(AuthMode::Bearer {
        service: "svc".into(),
        creds: None,
        omit_realm: false,
        token_body: None,
        access_token: true,
    })
    .await;
    simple_image(&reg, "foo/bar", "v1", DOCKER_MANIFEST);
    let host = reg.host();
    let kc: Arc<dyn Keychain> = Arc::new(MapKeychain(HashMap::new()));
    remote::head(
        &r(&format!("{host}/foo/bar:v1")),
        &[remote::with_auth_from_keychain(kc)],
    )
    .await
    .unwrap();
    let tok = reg.requests_to("/token");
    assert_eq!(tok[0].authorization, "");
}

#[tokio::test]
async fn token_endpoint_rejects_credentials() {
    let reg = FakeRegistry::start(bearer(Some(("user", "secret")))).await;
    let host = reg.host();
    let kc = keychain(&host, "user", "wrong");
    let err = remote::head(
        &r(&format!("{host}/foo/bar:v1")),
        &[remote::with_auth_from_keychain(kc)],
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "GET http://{host}/token?scope=repository%3Afoo%2Fbar%3Apull&service=fake-registry: UNAUTHORIZED: access to the requested resource is not authorized; [map[Action:pull Type:repository]]"
        )
    );
    assert_eq!(registry::status_code(&err), Some(401));
    assert!(registry::has_error_code(
        &err,
        transport::codes::UNAUTHORIZED
    ));
    assert!(registry::is_auth_error(&err));
    assert!(err.to_string().contains("UNAUTHORIZED"));
}

#[tokio::test]
async fn missing_realm_and_missing_token() {
    let reg = FakeRegistry::start(AuthMode::Bearer {
        service: "svc".into(),
        creds: None,
        omit_realm: true,
        token_body: None,
        access_token: false,
    })
    .await;
    let host = reg.host();
    let err = remote::head(&r(&format!("{host}/foo/bar:v1")), &[])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "malformed www-authenticate, missing realm: map[service:svc]"
    );

    let reg = FakeRegistry::start(AuthMode::Bearer {
        service: "svc".into(),
        creds: None,
        omit_realm: false,
        token_body: Some(r#"{"expires_in":300}"#.into()),
        access_token: false,
    })
    .await;
    let host = reg.host();
    let err = remote::head(&r(&format!("{host}/foo/bar:v1")), &[])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "no token in bearer response:\n{\"expires_in\":300}"
    );

    let reg = FakeRegistry::start(AuthMode::Bearer {
        service: "svc".into(),
        creds: None,
        omit_realm: false,
        token_body: Some("<html>".into()),
        access_token: false,
    })
    .await;
    let host = reg.host();
    let err = remote::head(&r(&format!("{host}/foo/bar:v1")), &[])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid character '<' looking for beginning of value"
    );
}

#[tokio::test]
async fn basic_auth_registry() {
    let reg = FakeRegistry::start(AuthMode::Basic {
        user: "u".into(),
        pass: "p".into(),
    })
    .await;
    simple_image(&reg, "foo/bar", "v1", OCI_MANIFEST);
    let host = reg.host();
    let img = remote::image(
        &r(&format!("{host}/foo/bar:v1")),
        &[remote::with_auth_from_keychain(keychain(&host, "u", "p"))],
    )
    .await
    .unwrap();
    let cfg = img.config_file().await.unwrap();
    assert_eq!(cfg.config.labels["io.openshift.release"], "4.19.0");
    for l in reg.log().iter().skip(1) {
        assert_eq!(l.authorization, "Basic dTpw", "{}", l.path);
    }

    // Anonymous against a basic-auth registry: the 401 surfaces as a structured error.
    let err = remote::head(&r(&format!("{host}/foo/bar:v1")), &[])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "HEAD http://{host}/v2/foo/bar/manifests/v1: unexpected status code 401 Unauthorized (HEAD responses have no body, use GET for details)"
        )
    );
    assert!(registry::is_auth_error(&err));
    let err = remote::get(&r(&format!("{host}/foo/bar:v1")), &[])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("GET http://{host}/v2/foo/bar/manifests/v1: UNAUTHORIZED: authentication required")
    );
}

#[tokio::test]
async fn not_found_errors_match_go() {
    let reg = FakeRegistry::start(AuthMode::None).await;
    let host = reg.host();
    let err = remote::head(&r(&format!("{host}/foo/bar:nope")), &[])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "HEAD http://{host}/v2/foo/bar/manifests/nope: unexpected status code 404 Not Found (HEAD responses have no body, use GET for details)"
        )
    );
    assert_eq!(registry::status_code(&err), Some(404));
    assert!(registry::is_not_found_error(&err));

    let err = remote::image(&r(&format!("{host}/foo/bar:nope")), &[])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "GET http://{host}/v2/foo/bar/manifests/nope: MANIFEST_UNKNOWN: manifest unknown; map[Tag:nope]"
        )
    );
    assert!(registry::has_error_code(
        &err,
        transport::codes::MANIFEST_UNKNOWN
    ));
    let te = registry::as_transport_error(&err).unwrap();
    assert_eq!(te.errors[0].code, "MANIFEST_UNKNOWN");
    assert_eq!(te.errors[0].message, "manifest unknown");
}

#[tokio::test]
async fn image_from_index_picks_linux_amd64() {
    let reg = FakeRegistry::start(bearer(None)).await;
    let mut arm = BTreeMap::new();
    arm.insert("arch", "arm64");
    let arm_digest = reg.push_image("foo/bar", None, DOCKER_MANIFEST, &config_json(&arm), &[]);
    let mut amd = BTreeMap::new();
    amd.insert("arch", "amd64");
    let amd_digest = reg.push_image("foo/bar", None, DOCKER_MANIFEST, &config_json(&amd), &[]);
    let idx = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.list.v2+json",
        "manifests": [
            {"mediaType": DOCKER_MANIFEST, "size": 1, "digest": arm_digest, "platform": {"architecture": "arm64", "os": "linux"}},
            {"mediaType": DOCKER_MANIFEST, "size": 1, "digest": amd_digest, "platform": {"architecture": "amd64", "os": "linux"}},
        ],
    });
    let host = reg.host();
    let idx_digest = reg.add_manifest(
        "foo/bar",
        Some("multi"),
        "application/vnd.docker.distribution.manifest.list.v2+json",
        &serde_json::to_vec(&idx).unwrap(),
    );
    let desc = remote::get(&r(&format!("{host}/foo/bar:multi")), &[])
        .await
        .unwrap();
    assert!(desc.media_type().is_index());
    assert_eq!(desc.descriptor.digest.to_string(), idx_digest);
    let img = desc.image().await.unwrap();
    assert_eq!(img.digest().to_string(), amd_digest);
    assert_eq!(
        img.config_file().await.unwrap().config.labels["arch"],
        "amd64"
    );
    // The child manifest is fetched by digest with its own media type as Accept.
    let child = reg
        .log()
        .into_iter()
        .find(|l| l.path == format!("/v2/foo/bar/manifests/{amd_digest}"))
        .unwrap();
    assert_eq!(child.accept, DOCKER_MANIFEST);

    // No matching platform.
    let idx = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_INDEX,
        "manifests": [{"mediaType": DOCKER_MANIFEST, "size": 1, "digest": arm_digest, "platform": {"architecture": "arm64", "os": "linux"}}],
    });
    reg.add_manifest(
        "foo/bar",
        Some("armonly"),
        OCI_INDEX,
        &serde_json::to_vec(&idx).unwrap(),
    );
    let err = remote::image(&r(&format!("{host}/foo/bar:armonly")), &[])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("no child with platform linux/amd64 in index {host}/foo/bar:armonly")
    );
}

#[tokio::test]
async fn digest_references() {
    let reg = FakeRegistry::start(AuthMode::None).await;
    let digest = simple_image(&reg, "foo/bar", "v1", OCI_MANIFEST);
    let host = reg.host();
    let img = remote::image(&r(&format!("{host}/foo/bar@{digest}")), &[])
        .await
        .unwrap();
    assert_eq!(img.digest().to_string(), digest);

    // A registry that serves different content for a digest.
    let other = format!("sha256:{}", "a".repeat(64));
    let (mt, body) = reg.lock().manifests[&("foo/bar".to_string(), digest.clone())].clone();
    reg.lock()
        .manifests
        .insert(("foo/bar".to_string(), other.clone()), (mt, body));
    let reference = format!("{host}/foo/bar@{other}");
    let err = remote::get(&r(&reference), &[]).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "manifest digest: \"{digest}\" does not match requested digest: \"{other}\" for \"{reference}\""
        )
    );
    let err = remote::head(&r(&reference), &[]).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "manifest digest: \"{digest}\" does not match requested digest: \"{other}\" for \"{reference}\""
        )
    );
}

#[tokio::test]
async fn config_blob_is_verified() {
    let reg = FakeRegistry::start(AuthMode::None).await;
    let mut labels = BTreeMap::new();
    labels.insert("a", "b");
    let cfg = config_json(&labels);
    let cfg_digest = sha256(&cfg);
    simple_image(&reg, "foo/bar", "v1", OCI_MANIFEST);
    // Replace the config of a new manifest with tampered bytes of the same size.
    let m = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_MANIFEST,
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "size": cfg.len(), "digest": cfg_digest},
        "layers": [],
    });
    reg.add_manifest(
        "foo/bar",
        Some("bad"),
        OCI_MANIFEST,
        &serde_json::to_vec(&m).unwrap(),
    );
    let mut tampered = cfg.clone();
    let last = tampered.len() - 2;
    tampered[last] = b' ';
    reg.lock()
        .blobs
        .insert(cfg_digest.clone(), tampered.clone().into());
    let host = reg.host();
    let img = remote::image(&r(&format!("{host}/foo/bar:bad")), &[])
        .await
        .unwrap();
    let err = img.config_file().await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "error verifying sha256 checksum after reading {} bytes; got \"{}\", want \"{}\"",
            cfg.len(),
            sha256(&tampered),
            cfg_digest
        )
    );

    // Content-Length mismatch with the descriptor size.
    let m = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_MANIFEST,
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "size": cfg.len() + 5, "digest": cfg_digest},
        "layers": [],
    });
    reg.add_manifest(
        "foo/bar",
        Some("badsize"),
        OCI_MANIFEST,
        &serde_json::to_vec(&m).unwrap(),
    );
    let img = remote::image(&r(&format!("{host}/foo/bar:badsize")), &[])
        .await
        .unwrap();
    let err = img.config_file().await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "GET http://{host}/v2/foo/bar/blobs/{cfg_digest}: Content-Length header {} does not match expected size {}",
            cfg.len(),
            cfg.len() + 5
        )
    );
}

#[tokio::test]
async fn extract_release_image_references() {
    let reg = FakeRegistry::start(bearer(Some(("u", "p")))).await;
    let base = gz_layer(&[
        ("release-manifests/", b'5', ""),
        (
            "release-manifests/image-references",
            b'0',
            r#"{"spec":{"tags":[{"name":"pod","from":{"name":"old"}}]}}"#,
        ),
        ("release-manifests/other", b'0', "x"),
        ("usr/", b'5', ""),
        ("usr/bin/", b'5', ""),
        ("usr/bin/tool", b'0', "tool"),
    ]);
    let top = gz_layer(&[
        ("release-manifests/", b'5', ""),
        (
            "release-manifests/image-references",
            b'0',
            r#"{"spec":{"tags":[{"name":"pod","from":{"name":"quay.io/x@sha256:new"}}]}}"#,
        ),
        ("usr/.wh.bin", b'0', ""),
    ]);
    let cfg = config_json(&BTreeMap::new());
    reg.push_image(
        "ocp/release",
        Some("4.19.0-aarch64"),
        OCI_MANIFEST,
        &cfg,
        &[base, top],
    );
    let host = reg.host();
    let img = remote::image(
        &r(&format!("{host}/ocp/release:4.19.0-aarch64")),
        &[remote::with_auth_from_keychain(keychain(&host, "u", "p"))],
    )
    .await
    .unwrap();
    let mut ex = mutate::extract(&img);
    let mut names = Vec::new();
    let mut refs = None;
    while let Some(h) = ex.next_entry().await.unwrap() {
        names.push(h.name.clone());
        if h.name == "release-manifests/image-references" {
            refs = Some(String::from_utf8(ex.read_to_end().await.unwrap()).unwrap());
        }
    }
    assert_eq!(
        refs.unwrap(),
        r#"{"spec":{"tags":[{"name":"pod","from":{"name":"quay.io/x@sha256:new"}}]}}"#
    );
    assert_eq!(
        names,
        vec![
            "release-manifests",
            "release-manifests/image-references",
            "release-manifests/other",
            "usr"
        ]
    );
    // Blob fetches carried the bearer token.
    for l in reg.requests_to("/v2/ocp/release/blobs/") {
        assert_eq!(l.authorization, "Bearer tok-1");
    }
}

#[tokio::test]
async fn extract_stops_cleanly_when_a_layer_is_missing() {
    // Go's Extract swallows errors at entry boundaries (the tar trailer is still written).
    let reg = FakeRegistry::start(AuthMode::None).await;
    let present = gz_layer(&[("a", b'0', "a")]);
    let missing = gz_layer(&[("b", b'0', "b")]);
    let cfg = config_json(&BTreeMap::new());
    reg.push_image(
        "foo/bar",
        Some("v1"),
        OCI_MANIFEST,
        &cfg,
        &[present, missing.clone()],
    );
    reg.lock().blobs.remove(&sha256(&missing));
    let host = reg.host();
    let img = remote::image(&r(&format!("{host}/foo/bar:v1")), &[])
        .await
        .unwrap();
    let mut ex = mutate::extract(&img);
    assert!(ex.next_entry().await.unwrap().is_none());

    // Dropping the reader early stops the worker.
    let mut ex = mutate::extract(&img);
    drop(ex.next_entry());
}

#[tokio::test]
async fn reauth_on_challenge_adds_scope() {
    let reg = FakeRegistry::start(bearer(None)).await;
    simple_image(&reg, "foo/bar", "v1", OCI_MANIFEST);
    reg.lock().reject_once.push("tok-1".into());
    let host = reg.host();
    remote::head(&r(&format!("{host}/foo/bar:v1")), &[])
        .await
        .unwrap();
    let tok = reg.requests_to("/token");
    assert_eq!(tok.len(), 2);
    assert_eq!(
        tok[1].query,
        "scope=repository%3Aextra%2Fscope%3Apull&scope=repository%3Afoo%2Fbar%3Apull&service=fake-registry"
    );
    let heads: Vec<_> = reg
        .log()
        .into_iter()
        .filter(|l| l.method == "HEAD")
        .collect();
    assert_eq!(heads.len(), 2);
    assert_eq!(heads[1].authorization, "Bearer tok-2");
}

#[tokio::test]
async fn retries_temporary_status_codes() {
    let reg = FakeRegistry::start(AuthMode::None).await;
    simple_image(&reg, "foo/bar", "v1", OCI_MANIFEST);
    let host = reg.host();
    // HEAD always 503: 3 attempts, body drained, Go's HEAD message.
    reg.hook(|l, _| {
        (l.method == "HEAD" && l.path.ends_with("/always503")).then(|| {
            Response::builder()
                .status(503)
                .body(Body::from("busy"))
                .unwrap()
        })
    });
    let err = remote::head(&r(&format!("{host}/foo/bar:always503")), &[fast()])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "HEAD http://{host}/v2/foo/bar/manifests/always503: unexpected status code 503 Service Unavailable (HEAD responses have no body, use GET for details)"
        )
    );
    assert!(registry::is_temporary(&err));
    assert_eq!(reg.requests_to("/v2/foo/bar/manifests/always503").len(), 3);

    // GET 500 with a JSON body: the retry transport drains the body, so the final error
    // has no diagnostics (exactly like Go).
    reg.hook(|l, _| {
        (l.method == "GET" && l.path.ends_with("/always500")).then(|| {
            Response::builder()
                .status(500)
                .body(Body::from(
                    r#"{"errors":[{"code":"UNKNOWN","message":"boom"}]}"#,
                ))
                .unwrap()
        })
    });
    let err = remote::get(&r(&format!("{host}/foo/bar:always500")), &[fast()])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "GET http://{host}/v2/foo/bar/manifests/always500: unexpected status code 500 Internal Server Error"
        )
    );

    // Transient failures then success.
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = counter.clone();
    reg.hook(move |l, _| {
        if l.method == "GET" && l.path == "/v2/foo/bar/manifests/v1" {
            let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < 2 {
                return Some(Response::builder().status(502).body(Body::empty()).unwrap());
            }
        }
        None
    });
    remote::get(&r(&format!("{host}/foo/bar:v1")), &[fast()])
        .await
        .unwrap();
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 3);

    // 429 is not in the retry list.
    reg.hook(|l, _| {
        l.path.ends_with("/limited").then(|| {
            Response::builder()
                .status(429)
                .body(Body::from(
                    r#"{"errors":[{"code":"TOOMANYREQUESTS","message":"slow down"}]}"#,
                ))
                .unwrap()
        })
    });
    let err = remote::get(&r(&format!("{host}/foo/bar:limited")), &[fast()])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("GET http://{host}/v2/foo/bar/manifests/limited: TOOMANYREQUESTS: slow down")
    );
    assert_eq!(reg.requests_to("/v2/foo/bar/manifests/limited").len(), 1);
}

#[tokio::test]
async fn ping_errors() {
    let reg = FakeRegistry::start(AuthMode::None).await;
    reg.hook(|l, _| {
        (l.path == "/v2/").then(|| {
            Response::builder()
                .status(404)
                .body(Body::from("no registry here"))
                .unwrap()
        })
    });
    let host = reg.host();
    let err = remote::head(&r(&format!("{host}/foo/bar:v1")), &[])
        .await
        .unwrap_err();
    // The https attempt fails (plain HTTP server), then the http fallback gets a 404.
    assert_eq!(
        err.to_string(),
        format!(
            "Get \"https://{host}/v2/\": http: server gave HTTP response to HTTPS client; GET http://{host}/v2/: unexpected status code 404 Not Found: no registry here"
        )
    );
    assert_eq!(registry::status_code(&err), Some(404));

    // Nothing listening.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = l.local_addr().unwrap().to_string();
    drop(l);
    let err = remote::head(&r(&format!("{dead}/foo/bar:v1")), &[fast()])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "Get \"https://{dead}/v2/\": dial tcp {dead}: connect: connection refused; Get \"http://{dead}/v2/\": dial tcp {dead}: connect: connection refused"
        )
    );
}

#[tokio::test]
async fn blob_redirects_drop_auth_for_other_hosts() {
    let reg = FakeRegistry::start(bearer(None)).await;
    let cdn = FakeRegistry::start(AuthMode::None).await;
    let layer = gz_layer(&[("f", b'0', "content")]);
    let cfg = config_json(&BTreeMap::new());
    reg.push_image(
        "foo/bar",
        Some("v1"),
        OCI_MANIFEST,
        &cfg,
        std::slice::from_ref(&layer),
    );
    let layer_digest = sha256(&layer);
    cdn.lock()
        .blobs
        .insert(layer_digest.clone(), layer.clone().into());
    let cdn_host = cdn.host();
    let d2 = layer_digest.clone();
    reg.hook(move |l, _| {
        (l.method == "GET" && l.path.ends_with(&format!("/blobs/{d2}"))).then(|| {
            Response::builder()
                .status(307)
                .header(
                    "Location",
                    format!("http://{cdn_host}/v2/cdn/blobs/{d2}?sig=secret"),
                )
                .body(Body::empty())
                .unwrap()
        })
    });
    let host = reg.host();
    let img = remote::image(&r(&format!("{host}/foo/bar:v1")), &[])
        .await
        .unwrap();
    let mut ex = mutate::extract(&img);
    let h = ex.next_entry().await.unwrap().unwrap();
    assert_eq!(h.name, "f");
    assert_eq!(ex.read_to_end().await.unwrap(), b"content");
    let cdn_req = cdn.requests_to("/v2/cdn/blobs/");
    assert_eq!(cdn_req.len(), 1);
    assert_eq!(cdn_req[0].authorization, "");
    assert_eq!(cdn_req[0].query, "sig=secret");
}

#[tokio::test]
async fn auth_options_are_exclusive() {
    let err = remote::head(
        &r("quay.io/foo/bar:v1"),
        &[
            remote::with_auth(authn::anonymous()),
            remote::with_auth_from_keychain(Arc::new(MapKeychain(HashMap::new()))),
        ],
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "provide an option for either authn.Authenticator or authn.Keychain, not both"
    );
}

// ------------------------------------------------------------------ keychain_test.go

fn docker_config_keychain(json: &str) -> MapKeychain {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let auths: HashMap<String, AuthConfig> = serde_json::from_value(v["auths"].clone()).unwrap();
    MapKeychain(auths)
}

fn parse_auth_target(r: &str) -> Box<dyn Resource> {
    if !r.contains('/') {
        return Box::new(name::new_registry(r, &[]).unwrap());
    }
    Box::new(name::new_repository(r, &[]).unwrap())
}

#[test]
fn test_keychain_prefers_repository_scoped_auth() {
    let kc = docker_config_keychain(
        r#"{
        "auths": {
            "quay.io": {"username": "generic", "password": "generic-pass"},
            "quay.io/edge-infrastructure/bluefield-ocp": {"username": "repo", "password": "repo-pass"}
        }
    }"#,
    );
    for (name, target, want) in [
        (
            "repository-scoped entry wins for that repository",
            "quay.io/edge-infrastructure/bluefield-ocp",
            "repo",
        ),
        (
            "registry entry is used for the registry itself",
            "quay.io",
            "generic",
        ),
    ] {
        let t = parse_auth_target(target);
        let auth = kc.resolve(t.as_ref()).unwrap();
        let cfg = auth.authorization().unwrap();
        assert_eq!(cfg.username, want, "{name}");
    }
}

#[test]
fn test_keychain_falls_back_to_registry_auth() {
    let kc = docker_config_keychain(
        r#"{"auths": {"quay.io": {"username": "generic", "password": "generic-pass"}}}"#,
    );
    let repo = name::new_repository("quay.io/openshift-release-dev/ocp-release", &[]).unwrap();
    let cfg = kc.resolve(&repo).unwrap().authorization().unwrap();
    assert_eq!(cfg.username, "generic");
}

#[test]
fn test_keychain_anonymous_when_no_match() {
    let kc = docker_config_keychain(
        r#"{"auths": {"registry.redhat.io": {"username": "rh", "password": "rh-pass"}}}"#,
    );
    let repo = name::new_repository("quay.io/edge-infrastructure/bluefield-ocp", &[]).unwrap();
    let auth = kc.resolve(&repo).unwrap();
    let cfg = auth.authorization().unwrap();
    assert!(cfg.username.is_empty() && cfg.password.is_empty() && cfg.auth.is_empty());
    assert!(auth.is_anonymous());
}
