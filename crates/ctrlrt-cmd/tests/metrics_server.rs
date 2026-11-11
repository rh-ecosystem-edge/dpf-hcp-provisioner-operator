//! Metrics server + authn/authz filter tests (behaviour of controller-runtime's
//! metricsserver and filters.WithAuthenticationAndAuthorization).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctrlrt::client::{ClientBackend, DeleteOptions, ListOptions, Patch};
use ctrlrt::error::ApiError;
use ctrlrt::object::ResourceInfo;
use ctrlrt_cmd::metrics::{MetricsServer, MetricsServerOptions, TlsOpt, disable_http2_opt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct FakeApi {
    token_reviews: Mutex<Vec<Value>>,
    sars: Mutex<Vec<Value>>,
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
        match res.kind.as_str() {
            "TokenReview" => {
                self.token_reviews.lock().unwrap().push(obj.clone());
                let token = obj["spec"]["token"].as_str().unwrap_or("").to_string();
                obj["status"] = match token.as_str() {
                    "alice-token" => json!({"authenticated": true, "user": {"username": "alice", "uid": "u1", "groups": ["devs"]}}),
                    "bob-token" => json!({"authenticated": true, "user": {"username": "bob", "groups": ["system:authenticated"]}}),
                    "carol-token" => json!({"authenticated": true, "user": {"username": "carol"}}),
                    "err-token" => json!({"authenticated": false, "error": "token expired"}),
                    "api-error" => return Err(ApiError::forbidden("authentication.k8s.io", "tokenreviews", "", "nope").into()),
                    _ => json!({"authenticated": false}),
                };
                Ok(obj)
            }
            "SubjectAccessReview" => {
                self.sars.lock().unwrap().push(obj.clone());
                let user = obj["spec"]["user"].as_str().unwrap_or("").to_string();
                obj["status"] = match user.as_str() {
                    "alice" => json!({"allowed": true}),
                    "bob" => json!({"allowed": false, "reason": "no RBAC policy matched"}),
                    _ => return Err(ApiError::bad_request("bad SAR").into()),
                };
                Ok(obj)
            }
            k => panic!("unexpected create of {k}"),
        }
    }
    async fn update(&self, _: &ResourceInfo, _: Value) -> ctrlrt::Result<Value> {
        unimplemented!()
    }
    async fn update_subresource(&self, _: &ResourceInfo, _: &str, _: Value) -> ctrlrt::Result<Value> {
        unimplemented!()
    }
    async fn patch(&self, _: &ResourceInfo, _: &str, _: &str, _: &Patch) -> ctrlrt::Result<Value> {
        unimplemented!()
    }
    async fn patch_subresource(&self, _: &ResourceInfo, _: &str, _: &str, _: &str, _: &Patch) -> ctrlrt::Result<Value> {
        unimplemented!()
    }
    async fn delete(&self, _: &ResourceInfo, _: &str, _: &str, _: &DeleteOptions) -> ctrlrt::Result<()> {
        unimplemented!()
    }
}

#[derive(Debug)]
struct NoVerify;
impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn tls_client(alpn: &[&str]) -> tokio_rustls::TlsConnector {
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify))
        .with_no_client_auth();
    cfg.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    tokio_rustls::TlsConnector::from(Arc::new(cfg))
}

struct Resp {
    status: u16,
    headers: String,
    body: String,
}

async fn raw_request<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(mut s: S, path: &str, extra: &str) -> Resp {
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{extra}\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf).await;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split(' ').nth(1).unwrap_or("0").parse().unwrap();
    // de-chunk if needed
    let body = if head.to_lowercase().contains("transfer-encoding: chunked") {
        let mut out = String::new();
        let mut rest = body;
        while let Some((len, r)) = rest.split_once("\r\n") {
            let n = usize::from_str_radix(len.trim(), 16).unwrap_or(0);
            if n == 0 {
                break;
            }
            out.push_str(&r[..n]);
            rest = &r[n + 2..];
        }
        out
    } else {
        body.to_string()
    };
    Resp {
        status,
        headers: head.to_lowercase(),
        body,
    }
}

