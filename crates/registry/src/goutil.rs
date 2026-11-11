//! Small helpers reproducing Go standard library formatting/parsing behaviour.

use std::fmt::Write as _;

/// Go `strconv.Quote` (as used by `%q` on strings).
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if is_go_printable(c) => out.push(c),
            c if (c as u32) < 0x80 => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c if (c as u32) < 0x10000 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => {
                let _ = write!(out, "\\U{:08x}", c as u32);
            }
        }
    }
    out.push('"');
    out
}

/// Approximation of Go's `strconv.IsPrint`: printable ASCII plus non-control Unicode.
fn is_go_printable(c: char) -> bool {
    let u = c as u32;
    if u < 0x80 {
        return (0x20..0x7f).contains(&u);
    }
    // C1 controls, soft hyphen, and a few format characters are not printable in Go.
    if (0x80..0xa0).contains(&u) || u == 0xad {
        return false;
    }
    !matches!(
        u,
        0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x206f | 0xfeff | 0xfff9..=0xfffb
    ) && !c.is_control()
        && !(0xe000..=0xf8ff).contains(&u)
        && !(0xd800..=0xdfff).contains(&u)
}

/// Go `path.Clean` / `filepath.Clean` on Unix.
pub fn clean_path(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    let rooted = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if let Some(last) = parts.last()
                    && *last != ".."
                {
                    parts.pop();
                    continue;
                }
                if !rooted {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    if rooted {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

/// Go `filepath.Base` on Unix.
pub fn base(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    match trimmed.rfind('/') {
        Some(i) => trimmed[i + 1..].to_string(),
        None => trimmed.to_string(),
    }
}

/// Go `filepath.Dir` on Unix.
pub fn dir(p: &str) -> String {
    let i = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    clean_path(&p[..i])
}

/// Go `filepath.Join` on Unix.
pub fn join(elems: &[&str]) -> String {
    let non_empty: Vec<&str> = elems.iter().copied().filter(|e| !e.is_empty()).collect();
    if non_empty.is_empty() {
        return String::new();
    }
    clean_path(&non_empty.join("/"))
}

/// Go `http.StatusText`.
pub fn status_text(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Request Entity Too Large",
        414 => "Request URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Requested Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Entity",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "",
    }
}

/// Go `net.SplitHostPort`.
pub fn split_host_port(hostport: &str) -> Result<(String, String), String> {
    let missing_port = || format!("address {hostport}: missing port in address");
    let too_many = || format!("address {hostport}: too many colons in address");
    let i = hostport.rfind(':').ok_or_else(missing_port)?;
    let (host, j, k);
    if hostport.starts_with('[') {
        let end = hostport
            .find(']')
            .ok_or_else(|| format!("address {hostport}: missing ']' in address"))?;
        if end + 1 == hostport.len() {
            return Err(missing_port());
        } else if end + 1 != i {
            if hostport.as_bytes()[end + 1] == b':' {
                return Err(too_many());
            }
            return Err(missing_port());
        }
        host = hostport[1..end].to_string();
        j = 1;
        k = end + 1;
    } else {
        host = hostport[..i].to_string();
        if host.contains(':') {
            return Err(too_many());
        }
        j = 0;
        k = 0;
    }
    if hostport[j..].contains('[') {
        return Err(format!("address {hostport}: unexpected '[' in address"));
    }
    if hostport[k..].contains(']') {
        return Err(format!("address {hostport}: unexpected ']' in address"));
    }
    Ok((host, hostport[i + 1..].to_string()))
}

/// Go `net.JoinHostPort`.
pub fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Render a JSON value the way Go's `fmt.Sprintf("%v", v)` renders the result of
/// unmarshalling it into `interface{}`.
pub fn go_fmt_json_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "<nil>".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => go_fmt_float(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => {
            let parts: Vec<String> = a.iter().map(go_fmt_json_value).collect();
            format!("[{}]", parts.join(" "))
        }
        serde_json::Value::Object(o) => {
            // serde_json::Map without preserve_order iterates in sorted order, as Go's
            // fmt does for maps.
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", k, go_fmt_json_value(&o[k])))
                .collect();
            format!("map[{}]", parts.join(" "))
        }
    }
}

/// Go `%v` for a float64 (`strconv.FormatFloat(f, 'g', -1, 64)` semantics).
pub fn go_fmt_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "+Inf".into()
        } else {
            "-Inf".into()
        };
    }
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    // Shortest repr digits via Rust's `{:e}` formatting.
    let s = format!("{f:e}");
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let neg = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    // strconv.FormatFloat(f, 'g', -1, 64): shortest digits, %e when exp < -4 || exp >= 6.
    let nd = digits.len() as i32;
    let eprec = 6;
    if exp < -4 || exp >= eprec {
        // d.ddddde±dd
        out.push_str(&digits[..1]);
        if nd > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        if exp < 0 {
            out.push('-');
        } else {
            out.push('+');
        }
        let a = exp.abs();
        if a < 10 {
            out.push('0');
        }
        out.push_str(&a.to_string());
        return out;
    }
    // %f with shortest digits
    let dp = exp + 1; // position of decimal point relative to digits
    if dp <= 0 {
        out.push_str("0.");
        for _ in 0..(-dp) {
            out.push('0');
        }
        out.push_str(&digits);
    } else if dp >= nd {
        out.push_str(&digits);
        for _ in 0..(dp - nd) {
            out.push('0');
        }
    } else {
        out.push_str(&digits[..dp as usize]);
        out.push('.');
        out.push_str(&digits[dp as usize..]);
    }
    out
}

