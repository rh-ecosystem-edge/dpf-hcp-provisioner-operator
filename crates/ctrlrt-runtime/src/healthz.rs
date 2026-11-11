//! Health probes: port of controller-runtime's `pkg/healthz` handler and the manager's health
//! probe server (`addHealthProbeServer`: Go `http.ServeMux` + `http.StripPrefix`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bytes::Bytes;
use ctrlrt::{Logger, kv};
use http::{HeaderValue, Request, Response, StatusCode};
use http_body_util::Full;
use serde_json::json;

/// `healthz.Checker`: returns an error message on failure.
pub type Checker = Arc<dyn Fn(&Request<()>) -> std::result::Result<(), String> + Send + Sync>;

/// `healthz.Ping`
pub fn ping() -> Checker {
    Arc::new(|_req| Ok(()))
}

fn log() -> Logger {
    Logger::root()
        .with_name("controller-runtime")
        .with_name("healthz")
}

/// `healthz.Handler`
#[derive(Clone, Default)]
pub struct Handler {
    pub checks: BTreeMap<String, Checker>,
}

struct CheckStatus {
    name: String,
    healthy: bool,
    excluded: bool,
}

type Resp = Response<Full<Bytes>>;

fn text_response(status: StatusCode, body: String, extra_nosniff: bool) -> Resp {
    let mut r = Response::new(Full::new(Bytes::from(body)));
    *r.status_mut() = status;
    r.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    if extra_nosniff {
        r.headers_mut().insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
    }
    r
}

