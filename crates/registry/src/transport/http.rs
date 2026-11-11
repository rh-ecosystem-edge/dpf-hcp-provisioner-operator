//! The base HTTP round tripper (Go's `remote.DefaultTransport`): reqwest + rustls with the
//! system trust store, Go `http.ProxyFromEnvironment` semantics, no automatic redirects
//! (redirects are followed by [`super::Client`] like Go's `http.Client`).

use std::fmt;
use std::io;
use std::net::IpAddr;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_trait::async_trait;
use http::HeaderValue;

use super::{Body, Request, Response, RoundTripper};
use crate::gourl::GoUrl;
use crate::goutil;

/// A network-level failure (Go: `*net.OpError`, `io.ErrUnexpectedEOF`, TLS errors, ...).
#[derive(Debug, Clone)]
pub struct NetError {
    pub message: String,
    /// Go `Temporary()` / timeouts, ECONNRESET, EPIPE, EOF: retried by the default
    /// retry predicate.
    pub retryable: bool,
    pub timeout: bool,
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NetError {}

impl NetError {
    pub fn new(message: impl Into<String>) -> Self {
        NetError {
            message: message.into(),
            retryable: false,
            timeout: false,
        }
    }

    /// Map an I/O error from reading a body, keeping typed inner errors reachable.
    pub fn from_io(e: io::Error) -> anyhow::Error {
        let typed = e
            .get_ref()
            .is_some_and(|i| i.is::<NetError>() || i.is::<crate::remote::VerifyError>());
        if typed {
            let message = go_io_message(&e);
            if let Some(inner) = e.into_inner() {
                return match inner.downcast::<NetError>() {
                    Ok(n) => anyhow::Error::new(*n),
                    Err(other) => match other.downcast::<crate::remote::VerifyError>() {
                        Ok(v) => anyhow::Error::new(*v),
                        Err(_) => anyhow::anyhow!(message),
                    },
                };
            }
            return anyhow::anyhow!(message);
        }
        anyhow::Error::new(NetError {
            message: go_io_message(&e),
            retryable: matches!(
                e.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::TimedOut
            ),
            timeout: e.kind() == io::ErrorKind::TimedOut,
        })
    }
}

/// Render an I/O error the way the equivalent Go error would read.
pub fn go_io_message(e: &io::Error) -> String {
    if let Some(inner) = e.get_ref() {
        if let Some(n) = inner.downcast_ref::<NetError>() {
            return n.message.clone();
        }
        if let Some(v) = inner.downcast_ref::<crate::remote::VerifyError>() {
            return v.to_string();
        }
    }
    match e.kind() {
        io::ErrorKind::UnexpectedEof => "unexpected EOF".to_string(),
        io::ErrorKind::ConnectionReset => "connection reset by peer".to_string(),
        io::ErrorKind::BrokenPipe => "broken pipe".to_string(),
        _ => {
            let s = e.to_string();
            if s.contains("corrupt deflate stream") || s.contains("invalid deflate") {
                "flate: corrupt input".to_string()
            } else if s.contains("checksum") && s.contains("gzip") {
                "gzip: invalid checksum".to_string()
            } else if s.contains("invalid gzip header") {
                "gzip: invalid header".to_string()
            } else {
                s
            }
        }
    }
}

fn find_source<'a, T: std::error::Error + 'static>(
    e: &'a (dyn std::error::Error + 'static),
) -> Option<&'a T> {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(c) = cur {
        if let Some(t) = c.downcast_ref::<T>() {
            return Some(t);
        }
        // io::Error::source() skips the wrapped error itself; look inside explicitly.
        if let Some(ioe) = c.downcast_ref::<io::Error>()
            && let Some(inner) = ioe.get_ref()
                && let Some(t) = find_source::<T>(inner) {
                    return Some(t);
                }
        cur = c.source();
    }
    None
}

