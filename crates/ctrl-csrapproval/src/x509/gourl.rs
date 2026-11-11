//! The validation part of Go's `net/url.Parse` (Go 1.26), as needed by
//! `x509.parseSANExtension`: reports the same errors and returns the parsed `URL.Host`.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use ctrlrt::error::go_quote;

/// `*url.Error{Op: "parse", URL, Err}`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlError {
    pub url: String,
    pub err: String,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse {}: {}", go_quote(&self.url), self.err)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Path,
    Host,
    Zone,
    UserPassword,
    Fragment,
}

fn ishex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

fn unhex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

/// `shouldEscape(c, encodeHost|encodeZone)` — the only modes `unescape` validates bytes for.
fn should_escape_host(c: u8) -> bool {
    if c.is_ascii_alphanumeric() {
        return false;
    }
    !matches!(
        c,
        b'!' | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':'
            | b'['
            | b']'
            | b'<'
            | b'>'
            | b'"'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
    )
}

fn escape_error(s: &[u8]) -> String {
    format!(
        "invalid URL escape {}",
        go_quote(&String::from_utf8_lossy(s))
    )
}

/// `unescape(s, mode)`: returns the unescaped bytes or the error message.
fn unescape(s: &[u8], mode: Mode) -> Result<Vec<u8>, String> {
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' => {
                if i + 2 >= s.len() || !ishex(s[i + 1]) || !ishex(s[i + 2]) {
                    let tail = &s[i..];
                    return Err(escape_error(&tail[..tail.len().min(3)]));
                }
                if mode == Mode::Host && unhex(s[i + 1]) < 8 && &s[i..i + 3] != b"%25" {
                    return Err(escape_error(&s[i..i + 3]));
                }
                if mode == Mode::Zone {
                    let v = (unhex(s[i + 1]) << 4) | unhex(s[i + 2]);
                    if &s[i..i + 3] != b"%25" && v != b' ' && should_escape_host(v) {
                        return Err(escape_error(&s[i..i + 3]));
                    }
                }
                i += 3;
            }
            c => {
                if (mode == Mode::Host || mode == Mode::Zone) && c < 0x80 && should_escape_host(c) {
                    return Err(format!(
                        "invalid character {} in host name",
                        go_quote(&String::from_utf8_lossy(&s[i..i + 1]))
                    ));
                }
                i += 1;
            }
        }
    }
    let mut t = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' {
            t.push((unhex(s[i + 1]) << 4) | unhex(s[i + 2]));
            i += 3;
        } else {
            t.push(s[i]);
            i += 1;
        }
    }
    Ok(t)
}

/// `getScheme`: (scheme, rest)
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
                    return Err("missing protocol scheme".into());
                }
                return Ok((&raw[..i], &raw[i + 1..]));
            }
            _ => return Ok(("", raw)),
        }
    }
    Ok(("", raw))
}

fn valid_optional_port(port: &str) -> bool {
    if port.is_empty() {
        return true;
    }
    let b = port.as_bytes();
    b[0] == b':' && b[1..].iter().all(u8::is_ascii_digit)
}

fn valid_userinfo(s: &str) -> bool {
    s.chars().all(|r| {
        r.is_ascii_alphanumeric()
            || matches!(
                r,
                '-' | '.'
                    | '_'
                    | ':'
                    | '~'
                    | '!'
                    | '$'
                    | '&'
                    | '\''
                    | '('
                    | ')'
                    | '*'
                    | '+'
                    | ','
                    | ';'
                    | '='
                    | '%'
                    | '@'
            )
    })
}

/// `netip.ParseAddr` (validity and "is IPv4").
fn parse_addr(s: &str) -> Result<bool, String> {
    if s.parse::<Ipv4Addr>().is_ok() {
        return Ok(true);
    }
    let (addr, zone) = match s.split_once('%') {
        Some((a, z)) => (a, Some(z)),
        None => (s, None),
    };
    if zone == Some("") {
        return Err(format!(
            "ParseAddr({}): zone must be a non-empty string",
            go_quote(s)
        ));
    }
    if addr.contains(':') && addr.parse::<Ipv6Addr>().is_ok() {
        return Ok(false);
    }
    Err(format!("ParseAddr({}): unable to parse IP", go_quote(s)))
}

