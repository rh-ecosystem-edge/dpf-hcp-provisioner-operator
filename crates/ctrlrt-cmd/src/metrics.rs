//! controller-runtime `pkg/metrics/server`: serves the Prometheus default registry on
//! `/metrics` over HTTP or HTTPS, optionally behind a filter (authn/authz, see
//! [`crate::filters`]).
//!
//! ```text
//! let mut opts = MetricsServerOptions {
//!     bind_address: flags.metrics_addr.clone(),
//!     secure_serving: flags.secure_metrics,
//!     tls_opts: tls_opts.clone(),
//!     ..Default::default()
//! };
//! if flags.secure_metrics {
//!     opts.filter_provider = Some(Arc::new(ctrlrt_cmd::filters::with_authentication_and_authorization));
//! }
//! if let Some(srv) = MetricsServer::new(opts, &client)? {      // None when BindAddress == "0"
//!     tokio::spawn(async move { srv.start(cancel).await });   // manager runnable
//! }
//! ```

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use ctrlrt::kv;
use ctrlrt::log::Logger;
use futures::future::BoxFuture;
use http_body_util::Full;
use hyper::http::{HeaderValue, Response, StatusCode, header, request::Parts};
use rustls::server::ResolvesServerCert;
use rustls::sign::CertifiedKey;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::certwatcher::CertWatcher;
use crate::tlsutil;

/// Response type of [`Handler`]s.
pub type HttpResponse = Response<Full<Bytes>>;

/// `http.Handler` (request body is not read).
pub type Handler = Arc<dyn Fn(Parts) -> BoxFuture<'static, HttpResponse> + Send + Sync>;

/// `metricsserver.Filter`: wraps a handler (the logger has `path` set).
pub type Filter = Arc<dyn Fn(Logger, Handler) -> ctrlrt::Result<Handler> + Send + Sync>;

/// `Options.FilterProvider` (Go receives the rest config and HTTP client; here the API
/// client).
pub type FilterProvider = Arc<dyn Fn(&ctrlrt::Client) -> ctrlrt::Result<Filter> + Send + Sync>;

/// The subset of `tls.Config` the operator manipulates through `TLSOpts`.
#[derive(Clone, Default)]
pub struct TlsConfig {
    /// `NextProtos` (ALPN), e.g. `["h2"]` or `["http/1.1"]`.
    pub next_protos: Vec<String>,
    /// `GetCertificate` (e.g. a [`CertWatcher`]).
    pub cert_resolver: Option<Arc<dyn ResolvesServerCert>>,
    /// `Certificates[0]`.
    pub certificate: Option<Arc<CertifiedKey>>,
}

impl std::fmt::Debug for TlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsConfig")
            .field("next_protos", &self.next_protos)
            .field("cert_resolver", &self.cert_resolver.is_some())
            .finish()
    }
}

/// `func(*tls.Config)`
pub type TlsOpt = Arc<dyn Fn(&mut TlsConfig) + Send + Sync>;

/// `metricsserver.DefaultBindAddress`
pub const DEFAULT_BIND_ADDRESS: &str = ":8080";
/// `defaultMetricsEndpoint`
pub const DEFAULT_METRICS_ENDPOINT: &str = "/metrics";

/// `metricsserver.Options`
#[derive(Clone, Default)]
pub struct MetricsServerOptions {
    pub secure_serving: bool,
    /// `""` = `:8080`, `"0"` = disabled.
    pub bind_address: String,
    pub extra_handlers: BTreeMap<String, Handler>,
    pub filter_provider: Option<FilterProvider>,
    /// Default `<tmp>/k8s-metrics-server/serving-certs`.
    pub cert_dir: String,
    pub cert_name: String,
    pub key_name: String,
    pub tls_opts: Vec<TlsOpt>,
}