/// `url.ParseQuery` (subset: repeated keys, `+` and percent decoding).
fn parse_query(q: Option<&str>) -> Vec<(String, String)> {
    match q {
        None => Vec::new(),
        Some(q) => form_urlencoded::parse(q.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect(),
    }
}

/// Go's `path.Clean`.
pub fn clean_path(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let rooted = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if !out.is_empty() && *out.last().unwrap_or(&"") != ".." {
                    out.pop();
                } else if !rooted {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if rooted {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".into()
    } else {
        joined
    }
}

impl Handler {
    pub fn new() -> Self {
        Self::default()
    }

    fn serve_aggregated(&self, req: &Request<()>) -> Resp {
        let query = parse_query(req.uri().query());
        let mut excluded: BTreeSet<String> = query
            .iter()
            .filter(|(k, _)| k == "exclude")
            .map(|(_, v)| v.clone())
            .collect();
        let force_verbose = query.iter().any(|(k, _)| k == "verbose");
        let mut failed = false;
        let mut parts: Vec<CheckStatus> = Vec::with_capacity(self.checks.len());
        for (name, check) in &self.checks {
            if excluded.remove(name) {
                parts.push(CheckStatus {
                    name: name.clone(),
                    healthy: true,
                    excluded: true,
                });
                continue;
            }
            match check(req) {
                Err(e) => {
                    log().v(1).info(
                        "healthz check failed",
                        kv!["checker" => name, "error" => e],
                    );
                    parts.push(CheckStatus {
                        name: name.clone(),
                        healthy: false,
                        excluded: false,
                    });
                    failed = true;
                }
                Ok(()) => parts.push(CheckStatus {
                    name: name.clone(),
                    healthy: true,
                    excluded: false,
                }),
            }
        }
        if self.checks.is_empty() {
            parts.push(CheckStatus {
                name: "ping".into(),
                healthy: true,
                excluded: false,
            });
        }
        for c in &excluded {
            log().v(1).info(
                "cannot exclude health check, no matches for it",
                kv!["checker" => c],
            );
        }
        parts.sort_by(|a, b| a.name.cmp(&b.name));
        write_statuses_as_text(&parts, &excluded, failed, force_verbose)
    }

    /// `ServeHTTP` for a request whose path already had the endpoint prefix stripped.
    pub fn serve(&self, req: &Request<()>, stripped_path: &str) -> Resp {
        let mut req_path = stripped_path.to_string();
        if req_path.is_empty() || !req_path.starts_with('/') {
            req_path = format!("/{req_path}");
        }
        let req_path = clean_path(&req_path);
        if req_path == "/" {
            return self.serve_aggregated(req);
        }
        let name = &req_path[1..];
        if self.checks.is_empty() && name == "ping" {
            return check_handler(&ping(), req);
        }
        match self.checks.get(name) {
            None => not_found(),
            Some(c) => check_handler(c, req),
        }
    }
}

fn write_statuses_as_text(
    parts: &[CheckStatus],
    unknown_excludes: &BTreeSet<String>,
    failed: bool,
    force_verbose: bool,
) -> Resp {
    let status = if failed {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::OK
    };
    if !failed && !force_verbose {
        return text_response(status, "ok".into(), true);
    }
    let mut body = String::new();
    for c in parts {
        if c.excluded {
            body.push_str(&format!("[+]{} excluded: ok\n", c.name));
        } else if c.healthy {
            body.push_str(&format!("[+]{} ok\n", c.name));
        } else {
            body.push_str(&format!("[-]{} failed: reason withheld\n", c.name));
        }
    }
    if !unknown_excludes.is_empty() {
        let quoted: Vec<String> = unknown_excludes
            .iter()
            .map(|n| ctrlrt::error::go_quote(n))
            .collect();
        body.push_str(&format!(
            "warn: some health checks cannot be excluded: no matches for {}\n",
            quoted.join(",")
        ));
    }
    if failed {
        // checkStatus has only unexported fields: zap renders each as `{}`.
        let statuses: Vec<serde_json::Value> = parts.iter().map(|_| json!({})).collect();
        log().info("healthz check failed", kv!["statuses" => statuses]);
        body.push_str("healthz check failed\n");
    } else {
        body.push_str("healthz check passed\n");
    }
    text_response(status, body, true)
}

/// `healthz.CheckHandler`
fn check_handler(c: &Checker, req: &Request<()>) -> Resp {
    match c(req) {
        Err(e) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("internal server error: {e}\n"),
            true,
        ),
        Ok(()) => text_response(StatusCode::OK, "ok".into(), false),
    }
}

/// `http.NotFoundHandler()`
fn not_found() -> Resp {
    text_response(StatusCode::NOT_FOUND, "404 page not found\n".into(), true)
}

/// The probe server mux: `/readyz`, `/readyz/`, `/healthz`, `/healthz/` (Go `ServeMux`
/// semantics incl. redirects to the cleaned path).
#[derive(Clone)]
pub struct ProbeMux {
    pub readiness_endpoint: String,
    pub liveness_endpoint: String,
    pub readyz: Option<Handler>,
    pub healthz: Option<Handler>,
}

impl ProbeMux {
    pub fn serve(&self, req: &Request<()>) -> Resp {
        let path = req.uri().path();
        // ServeMux redirects unclean paths (keeping a trailing slash).
        let mut cleaned = clean_path(if path.is_empty() { "/" } else { path });
        if path.ends_with('/') && cleaned != "/" {
            cleaned.push('/');
        }
        if cleaned != path {
            let mut loc = cleaned;
            if let Some(q) = req.uri().query() {
                loc.push('?');
                loc.push_str(q);
            }
            let mut r = Response::new(Full::new(Bytes::from(format!(
                "<a href=\"{}\">Moved Permanently</a>.\n\n",
                html_escape(&loc)
            ))));
            *r.status_mut() = StatusCode::MOVED_PERMANENTLY;
            if let Ok(v) = HeaderValue::from_str(&loc) {
                r.headers_mut().insert(http::header::LOCATION, v);
            }
            r.headers_mut().insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            return r;
        }
        for (ep, h) in [
            (&self.readiness_endpoint, &self.readyz),
            (&self.liveness_endpoint, &self.healthz),
        ] {
            let Some(h) = h else { continue };
            if path == ep.as_str() {
                return h.serve(req, "");
            }
            let prefix = format!("{ep}/");
            if path.starts_with(&prefix) {
                return h.serve(req, &path[ep.len()..]);
            }
        }
        not_found()
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    fn req(uri: &str) -> Request<()> {
        Request::builder().uri(uri).body(()).unwrap()
    }

    async fn body(r: Resp) -> (u16, String) {
        let status = r.status().as_u16();
        let b = r.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(b.to_vec()).unwrap())
    }

    fn mux(checks: Vec<(&str, Checker)>) -> ProbeMux {
        let mut h = Handler::new();
        for (n, c) in checks {
            h.checks.insert(n.into(), c);
        }
        ProbeMux {
            readiness_endpoint: "/readyz".into(),
            liveness_endpoint: "/healthz".into(),
            readyz: Some(h.clone()),
            healthz: Some(h),
        }
    }

    fn failing() -> Checker {
        Arc::new(|_| Err("not ready".into()))
    }

    // healthz_test.go: "should return healthy if all checks succeed"
    #[tokio::test]
    async fn aggregated_ok_and_verbose() {
        let m = mux(vec![("ok1", ping()), ("ok2", ping())]);
        assert_eq!(body(m.serve(&req("/healthz"))).await, (200, "ok".into()));
        assert_eq!(body(m.serve(&req("/healthz/"))).await, (200, "ok".into()));
        assert_eq!(
            body(m.serve(&req("/healthz?verbose=true"))).await,
            (200, "[+]ok1 ok\n[+]ok2 ok\nhealthz check passed\n".into())
        );
    }

    // "should return unhealthy if at least one check fails"
    #[tokio::test]
    async fn aggregated_failure_is_verbose() {
        let m = mux(vec![("ok1", ping()), ("bad1", failing())]);
        assert_eq!(
            body(m.serve(&req("/readyz"))).await,
            (
                500,
                "[-]bad1 failed: reason withheld\n[+]ok1 ok\nhealthz check failed\n".into()
            )
        );
    }

    // "should ignore excluded checks when determining health"
    #[tokio::test]
    async fn excluded_checks() {
        let m = mux(vec![("ok1", ping()), ("bad1", failing())]);
        assert_eq!(
            body(m.serve(&req("/readyz?exclude=bad1"))).await,
            (200, "ok".into())
        );
        assert_eq!(
            body(m.serve(&req("/readyz?exclude=bad1&exclude=nope&verbose"))).await,
            (
                200,
                "[+]bad1 excluded: ok\n[+]ok1 ok\nwarn: some health checks cannot be excluded: no matches for \"nope\"\nhealthz check passed\n".into()
            )
        );
    }

    // "should be fine if asked to check a single check"
    #[tokio::test]
    async fn individual_checks() {
        let m = mux(vec![("ok1", ping()), ("bad1", failing())]);
        assert_eq!(body(m.serve(&req("/readyz/ok1"))).await, (200, "ok".into()));
        assert_eq!(
            body(m.serve(&req("/readyz/bad1"))).await,
            (500, "internal server error: not ready\n".into())
        );
        assert_eq!(
            body(m.serve(&req("/readyz/missing"))).await,
            (404, "404 page not found\n".into())
        );
    }

    // "should return healthy if there are no checks" / default ping
    #[tokio::test]
    async fn no_checks_defaults_to_ping() {
        let m = mux(vec![]);
        assert_eq!(
            body(m.serve(&req("/healthz?verbose"))).await,
            (200, "[+]ping ok\nhealthz check passed\n".into())
        );
        assert_eq!(body(m.serve(&req("/healthz/ping"))).await, (200, "ok".into()));
    }

    #[tokio::test]
    async fn other_paths_and_redirects() {
        let m = mux(vec![("readyz", ping())]);
        assert_eq!(body(m.serve(&req("/metrics"))).await.0, 404);
        assert_eq!(body(m.serve(&req("/"))).await.0, 404);
        let r = m.serve(&req("//readyz"));
        assert_eq!(r.status(), 301);
        assert_eq!(r.headers()[http::header::LOCATION], "/readyz");
    }

    #[test]
    fn clean_path_like_go() {
        assert_eq!(clean_path("/a/../b"), "/b");
        assert_eq!(clean_path("/a//b/"), "/a/b");
        assert_eq!(clean_path("/"), "/");
        assert_eq!(clean_path(""), ".");
        assert_eq!(clean_path("/.."), "/");
    }
}
