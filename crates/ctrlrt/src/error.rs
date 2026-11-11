//! Go-compatible error model.
//!
//! The Go code base relies on `fmt.Errorf("...: %w", err)` wrapping together with
//! `apierrors.IsNotFound(err)` style checks that unwrap the chain. We reproduce this with
//! `anyhow::Error` as the universal error type:
//!
//! * [`wrap`] / [`wrapf!`] produce an error whose `Display` is exactly the Go message
//!   (e.g. `"getting secret ns/name: secrets \"name\" not found"`) while keeping the
//!   wrapped error reachable through `source()`, so [`is_not_found`] & co. still work.
//! * [`ApiError`] mirrors `*apierrors.StatusError` (a `metav1.Status`).
//! * [`NoKindMatchError`] mirrors `*meta.NoKindMatchError`.
//!
//! IMPORTANT: always format errors with `{}` (not `{:#}`), since the Go-style message
//! already contains the whole chain.

use std::fmt;

pub type Error = anyhow::Error;
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub const REASON_NOT_FOUND: &str = "NotFound";
pub const REASON_ALREADY_EXISTS: &str = "AlreadyExists";
pub const REASON_CONFLICT: &str = "Conflict";
pub const REASON_FORBIDDEN: &str = "Forbidden";
pub const REASON_INVALID: &str = "Invalid";
pub const REASON_BAD_REQUEST: &str = "BadRequest";
pub const REASON_GONE: &str = "Gone";
pub const REASON_UNAUTHORIZED: &str = "Unauthorized";
pub const REASON_METHOD_NOT_ALLOWED: &str = "MethodNotAllowed";
pub const REASON_INTERNAL_ERROR: &str = "InternalError";
pub const REASON_TIMEOUT: &str = "Timeout";
pub const REASON_SERVER_TIMEOUT: &str = "ServerTimeout";
pub const REASON_TOO_MANY_REQUESTS: &str = "TooManyRequests";
pub const REASON_SERVICE_UNAVAILABLE: &str = "ServiceUnavailable";
pub const REASON_UNKNOWN: &str = "";

/// `metav1.StatusCause`
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StatusCause {
    #[serde(rename = "reason", default, skip_serializing_if = "String::is_empty")]
    pub type_: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub field: String,
}