impl MetricsServerOptions {
    fn set_defaults(&mut self) {
        if self.bind_address.is_empty() {
            self.bind_address = DEFAULT_BIND_ADDRESS.into();
        }
        if self.cert_dir.is_empty() {
            self.cert_dir = std::env::temp_dir()
                .join("k8s-metrics-server")
                .join("serving-certs")
                .to_string_lossy()
                .into_owned();
        }
        if self.cert_name.is_empty() {
            self.cert_name = "tls.crt".into();
        }
        if self.key_name.is_empty() {
            self.key_name = "tls.key".into();
        }
    }
}

/// Logger of the package (`logf.RuntimeLog.WithName("metrics")`).
pub fn log() -> Logger {
    Logger::root()
        .with_name("controller-runtime")
        .with_name("metrics")
}

/// `http.Error(w, msg, code)`
pub fn http_error(msg: &str, code: StatusCode) -> HttpResponse {
    let mut r = Response::new(Full::new(Bytes::from(format!("{msg}\n"))));
    *r.status_mut() = code;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    r
}

/// The metrics server (`metricsserver.Server`).
pub struct MetricsServer {
    options: MetricsServerOptions,
    metrics_filter: Option<Filter>,
    extra: Mutex<BTreeMap<String, Handler>>,
    bind_addr: Mutex<String>,
}

impl std::fmt::Debug for MetricsServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsServer")
            .field("bind_address", &self.options.bind_address)
            .field("secure", &self.options.secure_serving)
            .finish()
    }
}

impl MetricsServer {
    /// `metricsserver.NewServer(o, config, httpClient)`; `Ok(None)` when the bind address is
    /// `"0"` (metrics disabled).
    pub fn new(
        mut o: MetricsServerOptions,
        client: &ctrlrt::Client,
    ) -> ctrlrt::Result<Option<Arc<Self>>> {
        o.set_defaults();
        if o.bind_address == "0" {
            return Ok(None);
        }
        if o.extra_handlers.contains_key(DEFAULT_METRICS_ENDPOINT) {
            return Err(anyhow::anyhow!(
                "overriding builtin {DEFAULT_METRICS_ENDPOINT} endpoint is not allowed"
            ));
        }
        let metrics_filter = match &o.filter_provider {
            Some(p) => Some(p(client).map_err(|e| {
                ctrlrt::wrap(
                    anyhow::anyhow!("{e}"),
                    format!("filter provider failed to create filter for the metrics server: {e}"),
                )
            })?),
            None => None,
        };
        let extra = o.extra_handlers.clone();
        Ok(Some(Arc::new(Self {
            options: o,
            metrics_filter,
            extra: Mutex::new(extra),
            bind_addr: Mutex::new(String::new()),
        })))
    }

    /// `NeedLeaderElection`: false.
    pub fn need_leader_election(&self) -> bool {
        false
    }

    /// `AddExtraHandler(path, handler)`
    pub fn add_extra_handler(&self, path: &str, handler: Handler) -> ctrlrt::Result<()> {
        let mut extra = self
            .extra
            .lock()
            .map_err(|_| anyhow::anyhow!("lock poisoned"))?;
        if path == DEFAULT_METRICS_ENDPOINT {
            return Err(anyhow::anyhow!(
                "overriding builtin {DEFAULT_METRICS_ENDPOINT} endpoint is not allowed"
            ));
        }
        if extra.contains_key(path) {
            return Err(anyhow::anyhow!(
                "can't register extra handler by duplicate path {} on metrics http server",
                ctrlrt::error::go_quote(path)
            ));
        }
        extra.insert(path.to_string(), handler);
        Ok(())
    }

