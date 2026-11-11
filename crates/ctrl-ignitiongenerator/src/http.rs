//! The `net/http` client of `downloadHCPIgnition`:
//!
//! ```go
//! certPool := x509.NewCertPool()
//! if !certPool.AppendCertsFromPEM(caCert) { ... "failed to parse CA certificate" }
//! httpClient := &http.Client{
//!     Timeout:   httpClientTimeout,
//!     Transport: &http.Transport{TLSClientConfig: &tls.Config{RootCAs: certPool, MinVersion: tls.VersionTLS12}},
//! }
//! req, err := http.NewRequestWithContext(ctx, http.MethodGet, ignitionURL, nil)
//! req.Header.Set("Authorization", fmt.Sprintf("Bearer %s", token))
//! resp, err := httpClient.Do(req)
//! ```
//!
//! Mirrored properties: only the given CA pool is trusted (no system roots), TLS 1.2+, no
//! proxy (a custom `http.Transport` has no `Proxy` func), HTTP/1.1 only (a custom transport
//! with a `TLSClientConfig` does not negotiate h2), Go's default `User-Agent`, redirects
//! followed (at most 10), a 30s whole-request timeout, cancellation through the context.
//! `Do` errors are rendered like Go's `*url.Error` (`Get "<url>": <cause>`) with the cause
//! mapped to Go's wording for the common cases (connection refused, timeouts, unknown
//! certificate authority, host name mismatch, DNS failure, EOF).

use std::sync::Arc;
use std::time::Duration;

use ctrlrt::Ctx;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

/// Go's default `User-Agent` of `net/http`.
const GO_HTTP_USER_AGENT: &str = "Go-http-client/1.1";

/// The configured HTTPS client (`*http.Client`).
pub(crate) struct HttpClient {
    client: reqwest::Client,
}

/// Errors of `http.NewRequestWithContext` / `httpClient.Do`.
#[derive(Debug)]
pub(crate) enum GetError {
    NewRequest(anyhow::Error),
    Do(anyhow::Error),
}

/// `x509.CertPool.AppendCertsFromPEM`: adds every parsable `CERTIFICATE` PEM block; reports
/// whether at least one certificate was added.
pub(crate) fn append_certs_from_pem(pool: &mut rustls::RootCertStore, pem: &[u8]) -> bool {
    let mut ok = false;
    for cert in CertificateDer::pem_slice_iter(pem) {
        let Ok(cert) = cert else {
            break;
        };
        if pool.add(cert).is_ok() {
            ok = true;
        }
    }
    ok
}

