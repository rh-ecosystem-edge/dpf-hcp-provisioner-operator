//! Port of `github.com/google/go-containerregistry/pkg/v1/remote/transport`.
//!
//! Go's `http.RoundTripper` chain is reproduced with the [`RoundTripper`] trait:
//! `bearer|basic -> scheme -> user-agent -> retry -> HTTP`, and [`Client`] mirrors
//! `http.Client` (redirect following, `*url.Error` wrapping).

mod bearer;
mod challenge;
pub mod error;
pub mod http;
mod ping;
mod retry;

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use ::http::{HeaderMap, HeaderValue, Method};
use tokio::io::AsyncRead;

use crate::authn::Authenticator;
use crate::gourl::GoUrl;
use crate::goutil;
use crate::name::Registry;

pub use bearer::Token;
pub use challenge::{Challenge as AuthChallenge, parse_www_authenticate, response_challenges};
pub use error::{Diagnostic, Error, RequestInfo, check_error, codes};
pub use http::{HttpTransport, NetError, ProxyConfig};
pub use ping::{Challenge, MultiError, ping};
pub use retry::{Backoff, Predicate, RetryTransport, default_retry_predicate};

/// `transport.PullScope`
pub const PULL_SCOPE: &str = "pull";
/// `transport.PushScope`
pub const PUSH_SCOPE: &str = "push,pull";
/// `transport.CatalogScope`
pub const CATALOG_SCOPE: &str = "catalog";

/// go-containerregistry's `defaultUserAgent`.
pub const DEFAULT_USER_AGENT: &str = "go-containerregistry";
/// The go-containerregistry module version vendored by the Go operator.
pub const GGCR_VERSION: &str = "v0.21.2";

/// `ggcrVersion`: the User-Agent go-containerregistry sends.
pub fn ggcr_user_agent() -> String {
    format!("{DEFAULT_USER_AGENT}/{GGCR_VERSION}")
}

/// Boxed async reader for response / blob bodies.
pub type BoxAsyncRead = Pin<Box<dyn AsyncRead + Send + Unpin>>;

/// An outgoing request (Go `*http.Request` subset).
#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: GoUrl,
    /// Go `Request.Host` (set from the URL by `http.NewRequest`, empty on redirects).
    pub host: String,
    pub headers: HeaderMap,
    pub body: Option<Bytes>,
}

impl Request {
    /// `http.NewRequest(method, url, body)`
    pub fn new(method: Method, url: &str) -> anyhow::Result<Request> {
        let u = GoUrl::parse(url)?;
        Ok(Request {
            method,
            host: u.host.clone(),
            url: u,
            headers: HeaderMap::new(),
            body: None,
        })
    }

    /// `Header.Set(k, v)`
    pub fn set_header(&mut self, k: ::http::header::HeaderName, v: &str) {
        if let Ok(v) = HeaderValue::from_str(v) {
            self.headers.insert(k, v);
        }
    }
}

/// Response body.
pub enum Body {
    Empty,
    Bytes(Bytes),
    Stream(reqwest::Response),
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Body::Empty => f.write_str("Body::Empty"),
            Body::Bytes(b) => write!(f, "Body::Bytes({} bytes)", b.len()),
            Body::Stream(_) => f.write_str("Body::Stream"),
        }
    }
}

fn reqwest_body_error(e: reqwest::Error) -> std::io::Error {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(&e);
    while let Some(c) = cur {
        if let Some(io) = c.downcast_ref::<std::io::Error>() {
            return std::io::Error::new(io.kind(), NetError {
                message: http::go_io_message(io),
                retryable: true,
                timeout: false,
            });
        }
        let s = c.to_string();
        if s.contains("IncompleteBody")
            || s.contains("end of file before message length reached")
            || s.contains("connection closed before message completed")
            || s.contains("incomplete")
        {
            return std::io::Error::new(std::io::ErrorKind::UnexpectedEof, NetError {
                message: "unexpected EOF".to_string(),
                retryable: true,
                timeout: false,
            });
        }
        cur = c.source();
    }
    std::io::Error::other(NetError::new(e.to_string()))
}

