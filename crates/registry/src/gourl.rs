//! Minimal port of Go's `net/url` (parse / String / Query / Encode / Redacted) for the
//! shapes of URLs go-containerregistry builds and prints in errors.

use std::collections::BTreeMap;
use std::fmt;

/// `url.Values`
pub type Values = BTreeMap<String, Vec<String>>;

/// `url.URL` (subset).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoUrl {
    pub scheme: String,
    pub opaque: String,
    /// `None` when there is no userinfo; `Some((user, password))`.
    pub user: Option<(String, Option<String>)>,
    pub host: String,
    /// Escaped path as found in / destined for the wire.
    pub path: String,
    pub force_query: bool,
    pub raw_query: String,
    pub fragment: String,
}

/// Error from [`GoUrl::parse`], rendered like Go's `*url.Error` (`parse "<url>": <err>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub url: String,
    pub err: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse {}: {}", crate::goutil::quote(&self.url), self.err)
    }
}

impl std::error::Error for ParseError {}

fn get_scheme(raw: &str) -> Result<(&str, &str), String> {
    for (i, c) in raw.bytes().enumerate() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' => {}
            b'0'..=b'9' | b'+' | b'-' | b'.' => {
                if i == 0 {
                    return Ok(("", raw));
                }
            }
            b':' => {
                if i == 0 {
                    return Err("missing protocol scheme".to_string());
                }
                return Ok((&raw[..i], &raw[i + 1..]));
            }
            _ => return Ok(("", raw)),
        }
    }
    Ok(("", raw))
}

impl GoUrl {
    /// `url.Parse`
    pub fn parse(raw: &str) -> Result<GoUrl, ParseError> {
        let err = |e: String| ParseError {
            url: raw.to_string(),
            err: e,
        };
        let (u, frag) = match raw.split_once('#') {
            Some((u, f)) => (u, f.to_string()),
            None => (raw, String::new()),
        };
        if u.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return Err(err("net/url: invalid control character in URL".to_string()));
        }
        if u.is_empty() {
            return Err(err("empty url".to_string()));
        }
        let mut url = GoUrl {
            fragment: frag,
            ..Default::default()
        };
        let (scheme, mut rest) = get_scheme(u).map_err(err)?;
        url.scheme = scheme.to_ascii_lowercase();
        let rest_owned;
        if rest.ends_with('?') && rest.matches('?').count() == 1 {
            url.force_query = true;
            rest = &rest[..rest.len() - 1];
        } else if let Some((r, q)) = rest.split_once('?') {
            url.raw_query = q.to_string();
            rest_owned = r.to_string();
            rest = &rest_owned;
        }
        if !rest.starts_with('/') {
            if !url.scheme.is_empty() {
                url.opaque = rest.to_string();
                return Ok(url);
            }
            let segment = rest.split('/').next().unwrap_or("");
            if segment.contains(':') {
                return Err(err("first path segment in URL cannot contain colon".to_string()));
            }
        }
        let mut rest = rest.to_string();
        if (!url.scheme.is_empty() || !rest.starts_with("///")) && rest.starts_with("//") {
            let after = &rest[2..];
            let (authority, path) = match after.find('/') {
                Some(i) => (&after[..i], after[i..].to_string()),
                None => (after, String::new()),
            };
            if let Some(i) = authority.rfind('@') {
                let userinfo = &authority[..i];
                url.user = Some(match userinfo.split_once(':') {
                    Some((u, p)) => (u.to_string(), Some(p.to_string())),
                    None => (userinfo.to_string(), None),
                });
                url.host = authority[i + 1..].to_string();
            } else {
                url.host = authority.to_string();
            }
            rest = path;
        }
        url.path = rest;
        Ok(url)
    }

    /// `URL.String()`
    pub fn string(&self) -> String {
        let mut buf = String::new();
        if !self.opaque.is_empty() {
            buf.push_str(&self.scheme);
            buf.push(':');
            buf.push_str(&self.opaque);
        } else {
            if !self.scheme.is_empty() || !self.host.is_empty() || self.user.is_some() {
                if !self.scheme.is_empty() {
                    buf.push_str(&self.scheme);
                    buf.push(':');
                }
                if !self.host.is_empty() || !self.path.is_empty() || self.user.is_some() {
                    buf.push_str("//");
                }
                if let Some((u, p)) = &self.user {
                    buf.push_str(u);
                    if let Some(p) = p {
                        buf.push(':');
                        buf.push_str(p);
                    }
                    buf.push('@');
                }
                buf.push_str(&self.host);
            }
            if !self.path.is_empty() && !self.path.starts_with('/') && !self.host.is_empty() {
                buf.push('/');
            }
            buf.push_str(&self.path);
        }
        if self.force_query || !self.raw_query.is_empty() {
            buf.push('?');
            buf.push_str(&self.raw_query);
        }
        if !self.fragment.is_empty() {
            buf.push('#');
            buf.push_str(&self.fragment);
        }
        buf
    }

    /// `URL.Redacted()`
    pub fn redacted(&self) -> String {
        let mut u = self.clone();
        if let Some((_, Some(p))) = &mut u.user {
            *p = "xxxxx".to_string();
        }
        u.string()
    }

    /// `URL.Query()`
    pub fn query(&self) -> Values {
        parse_query(&self.raw_query)
    }

    /// Go's `stripPassword` used by `http.Client` in `*url.Error`.
    pub fn strip_password(&self) -> String {
        let mut u = self.clone();
        if let Some((_, Some(p))) = &mut u.user {
            *p = "***".to_string();
        }
        u.string()
    }

    /// `URL.Hostname()`
    pub fn hostname(&self) -> String {
        let (h, _) = split_host_port_loose(&self.host);
        h.trim_start_matches('[').trim_end_matches(']').to_string()
    }

    /// `URL.Port()`
    pub fn port(&self) -> String {
        split_host_port_loose(&self.host).1
    }

    /// `URL.Parse(ref)` — resolve a (possibly relative) reference against this URL.
    pub fn resolve(&self, reference: &str) -> Result<GoUrl, ParseError> {
        let r = GoUrl::parse(reference)?;
        if !r.scheme.is_empty() {
            return Ok(r);
        }
        let base = url::Url::parse(&self.string()).map_err(|e| ParseError {
            url: reference.to_string(),
            err: e.to_string(),
        })?;
        let joined = base.join(reference).map_err(|e| ParseError {
            url: reference.to_string(),
            err: e.to_string(),
        })?;
        GoUrl::parse(joined.as_str())
    }
}