/// `metav1.StatusDetails`
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StatusDetails {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uid: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub causes: Vec<StatusCause>,
    #[serde(rename = "retryAfterSeconds", default, skip_serializing_if = "is_zero")]
    pub retry_after_seconds: i32,
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

/// `*apierrors.StatusError` — an error returned by the API server (or the fake client).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApiError {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub details: Option<StatusDetails>,
    #[serde(default)]
    pub code: u16,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ApiError {}

/// Go `%q` formatting of a string (sufficient for Kubernetes names and messages).
pub fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl ApiError {
    fn failure(code: u16, reason: &str, message: String, details: Option<StatusDetails>) -> Self {
        Self {
            status: "Failure".into(),
            message,
            reason: reason.into(),
            details,
            code,
        }
    }

    fn gr_details(group: &str, resource: &str, name: &str) -> Option<StatusDetails> {
        Some(StatusDetails {
            name: name.into(),
            group: group.into(),
            kind: resource.into(),
            ..Default::default()
        })
    }

    /// `schema.GroupResource{Group, Resource}.String()`
    fn gr_string(group: &str, resource: &str) -> String {
        if group.is_empty() {
            resource.to_string()
        } else {
            format!("{resource}.{group}")
        }
    }

    /// `apierrors.NewNotFound(schema.GroupResource{group, resource}, name)`
    pub fn not_found(group: &str, resource: &str, name: &str) -> Self {
        Self::failure(
            404,
            REASON_NOT_FOUND,
            format!(
                "{} {} not found",
                Self::gr_string(group, resource),
                go_quote(name)
            ),
            Self::gr_details(group, resource, name),
        )
    }

    /// `apierrors.NewAlreadyExists`
    pub fn already_exists(group: &str, resource: &str, name: &str) -> Self {
        Self::failure(
            409,
            REASON_ALREADY_EXISTS,
            format!(
                "{} {} already exists",
                Self::gr_string(group, resource),
                go_quote(name)
            ),
            Self::gr_details(group, resource, name),
        )
    }

    /// `apierrors.NewConflict(gr, name, err)`
    pub fn conflict(group: &str, resource: &str, name: &str, err: impl fmt::Display) -> Self {
        Self::failure(
            409,
            REASON_CONFLICT,
            format!(
                "Operation cannot be fulfilled on {} {}: {}",
                Self::gr_string(group, resource),
                go_quote(name),
                err
            ),
            Self::gr_details(group, resource, name),
        )
    }

    /// `apierrors.NewForbidden(gr, name, err)`
    pub fn forbidden(group: &str, resource: &str, name: &str, err: impl fmt::Display) -> Self {
        let message = if group.is_empty() && resource.is_empty() {
            format!("forbidden: {err}")
        } else if name.is_empty() {
            format!("{} is forbidden: {}", Self::gr_string(group, resource), err)
        } else {
            format!(
                "{} {} is forbidden: {}",
                Self::gr_string(group, resource),
                go_quote(name),
                err
            )
        };
        Self::failure(
            403,
            REASON_FORBIDDEN,
            message,
            Self::gr_details(group, resource, name),
        )
    }

    /// `apierrors.NewInvalid(gk, name, errs)`; `causes` messages are `field: detail` strings
    /// already rendered as Go's `field.ErrorList.ToAggregate()` would.
    pub fn invalid(group: &str, kind: &str, name: &str, causes: Vec<StatusCause>) -> Self {
        let gk = if group.is_empty() {
            kind.to_string()
        } else {
            format!("{kind}.{group}")
        };
        let message = if causes.is_empty() {
            format!("{} {} is invalid", gk, go_quote(name))
        } else {
            let parts: Vec<String> = causes
                .iter()
                .map(|c| {
                    if c.field.is_empty() {
                        c.message.clone()
                    } else {
                        format!("{}: {}", c.field, c.message)
                    }
                })
                .collect();
            let agg = if parts.len() == 1 {
                parts[0].clone()
            } else {
                format!("[{}]", parts.join(", "))
            };
            format!("{} {} is invalid: {}", gk, go_quote(name), agg)
        };
        Self::failure(
            422,
            REASON_INVALID,
            message,
            Some(StatusDetails {
                name: name.into(),
                group: group.into(),
                kind: kind.into(),
                causes,
                ..Default::default()
            }),
        )
    }

    /// `apierrors.NewBadRequest(reason)`
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::failure(400, REASON_BAD_REQUEST, message.into(), None)
    }

    /// `apierrors.NewGone(message)`
    pub fn gone(message: impl Into<String>) -> Self {
        Self::failure(410, REASON_GONE, message.into(), None)
    }

    /// `apierrors.NewInternalError(err)`
    pub fn internal(err: impl fmt::Display) -> Self {
        Self::failure(
            500,
            REASON_INTERNAL_ERROR,
            format!("Internal error occurred: {err}"),
            Some(StatusDetails {
                causes: vec![StatusCause {
                    message: err.to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        )
    }

    /// `apierrors.NewMethodNotSupported`-like generic constructor.
    pub fn new(code: u16, reason: &str, message: impl Into<String>) -> Self {
        Self::failure(code, reason, message.into(), None)
    }

    /// `apierrors.reasonAndCodeForError` + `ReasonForError`.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// `*meta.NoKindMatchError` — the kind is not served by the API server (CRD not installed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoKindMatchError {
    pub group: String,
    pub kind: String,
    pub searched_versions: Vec<String>,
}

impl fmt::Display for NoKindMatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut versions: Vec<String> = self
            .searched_versions
            .iter()
            .map(|v| {
                if self.group.is_empty() {
                    v.clone()
                } else {
                    format!("{}/{}", self.group, v)
                }
            })
            .collect();
        versions.sort();
        versions.dedup();
        match versions.len() {
            0 => write!(
                f,
                "no matches for kind {} in group {}",
                go_quote(&self.kind),
                go_quote(&self.group)
            ),
            1 => write!(
                f,
                "no matches for kind {} in version {}",
                go_quote(&self.kind),
                go_quote(&versions[0])
            ),
            _ => write!(
                f,
                "no matches for kind {} in versions {}",
                go_quote(&self.kind),
                format!("{:?}", versions).replace(',', "")
            ),
        }
    }
}
impl std::error::Error for NoKindMatchError {}

/// `*meta.NoResourceMatchError` — the resource is not served (e.g. a group-version vanished
/// from discovery). `meta.IsNoMatchError` matches it as well as [`NoKindMatchError`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoResourceMatchError {
    pub group: String,
    pub version: String,
    pub resource: String,
}

impl fmt::Display for NoResourceMatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // "no matches for %v" with schema.GroupVersionResource.String()
        write!(
            f,
            "no matches for {}/{}, Resource={}",
            self.group, self.version, self.resource
        )
    }
}
impl std::error::Error for NoResourceMatchError {}

/// Error with a Go-style pre-rendered message that wraps a cause (`fmt.Errorf("...%w", err)`).
#[derive(Debug)]
pub struct Wrapped {
    message: String,
    source: Error,
}