impl Body {
    /// `io.ReadAll(resp.Body)`; the body is left empty afterwards.
    pub async fn read_all(&mut self) -> std::io::Result<Bytes> {
        match std::mem::replace(self, Body::Empty) {
            Body::Empty => Ok(Bytes::new()),
            Body::Bytes(b) => Ok(b),
            Body::Stream(r) => {
                let mut out = Vec::new();
                let mut stream = r.bytes_stream();
                while let Some(chunk) = stream.try_next().await.map_err(reqwest_body_error)? {
                    out.extend_from_slice(&chunk);
                }
                Ok(Bytes::from(out))
            }
        }
    }

    /// `io.ReadAll(io.LimitReader(resp.Body, limit))`
    pub async fn read_limited(&mut self, limit: usize) -> std::io::Result<Bytes> {
        match std::mem::replace(self, Body::Empty) {
            Body::Empty => Ok(Bytes::new()),
            Body::Bytes(b) => Ok(b.slice(..b.len().min(limit))),
            Body::Stream(r) => {
                let mut out = Vec::new();
                let mut stream = r.bytes_stream();
                while out.len() < limit {
                    match stream.try_next().await.map_err(reqwest_body_error)? {
                        Some(chunk) => {
                            let take = chunk.len().min(limit - out.len());
                            out.extend_from_slice(&chunk[..take]);
                        }
                        None => break,
                    }
                }
                Ok(Bytes::from(out))
            }
        }
    }

    /// Stream the body as an `AsyncRead`.
    pub fn into_async_read(self) -> BoxAsyncRead {
        match self {
            Body::Empty => Box::pin(std::io::Cursor::new(Bytes::new())),
            Body::Bytes(b) => Box::pin(std::io::Cursor::new(b)),
            Body::Stream(r) => Box::pin(tokio_util::io::StreamReader::new(
                r.bytes_stream().map_err(reqwest_body_error),
            )),
        }
    }
}

/// An HTTP response (Go `*http.Response` subset).
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    /// Go `Response.ContentLength` (-1 when unknown).
    pub content_length: i64,
    pub body: Body,
    /// The request that produced this response (Go `Response.Request`).
    pub request: Request,
}

impl Response {
    /// `Header.Get(k)`
    pub fn header(&self, k: &str) -> String {
        self.headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }
}

/// Go `http.RoundTripper`.
#[async_trait]
pub trait RoundTripper: Send + Sync {
    async fn round_trip(&self, req: Request) -> anyhow::Result<Response>;

    /// `t.(*userAgentTransport)`
    fn is_user_agent(&self) -> bool {
        false
    }

    /// `t.(*transport.Wrapper)`
    fn is_wrapper(&self) -> bool {
        false
    }
}

#[async_trait]
impl<T: RoundTripper + ?Sized> RoundTripper for Arc<T> {
    async fn round_trip(&self, req: Request) -> anyhow::Result<Response> {
        (**self).round_trip(req).await
    }
    fn is_user_agent(&self) -> bool {
        (**self).is_user_agent()
    }
    fn is_wrapper(&self) -> bool {
        (**self).is_wrapper()
    }
}

/// Go `*url.Error` as produced by `http.Client.Do`.
#[derive(Debug)]
pub struct UrlError {
    pub op: String,
    pub url: String,
    pub err: anyhow::Error,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.op, goutil::quote(&self.url), self.err)
    }
}

impl std::error::Error for UrlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.err.as_ref())
    }
}

fn url_error_op(method: &Method) -> String {
    let m = method.as_str();
    if m.is_empty() {
        return "Get".to_string();
    }
    let mut out = m[..1].to_string();
    out.push_str(&m[1..].to_ascii_lowercase());
    out
}

/// Whether a redirect target may receive the original sensitive headers
/// (Go `shouldCopyHeaderOnRedirect`).
fn is_domain_or_subdomain(sub: &str, parent: &str) -> bool {
    if sub == parent {
        return true;
    }
    if sub.contains([':', '%']) {
        return false;
    }
    sub.len() > parent.len() && sub.ends_with(parent) && sub.as_bytes()[sub.len() - parent.len() - 1] == b'.'
}

/// Go `http.Client` with a custom transport.
#[derive(Clone)]
pub struct Client {
    pub transport: Arc<dyn RoundTripper>,
}

impl Client {
    pub fn new(transport: Arc<dyn RoundTripper>) -> Self {
        Client { transport }
    }

