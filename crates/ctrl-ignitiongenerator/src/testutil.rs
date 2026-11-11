//! Test helpers: an HTTPS ignition server (`httptest.NewTLSServer` analogue) and the fake
//! client scheme of the Go tests (`newTestScheme`).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::dpf::operator::v1alpha1 as operatorv1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use bytes::Bytes;
use ctrlrt::{FakeRecorder, Recorder};
use ctrlrt_fake::Scheme;
use http_body_util::Full;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// `newTestScheme()`
pub(crate) fn new_test_scheme() -> Scheme {
    Scheme::new()
        .with_client_go()
        .register::<provisioningv1alpha1::DPFHCPProvisioner>()
        .register::<hyperv1::HostedCluster>()
        .register::<hyperv1::NodePool>()
        .register::<dpuservicev1alpha1::DPUDeployment>()
        .register::<dpuprovisioningv1alpha1::DPUFlavor>()
        .register::<operatorv1alpha1::DPFOperatorConfig>()
}

/// `record.NewFakeRecorder(n)` as a shared handle plus the recorder for assertions.
pub(crate) fn fake_recorder() -> (Arc<FakeRecorder>, Recorder) {
    let r = Arc::new(FakeRecorder::new(100));
    (r.clone(), r as Recorder)
}

/// A request seen by the server: (path, Authorization header).
pub(crate) type SeenRequest = (String, Option<String>);

/// Response of a handler: (status, body).
pub(crate) type Handler = Arc<dyn Fn(&str, Option<&str>) -> (u16, Vec<u8>) + Send + Sync>;

/// CA certificate (PEM) and the issuer used to sign serving certificates.
pub(crate) struct TestCa {
    pub pem: String,
    issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
}

impl TestCa {
    pub(crate) fn new(cn: &str) -> Self {
        use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, cn);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let cert = params.self_signed(&key).unwrap();
        Self {
            pem: cert.pem(),
            issuer: rcgen::Issuer::new(params, key),
        }
    }

    /// A serving certificate with the SANs of Go's `httptest` certificate
    /// (`example.com`, `*.example.com`, `127.0.0.1`, `::1`).
    fn serving_cert(&self) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
        use rcgen::{CertificateParams, ExtendedKeyUsagePurpose, KeyPair, SanType};
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params =
            CertificateParams::new(vec!["example.com".to_string(), "*.example.com".to_string()])
                .unwrap();
        params.subject_alt_names.push(SanType::IpAddress(
            std::net::Ipv4Addr::new(127, 0, 0, 1).into(),
        ));
        params
            .subject_alt_names
            .push(SanType::IpAddress(std::net::Ipv6Addr::LOCALHOST.into()));
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let cert = params.signed_by(&key, &self.issuer).unwrap();
        (
            cert.der().clone(),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
    }
}

/// An HTTPS server on 127.0.0.1 (aborted on drop).
pub(crate) struct IgnitionServer {
    pub addr: SocketAddr,
    /// PEM of the CA that signed the serving certificate.
    pub ca_pem: Vec<u8>,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for IgnitionServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl IgnitionServer {
    /// Start a server answering every request with `handler(path, authorization)`.
    pub(crate) async fn start(handler: Handler) -> Self {
        let ca = TestCa::new("ignition-test-ca");
        let (cert, key) = ca.serving_cert();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<SeenRequest>>> = Arc::default();
        let seen2 = seen.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let acceptor = acceptor.clone();
                let handler = handler.clone();
                let seen = seen2.clone();
                tokio::spawn(async move {
                    let Ok(tls_stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let svc = hyper::service::service_fn(
                        move |req: hyper::Request<hyper::body::Incoming>| {
                            let handler = handler.clone();
                            let seen = seen.clone();
                            async move {
                                let path = req.uri().path().to_string();
                                let auth = req
                                    .headers()
                                    .get("Authorization")
                                    .and_then(|v| v.to_str().ok())
                                    .map(str::to_string);
                                seen.lock().unwrap().push((path.clone(), auth.clone()));
                                let (status, body) = handler(&path, auth.as_deref());
                                let mut resp = hyper::Response::new(Full::new(Bytes::from(body)));
                                *resp.status_mut() = hyper::StatusCode::from_u16(status).unwrap();
                                resp.headers_mut().insert(
                                    "Content-Type",
                                    hyper::header::HeaderValue::from_static("application/json"),
                                );
                                Ok::<_, std::convert::Infallible>(resp)
                            }
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(tls_stream), svc)
                        .await;
                });
            }
        });
        Self {
            addr,
            ca_pem: ca.pem.into_bytes(),
            seen,
            task,
        }
    }

    /// `startIgnitionServer(hcpJSON)` of the Go integration test: `/ignition` serves the
    /// payload when a `Bearer ` Authorization header is present (401 otherwise, 404 for other
    /// paths).
    pub(crate) async fn serve_hcp(hcp_json: Vec<u8>) -> Self {
        Self::start(Arc::new(move |path, auth| {
            if path != "/ignition" {
                return (404, Vec::new());
            }
            if !auth.is_some_and(|a| a.starts_with("Bearer ")) {
                return (401, Vec::new());
            }
            (200, hcp_json.clone())
        }))
        .await
    }

    /// `serverURL.Host`
    pub(crate) fn host(&self) -> String {
        self.addr.to_string()
    }

    /// Requests seen so far.
    pub(crate) fn seen(&self) -> Vec<SeenRequest> {
        self.seen.lock().unwrap().clone()
    }
}