    /// `GetBindAddr()`: the actual listen address once started.
    pub fn bind_addr(&self) -> String {
        self.bind_addr.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// `Start(ctx)`: serve until `cancel` fires, then shut down gracefully (1 minute).
    pub async fn start(self: Arc<Self>, cancel: CancellationToken) -> ctrlrt::Result<()> {
        let log = log();
        log.info("Starting metrics server", kv![]);

        let (listener, tls) = self.create_listener(&cancel, &log).await.map_err(|e| {
            ctrlrt::wrap(
                anyhow::anyhow!("{e}"),
                format!("failed to start metrics server: failed to create listener: {e}"),
            )
        })?;
        if let (Ok(mut b), Ok(a)) = (self.bind_addr.lock(), listener.local_addr()) {
            *b = go_addr_string(&a);
        }

        let mut routes: BTreeMap<String, Handler> = BTreeMap::new();
        let mut handler: Handler =
            Arc::new(|parts: Parts| Box::pin(async move { prometheus_handler(&parts) }));
        if let Some(f) = &self.metrics_filter {
            let l = log.with_values(kv!["path" => DEFAULT_METRICS_ENDPOINT]);
            handler = f(l, handler).map_err(|e| {
                ctrlrt::wrap(
                    anyhow::anyhow!("{e}"),
                    format!("failed to start metrics server: failed to add metrics filter: {e}"),
                )
            })?;
        }
        routes.insert(DEFAULT_METRICS_ENDPOINT.into(), handler);
        let extra = self.extra.lock().map(|e| e.clone()).unwrap_or_default();
        for (path, mut h) in extra {
            if let Some(f) = &self.metrics_filter {
                let l = log.with_values(kv!["path" => path]);
                h = f(l, h).map_err(|e| {
                    ctrlrt::wrap(
                        anyhow::anyhow!("{e}"),
                        format!(
                            "failed to start metrics server: failed to add metrics filter to extra handler for path {path}: {e}"
                        ),
                    )
                })?;
            }
            routes.insert(path, h);
        }
        let mux = Arc::new(Mux { routes });

        log.info(
            "Serving metrics server",
            kv!["bindAddress" => self.options.bind_address, "secure" => self.options.secure_serving],
        );

        serve(listener, tls, mux, cancel, log).await
    }

    async fn create_listener(
        &self,
        cancel: &CancellationToken,
        log: &Logger,
    ) -> Result<(TcpListener, Option<Arc<rustls::ServerConfig>>), String> {
        if !self.options.secure_serving {
            return Ok((listen(&self.options.bind_address).await?, None));
        }
        let mut cfg = TlsConfig {
            next_protos: vec!["h2".into()],
            ..Default::default()
        };
        for op in &self.options.tls_opts {
            op(&mut cfg);
        }

        if cfg.cert_resolver.is_none() {
            let cert_path =
                std::path::Path::new(&self.options.cert_dir).join(&self.options.cert_name);
            let key_path =
                std::path::Path::new(&self.options.cert_dir).join(&self.options.key_name);
            let exists = |p: &std::path::Path| match std::fs::metadata(p) {
                Ok(_) => true,
                Err(e) => e.kind() != std::io::ErrorKind::NotFound,
            };
            if exists(&cert_path) && exists(&key_path) {
                let cw = CertWatcher::new(
                    cert_path.to_string_lossy().into_owned(),
                    key_path.to_string_lossy().into_owned(),
                )
                .map_err(|e| e.to_string())?;
                cfg.cert_resolver = Some(Arc::new(cw.clone()));
                let cancel = cancel.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    if let Err(err) = cw.start(cancel).await {
                        log.error(&err, "certificate watcher error", kv![]);
                    }
                });
            }
        }

        if cfg.cert_resolver.is_none() && cfg.certificate.is_none() {
            let key = generate_self_signed_cert_key("localhost").map_err(|e| {
                format!("failed to generate self-signed certificate for metrics server: {e}")
            })?;
            cfg.certificate = Some(key);
        }

        let resolver: Arc<dyn ResolvesServerCert> = match (&cfg.cert_resolver, &cfg.certificate) {
            (Some(r), _) => r.clone(),
            (None, Some(c)) => Arc::new(rustls::sign::SingleCertAndKey::from(c.clone())),
            (None, None) => return Err("no certificate".into()),
        };
        let mut server_cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_cert_resolver(resolver);
        server_cfg.alpn_protocols = alpn_protocols(&cfg.next_protos);

