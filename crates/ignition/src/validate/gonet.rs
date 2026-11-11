//! The parts of Go's `net/url` (`Parse`, `ParseQuery`), `net/netip` (`ParseAddr`) and
//! `aws-sdk-go-v2/aws/arn` that the ignition validators depend on. Only success/failure and
//! the parsed scheme/opaque/query matter (every parse error maps to one ignition error).

use std::collections::HashMap;

/// The fields of a parsed `url.URL` the validators read.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Url {
    pub scheme: String,
    pub opaque: String,
    pub raw_query: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Path,
    Host,
    Zone,
    UserPassword,
    QueryComponent,
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

/// `shouldEscape` for the host/zone modes (the only modes `unescape` consults it for).
fn should_escape_host(c: u8) -> bool {
    if c.is_ascii_alphanumeric() {
        return false;
    }
    match c {
        b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':'
        | b'[' | b']' | b'<' | b'>' | b'"' => false,
        b'-' | b'_' | b'.' | b'~' => false,
        // reserved chars not in the host set above: '/', '?', '@' -> escaped in host mode
        _ => true,
    }
}

/// `unescape(s, mode)`: returns the unescaped bytes or an error.
fn unescape(s: &[u8], mode: Mode) -> Result<Vec<u8>, ()> {
    let mut n = 0;
    let mut has_plus = false;
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' => {
                n += 1;
                if i + 2 >= s.len() || !ishex(s[i + 1]) || !ishex(s[i + 2]) {
                    return Err(());
                }
                if mode == Mode::Host && unhex(s[i + 1]) < 8 && &s[i..i + 3] != b"%25" {
                    return Err(());
                }
                if mode == Mode::Zone {
                    let v = (unhex(s[i + 1]) << 4) | unhex(s[i + 2]);
                    if &s[i..i + 3] != b"%25" && v != b' ' && should_escape_host(v) {
                        return Err(());
                    }
                }
                i += 3;
            }
            b'+' => {
                has_plus = mode == Mode::QueryComponent;
                i += 1;
            }
            c => {
                if (mode == Mode::Host || mode == Mode::Zone) && c < 0x80 && should_escape_host(c) {
                    return Err(());
                }
                i += 1;
            }
        }
    }
    if n == 0 && !has_plus {
        return Ok(s.to_vec());
    }
    let mut t = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' => {
                t.push((unhex(s[i + 1]) << 4) | unhex(s[i + 2]));
                i += 3;
            }
            b'+' => {
                t.push(if mode == Mode::QueryComponent {
                    b' '
                } else {
                    b'+'
                });
                i += 1;
            }
            c => {
                t.push(c);
                i += 1;
            }
        }
    }
    Ok(t)
}