/// Convert a reqwest send error into a Go-like network error.
fn map_reqwest_error(e: &reqwest::Error, url: &GoUrl) -> NetError {
    let hostport = {
        let port = url.port();
        let port = if port.is_empty() {
            if url.scheme == "http" { "80".to_string() } else { "443".to_string() }
        } else {
            port
        };
        goutil::join_host_port(&url.hostname(), &port)
    };
    if e.is_timeout() {
        return NetError {
            message: format!("dial tcp {hostport}: i/o timeout"),
            retryable: true,
            timeout: true,
        };
    }
    if let Some(tls) = find_source::<rustls::Error>(e) {
        let message = match tls {
            rustls::Error::InvalidMessage(rustls::InvalidMessage::InvalidContentType) => {
                "http: server gave HTTP response to HTTPS client".to_string()
            }
            rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer) => {
                "tls: failed to verify certificate: x509: certificate signed by unknown authority"
                    .to_string()
            }
            rustls::Error::InvalidCertificate(rustls::CertificateError::NotValidForName)
            | rustls::Error::InvalidCertificate(
                rustls::CertificateError::NotValidForNameContext { .. },
            ) => format!(
                "tls: failed to verify certificate: x509: certificate is not valid for {}",
                url.hostname()
            ),
            rustls::Error::InvalidCertificate(rustls::CertificateError::Expired)
            | rustls::Error::InvalidCertificate(rustls::CertificateError::ExpiredContext { .. }) => {
                "tls: failed to verify certificate: x509: certificate has expired or is not yet valid"
                    .to_string()
            }
            other => format!("tls: {other}"),
        };
        return NetError::new(message);
    }
    if let Some(ioe) = find_source::<io::Error>(e) {
        let msg = match ioe.kind() {
            io::ErrorKind::ConnectionRefused => {
                format!("dial tcp {hostport}: connect: connection refused")
            }
            io::ErrorKind::ConnectionReset => {
                format!("read tcp {hostport}: read: connection reset by peer")
            }
            io::ErrorKind::BrokenPipe => format!("write tcp {hostport}: write: broken pipe"),
            io::ErrorKind::UnexpectedEof => "EOF".to_string(),
            io::ErrorKind::TimedOut => format!("dial tcp {hostport}: i/o timeout"),
            _ => {
                let s = ioe.to_string();
                if s.contains("failed to lookup address") || s.contains("dns error") {
                    format!("dial tcp: lookup {}: no such host", url.hostname())
                } else {
                    format!("dial tcp {hostport}: {s}")
                }
            }
        };
        return NetError {
            retryable: matches!(
                ioe.kind(),
                io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::TimedOut
            ),
            timeout: ioe.kind() == io::ErrorKind::TimedOut,
            message: msg,
        };
    }
    // hyper: connection closed before the response was complete.
    let text = {
        let mut parts = Vec::new();
        let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
        while let Some(c) = cur {
            parts.push(c.to_string());
            cur = c.source();
        }
        parts
    };
    if text.iter().any(|t| {
        t.contains("connection closed before message completed")
            || t.contains("IncompleteMessage")
            || t.contains("unexpected end of file")
    }) {
        return NetError {
            message: "EOF".to_string(),
            retryable: true,
            timeout: false,
        };
    }
    if text
        .iter()
        .any(|t| t.contains("dns error") || t.contains("failed to lookup address"))
    {
        return NetError::new(format!("dial tcp: lookup {}: no such host", url.hostname()));
    }
    NetError::new(text.last().cloned().unwrap_or_else(|| e.to_string()))
}

// ------------------------------------------------------------------ proxy

/// Port of `golang.org/x/net/http/httpproxy` (`http.ProxyFromEnvironment`).
#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    http_proxy: Option<url::Url>,
    https_proxy: Option<url::Url>,
    cgi: bool,
    all: bool,
    ip_matchers: Vec<IpMatcher>,
    domain_matchers: Vec<DomainMatcher>,
}

#[derive(Debug, Clone)]
enum IpMatcher {
    Cidr(IpAddr, u8),
    Ip(IpAddr, String),
}

#[derive(Debug, Clone)]
struct DomainMatcher {
    host: String,
    port: String,
    match_host: bool,
}

fn get_env_any(names: &[&str]) -> String {
    for n in names {
        if let Ok(v) = std::env::var(n)
            && !v.is_empty() {
                return v;
            }
    }
    String::new()
}

fn parse_proxy(p: &str) -> Option<url::Url> {
    if p.is_empty() {
        return None;
    }
    match url::Url::parse(p) {
        Ok(u) if u.has_host() && !u.scheme().is_empty() => Some(u),
        _ => url::Url::parse(&format!("http://{p}")).ok(),
    }
}

fn parse_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let (ip, bits) = s.split_once('/')?;
    let ip: IpAddr = ip.parse().ok()?;
    let bits: u8 = bits.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if bits > max {
        return None;
    }
    Some((ip, bits))
}

fn cidr_contains(net: IpAddr, bits: u8, ip: IpAddr) -> bool {
    let (net, ip) = match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(i)) => (u128::from(u32::from(n)) << 96, u128::from(u32::from(i)) << 96),
        (IpAddr::V6(n), IpAddr::V6(i)) => (u128::from(n), u128::from(i)),
        (IpAddr::V4(n), IpAddr::V6(i)) => match i.to_ipv4_mapped() {
            Some(i) => (u128::from(u32::from(n)) << 96, u128::from(u32::from(i)) << 96),
            None => return false,
        },
        (IpAddr::V6(n), IpAddr::V4(i)) => (u128::from(n), u128::from(i.to_ipv6_mapped())),
    };
    if bits == 0 {
        return true;
    }
    let mask = u128::MAX << (128 - u32::from(bits));
    net & mask == ip & mask
}

