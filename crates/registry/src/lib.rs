//! Rust port of the subset of `github.com/google/go-containerregistry` used by the
//! operator:
//!
//! * [`name`] — image reference parsing (`name.ParseReference`, `NewTag`, ...),
//! * [`authn`] — `AuthConfig`, `Authenticator`, `Keychain`, `Resource`,
//! * [`remote`] — `Head`, `Get`, `Image` with the full `remote/transport` auth flow,
//! * [`mutate`] — `mutate.Extract` (flattened filesystem of an image),
//! * [`v1`] — manifests, descriptors, config files, media types.
//!
//! Errors are `anyhow::Error`s whose `Display` matches the Go `Error()` strings and whose
//! chain contains the typed Go-equivalent errors ([`transport::Error`],
//! [`transport::UrlError`], [`name::ErrBadName`], ...). Use the helpers below (or
//! `ctrlrt::error::find_in_chain`) to classify them like Go's `errors.As`.

pub mod authn;
mod gobase64;
mod gotar;
pub mod gourl;
pub mod goutil;
pub mod mutate;
pub mod name;
pub mod remote;
pub mod transport;
pub mod v1;

pub use authn::{AuthConfig, Authenticator, Keychain, Resource};
pub use name::{Reference, Registry, Repository, parse_reference};

/// Go `errors.As(err, &*transport.Error)`: the first structured registry error in the
/// chain (looking inside the ping `multierrs` too).
pub fn as_transport_error(err: &anyhow::Error) -> Option<&transport::Error> {
    fn search<'a>(e: &'a (dyn std::error::Error + 'static)) -> Option<&'a transport::Error> {
        let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
        while let Some(c) = cur {
            if let Some(t) = c.downcast_ref::<transport::Error>() {
                return Some(t);
            }
            if let Some(m) = c.downcast_ref::<transport::MultiError>() {
                for inner in &m.0 {
                    if let Some(t) = search(inner.as_ref()) {
                        return Some(t);
                    }
                }
            }
            cur = c.source();
        }
        None
    }
    for e in err.chain() {
        if let Some(t) = search(e) {
            return Some(t);
        }
    }
    None
}

/// The HTTP status code of the registry error in the chain, if any.
pub fn status_code(err: &anyhow::Error) -> Option<u16> {
    as_transport_error(err).map(|e| e.status_code)
}

/// Whether the registry error in the chain carries the given diagnostic code
/// (e.g. [`transport::codes::MANIFEST_UNKNOWN`]).
pub fn has_error_code(err: &anyhow::Error, code: &str) -> bool {
    as_transport_error(err).is_some_and(|e| e.has_code(code))
}

/// 401 / 403 or an `UNAUTHORIZED` / `DENIED` diagnostic.
pub fn is_auth_error(err: &anyhow::Error) -> bool {
    as_transport_error(err).is_some_and(|e| {
        e.status_code == 401
            || e.status_code == 403
            || e.has_code(transport::codes::UNAUTHORIZED)
            || e.has_code(transport::codes::DENIED)
    })
}

/// 404 or a `MANIFEST_UNKNOWN` / `NAME_UNKNOWN` / `BLOB_UNKNOWN` diagnostic.
pub fn is_not_found_error(err: &anyhow::Error) -> bool {
    as_transport_error(err).is_some_and(|e| {
        e.status_code == 404
            || e.has_code(transport::codes::MANIFEST_UNKNOWN)
            || e.has_code(transport::codes::NAME_UNKNOWN)
            || e.has_code(transport::codes::BLOB_UNKNOWN)
    })
}

/// Go `retry.IsTemporary` on the chain.
pub fn is_temporary(err: &anyhow::Error) -> bool {
    as_transport_error(err).is_some_and(transport::Error::temporary)
        || err
            .chain()
            .any(|e| e.downcast_ref::<transport::NetError>().is_some_and(|n| n.retryable))
}