fn get_scheme(raw: &str) -> Result<(&str, &str), ()> {
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
                    return Err(());
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
    let Some(rest) = port.strip_prefix(':') else {
        return false;
    };
    rest.bytes().all(|b| b.is_ascii_digit())
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

fn parse_host(scheme: &str, host: &str) -> Result<(), ()> {
    let open = host.rfind('[');
    if let Some(open) = open {
        if open > 0 {
            return Err(());
        }
        let close = host.rfind(']').ok_or(())?;
        let colon_port = &host[close + 1..];
        if !valid_optional_port(colon_port) {
            return Err(());
        }
        unescape(colon_port.as_bytes(), Mode::Host)?;
        // the closing bracket may precede the opening one ("]...[" is caught above as open > 0)
        if close < 1 {
            return Err(());
        }
        let hostname = &host[1..close];
        let unescaped = match hostname.find("%25") {
            Some(z) => {
                let mut h = unescape(&hostname.as_bytes()[..z], Mode::Host)?;
                h.extend(unescape(&hostname.as_bytes()[z..], Mode::Zone)?);
                h
            }
            None => unescape(hostname.as_bytes(), Mode::Host)?,
        };
        let addr = String::from_utf8_lossy(&unescaped).into_owned();
        match parse_addr(&addr) {
            Ok(true) => return Err(()), // IPv4 in brackets
            Ok(false) => return Ok(()),
            Err(()) => return Err(()),
        }
    } else if let Some(i) = host.find(':') {
        let mut i = i;
        let last = host.rfind(':').unwrap_or(i);
        // urlstrictcolons=1 (Go 1.26 default): http/https keep the first colon.
        if last != i && scheme != "http" && scheme != "https" {
            i = last;
        }
        if !valid_optional_port(&host[i..]) {
            return Err(());
        }
    }
    unescape(host.as_bytes(), Mode::Host)?;
    Ok(())
}

fn parse_authority(scheme: &str, authority: &str) -> Result<(), ()> {
    let at = authority.rfind('@');
    match at {
        None => parse_host(scheme, authority)?,
        Some(i) => parse_host(scheme, &authority[i + 1..])?,
    }
    let Some(i) = at else { return Ok(()) };
    let userinfo = &authority[..i];
    if !valid_userinfo(userinfo) {
        return Err(());
    }
    match userinfo.split_once(':') {
        None => {
            unescape(userinfo.as_bytes(), Mode::UserPassword)?;
        }
        Some((u, p)) => {
            unescape(u.as_bytes(), Mode::UserPassword)?;
            unescape(p.as_bytes(), Mode::UserPassword)?;
        }
    }
    Ok(())
}

fn parse(raw: &str) -> Result<Url, ()> {
    if raw.bytes().any(|b| b < b' ' || b == 0x7f) {
        return Err(());
    }
    let mut url = Url::default();
    if raw == "*" {
        return Ok(url);
    }
    let (scheme, rest) = get_scheme(raw)?;
    url.scheme = scheme.to_ascii_lowercase();
    let mut rest = rest;
    if rest.ends_with('?') && rest.matches('?').count() == 1 {
        rest = &rest[..rest.len() - 1];
    } else if let Some((r, q)) = rest.split_once('?') {
        rest = r;
        url.raw_query = q.to_string();
    }
    if !rest.starts_with('/') {
        if !url.scheme.is_empty() {
            url.opaque = rest.to_string();
            return Ok(url);
        }
        let segment = rest.split_once('/').map_or(rest, |(s, _)| s);
        if segment.contains(':') {
            return Err(());
        }
    }
    if (!url.scheme.is_empty() || !rest.starts_with("///")) && rest.starts_with("//") {
        let mut authority = &rest[2..];
        rest = "";
        if let Some(i) = authority.find('/') {
            rest = &authority[i..];
            authority = &authority[..i];
        }
        parse_authority(&url.scheme, authority)?;
    }
    unescape(rest.as_bytes(), Mode::Path)?;
    Ok(url)
}

impl Url {
    /// `url.Parse`.
    pub(crate) fn parse(raw: &str) -> Result<Url, ()> {
        let (u, frag) = match raw.split_once('#') {
            Some((u, f)) => (u, f),
            None => (raw, ""),
        };
        let url = parse(u)?;
        if !frag.is_empty() {
            unescape(frag.as_bytes(), Mode::Fragment)?;
        }
        Ok(url)
    }
}

/// `url.ParseQuery` (values only; the error is ignored by `URL.Query`).
pub(crate) fn parse_query(query: &str) -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    if query.matches('&').count() + 1 > 10000 {
        return m;
    }
    for key in query.split('&') {
        if key.contains(';') || key.is_empty() {
            continue;
        }
        let (k, v) = key.split_once('=').unwrap_or((key, ""));
        let Ok(k) = unescape(k.as_bytes(), Mode::QueryComponent) else {
            continue;
        };
        let Ok(v) = unescape(v.as_bytes(), Mode::QueryComponent) else {
            continue;
        };
        m.entry(String::from_utf8_lossy(&k).into_owned())
            .or_default()
            .push(String::from_utf8_lossy(&v).into_owned());
    }
    m
}

/// `netip.ParseAddr`: `Ok(true)` for IPv4, `Ok(false)` for IPv6.
fn parse_addr(s: &str) -> Result<bool, ()> {
    for c in s.bytes() {
        match c {
            b'.' => return parse_ipv4_fields(s.as_bytes()).map(|_| true),
            b':' => return parse_ipv6(s).map(|_| false),
            b'%' => return Err(()),
            _ => {}
        }
    }
    Err(())
}

