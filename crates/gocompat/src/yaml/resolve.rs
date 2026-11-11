//! Port of `go.yaml.in/yaml/v2` resolve.go (implicit tag resolution of plain scalars) and the
//! timestamp / base-60 helpers it depends on.

use crate::strconv;

/// The result of resolving a plain scalar.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Resolved {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Merge,
    Timestamp,
    Str,
}

fn lookup(s: &str) -> Option<Resolved> {
    Some(match s {
        "y" | "Y" | "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on" | "On" | "ON" => Resolved::Bool(true),
        "n" | "N" | "no" | "No" | "NO" | "false" | "False" | "FALSE" | "off" | "Off" | "OFF" => {
            Resolved::Bool(false)
        }
        "" | "~" | "null" | "Null" | "NULL" => Resolved::Null,
        ".nan" | ".NaN" | ".NAN" => Resolved::Float(f64::NAN),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => Resolved::Float(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => Resolved::Float(f64::NEG_INFINITY),
        "<<" => Resolved::Merge,
        _ => return None,
    })
}

fn hint(c: u8) -> u8 {
    match c {
        b'+' | b'-' => b'S',
        b'0'..=b'9' => b'D',
        b'y' | b'Y' | b'n' | b'N' | b't' | b'T' | b'f' | b'F' | b'o' | b'O' | b'~' => b'M',
        b'.' => b'.',
        _ => 0,
    }
}

/// `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`
fn is_yaml_style_float(s: &[u8]) -> bool {
    let mut i = 0;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let st = *i;
        while *i < s.len() && s[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - st
    };
    if i < s.len() && s[i] == b'.' {
        i += 1;
        if digits(&mut i) == 0 {
            return false;
        }
    } else {
        if digits(&mut i) == 0 {
            return false;
        }
        if i < s.len() && s[i] == b'.' {
            i += 1;
            digits(&mut i);
        }
    }
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        i += 1;
        if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == s.len()
}

/// `isBase60Float` from encode.go:
/// `^[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+(?:\.[0-9_]*)?$`
pub(crate) fn is_base60_float(s: &str) -> bool {
    let s = s.as_bytes();
    if s.is_empty() {
        return false;
    }
    let c = s[0];
    if !(c == b'+' || c == b'-' || c.is_ascii_digit()) || !s.contains(&b':') {
        return false;
    }
    let mut i = 0;
    if s[i] == b'+' || s[i] == b'-' {
        i += 1;
    }
    if i >= s.len() || !s[i].is_ascii_digit() {
        return false;
    }
    i += 1;
    while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
        i += 1;
    }
    // (?::[0-5]?[0-9])+ — the regexp engine backtracks; a group is ':' followed by one or
    // two digits where a two-digit group starts with 0-5.
    let mut groups = 0;
    while i < s.len() && s[i] == b':' {
        let j = i + 1;
        if j < s.len() && s[j].is_ascii_digit() {
            if j + 1 < s.len() && s[j + 1].is_ascii_digit() && (b'0'..=b'5').contains(&s[j]) {
                // two digits possible; prefer two unless what follows only fits with one
                // (a third digit can never follow either way, so two is correct when valid).
                i = j + 2;
            } else {
                i = j + 1;
            }
            groups += 1;
        } else {
            return false;
        }
    }
    if groups == 0 {
        return false;
    }
    if i < s.len() && s[i] == b'.' {
        i += 1;
        while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
            i += 1;
        }
    }
    i == s.len()
}

// ----- time.Parse subset used by parseTimestamp -----

fn is_digit_at(s: &[u8], i: usize) -> bool {
    i < s.len() && s[i].is_ascii_digit()
}

/// Go `getnum(s, fixed)`.
fn getnum(s: &[u8], fixed: bool) -> Option<(i64, &[u8])> {
    if !is_digit_at(s, 0) {
        return None;
    }
    if !is_digit_at(s, 1) {
        if fixed {
            return None;
        }
        return Some(((s[0] - b'0') as i64, &s[1..]));
    }
    Some((((s[0] - b'0') as i64) * 10 + (s[1] - b'0') as i64, &s[2..]))
}