    /// `client.Do(req)`, following up to 10 redirects.
    pub async fn do_request(&self, ireq: Request) -> anyhow::Result<Response> {
        let op = url_error_op(&ireq.method);
        let initial_headers = ireq.headers.clone();
        let initial_url = ireq.url.clone();
        let mut req = ireq.clone();
        let mut include_body = true;
        let mut via = 0usize;
        loop {
            let url_str = req.url.strip_password();
            let resp = match self.transport.round_trip(req.clone()).await {
                Ok(r) => r,
                Err(e) => {
                    return Err(UrlError {
                        op,
                        url: url_str,
                        err: e,
                    }
                    .into());
                }
            };
            let (redirect_method, should_redirect, include_on_hop) = match resp.status {
                301..=303 => {
                    let m = if req.method != Method::GET && req.method != Method::HEAD {
                        Method::GET
                    } else {
                        req.method.clone()
                    };
                    (m, true, false)
                }
                307 | 308 => (req.method.clone(), true, true),
                _ => (req.method.clone(), false, false),
            };
            if !should_redirect {
                return Ok(resp);
            }
            if !include_on_hop {
                include_body = false;
            }
            let loc = resp.header("Location");
            if loc.is_empty() {
                return Ok(resp);
            }
            let final_url = resp.request.url.strip_password();
            let u = match req.url.resolve(&loc) {
                Ok(u) => u,
                Err(e) => {
                    return Err(UrlError {
                        op,
                        url: final_url,
                        err: anyhow::anyhow!(
                            "failed to parse Location header {}: {}",
                            goutil::quote(&loc),
                            e.err
                        ),
                    }
                    .into());
                }
            };
            let mut headers = HeaderMap::new();
            let copy_sensitive = is_domain_or_subdomain(
                &u.hostname().to_ascii_lowercase(),
                &initial_url.hostname().to_ascii_lowercase(),
            );
            for (k, v) in initial_headers.iter() {
                let sensitive = matches!(
                    k.as_str(),
                    "authorization" | "www-authenticate" | "cookie" | "cookie2"
                );
                if sensitive && !copy_sensitive {
                    continue;
                }
                headers.append(k.clone(), v.clone());
            }
            // Referer from the most recent request, unless https -> http.
            if !(req.url.scheme == "https" && u.scheme == "http") {
                let mut r = req.url.clone();
                r.user = None;
                r.fragment.clear();
                if let Ok(v) = HeaderValue::from_str(&r.string()) {
                    headers.insert(::http::header::REFERER, v);
                }
            }
            via += 1;
            if via >= 10 {
                return Err(UrlError {
                    op,
                    url: loc,
                    err: anyhow::anyhow!("stopped after 10 redirects"),
                }
                .into());
            }
            req = Request {
                method: redirect_method,
                url: u,
                host: String::new(),
                headers,
                body: if include_body { ireq.body.clone() } else { None },
            };
        }
    }
}

// ------------------------------------------------------------------ simple transports

/// `userAgentTransport`
pub struct UserAgentTransport {
    inner: Arc<dyn RoundTripper>,
    ua: String,
}

impl UserAgentTransport {
    /// `transport.NewUserAgent(inner, ua)`
    pub fn new(inner: Arc<dyn RoundTripper>, ua: &str) -> Self {
        let ua = if ua.is_empty() {
            ggcr_user_agent()
        } else {
            format!("{ua} {}", ggcr_user_agent())
        };
        UserAgentTransport { inner, ua }
    }
}

#[async_trait]
impl RoundTripper for UserAgentTransport {
    async fn round_trip(&self, mut req: Request) -> anyhow::Result<Response> {
        req.set_header(::http::header::USER_AGENT, &self.ua);
        self.inner.round_trip(req).await
    }
    fn is_user_agent(&self) -> bool {
        true
    }
}

/// `schemeTransport`
struct SchemeTransport {
    scheme: &'static str,
    registry: Registry,
    inner: Arc<dyn RoundTripper>,
}

#[async_trait]
impl RoundTripper for SchemeTransport {
    async fn round_trip(&self, mut req: Request) -> anyhow::Result<Response> {
        if matches_host(&self.registry.to_string(), &req, self.scheme) {
            req.url.scheme = self.scheme.to_string();
        }
        self.inner.round_trip(req).await
    }
}

/// `basicTransport`
pub(crate) struct BasicTransport {
    inner: Arc<dyn RoundTripper>,
    auth: Arc<dyn Authenticator>,
    target: String,
}