        let l = listen(&self.options.bind_address).await?;
        Ok((l, Some(Arc::new(server_cfg))))
    }
}

/// Go's TLS server accepts an `http/1.1`-only client even when `NextProtos` lacks it (the
/// connection proceeds without ALPN); rustls rejects mismatches, so `http/1.1` is appended
/// whenever `h2` is offered.
fn alpn_protocols(next_protos: &[String]) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = next_protos.iter().map(|p| p.as_bytes().to_vec()).collect();
    if next_protos.iter().any(|p| p == "h2") && !next_protos.iter().any(|p| p == "http/1.1") {
        v.push(b"http/1.1".to_vec());
    }
    v
}

/// `certutil.GenerateSelfSignedCertKeyWithFixtures(host, [127.0.0.1], nil, "")`: a CA plus a
/// serving certificate for `host` and 127.0.0.1, valid for one year; the served chain is
/// `[serving, CA]`. (ECDSA P-256 keys instead of Go's RSA-2048.)
pub fn generate_self_signed_cert_key(host: &str) -> Result<Arc<CertifiedKey>, String> {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
        IsCa, KeyPair, KeyUsagePurpose, SanType,
    };
    let now = std::time::SystemTime::now();
    let unix = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let not_before = time_from_system(now);
    let not_after = time_from_system(now + Duration::from_secs(365 * 24 * 3600));

    let ca_key =
        KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(|e| e.to_string())?;
    let mut ca = CertificateParams::default();
    ca.distinguished_name = DistinguishedName::new();
    ca.distinguished_name
        .push(DnType::CommonName, format!("{host}-ca@{unix}"));
    ca.serial_number = Some(rcgen::SerialNumber::from(1u64));
    ca.not_before = not_before;
    ca.not_after = not_after;
    ca.key_usages = vec![
        KeyUsagePurpose::KeyEncipherment,
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
    ];
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_cert = ca.self_signed(&ca_key).map_err(|e| e.to_string())?;
    let issuer = rcgen::Issuer::new(ca, &ca_key);

    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(|e| e.to_string())?;
    let mut p = CertificateParams::default();
    p.distinguished_name = DistinguishedName::new();
    p.distinguished_name
        .push(DnType::CommonName, format!("{host}@{unix}"));
    p.serial_number = Some(rcgen::SerialNumber::from(2u64));
    p.not_before = not_before;
    p.not_after = not_after;
    p.key_usages = vec![
        KeyUsagePurpose::KeyEncipherment,
        KeyUsagePurpose::DigitalSignature,
    ];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    p.subject_alt_names = vec![
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))),
        SanType::DnsName(host.try_into().map_err(|e: rcgen::Error| e.to_string())?),
    ];
    let cert = p.signed_by(&key, &issuer).map_err(|e| e.to_string())?;

    let chain = format!("{}{}", cert.pem(), ca_cert.pem());
    tlsutil::x509_key_pair(chain.as_bytes(), key.serialize_pem().as_bytes())
        .map_err(|e| format!("failed to create self-signed key pair for metrics server: {e}"))
}

fn time_from_system(t: std::time::SystemTime) -> time::OffsetDateTime {
    time::OffsetDateTime::from(t)
}

/// Go `net.Addr.String()` for a TCP address.
fn go_addr_string(a: &SocketAddr) -> String {
    match a {
        SocketAddr::V4(v) => format!("{}:{}", v.ip(), v.port()),
        SocketAddr::V6(v) => format!("[{}]:{}", v.ip(), v.port()),
    }
}