impl fmt::Display for GoUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.string())
    }
}

/// `url.URL.Hostname`/`Port` splitting (`hostport` without validation).
fn split_host_port_loose(hostport: &str) -> (String, String) {
    let colon = hostport.rfind(':');
    match colon {
        Some(i) if !hostport[i..].contains(']') => {
            let port = &hostport[i + 1..];
            if port.bytes().all(|b| b.is_ascii_digit()) {
                return (hostport[..i].to_string(), port.to_string());
            }
            (hostport.to_string(), String::new())
        }
        _ => (hostport.to_string(), String::new()),
    }
}

fn unhex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// `url.QueryUnescape`
pub fn query_unescape(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let (Some(h), Some(l)) = (
                    b.get(i + 1).copied().and_then(unhex),
                    b.get(i + 2).copied().and_then(unhex),
                ) else {
                    let end = (i + 3).min(b.len());
                    return Err(format!(
                        "invalid URL escape {}",
                        crate::goutil::quote(&String::from_utf8_lossy(&b[i..end]))
                    ));
                };
                out.push(h << 4 | l);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// `url.QueryEscape`
pub fn query_escape(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            out.push(c as char);
        } else if c == b' ' {
            out.push('+');
        } else {
            out.push('%');
            out.push(HEX[(c >> 4) as usize] as char);
            out.push(HEX[(c & 15) as usize] as char);
        }
    }
    out
}

/// `url.ParseQuery` (errors are dropped, as `URL.Query()` does).
pub fn parse_query(q: &str) -> Values {
    let mut m = Values::new();
    for key in q.split('&') {
        if key.is_empty() || key.contains(';') {
            continue;
        }
        let (k, v) = key.split_once('=').unwrap_or((key, ""));
        let (Ok(k), Ok(v)) = (query_unescape(k), query_unescape(v)) else {
            continue;
        };
        m.entry(k).or_default().push(v);
    }
    m
}

/// `url.Values.Encode()`
pub fn encode_query(v: &Values) -> String {
    let mut buf = String::new();
    for (k, vs) in v {
        let ke = query_escape(k);
        for val in vs {
            if !buf.is_empty() {
                buf.push('&');
            }
            buf.push_str(&ke);
            buf.push('=');
            buf.push_str(&query_escape(val));
        }
    }
    buf
}

/// `redact.URL` from go-containerregistry's internal/redact package.
pub fn redact_url(u: &GoUrl) -> String {
    const ALLOW: &[&str] = &["scope", "service", "mount", "from", "digest", "n", "last"];
    let mut qs = u.query();
    for (k, vs) in qs.iter_mut() {
        if !ALLOW.contains(&k.as_str()) {
            for v in vs.iter_mut() {
                *v = "REDACTED".to_string();
            }
        }
    }
    let mut r = u.clone();
    r.raw_query = encode_query(&qs);
    r.redacted()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for s in [
            "https://quay.io/v2/foo/manifests/latest",
            "https://auth.example.com/token?scope=a&service=b",
            "http://127.0.0.1:5000/v2/",
            "https://user:pass@host/x",
        ] {
            assert_eq!(GoUrl::parse(s).unwrap().string(), s);
        }
        let u = GoUrl::parse("https://user:pass@quay.io/v2/auth?b=2&a=1&a=3&scope=s").unwrap();
        assert_eq!(
            redact_url(&u),
            "https://user:xxxxx@quay.io/v2/auth?a=REDACTED&a=REDACTED&b=REDACTED&scope=s"
        );
        assert_eq!(query_escape("repository:foo/bar:pull"), "repository%3Afoo%2Fbar%3Apull");
        assert_eq!(query_escape("a b"), "a+b");
    }
}