fn days_in(month: i64, year: i64) -> i64 {
    match month {
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sep {
    UpperT,
    LowerT,
    Space,
}

/// One of the layouts in `allowedTimestampFormats`.
fn parse_layout(s: &[u8], time: Option<(Sep, bool)>) -> bool {
    // "2006-1-2"
    if s.len() < 4 {
        return false;
    }
    let mut v = s;
    let year_digits = &v[..4];
    if !year_digits.iter().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let year: i64 = std::str::from_utf8(year_digits).ok().and_then(|y| y.parse().ok()).unwrap_or(0);
    v = &v[4..];
    let lit = |v: &mut &[u8], c: u8| -> bool {
        if v.first() == Some(&c) {
            *v = &v[1..];
            true
        } else {
            false
        }
    };
    if !lit(&mut v, b'-') {
        return false;
    }
    let Some((month, rest)) = getnum(v, false) else { return false };
    v = rest;
    if !(1..=12).contains(&month) {
        return false;
    }
    if !lit(&mut v, b'-') {
        return false;
    }
    let Some((day, rest)) = getnum(v, false) else { return false };
    v = rest;
    if !(0..=31).contains(&day) {
        return false;
    }
    if let Some((sep, tz)) = time {
        match sep {
            Sep::UpperT => {
                if !lit(&mut v, b'T') {
                    return false;
                }
            }
            Sep::LowerT => {
                if !lit(&mut v, b't') {
                    return false;
                }
            }
            Sep::Space => {
                // skip(): a space in the layout matches one or more spaces (or end of input)
                if !v.is_empty() && v[0] != b' ' {
                    return false;
                }
                while v.first() == Some(&b' ') {
                    v = &v[1..];
                }
            }
        }
        let Some((hour, rest)) = getnum(v, false) else { return false };
        v = rest;
        if !(0..24).contains(&hour) {
            return false;
        }
        if !lit(&mut v, b':') {
            return false;
        }
        let Some((min, rest)) = getnum(v, false) else { return false };
        v = rest;
        if !(0..60).contains(&min) {
            return false;
        }
        if !lit(&mut v, b':') {
            return false;
        }
        let Some((sec, rest)) = getnum(v, false) else { return false };
        v = rest;
        if !(0..60).contains(&sec) {
            return false;
        }
        // stdFracSecond9 (".999999999"): optional fraction introduced by '.' or ','.
        if v.len() >= 2 && (v[0] == b'.' || v[0] == b',') && v[1].is_ascii_digit() {
            let mut i = 0;
            while i + 1 < v.len() && v[i + 1].is_ascii_digit() {
                i += 1;
            }
            v = &v[1 + i..];
        }
        if tz {
            // stdISO8601ColonTZ ("Z07:00")
            if v.first() == Some(&b'Z') {
                v = &v[1..];
            } else {
                if v.len() < 6 || v[3] != b':' {
                    return false;
                }
                let sign = v[0];
                let (Some((hr, _)), Some((mm, _))) = (getnum(&v[1..3], true), getnum(&v[4..6], true)) else {
                    return false;
                };
                if hr > 24 || mm > 60 {
                    return false;
                }
                if sign != b'+' && sign != b'-' {
                    return false;
                }
                v = &v[6..];
            }
        }
    }
    if !v.is_empty() {
        return false;
    }
    day >= 1 && day <= days_in(month, year)
}

/// `parseTimestamp(s)` reports whether `s` parses with one of `allowedTimestampFormats`.
pub(crate) fn is_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i != 4 || i == b.len() || b[i] != b'-' {
        return false;
    }
    parse_layout(b, Some((Sep::UpperT, true)))
        || parse_layout(b, Some((Sep::LowerT, true)))
        || parse_layout(b, Some((Sep::Space, false)))
        || parse_layout(b, None)
}

/// Go `resolve("", in)`.
pub(crate) fn resolve(input: &str) -> Resolved {
    let h = if input.is_empty() { b'N' } else { hint(input.as_bytes()[0]) };
    if h == 0 {
        return Resolved::Str;
    }
    if let Some(r) = lookup(input) {
        return r;
    }
    match h {
        b'.' => {
            if let Ok(f) = strconv::parse_float(input, 64) {
                return Resolved::Float(f);
            }
        }
        b'D' | b'S' => {
            if is_timestamp(input) {
                return Resolved::Timestamp;
            }
            let plain = input.replace('_', "");
            if let Ok(i) = strconv::parse_int(&plain, 0, 64) {
                return Resolved::Int(i);
            }
            if let Ok(u) = strconv::parse_uint(&plain, 0, 64) {
                return Resolved::Uint(u);
            }
            if is_yaml_style_float(plain.as_bytes())
                && let Ok(f) = strconv::parse_float(&plain, 64)
            {
                return Resolved::Float(f);
            }
            if let Some(rest) = plain.strip_prefix("0b") {
                if let Ok(i) = strconv::parse_int(rest, 2, 64) {
                    return Resolved::Int(i);
                }
                if let Ok(u) = strconv::parse_uint(rest, 2, 64) {
                    return Resolved::Uint(u);
                }
            } else if let Some(rest) = plain.strip_prefix("-0b")
                && let Ok(i) = strconv::parse_int(format!("-{rest}"), 2, 64)
            {
                return Resolved::Int(i);
            }
        }
        _ => {}
    }
    Resolved::Str
}