/// `parseHost(scheme, host)`: the unescaped host.
fn parse_host(scheme: &str, host: &str) -> Result<String, String> {
    if let Some(open) = host.rfind('[') {
        if open > 0 {
            return Err("invalid IP-literal".into());
        }
        let Some(close) = host.rfind(']') else {
            return Err("missing ']' in host".into());
        };
        let colon_port = &host[close + 1..];
        if !valid_optional_port(colon_port) {
            return Err(format!("invalid port {} after host", go_quote(colon_port)));
        }
        let unescaped_colon_port = unescape(colon_port.as_bytes(), Mode::Host)?;
        let hostname = host.get(1..close).unwrap_or("");
        let unescaped_hostname = match hostname.find("%25") {
            Some(zone_idx) => {
                let mut h = unescape(&hostname.as_bytes()[..zone_idx], Mode::Host)?;
                h.extend(unescape(&hostname.as_bytes()[zone_idx..], Mode::Zone)?);
                h
            }
            None => unescape(hostname.as_bytes(), Mode::Host)?,
        };
        let unescaped_hostname = String::from_utf8_lossy(&unescaped_hostname).into_owned();
        let is4 = parse_addr(&unescaped_hostname).map_err(|e| format!("invalid host: {e}"))?;
        if is4 {
            return Err("invalid IP-literal".into());
        }
        return Ok(format!(
            "[{}]{}",
            unescaped_hostname,
            String::from_utf8_lossy(&unescaped_colon_port)
        ));
    } else if let Some(mut i) = host.find(':') {
        let last_colon = host.rfind(':').unwrap_or(i);
        if last_colon != i {
            // Strict colons only for http and https URLs (urlstrictcolons=1 default).
            if scheme != "http" && scheme != "https" {
                i = last_colon;
            }
        }
        let colon_port = &host[i..];
        if !valid_optional_port(colon_port) {
            return Err(format!("invalid port {} after host", go_quote(colon_port)));
        }
    }
    let h = unescape(host.as_bytes(), Mode::Host)?;
    Ok(String::from_utf8_lossy(&h).into_owned())
}

fn parse_authority(scheme: &str, authority: &str) -> Result<String, String> {
    let at = authority.rfind('@');
    let host = match at {
        None => parse_host(scheme, authority)?,
        Some(i) => parse_host(scheme, &authority[i + 1..])?,
    };
    let Some(i) = at else {
        return Ok(host);
    };
    let userinfo = &authority[..i];
    if !valid_userinfo(userinfo) {
        return Err("net/url: invalid userinfo".into());
    }
    match userinfo.split_once(':') {
        None => {
            unescape(userinfo.as_bytes(), Mode::UserPassword)?;
        }
        Some((user, pass)) => {
            unescape(user.as_bytes(), Mode::UserPassword)?;
            unescape(pass.as_bytes(), Mode::UserPassword)?;
        }
    }
    Ok(host)
}

/// `parse(rawURL, viaRequest=false)`: returns `URL.Host`.
fn parse_inner(raw: &str) -> Result<String, String> {
    if raw.bytes().any(|b| b < b' ' || b == 0x7f) {
        return Err("net/url: invalid control character in URL".into());
    }
    if raw == "*" {
        return Ok(String::new());
    }
    let (scheme, rest) = get_scheme(raw)?;
    let scheme = scheme.to_ascii_lowercase();

    let mut rest = if rest.ends_with('?') && rest.matches('?').count() == 1 {
        &rest[..rest.len() - 1]
    } else {
        rest.split_once('?').map_or(rest, |(r, _)| r)
    };

    if !rest.starts_with('/') {
        if !scheme.is_empty() {
            // Rootless paths are opaque.
            return Ok(String::new());
        }
        let segment = rest.split_once('/').map_or(rest, |(s, _)| s);
        if segment.contains(':') {
            return Err("first path segment in URL cannot contain colon".into());
        }
    }

    let mut host = String::new();
    if (!scheme.is_empty() || !rest.starts_with("///")) && rest.starts_with("//") {
        let mut authority = &rest[2..];
        rest = "";
        if let Some(i) = authority.find('/') {
            rest = &authority[i..];
            authority = &authority[..i];
        }
        host = parse_authority(&scheme, authority)?;
    }

    unescape(rest.as_bytes(), Mode::Path)?;
    Ok(host)
}

/// `url.Parse(rawURL)`: on success returns the parsed `URL.Host`.
pub fn parse(raw: &str) -> Result<String, UrlError> {
    let (u, frag) = match raw.split_once('#') {
        Some((u, f)) => (u, Some(f)),
        None => (raw, None),
    };
    let host = parse_inner(u).map_err(|err| UrlError {
        url: u.to_string(),
        err,
    })?;
    if let Some(frag) = frag.filter(|f| !f.is_empty()) {
        unescape(frag.as_bytes(), Mode::Fragment).map_err(|err| UrlError {
            url: raw.to_string(),
            err,
        })?;
    }
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts_and_errors() {
        assert_eq!(
            parse("spiffe://cluster.local/ns/a").unwrap(),
            "cluster.local"
        );
        assert_eq!(parse("https://[::1]:443/x").unwrap(), "[::1]:443");
        assert_eq!(parse("urn:foo").unwrap(), "");
        assert_eq!(
            parse("http://a b").unwrap_err().to_string(),
            "parse \"http://a b\": invalid character \" \" in host name"
        );
        assert_eq!(
            parse(":foo").unwrap_err().to_string(),
            "parse \":foo\": missing protocol scheme"
        );
        assert_eq!(
            parse("http://x:abc").unwrap_err().to_string(),
            "parse \"http://x:abc\": invalid port \":abc\" after host"
        );
        assert_eq!(
            parse("/a%zz").unwrap_err().to_string(),
            "parse \"/a%zz\": invalid URL escape \"%zz\""
        );
        assert!(parse("http://[1.2.3.4]/").is_err());
    }
}