/// `net.SplitHostPort`
pub fn split_host_port(hostport: &str) -> Result<(String, String), String> {
    let missing = || format!("address {hostport}: missing port in address");
    let too_many = || format!("address {hostport}: too many colons in address");
    let Some(i) = hostport.rfind(':') else {
        return Err(missing());
    };
    let (host, port);
    if hostport.starts_with('[') {
        let Some(end) = hostport.find(']') else {
            return Err(format!("address {hostport}: missing ']' in address"));
        };
        if end + 1 == hostport.len() {
            return Err(missing());
        } else if end + 1 != i {
            if hostport.as_bytes()[end + 1] == b':' {
                return Err(too_many());
            }
            return Err(missing());
        }
        host = hostport[1..end].to_string();
        if host.contains('[') || hostport[end + 1..].contains(']') {
            return Err(format!("address {hostport}: unexpected '[' in address"));
        }
    } else {
        host = hostport[..i].to_string();
        if host.contains(':') {
            return Err(too_many());
        }
        if host.contains('[') {
            return Err(format!("address {hostport}: unexpected '[' in address"));
        }
    }
    port = hostport[i + 1..].to_string();
    if hostport[i + 1..].contains(']') {
        return Err(format!("address {hostport}: unexpected ']' in address"));
    }
    Ok((host, port))
}

/// `net.Listen("tcp", address)` with Go's error messages (`listen tcp :8080: bind: address
/// already in use`).
pub async fn listen(address: &str) -> Result<TcpListener, String> {
    let (host, port) = split_host_port(address).map_err(|e| format!("listen tcp: {e}"))?;
    let port: u16 = if port.is_empty() {
        0
    } else {
        port.parse()
            .map_err(|_| format!("listen tcp: lookup tcp/{port}: unknown port"))?
    };
    let candidates: Vec<SocketAddr> = if host.is_empty() {
        vec![
            SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)),
            SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, port)),
        ]
    } else if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|e| format!("listen tcp: lookup {host}: {}", tlsutil::go_errno_text(&e)))?
            .collect()
    };
    let mut last = None;
    for a in candidates {
        match TcpListener::bind(a).await {
            Ok(l) => return Ok(l),
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) => format!("listen tcp {address}: bind: {}", tlsutil::go_errno_text(&e)),
        None => format!("listen tcp {address}: no suitable address found"),
    })
}

/// `http.ServeMux` with the registered exact paths (and `/`-terminated subtree patterns).
struct Mux {
    routes: BTreeMap<String, Handler>,
}

impl Mux {
    fn find(&self, path: &str) -> Option<&Handler> {
        if let Some(h) = self.routes.get(path) {
            return Some(h);
        }
        // Longest subtree pattern ending with "/".
        self.routes
            .iter()
            .filter(|(p, _)| p.ends_with('/') && path.starts_with(p.as_str()))
            .max_by_key(|(p, _)| p.len())
            .map(|(_, h)| h)
    }

    async fn serve(&self, parts: Parts) -> HttpResponse {
        let path = parts.uri.path().to_string();
        let path = if path.is_empty() {
            "/".to_string()
        } else {
            path
        };
        let clean = clean_path(&path);
        if clean != path && parts.method != hyper::Method::CONNECT {
            let mut loc = clean;
            if let Some(q) = parts.uri.query() {
                loc.push('?');
                loc.push_str(q);
            }
            return redirect(
                &loc,
                parts.method == hyper::Method::GET || parts.method == hyper::Method::HEAD,
            );
        }
        match self.find(&path) {
            Some(h) => h(parts).await,
            None => http_error("404 page not found", StatusCode::NOT_FOUND),
        }
    }
}

fn redirect(loc: &str, with_body: bool) -> HttpResponse {
    let body = if with_body {
        format!(
            "<a href=\"{}\">Moved Permanently</a>.\n\n",
            html_escape(loc)
        )
    } else {
        String::new()
    };
    let mut r = Response::new(Full::new(Bytes::from(body)));
    *r.status_mut() = StatusCode::MOVED_PERMANENTLY;
    if let Ok(v) = HeaderValue::from_str(loc) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    if with_body {
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
    }
    r
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

/// `http.cleanPath` (path.Clean, keeping a trailing slash).
fn clean_path(p: &str) -> String {
    let p = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    };
    let mut stack: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            s => stack.push(s),
        }
    }
    let mut np = format!("/{}", stack.join("/"));
    if p.ends_with('/') && np != "/" {
        np.push('/');
    }
    np
}