/// Go `encoding/json` string encoding (with HTML escaping, as `json.Marshal` does).
pub fn json_string(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                let u = c as u32;
                out.push_str("\\u");
                for shift in [12, 8, 4, 0] {
                    out.push(HEX[((u >> shift) & 0xf) as usize] as char);
                }
            }
            c if (c as u32) < 0x20 => {
                let u = c as u32;
                out.push_str("\\u00");
                out.push(HEX[(u >> 4) as usize] as char);
                out.push(HEX[(u & 0xf) as usize] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Look up a struct field in a JSON object the way Go's `encoding/json` does:
/// exact key match first, otherwise a case-insensitive match.
pub fn json_field<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Option<&'a serde_json::Value> {
    if let Some(v) = obj.get(name) {
        return Some(v);
    }
    obj.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

/// Go's name for a JSON value kind in `UnmarshalTypeError` messages.
pub fn json_kind(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(_) => "bool".into(),
        serde_json::Value::Number(_) => "number".into(),
        serde_json::Value::String(_) => "string".into(),
        serde_json::Value::Array(_) => "array".into(),
        serde_json::Value::Object(_) => "object".into(),
    }
}

/// Decode an optional Go `string` struct field: `null`/missing → None, wrong type → Go
/// `UnmarshalTypeError` message.
pub fn json_string_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    go_struct: &str,
    name: &str,
) -> Result<Option<String>, String> {
    match json_field(obj, name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(format!(
            "json: cannot unmarshal {} into Go struct field {go_struct}.{name} of type string",
            json_kind(other)
        )),
    }
}

/// Map a `serde_json` parse failure of `data` to Go's `encoding/json` syntax error message
/// where the common cases can be reproduced.
pub fn json_syntax_error(data: &[u8], err: &serde_json::Error) -> String {
    let trimmed: Vec<u8> = data
        .iter()
        .copied()
        .skip_while(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        .collect();
    match trimmed.first() {
        None => "unexpected end of JSON input".to_string(),
        Some(&c)
            if !matches!(
                c,
                b'{' | b'[' | b'"' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n'
            ) =>
        {
            format!(
                "invalid character {} looking for beginning of value",
                quote_char(c)
            )
        }
        _ => {
            if err.is_eof() {
                "unexpected end of JSON input".to_string()
            } else {
                err.to_string()
            }
        }
    }
}

/// Go's `quoteChar` used in JSON syntax errors.
fn quote_char(c: u8) -> String {
    match c {
        b'\'' => "'\\''".to_string(),
        b'"' => "'\"'".to_string(),
        c if (0x20..0x7f).contains(&c) => format!("'{}'", c as char),
        c => {
            let q = quote(&(c as char).to_string());
            format!("'{}'", &q[1..q.len() - 1])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_matches_go() {
        assert_eq!(clean_path("./"), ".");
        assert_eq!(clean_path("./etc/"), "etc");
        assert_eq!(clean_path("var//log/../lib/./x"), "var/lib/x");
        assert_eq!(clean_path("/abs/path"), "/abs/path");
        assert_eq!(clean_path("/../x"), "/x");
        assert_eq!(clean_path("../x"), "../x");
        assert_eq!(clean_path("a/../.."), "..");
        assert_eq!(dir("etc/.wh.x"), "etc");
        assert_eq!(dir("x"), ".");
        assert_eq!(dir("/x"), "/");
        assert_eq!(base("etc/.wh.x"), ".wh.x");
        assert_eq!(join(&[".", "x"]), "x");
    }

    #[test]
    fn floats_match_go() {
        assert_eq!(go_fmt_float(1.5), "1.5");
        assert_eq!(go_fmt_float(1.0), "1");
        assert_eq!(go_fmt_float(100.0), "100");
        assert_eq!(go_fmt_float(1e21), "1e+21");
        assert_eq!(go_fmt_float(123456789.0), "1.23456789e+08");
        assert_eq!(go_fmt_float(0.0001), "0.0001");
        assert_eq!(go_fmt_float(0.00001), "1e-05");
        assert_eq!(go_fmt_float(1e6), "1e+06");
        assert_eq!(go_fmt_float(999999.0), "999999");
        assert_eq!(go_fmt_float(12345.678), "12345.678");
    }

    #[test]
    fn host_port() {
        assert_eq!(split_host_port("a:1").unwrap(), ("a".into(), "1".into()));
        assert_eq!(
            split_host_port("[::1]:1").unwrap(),
            ("::1".into(), "1".into())
        );
        assert!(split_host_port("a").is_err());
        assert_eq!(join_host_port("::1", "443"), "[::1]:443");
    }
}