#[async_trait]
impl RoundTripper for BasicTransport {
    async fn round_trip(&self, mut req: Request) -> anyhow::Result<Response> {
        if !self.auth.is_anonymous() {
            let auth = self.auth.authorization()?;
            if req.host == self.target || req.url.host == self.target {
                if !auth.registry_token.is_empty() {
                    req.set_header(
                        ::http::header::AUTHORIZATION,
                        &format!("Bearer {}", auth.registry_token),
                    );
                } else if !auth.username.is_empty() && !auth.password.is_empty() {
                    let enc = crate::gobase64::encode_std(
                        format!("{}:{}", auth.username, auth.password).as_bytes(),
                    );
                    req.set_header(::http::header::AUTHORIZATION, &format!("Basic {enc}"));
                } else if !auth.auth.is_empty() {
                    req.set_header(::http::header::AUTHORIZATION, &format!("Basic {}", auth.auth));
                }
            }
        }
        self.inner.round_trip(req).await
    }
}

/// `transport.Wrapper`
pub struct Wrapper {
    inner: Arc<dyn RoundTripper>,
}

impl Wrapper {
    pub fn new(inner: Arc<dyn RoundTripper>) -> Self {
        Wrapper { inner }
    }
}

#[async_trait]
impl RoundTripper for Wrapper {
    async fn round_trip(&self, req: Request) -> anyhow::Result<Response> {
        self.inner.round_trip(req).await
    }
    fn is_wrapper(&self) -> bool {
        true
    }
}

fn port_for_scheme(scheme: &str) -> &'static str {
    match scheme {
        "http" => "80",
        "https" => "443",
        _ => "",
    }
}

/// `canonicalAddress`
fn canonical_address(host: &str, scheme: &str) -> String {
    let colons = host.matches(':').count();
    if colons == 1 || (colons >= 2 && host.contains("]:")) {
        let Ok((hostname, mut port)) = goutil::split_host_port(host) else {
            return host.to_string();
        };
        if port.is_empty() {
            port = port_for_scheme(scheme).to_string();
        }
        return goutil::join_host_port(&hostname, &port);
    }
    goutil::join_host_port(host, port_for_scheme(scheme))
}

/// `matchesHost`
fn matches_host(host: &str, req: &Request, scheme: &str) -> bool {
    let canonical_header_host = canonical_address(&req.host, scheme);
    let canonical_url_host = canonical_address(&req.url.host, scheme);
    let canonical_registry_host = canonical_address(host, scheme);
    canonical_header_host == canonical_registry_host || canonical_url_host == canonical_registry_host
}

/// `transport.NewWithContext`: ping the registry and return a transport that
/// authenticates according to the challenge.
pub async fn new_with_context(
    reg: &Registry,
    auth: Arc<dyn Authenticator>,
    t: Arc<dyn RoundTripper>,
    scopes: Vec<String>,
) -> anyhow::Result<Arc<dyn RoundTripper>> {
    if t.is_wrapper() {
        return Ok(t);
    }
    let pr = ping(reg, t.clone()).await?;

    let t: Arc<dyn RoundTripper> = if t.is_user_agent() {
        t
    } else {
        Arc::new(UserAgentTransport::new(t, ""))
    };

    let scheme = if pr.insecure { "http" } else { "https" };
    let t: Arc<dyn RoundTripper> = Arc::new(SchemeTransport {
        scheme,
        registry: reg.clone(),
        inner: t,
    });

    if !pr.scheme.eq_ignore_ascii_case("bearer") {
        return Ok(Arc::new(Wrapper::new(Arc::new(BasicTransport {
            inner: t,
            auth,
            target: reg.registry_str().to_string(),
        }))));
    }

    let bt = bearer::BearerTransport::from_challenge(reg, auth, t, &pr, scopes)?;
    bt.refresh().await?;
    Ok(Arc::new(Wrapper::new(Arc::new(bt))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical() {
        assert_eq!(canonical_address("quay.io", "https"), "quay.io:443");
        assert_eq!(canonical_address("quay.io:443", "https"), "quay.io:443");
        assert_eq!(canonical_address("localhost:5000", "http"), "localhost:5000");
        assert_eq!(canonical_address("", "https"), ":443");
        assert_eq!(canonical_address("[::1]:5000", "https"), "[::1]:5000");
        assert_eq!(canonical_address("::1", "https"), "[::1]:443");
        assert_eq!(url_error_op(&Method::HEAD), "Head");
        assert!(is_domain_or_subdomain("a.quay.io", "quay.io"));
        assert!(!is_domain_or_subdomain("aquay.io", "quay.io"));
    }
}