/// `promhttp.HandlerFor(registry, HandlerOpts{ErrorHandling: HTTPErrorOnError})` for the
/// `prometheus` crate's default registry (text exposition format).
pub fn prometheus_handler(parts: &Parts) -> HttpResponse {
    use prometheus::Encoder;
    let families = prometheus::default_registry().gather();
    let mut body = Vec::new();
    if let Err(e) = prometheus::TextEncoder::new().encode(&families, &mut body) {
        return http_error(
            &format!("An error has occurred while serving metrics:\n\n{e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    let content_type = format!(
        "text/plain; version=0.0.4; charset=utf-8; escaping={}",
        negotiate_escaping(parts)
    );
    let encoding = negotiate_content_encoding(parts, &["identity", "gzip"]);
    let mut resp_body = body;
    let mut gz = false;
    if encoding == "gzip" {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        if e.write_all(&resp_body).is_ok()
            && let Ok(v) = e.finish()
        {
            resp_body = v;
            gz = true;
        }
    }
    let mut r = Response::new(Full::new(Bytes::from(resp_body)));
    if let Ok(v) = HeaderValue::from_str(&content_type) {
        r.headers_mut().insert(header::CONTENT_TYPE, v);
    }
    if gz {
        r.headers_mut()
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    r
}

/// The `escaping` parameter chosen by `expfmt.Negotiate`.
fn negotiate_escaping(parts: &Parts) -> String {
    let mut scheme = "underscores".to_string();
    let accept = parts
        .headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    for ac in accept.split(',') {
        for param in ac.split(';').skip(1) {
            if let Some((k, v)) = param.trim().split_once('=')
                && k.trim().eq_ignore_ascii_case("escaping")
            {
                let v = v.trim().trim_matches('"');
                if matches!(v, "allow-utf-8" | "underscores" | "dots" | "values") {
                    scheme = v.to_string();
                }
            }
        }
    }
    scheme
}

/// `httputil.NegotiateContentEncoding`
fn negotiate_content_encoding(parts: &Parts, offers: &[&str]) -> String {
    let mut specs: Vec<(String, f64)> = Vec::new();
    for h in parts.headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(h) = h.to_str() else { continue };
        for item in h.split(',') {
            let mut it = item.split(';');
            let value = it.next().unwrap_or("").trim().to_lowercase();
            if value.is_empty() {
                continue;
            }
            let mut q = 1.0;
            for p in it {
                if let Some((k, v)) = p.trim().split_once('=')
                    && k.trim() == "q"
                {
                    q = v.trim().parse().unwrap_or(0.0);
                }
            }
            specs.push((value, q));
        }
    }
    let mut best = "identity".to_string();
    let mut best_q = -1.0;
    for offer in offers {
        for (v, q) in &specs {
            if *q > best_q && (v == "*" || v == offer) {
                best_q = *q;
                best = offer.to_string();
            }
        }
    }
    if best_q == 0.0 {
        return "identity".into();
    }
    best
}

async fn serve(
    listener: TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    mux: Arc<Mux>,
    cancel: CancellationToken,
    log: Logger,
) -> ctrlrt::Result<()> {
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};

    let graceful = hyper_util::server::graceful::GracefulShutdown::new();
    let acceptor = tls.map(tokio_rustls::TlsAcceptor::from);

    loop {
        let accepted = tokio::select! {
            _ = cancel.cancelled() => break,
            a = listener.accept() => a,
        };
        let (stream, _peer) = match accepted {
            Ok(s) => s,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
        };
        let mux = mux.clone();
        let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
            let mux = mux.clone();
            async move {
                let (parts, _body) = req.into_parts();
                Ok::<_, std::convert::Infallible>(mux.serve(parts).await)
            }
        });
        match &acceptor {
            None => {
                let conn = hyper::server::conn::http1::Builder::new()
                    .timer(TokioTimer::new())
                    .header_read_timeout(Duration::from_secs(32))
                    .max_buf_size(1 << 20 | 4096)
                    .serve_connection(TokioIo::new(stream), svc);
                let fut = graceful.watch(conn);
                tokio::spawn(async move {
                    let _ = fut.await;
                });
            }
            Some(acc) => {
                let acc = acc.clone();
                let watcher = graceful.watcher();
                tokio::spawn(async move {
                    let Ok(tls_stream) = acc.accept(stream).await else {
                        return;
                    };
                    let h2 = tls_stream.get_ref().1.alpn_protocol() == Some(b"h2");
                    if h2 {
                        let conn = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                            .timer(TokioTimer::new())
                            .serve_connection(TokioIo::new(tls_stream), svc);
                        let _ = watcher.watch(conn).await;
                    } else {
                        let conn = hyper::server::conn::http1::Builder::new()
                            .timer(TokioTimer::new())
                            .header_read_timeout(Duration::from_secs(32))
                            .max_buf_size(1 << 20 | 4096)
                            .serve_connection(TokioIo::new(tls_stream), svc);
                        let _ = watcher.watch(conn).await;
                    }
                });
            }
        }
    }
    drop(listener);
    log.info(
        "Shutting down metrics server with timeout of 1 minute",
        kv![],
    );
    if tokio::time::timeout(Duration::from_secs(60), graceful.shutdown())
        .await
        .is_err()
    {
        log.error(
            &"context deadline exceeded",
            "error shutting down the HTTP server",
            kv![],
        );
    }
    Ok(())
}