fn parse_ipv4_fields(s: &[u8]) -> Result<(), ()> {
    let (mut val, mut pos, mut dig_len) = (0u32, 0, 0);
    for (i, &c) in s.iter().enumerate() {
        if c.is_ascii_digit() {
            if dig_len == 1 && val == 0 {
                return Err(());
            }
            val = val * 10 + u32::from(c - b'0');
            dig_len += 1;
            if val > 255 {
                return Err(());
            }
        } else if c == b'.' {
            if i == 0 || i == s.len() - 1 || s[i - 1] == b'.' {
                return Err(());
            }
            if pos == 3 {
                return Err(());
            }
            pos += 1;
            val = 0;
            dig_len = 0;
        } else {
            return Err(());
        }
    }
    if pos < 3 {
        return Err(());
    }
    Ok(())
}

fn parse_ipv6(input: &str) -> Result<(), ()> {
    let mut s = input.as_bytes();
    let mut zone_len = 0;
    if let Some(i) = input.find('%') {
        if i + 1 == input.len() {
            return Err(());
        }
        zone_len = input.len() - i - 1;
        s = &s[..i];
    }
    let mut ellipsis: i32 = -1;
    if s.len() >= 2 && s[0] == b':' && s[1] == b':' {
        ellipsis = 0;
        s = &s[2..];
        if s.is_empty() {
            return Ok(());
        }
    }
    let mut i: i32 = 0;
    while i < 16 {
        let mut off = 0;
        let mut acc: u32 = 0;
        while off < s.len() {
            let c = s[off];
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => break,
            };
            acc = (acc << 4) + u32::from(d);
            if off > 3 || acc > 0xffff {
                return Err(());
            }
            off += 1;
        }
        if off == 0 {
            return Err(());
        }
        if off < s.len() && s[off] == b'.' {
            if ellipsis < 0 && i != 12 {
                return Err(());
            }
            if i + 4 > 16 {
                return Err(());
            }
            let mut end = input.len();
            if zone_len > 0 {
                end -= zone_len + 1;
            }
            parse_ipv4_fields(&input.as_bytes()[end - s.len()..end])?;
            s = &[];
            i += 4;
            break;
        }
        i += 2;
        s = &s[off..];
        if s.is_empty() {
            break;
        }
        if s[0] != b':' || s.len() == 1 {
            return Err(());
        }
        s = &s[1..];
        if s[0] == b':' {
            if ellipsis >= 0 {
                return Err(());
            }
            ellipsis = i;
            s = &s[1..];
            if s.is_empty() {
                break;
            }
        }
    }
    if !s.is_empty() {
        return Err(());
    }
    if i < 16 {
        if ellipsis < 0 {
            return Err(());
        }
    } else if ellipsis >= 0 {
        return Err(());
    }
    Ok(())
}

/// `arn.ARN` (the fields the validator reads).
pub(crate) struct Arn {
    pub service: String,
    pub resource: String,
}

/// `arn.IsARN`.
pub(crate) fn arn_is_arn(s: &str) -> bool {
    s.starts_with("arn:") && s.matches(':').count() >= 5
}

/// `arn.Parse`.
pub(crate) fn arn_parse(s: &str) -> Result<Arn, String> {
    if !s.starts_with("arn:") {
        return Err("arn: invalid prefix".into());
    }
    let sections: Vec<&str> = s.splitn(6, ':').collect();
    if sections.len() != 6 {
        return Err("arn: not enough sections".into());
    }
    Ok(Arn {
        service: sections[2].to_string(),
        resource: sections[5].to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basics() {
        assert!(Url::parse("data:,hello").is_ok());
        assert_eq!(Url::parse("DATA:,x").unwrap().scheme, "data");
        assert!(Url::parse(":foo").is_err());
        assert!(Url::parse("http://a b").is_err());
        assert!(Url::parse("http://[::1]:80/x").is_ok());
        assert!(Url::parse("http://[1.2.3.4]/").is_err());
        assert!(Url::parse("http://host:port/").is_err());
        assert!(Url::parse("a:b/c").is_ok());
        assert!(Url::parse("ab/c:d").is_ok());
        assert!(Url::parse("/x%zz").is_err());
        assert!(Url::parse("x#%zz").is_err());
        assert!(Url::parse("1a:b").is_err());
    }
}