fn ip_equal(a: IpAddr, b: IpAddr) -> bool {
    let norm = |ip: IpAddr| match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    norm(a) == norm(b)
}

impl ProxyConfig {
    /// `httpproxy.FromEnvironment()` + `init()`.
    pub fn from_env() -> Self {
        Self::new(
            &get_env_any(&["HTTP_PROXY", "http_proxy"]),
            &get_env_any(&["HTTPS_PROXY", "https_proxy"]),
            &get_env_any(&["NO_PROXY", "no_proxy"]),
            std::env::var("REQUEST_METHOD").is_ok_and(|v| !v.is_empty()),
        )
    }

    pub fn new(http_proxy: &str, https_proxy: &str, no_proxy: &str, cgi: bool) -> Self {
        let mut c = ProxyConfig {
            http_proxy: parse_proxy(http_proxy),
            https_proxy: parse_proxy(https_proxy),
            cgi,
            ..Default::default()
        };
        for p in no_proxy.split(',') {
            let p = p.trim().to_ascii_lowercase();
            if p.is_empty() {
                continue;
            }
            if p == "*" {
                c.all = true;
                return c;
            }
            if let Some((ip, bits)) = parse_cidr(&p) {
                c.ip_matchers.push(IpMatcher::Cidr(ip, bits));
                continue;
            }
            let (mut phost, pport) = match goutil::split_host_port(&p) {
                Ok((h, port)) => {
                    if h.is_empty() {
                        continue;
                    }
                    let h = if h.starts_with('[') && h.ends_with(']') {
                        h[1..h.len() - 1].to_string()
                    } else {
                        h
                    };
                    (h, port)
                }
                Err(_) => (p.clone(), String::new()),
            };
            if let Ok(ip) = phost.parse::<IpAddr>() {
                c.ip_matchers.push(IpMatcher::Ip(ip, pport));
                continue;
            }
            if phost.is_empty() {
                continue;
            }
            if let Some(stripped) = phost.strip_prefix("*.") {
                phost = format!(".{stripped}");
            }
            let mut match_host = false;
            if !phost.starts_with('.') {
                match_host = true;
                phost = format!(".{phost}");
            }
            c.domain_matchers.push(DomainMatcher {
                host: phost,
                port: pport,
                match_host,
            });
        }
        c
    }

    fn use_proxy(&self, addr: &str) -> bool {
        if addr.is_empty() {
            return true;
        }
        let Ok((host, port)) = goutil::split_host_port(addr) else {
            return false;
        };
        if host == "localhost" {
            return false;
        }
        let ip = host.parse::<IpAddr>().ok();
        if let Some(ip) = ip {
            let loopback = match ip {
                IpAddr::V4(v4) => v4.is_loopback(),
                IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v| v.is_loopback()),
            };
            if loopback {
                return false;
            }
        }
        if self.all {
            return false;
        }
        let addr = host.trim().to_ascii_lowercase();
        if let Some(ip) = ip {
            for m in &self.ip_matchers {
                let matched = match m {
                    IpMatcher::Cidr(net, bits) => cidr_contains(*net, *bits, ip),
                    IpMatcher::Ip(mip, mport) => {
                        ip_equal(*mip, ip) && (mport.is_empty() || *mport == port)
                    }
                };
                if matched {
                    return false;
                }
            }
        }
        for m in &self.domain_matchers {
            if ip.is_some() {
                continue;
            }
            if (addr.ends_with(&m.host) || (m.match_host && addr == m.host[1..]))
                && (m.port.is_empty() || m.port == port)
            {
                return false;
            }
        }
        true
    }

    /// `proxyForURL`: the proxy to use for a request URL, if any.
    pub fn proxy_for_url(&self, u: &url::Url) -> Option<url::Url> {
        let proxy = match u.scheme() {
            "https" => self.https_proxy.as_ref(),
            "http" => {
                if self.http_proxy.is_some() && self.cgi {
                    return None;
                }
                self.http_proxy.as_ref()
            }
            _ => None,
        }?;
        let host = u.host_str().unwrap_or("");
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let port = u
            .port()
            .map(|p| p.to_string())
            .unwrap_or_else(|| match u.scheme() {
                "http" => "80".to_string(),
                "https" => "443".to_string(),
                _ => String::new(),
            });
        if !self.use_proxy(&goutil::join_host_port(host, &port)) {
            return None;
        }
        Some(proxy.clone())
    }
}