/// `TLSOpts` entry of `cmd/main.go` when `--enable-http2=false`: logs `disabling http/2`
/// (on `setupLog`) and restricts ALPN to `http/1.1`.
pub fn disable_http2_opt(setup_log: Logger) -> TlsOpt {
    Arc::new(move |c: &mut TlsConfig| {
        setup_log.info("disabling http/2", kv![]);
        c.next_protos = vec!["http/1.1".into()];
    })
}

/// `TLSOpts` entry setting `GetCertificate` to a cert watcher.
pub fn cert_watcher_opt(cw: CertWatcher) -> TlsOpt {
    Arc::new(move |c: &mut TlsConfig| {
        c.cert_resolver = Some(Arc::new(cw.clone()));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_host_port_like_go() {
        assert_eq!(
            split_host_port(":8443").unwrap(),
            ("".into(), "8443".into())
        );
        assert_eq!(
            split_host_port("[::1]:80").unwrap(),
            ("::1".into(), "80".into())
        );
        assert_eq!(
            split_host_port("8443").unwrap_err(),
            "address 8443: missing port in address"
        );
        assert_eq!(
            split_host_port("a:b:c").unwrap_err(),
            "address a:b:c: too many colons in address"
        );
    }

    #[test]
    fn clean_paths() {
        assert_eq!(clean_path("/metrics"), "/metrics");
        assert_eq!(clean_path("//metrics"), "/metrics");
        assert_eq!(clean_path("/a/../metrics"), "/metrics");
        assert_eq!(clean_path("/x/"), "/x/");
    }

    #[test]
    fn self_signed() {
        let k = generate_self_signed_cert_key("localhost").unwrap();
        assert_eq!(k.cert.len(), 2);
        assert!(k.keys_match().is_ok());
    }

    #[test]
    fn alpn() {
        assert_eq!(
            alpn_protocols(&["h2".into()]),
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        assert_eq!(
            alpn_protocols(&["http/1.1".into()]),
            vec![b"http/1.1".to_vec()]
        );
    }
}
