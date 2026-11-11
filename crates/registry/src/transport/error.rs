//! Port of `remote/transport/error.go`.

use std::fmt;

use crate::gourl::{self, GoUrl};
use crate::goutil;

use super::{Response, http::NetError};

/// `transport.ErrorCode` values defined by the distribution spec.
pub mod codes {
    pub const BLOB_UNKNOWN: &str = "BLOB_UNKNOWN";
    pub const BLOB_UPLOAD_INVALID: &str = "BLOB_UPLOAD_INVALID";
    pub const BLOB_UPLOAD_UNKNOWN: &str = "BLOB_UPLOAD_UNKNOWN";
    pub const DIGEST_INVALID: &str = "DIGEST_INVALID";
    pub const MANIFEST_BLOB_UNKNOWN: &str = "MANIFEST_BLOB_UNKNOWN";
    pub const MANIFEST_INVALID: &str = "MANIFEST_INVALID";
    pub const MANIFEST_UNKNOWN: &str = "MANIFEST_UNKNOWN";
    pub const MANIFEST_UNVERIFIED: &str = "MANIFEST_UNVERIFIED";
    pub const NAME_INVALID: &str = "NAME_INVALID";
    pub const NAME_UNKNOWN: &str = "NAME_UNKNOWN";
    pub const SIZE_INVALID: &str = "SIZE_INVALID";
    pub const TAG_INVALID: &str = "TAG_INVALID";
    pub const UNAUTHORIZED: &str = "UNAUTHORIZED";
    pub const DENIED: &str = "DENIED";
    pub const UNSUPPORTED: &str = "UNSUPPORTED";
    pub const TOO_MANY_REQUESTS: &str = "TOOMANYREQUESTS";
    pub const UNKNOWN: &str = "UNKNOWN";
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
}

/// `transport.Diagnostic`
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    /// `Detail any` — `None` for a missing / `null` detail.
    pub detail: Option<serde_json::Value>,
}

impl fmt::Display for Diagnostic {
    /// `$Code: $Message[; $Detail]`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(d) = &self.detail {
            write!(f, "; {}", goutil::go_fmt_json_value(d))?;
        }
        Ok(())
    }
}

/// The request that produced an [`Error`] (Go: `Error.Request`).
#[derive(Debug, Clone, PartialEq)]
pub struct RequestInfo {
    pub method: String,
    pub url: GoUrl,
}

/// `transport.Error` — a structured registry error response.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub errors: Vec<Diagnostic>,
    /// The HTTP status code returned.
    pub status_code: u16,
    /// The request that failed.
    pub request: Option<RequestInfo>,
    raw_body: String,
    temporary: bool,
}

impl Error {
    /// Build an error the way `makeError` does from a status code, the final request and
    /// the response body.
    pub fn from_response_parts(status: u16, request: Option<RequestInfo>, body: &[u8]) -> Error {
        Error {
            errors: parse_diagnostics(body),
            status_code: status,
            request,
            raw_body: String::from_utf8_lossy(body).into_owned(),
            temporary: false,
        }
    }

    /// The raw response body.
    pub fn raw_body(&self) -> &str {
        &self.raw_body
    }

    /// `Temporary()`
    pub fn temporary(&self) -> bool {
        if self.temporary {
            return true;
        }
        if self.errors.is_empty() {
            return matches!(self.status_code, 408 | 500 | 502 | 503 | 504);
        }
        self.errors.iter().all(|d| {
            matches!(
                d.code.as_str(),
                codes::BLOB_UPLOAD_INVALID
                    | codes::TOO_MANY_REQUESTS
                    | codes::UNKNOWN
                    | codes::UNAVAILABLE
            )
        })
    }

    pub(crate) fn set_temporary(&mut self) {
        self.temporary = true;
    }

    /// Whether any diagnostic carries `code`.
    pub fn has_code(&self, code: &str) -> bool {
        self.errors.iter().any(|d| d.code == code)
    }

    fn response_err(&self) -> String {
        let text = goutil::status_text(self.status_code);
        match self.errors.len() {
            0 => {
                if self.raw_body.is_empty() {
                    if self.request.as_ref().is_some_and(|r| r.method == "HEAD") {
                        return format!(
                            "unexpected status code {} {} (HEAD responses have no body, use GET for details)",
                            self.status_code, text
                        );
                    }
                    return format!("unexpected status code {} {}", self.status_code, text);
                }
                format!(
                    "unexpected status code {} {}: {}",
                    self.status_code, text, self.raw_body
                )
            }
            1 => self.errors[0].to_string(),
            _ => {
                let parts: Vec<String> = self.errors.iter().map(|d| d.to_string()).collect();
                format!("multiple errors returned: {}", parts.join("; "))
            }
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(r) = &self.request {
            write!(f, "{} {}: ", r.method, gourl::redact_url(&r.url))?;
        }
        f.write_str(&self.response_err())
    }
}

impl std::error::Error for Error {}

/// Lenient decode of `{"errors":[...]}` mirroring `json.Unmarshal` into `transport.Error`
/// (type mismatches leave zero values, invalid JSON leaves no diagnostics).
fn parse_diagnostics(body: &[u8]) -> Vec<Diagnostic> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let serde_json::Value::Object(obj) = v else {
        return Vec::new();
    };
    let Some(serde_json::Value::Array(items)) = goutil::json_field(&obj, "errors") else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| {
            let mut d = Diagnostic::default();
            if let serde_json::Value::Object(o) = item {
                if let Some(serde_json::Value::String(s)) = goutil::json_field(o, "code") {
                    d.code = s.clone();
                }
                if let Some(serde_json::Value::String(s)) = goutil::json_field(o, "message") {
                    d.message = s.clone();
                }
                match goutil::json_field(o, "detail") {
                    None | Some(serde_json::Value::Null) => {}
                    Some(v) => d.detail = Some(v.clone()),
                }
            }
            d
        })
        .collect()
}

/// `transport.CheckError`: returns `Ok(response)` when the status is one of `codes`,
/// otherwise reads the body and returns the structured [`Error`].
pub async fn check_error(mut resp: Response, codes: &[u16]) -> anyhow::Result<Response> {
    if codes.contains(&resp.status) {
        return Ok(resp);
    }
    let body = resp.body.read_all().await.map_err(NetError::from_io)?;
    Err(make_error(&resp, &body).into())
}

pub(crate) fn make_error(resp: &Response, body: &[u8]) -> Error {
    Error::from_response_parts(
        resp.status,
        Some(RequestInfo {
            method: resp.request.method.to_string(),
            url: resp.request.url.clone(),
        }),
        body,
    )
}