async fn start(opts: MetricsServerOptions, api: Arc<FakeApi>) -> (Arc<MetricsServer>, CancellationToken, tokio::task::JoinHandle<ctrlrt::Result<()>>) {
    let client = ctrlrt::Client::new(api);
    let srv = MetricsServer::new(opts, &client).unwrap().unwrap();
    let cancel = CancellationToken::new();
    let h = tokio::spawn(srv.clone().start(cancel.clone()));
    for _ in 0..200 {
        if !srv.bind_addr().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (srv, cancel, h)
}

fn counter() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let c = prometheus::IntCounter::new("ctrlrt_cmd_test_total", "test counter").unwrap();
        prometheus::default_registry().register(Box::new(c.clone())).unwrap();
        c.inc_by(3);
    });
}

#[tokio::test]
async fn disabled_when_bind_address_is_zero() {
    let client = ctrlrt::Client::new(Arc::new(FakeApi::default()));
    let opts = MetricsServerOptions {
        bind_address: "0".into(),
        ..Default::default()
    };
    assert!(MetricsServer::new(opts, &client).unwrap().is_none());
}

#[tokio::test]
async fn insecure_serves_metrics_and_404() {
    counter();
    let opts = MetricsServerOptions {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    };
    let (srv, cancel, h) = start(opts, Arc::new(FakeApi::default())).await;
    let addr = srv.bind_addr();
    let r = raw_request(tokio::net::TcpStream::connect(&addr).await.unwrap(), "/metrics", "").await;
    assert_eq!(r.status, 200);
    assert!(r.headers.contains("content-type: text/plain; version=0.0.4; charset=utf-8; escaping=underscores"), "{}", r.headers);
    assert!(r.body.contains("# TYPE ctrlrt_cmd_test_total counter\nctrlrt_cmd_test_total 3\n"), "{}", r.body);

    let r = raw_request(tokio::net::TcpStream::connect(&addr).await.unwrap(), "/nope", "").await;
    assert_eq!(r.status, 404);
    assert_eq!(r.body, "404 page not found\n");
    assert!(r.headers.contains("x-content-type-options: nosniff"));

    let r = raw_request(tokio::net::TcpStream::connect(&addr).await.unwrap(), "/metrics", "Accept-Encoding: gzip\r\n").await;
    assert_eq!(r.status, 200);
    assert!(r.headers.contains("content-encoding: gzip"));

    cancel.cancel();
    h.await.unwrap().unwrap();
}

#[tokio::test]
async fn listener_error_message() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let client = ctrlrt::Client::new(Arc::new(FakeApi::default()));
    let srv = MetricsServer::new(
        MetricsServerOptions {
            bind_address: addr.clone(),
            ..Default::default()
        },
        &client,
    )
    .unwrap()
    .unwrap();
    let err = srv.start(CancellationToken::new()).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("failed to start metrics server: failed to create listener: listen tcp {addr}: bind: address already in use")
    );
}

fn secure_opts(tls_opts: Vec<TlsOpt>) -> MetricsServerOptions {
    MetricsServerOptions {
        bind_address: "127.0.0.1:0".into(),
        secure_serving: true,
        cert_dir: "/nonexistent-ctrlrt-cmd-test".into(),
        filter_provider: Some(Arc::new(ctrlrt_cmd::filters::with_authentication_and_authorization)),
        tls_opts,
        ..Default::default()
    }
}