impl fmt::Display for Wrapped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Wrapped {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// Wrap `cause` with a fully rendered Go-style message (the message must already embed
/// the cause text where Go's `%w` would place it).
pub fn wrap(cause: impl Into<Error>, message: impl Into<String>) -> Error {
    Error::new(Wrapped {
        message: message.into(),
        source: cause.into(),
    })
}

/// `fmt.Errorf(format, args..., err)` where the format contains a `%w` placeholder.
///
/// Usage: `wrapf!(err, "getting secret {}/{}: {}", ns, name, err)` — the first argument is the
/// wrapped cause, the remaining arguments are an ordinary `format!` invocation which must
/// render the cause text itself (exactly like the Go format string does with `%w`).
#[macro_export]
macro_rules! wrapf {
    ($cause:expr, $($fmt:tt)+) => {{
        let __msg = format!($($fmt)+);
        $crate::error::wrap($cause, __msg)
    }};
}

/// `errors.New` / `fmt.Errorf` without `%w`.
#[macro_export]
macro_rules! errorf {
    ($($fmt:tt)+) => {
        $crate::__private::anyhow::anyhow!(format!($($fmt)+))
    };
}

/// Like Go's `errors.As(err, &*StatusError)`: first ApiError in the chain.
pub fn as_api_error(err: &Error) -> Option<&ApiError> {
    err.chain().find_map(|e| e.downcast_ref::<ApiError>())
}

fn reason_and_code(err: &Error) -> (String, u16) {
    match as_api_error(err) {
        Some(e) => (e.reason.clone(), e.code),
        None => (REASON_UNKNOWN.into(), 0),
    }
}

fn reason_is(err: &Error, reason: &str, code: u16) -> bool {
    let (r, c) = reason_and_code(err);
    if r == reason {
        return true;
    }
    // apimachinery: "Unknown" reason falls back to the HTTP status code
    if r.is_empty() && as_api_error(err).is_some() && c == code {
        return true;
    }
    false
}

/// `apierrors.IsNotFound`
pub fn is_not_found(err: &Error) -> bool {
    reason_is(err, REASON_NOT_FOUND, 404)
}
/// `apierrors.IsAlreadyExists`
pub fn is_already_exists(err: &Error) -> bool {
    reason_is(err, REASON_ALREADY_EXISTS, 0)
}
/// `apierrors.IsConflict`
pub fn is_conflict(err: &Error) -> bool {
    reason_is(err, REASON_CONFLICT, 409)
}
/// `apierrors.IsForbidden`
pub fn is_forbidden(err: &Error) -> bool {
    reason_is(err, REASON_FORBIDDEN, 403)
}
/// `apierrors.IsInvalid`
pub fn is_invalid(err: &Error) -> bool {
    reason_is(err, REASON_INVALID, 422)
}
/// `apierrors.IsBadRequest`
pub fn is_bad_request(err: &Error) -> bool {
    reason_is(err, REASON_BAD_REQUEST, 400)
}
/// `apierrors.IsGone`
pub fn is_gone(err: &Error) -> bool {
    reason_is(err, REASON_GONE, 410)
}
/// `apierrors.IsUnauthorized`
pub fn is_unauthorized(err: &Error) -> bool {
    reason_is(err, REASON_UNAUTHORIZED, 401)
}
/// `meta.IsNoMatchError`
pub fn is_no_match(err: &Error) -> bool {
    err.chain().any(|e| {
        e.downcast_ref::<NoKindMatchError>().is_some()
            || e.downcast_ref::<NoResourceMatchError>().is_some()
    })
}

/// `client.IgnoreNotFound`
pub fn ignore_not_found(res: Result<()>) -> Result<()> {
    match res {
        Err(e) if is_not_found(&e) => Ok(()),
        other => other,
    }
}

/// `errors.Is` for an arbitrary concrete error type `E` (first match in the chain).
pub fn find_in_chain<E: std::error::Error + Send + Sync + 'static>(err: &Error) -> Option<&E> {
    err.chain().find_map(|e| e.downcast_ref::<E>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_match_go() {
        let e = ApiError::not_found("", "secrets", "foo");
        assert_eq!(e.to_string(), r#"secrets "foo" not found"#);
        let e = ApiError::not_found("provisioning.dpu.nvidia.com", "dpuclusters", "c");
        assert_eq!(
            e.to_string(),
            r#"dpuclusters.provisioning.dpu.nvidia.com "c" not found"#
        );
        let e = ApiError::conflict("", "configmaps", "x", "object was modified");
        assert_eq!(
            e.to_string(),
            r#"Operation cannot be fulfilled on configmaps "x": object was modified"#
        );
    }

    #[test]
    fn wrapping_keeps_reason() {
        let base: Error = ApiError::not_found("", "secrets", "s").into();
        let msg = format!("getting secret ns/s: {base}");
        let w = wrap(base, msg);
        assert!(is_not_found(&w));
        assert_eq!(
            w.to_string(),
            r#"getting secret ns/s: secrets "s" not found"#
        );
        let w2 = wrapf!(w, "outer: {}", "x");
        assert!(is_not_found(&w2));
        assert!(!is_conflict(&w2));
    }

    #[test]
    fn no_kind_match() {
        let e: Error = NoKindMatchError {
            group: "metallb.io".into(),
            kind: "IPAddressPool".into(),
            searched_versions: vec!["v1beta1".into()],
        }
        .into();
        assert!(is_no_match(&e));
        assert_eq!(
            e.to_string(),
            r#"no matches for kind "IPAddressPool" in version "metallb.io/v1beta1""#
        );
    }
}