/// Builds the client (`AppendCertsFromPEM` + `http.Client{...}`).
pub(crate) fn new_client(ca_cert: &[u8], timeout: Duration) -> anyhow::Result<HttpClient> {
    let mut roots = rustls::RootCertStore::empty();
    if !append_certs_from_pem(&mut roots, ca_cert) {
        return Err(anyhow::anyhow!("failed to parse CA certificate"));
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|e| anyhow::anyhow!("{}", e))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .no_proxy()
        .http1_only()
        .timeout(timeout)
        .user_agent(GO_HTTP_USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(HttpClient { client })
}

/// Go's `*url.Error`: `<Op> "<URL>": <Err>`.
fn url_error(url: &str, cause: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("Get {}: {}", gocompat::strconv::quote(url), cause)
}

impl HttpClient {
    /// `http.NewRequestWithContext(ctx, GET, url, nil)` + `Authorization: Bearer <token>` +
    /// `httpClient.Do(req)`.
    pub(crate) async fn get(
        &self,
        ctx: &Ctx,
        url: &str,
        token: &str,
    ) -> Result<reqwest::Response, GetError> {
        let parsed = reqwest::Url::parse(url).map_err(|e| {
            GetError::NewRequest(anyhow::anyhow!(
                "parse {}: {}",
                gocompat::strconv::quote(url),
                e
            ))
        })?;
        let fut = self
            .client
            .get(parsed.clone())
            .header("Authorization", format!("Bearer {token}"))
            .send();
        let res = tokio::select! {
            r = fut => r,
            _ = ctx.cancel.cancelled() => {
                return Err(GetError::Do(url_error(url, "context canceled")));
            }
        };
        res.map_err(|e| {
            let u = e.url().cloned().unwrap_or_else(|| parsed.clone());
            // Go prints the request URL as given unless the error happened on a redirect.
            let shown = if u == parsed { url } else { u.as_str() };
            GetError::Do(url_error(shown, go_net_error(&e, &u, false)))
        })
    }

    /// `io.ReadAll(resp.Body)`.
    pub(crate) async fn read_body(
        &self,
        ctx: &Ctx,
        resp: reqwest::Response,
    ) -> anyhow::Result<Vec<u8>> {
        let url = resp.url().clone();
        let res = tokio::select! {
            r = resp.bytes() => r,
            _ = ctx.cancel.cancelled() => {
                return Err(anyhow::anyhow!("context canceled"));
            }
        };
        match res {
            Ok(b) => Ok(b.to_vec()),
            Err(e) => Err(anyhow::anyhow!("{}", go_net_error(&e, &url, true))),
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
        if let Some(ioe) = c.downcast_ref::<std::io::Error>()
            && let Some(inner) = ioe.get_ref()
            && let Some(t) = find_source::<T>(inner)
        {
            return Some(t);
        }
        cur = c.source();
    }
    None
}

/// `host:port` of the URL (Go's dial address; the default port is filled in).
fn host_port(url: &reqwest::Url) -> String {
    let host = url.host_str().unwrap_or_default();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = url.port_or_known_default().unwrap_or(443);
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn hostname(url: &reqwest::Url) -> String {
    url.host_str()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

/// Go's `x509.HostnameError.Error()` from rustls' presented names
/// (`DnsName("a")`, `IpAddress(1.2.3.4)`).
fn hostname_error(host: &str, presented: &[String]) -> String {
    let mut dns = Vec::new();
    let mut ips = Vec::new();
    for p in presented {
        if let Some(n) = p
            .strip_prefix("DnsName(\"")
            .and_then(|s| s.strip_suffix("\")"))
        {
            dns.push(n.to_string());
        } else if let Some(ip) = p
            .strip_prefix("IpAddress(")
            .and_then(|s| s.strip_suffix(')'))
        {
            ips.push(ip.to_string());
        }
    }
    let valid = if host.parse::<std::net::IpAddr>().is_ok() {
        if ips.is_empty() {
            return format!(
                "x509: cannot validate certificate for {host} because it doesn't contain any IP SANs"
            );
        }
        ips.join(", ")
    } else {
        dns.join(", ")
    };
    if valid.is_empty() {
        return format!("x509: certificate is not valid for any names, but wanted to match {host}");
    }
    format!("x509: certificate is valid for {valid}, not {host}")
}

fn rfc3339(t: &rustls::pki_types::UnixTime) -> String {
    ctrlrt::k8s_openapi::jiff::Timestamp::from_second(t.as_secs() as i64)
        .map(|ts| ts.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// The Go wording of a transport error (the `Err` of the `*url.Error`).
fn go_net_error(e: &reqwest::Error, url: &reqwest::Url, reading_body: bool) -> String {
    if e.is_timeout() {
        return if reading_body {
            "context deadline exceeded (Client.Timeout or context cancellation while reading body)"
                .to_string()
        } else {
            "context deadline exceeded (Client.Timeout exceeded while awaiting headers)".to_string()
        };
    }
    if e.is_redirect() {
        return "stopped after 10 redirects".to_string();
    }
    if let Some(tls) = find_source::<rustls::Error>(e) {
        use rustls::CertificateError as CE;
        let verify = |m: String| format!("tls: failed to verify certificate: {m}");
        return match tls {
            rustls::Error::InvalidMessage(rustls::InvalidMessage::InvalidContentType) => {
                "http: server gave HTTP response to HTTPS client".to_string()
            }
            rustls::Error::InvalidCertificate(CE::UnknownIssuer) => {
                verify("x509: certificate signed by unknown authority".to_string())
            }
            rustls::Error::InvalidCertificate(CE::NotValidForNameContext { presented, .. }) => {
                verify(hostname_error(&hostname(url), presented))
            }
            rustls::Error::InvalidCertificate(CE::NotValidForName) => {
                verify(hostname_error(&hostname(url), &[]))
            }
            rustls::Error::InvalidCertificate(CE::ExpiredContext { time, not_after }) => {
                verify(format!(
                    "x509: certificate has expired or is not yet valid: current time {} is after {}",
                    rfc3339(time),
                    rfc3339(not_after)
                ))
            }
            rustls::Error::InvalidCertificate(CE::NotValidYetContext { time, not_before }) => {
                verify(format!(
                    "x509: certificate has expired or is not yet valid: current time {} is before {}",
                    rfc3339(time),
                    rfc3339(not_before)
                ))
            }
            rustls::Error::InvalidCertificate(CE::Expired | CE::NotValidYet) => {
                verify("x509: certificate has expired or is not yet valid".to_string())
            }
            other => format!("tls: {other}"),
        };
    }
    if let Some(ioe) = find_source::<std::io::Error>(e) {
        use std::io::ErrorKind as K;
        let hp = host_port(url);
        return match ioe.kind() {
            K::ConnectionRefused => format!("dial tcp {hp}: connect: connection refused"),
            K::ConnectionReset => format!("read tcp {hp}: read: connection reset by peer"),
            K::BrokenPipe => format!("write tcp {hp}: write: broken pipe"),
            K::UnexpectedEof => "EOF".to_string(),
            K::TimedOut => format!("dial tcp {hp}: i/o timeout"),
            K::HostUnreachable => format!("dial tcp {hp}: connect: no route to host"),
            K::NetworkUnreachable => format!("dial tcp {hp}: connect: network is unreachable"),
            _ => {
                let s = ioe.to_string();
                if s.contains("failed to lookup address") || s.contains("dns error") {
                    format!("dial tcp: lookup {}: no such host", hostname(url))
                } else {
                    format!("dial tcp {hp}: {s}")
                }
            }
        };
    }
    let mut parts = Vec::new();
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(c) = cur {
        parts.push(c.to_string());
        cur = c.source();
    }
    if parts.iter().any(|t| {
        t.contains("connection closed before message completed")
            || t.contains("IncompleteMessage")
            || t.contains("unexpected end of file")
    }) {
        return if reading_body {
            "unexpected EOF".to_string()
        } else {
            "EOF".to_string()
        };
    }
    if parts
        .iter()
        .any(|t| t.contains("dns error") || t.contains("failed to lookup address"))
    {
        return format!("dial tcp: lookup {}: no such host", hostname(url));
    }
    parts.last().cloned().unwrap_or_else(|| e.to_string())
}