#[tokio::test]
async fn secure_with_authn_authz_filter() {
    counter();
    let api = Arc::new(FakeApi::default());
    let (srv, cancel, h) = start(secure_opts(vec![]), api.clone()).await;
    let addr = srv.bind_addr();
    let get = |extra: &'static str| {
        let addr = addr.clone();
        async move {
            let tcp = tokio::net::TcpStream::connect(&addr).await.unwrap();
            let tls = tls_client(&["http/1.1"])
                .connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(), tcp)
                .await
                .unwrap();
            raw_request(tls, "/metrics", extra).await
        }
    };

    let r = get("").await;
    assert_eq!((r.status, r.body.as_str()), (401, "Unauthorized\n"));
    let r = get("Authorization: Bearer unknown\r\n").await;
    assert_eq!((r.status, r.body.as_str()), (500, "Authentication failed\n"));
    let r = get("Authorization: Bearer err-token\r\n").await;
    assert_eq!((r.status, r.body.as_str()), (500, "Authentication failed\n"));
    let r = get("Authorization: Basic abc\r\n").await;
    assert_eq!((r.status, r.body.as_str()), (401, "Unauthorized\n"));
    let r = get("Authorization: Bearer bob-token\r\n").await;
    assert_eq!((r.status, r.body.as_str()), (403, "Authorization denied for user bob\n"));
    let r = get("Authorization: Bearer carol-token\r\n").await;
    assert_eq!((r.status, r.body.as_str()), (500, "Authorization for user carol failed\n"));
    let r = get("Authorization: Bearer alice-token\r\n").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("ctrlrt_cmd_test_total 3"));

    // TokenReview / SAR requests as sent by the delegating authenticator/authorizer.
    {
        let trs = api.token_reviews.lock().unwrap();
        assert_eq!(trs[0]["apiVersion"], "authentication.k8s.io/v1");
        assert_eq!(trs[0]["spec"], json!({"token": "unknown"}));
        let sars = api.sars.lock().unwrap();
        let alice = sars.iter().find(|s| s["spec"]["user"] == "alice").unwrap();
        assert_eq!(
            alice["spec"],
            json!({
                "user": "alice",
                "uid": "u1",
                "groups": ["devs", "system:authenticated"],
                "nonResourceAttributes": {"path": "/metrics", "verb": "get"}
            })
        );
        let bob = sars.iter().find(|s| s["spec"]["user"] == "bob").unwrap();
        assert_eq!(bob["spec"]["groups"], json!(["system:authenticated"]));
    }

    // Cached: token review (1m) and SAR (5m allow / 30s deny) are not repeated.
    let (ntr, nsar) = (api.token_reviews.lock().unwrap().len(), api.sars.lock().unwrap().len());
    let r = get("Authorization: Bearer alice-token\r\n").await;
    assert_eq!(r.status, 200);
    let r = get("Authorization: Bearer bob-token\r\n").await;
    assert_eq!(r.status, 403);
    let r = get("Authorization: Bearer unknown\r\n").await;
    assert_eq!(r.status, 500);
    assert_eq!(api.token_reviews.lock().unwrap().len(), ntr);
    assert_eq!(api.sars.lock().unwrap().len(), nsar);

    cancel.cancel();
    h.await.unwrap().unwrap();
}

#[tokio::test]
async fn http2_negotiation() {
    // Default: NextProtos ["h2"] -> h2 negotiated; http/1.1-only clients still work.
    let (srv, cancel, h) = start(secure_opts(vec![]), Arc::new(FakeApi::default())).await;
    let addr = srv.bind_addr();
    let tcp = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let tls = tls_client(&["h2", "http/1.1"])
        .connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
    drop(tls);
    cancel.cancel();
    h.await.unwrap().unwrap();

    // --enable-http2=false: ALPN http/1.1 only; h2-only clients are rejected.
    let opt = disable_http2_opt(ctrlrt::Logger::discard());
    let (srv, cancel, h) = start(secure_opts(vec![opt]), Arc::new(FakeApi::default())).await;
    let addr = srv.bind_addr();
    let tcp = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let tls = tls_client(&["h2", "http/1.1"])
        .connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"http/1.1"[..]));
    let r = raw_request(tls, "/metrics", "").await;
    assert_eq!(r.status, 401);
    let tcp = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let res = tls_client(&["h2"])
        .connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(), tcp)
        .await;
    assert!(res.is_err());
    cancel.cancel();
    h.await.unwrap().unwrap();
}

#[tokio::test]
async fn self_signed_certificate_is_served() {
    let (srv, cancel, h) = start(secure_opts(vec![]), Arc::new(FakeApi::default())).await;
    let tcp = tokio::net::TcpStream::connect(srv.bind_addr()).await.unwrap();
    let tls = tls_client(&["http/1.1"])
        .connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    let certs = tls.get_ref().1.peer_certificates().unwrap();
    assert_eq!(certs.len(), 2, "serving cert + CA");
    drop(tls);
    cancel.cancel();
    h.await.unwrap().unwrap();
}