// ------------------------------------------------------------------ client

static PROXY: LazyLock<ProxyConfig> = LazyLock::new(ProxyConfig::from_env);

static CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(build_client);

fn build_client() -> Result<reqwest::Client, String> {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    roots.add_parsable_certificates(native.certs);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .proxy(reqwest::Proxy::custom(|u: &url::Url| PROXY.proxy_for_url(u)))
        .connect_timeout(Duration::from_secs(30))
        .tcp_keepalive(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(50)
        .build()
        .map_err(|e| e.to_string())
}

/// Go's default `User-Agent` of `net/http` when none is set.
const GO_HTTP_USER_AGENT: &str = "Go-http-client/1.1";

/// The base transport (Go `remote.DefaultTransport`).
#[derive(Debug, Default, Clone, Copy)]
pub struct HttpTransport;

/// Content length the way Go's `Response.ContentLength` reports it.
fn content_length(status: u16, headers: &http::HeaderMap) -> i64 {
    if status == 204 || status == 304 || (100..200).contains(&status) {
        return 0;
    }
    if headers
        .get_all(http::header::TRANSFER_ENCODING)
        .iter()
        .any(|v| v.to_str().is_ok_and(|s| s.to_ascii_lowercase().contains("chunked")))
    {
        return -1;
    }
    headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(-1)
}

#[async_trait]
impl RoundTripper for HttpTransport {
    async fn round_trip(&self, mut req: Request) -> anyhow::Result<Response> {
        let client = CLIENT
            .as_ref()
            .map_err(|e| anyhow::anyhow!("building HTTP client: {e}"))?;
        let url_str = req.url.string();
        let url = reqwest::Url::parse(&url_str)
            .map_err(|e| NetError::new(format!("unsupported URL {url_str}: {e}")))?;
        if !req.headers.contains_key(http::header::USER_AGENT) {
            req.headers.insert(
                http::header::USER_AGENT,
                HeaderValue::from_static(GO_HTTP_USER_AGENT),
            );
        }
        let mut rb = client.request(req.method.clone(), url).headers(req.headers.clone());
        if let Some(b) = &req.body {
            rb = rb.body(b.clone());
        }
        let resp = rb.send().await.map_err(|e| map_reqwest_error(&e, &req.url))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let cl = content_length(status, &headers);
        let body = if req.method == http::Method::HEAD {
            Body::Empty
        } else {
            Body::Stream(resp)
        };
        Ok(Response {
            status,
            headers,
            content_length: cl,
            body,
            request: req,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn proxy_env_semantics() {
        let c = ProxyConfig::new(
            "http://proxy:3128",
            "proxy.example.com:8443",
            "example.com, .internal,10.0.0.0/8, 192.168.1.1, *.wild.org, foo.com:5000",
            false,
        );
        assert_eq!(
            c.proxy_for_url(&u("https://quay.io/v2/")).unwrap().as_str(),
            "http://proxy.example.com:8443/"
        );
        assert_eq!(
            c.proxy_for_url(&u("http://quay.io/v2/")).unwrap().as_str(),
            "http://proxy:3128/"
        );
        assert!(c.proxy_for_url(&u("https://localhost:5000/")).is_none());
        assert!(c.proxy_for_url(&u("https://127.0.0.1/")).is_none());
        assert!(c.proxy_for_url(&u("https://[::1]/")).is_none());
        assert!(c.proxy_for_url(&u("https://example.com/")).is_none());
        assert!(c.proxy_for_url(&u("https://sub.example.com/")).is_none());
        assert!(c.proxy_for_url(&u("https://notexample.com/")).is_some());
        assert!(c.proxy_for_url(&u("https://a.internal/")).is_none());
        assert!(c.proxy_for_url(&u("https://internal/")).is_some());
        assert!(c.proxy_for_url(&u("https://10.2.3.4/")).is_none());
        assert!(c.proxy_for_url(&u("https://192.168.1.1/")).is_none());
        assert!(c.proxy_for_url(&u("https://192.168.1.2/")).is_some());
        assert!(c.proxy_for_url(&u("https://x.wild.org/")).is_none());
        assert!(c.proxy_for_url(&u("https://wild.org/")).is_some());
        assert!(c.proxy_for_url(&u("https://foo.com:5000/")).is_none());
        assert!(c.proxy_for_url(&u("https://foo.com/")).is_some());

        let all = ProxyConfig::new("", "http://p:1", "*", false);
        assert!(all.proxy_for_url(&u("https://quay.io/")).is_none());
        let cgi = ProxyConfig::new("http://p:1", "", "", true);
        assert!(cgi.proxy_for_url(&u("http://quay.io/")).is_none());
    }
}
