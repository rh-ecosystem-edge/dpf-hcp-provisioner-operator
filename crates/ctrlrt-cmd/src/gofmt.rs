//! Go formatting helpers: float formatting (`strconv.AppendFloat`, `fmt` `%v`), JSON string
//! escaping as done by zap's JSON encoder and by `encoding/json` (with `SetEscapeHTML(false)`),
//! and a Go `encoding/json`-compatible renderer for `serde_json::Value`.

use serde_json::Value;

/// Shortest decimal digits and decimal exponent of `f` (`f = 0.d1d2d3... * 10^(dp)`), as Go's
/// `strconv` computes them. Returns (negative, digits, dp). `f` must be finite.
fn shortest_digits(f: f64) -> (bool, String, i32) {
    // Rust's `{:e}` prints the shortest round-trip representation: "1.2345e-7".
    let s = format!("{:e}", f);
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r.to_string()),
        None => (false, s),
    };
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    if digits == "0" {
        return (neg, "0".into(), 1);
    }
    // mant = d.ddd  => value = 0.dddd * 10^(exp+1)
    (neg, digits, exp + 1)
}

/// `strconv.FormatFloat(f, 'f', -1, 64)`
pub fn format_float_f(f: f64) -> String {
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
    let (neg, digits, dp) = shortest_digits(f);
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if digits == "0" {
        out.push('0');
        return out;
    }
    let nd = digits.len() as i32;
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

/// `strconv.FormatFloat(f, 'e', -1, 64)`
pub fn format_float_e(f: f64) -> String {
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
    let (neg, digits, dp) = shortest_digits(f);
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push_str(&digits[..1]);
    if digits.len() > 1 {
        out.push('.');
        out.push_str(&digits[1..]);
    }
    let exp = if digits == "0" { 0 } else { dp - 1 };
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
    out
}

/// `fmt.Sprint(f)` / `%v` for a float64 (`%g` with shortest precision).
pub fn format_float_v(f: f64) -> String {
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
    let (_, digits, dp) = shortest_digits(f);
    let exp = if digits == "0" { 0 } else { dp - 1 };
    // Shortest precision: %e is used if the exponent is < -4 or >= 6 (eprec = 6).
    if !(-4..6).contains(&exp) {
        format_float_e(f)
    } else {
        format_float_f(f)
    }
}

/// `encoding/json` float64 encoding.
pub fn format_float_json(f: f64) -> String {
    let abs = f.abs();
    if abs != 0.0 && !(1e-6..1e21).contains(&abs) {
        let mut s = format_float_e(f);
        // clean up e-09 to e-9
        let n = s.len();
        if n >= 4 && &s[n - 4..n - 2] == "e-" && s.as_bytes()[n - 2] == b'0' {
            s.remove(n - 2);
        }
        s
    } else {
        format_float_f(f)
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// zap's `safeAddString`: JSON-escape without surrounding quotes.
pub fn zap_escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        let b = c as u32;
        if b < 0x80 {
            let b = b as u8;
            if b >= 0x20 && b != b'\\' && b != b'"' {
                out.push(b as char);
                continue;
            }
            match b {
                b'\\' | b'"' => {
                    out.push('\\');
                    out.push(b as char);
                }
                b'\n' => out.push_str("\\n"),
                b'\r' => out.push_str("\\r"),
                b'\t' => out.push_str("\\t"),
                _ => {
                    out.push_str("\\u00");
                    out.push(HEX[(b >> 4) as usize] as char);
                    out.push(HEX[(b & 0xF) as usize] as char);
                }
            }
        } else {
            out.push(c);
        }
    }
}

/// `encoding/json` string escaping with `SetEscapeHTML(false)`, without quotes.
pub fn json_escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        let b = c as u32;
        if b < 0x80 {
            let b = b as u8;
            if b >= 0x20 && b != b'\\' && b != b'"' {
                out.push(b as char);
                continue;
            }
            match b {
                b'\\' | b'"' => {
                    out.push('\\');
                    out.push(b as char);
                }
                b'\x08' => out.push_str("\\b"),
                b'\x0c' => out.push_str("\\f"),
                b'\n' => out.push_str("\\n"),
                b'\r' => out.push_str("\\r"),
                b'\t' => out.push_str("\\t"),
                _ => {
                    out.push_str("\\u00");
                    out.push(HEX[(b >> 4) as usize] as char);
                    out.push(HEX[(b & 0xF) as usize] as char);
                }
            }
        } else if c == '\u{2028}' || c == '\u{2029}' {
            out.push_str(if c == '\u{2028}' {
                "\\u2028"
            } else {
                "\\u2029"
            });
        } else {
            out.push(c);
        }
    }
}

/// `corev1.ObjectReference` JSON field order (Go struct order).
const OBJECT_REFERENCE_FIELDS: &[&str] = &[
    "kind",
    "namespace",
    "name",
    "uid",
    "apiVersion",
    "resourceVersion",
    "fieldPath",
];

/// Keys of `map` in the order Go would encode them. `serde_json::Map` cannot tell a Go struct
/// from a Go map apart, so objects are emitted in map order (sorted, which matches Go maps),
/// except for objects that look like a `corev1.ObjectReference` (all keys are
/// ObjectReference fields and at least one is not `name`/`namespace`), which use the Go
/// struct field order.
pub fn go_key_order(map: &serde_json::Map<String, Value>) -> Vec<&String> {
    let keys: Vec<&String> = map.keys().collect();
    let is_ref = !keys.is_empty()
        && keys
            .iter()
            .all(|k| OBJECT_REFERENCE_FIELDS.contains(&k.as_str()))
        && keys.iter().any(|k| *k != "name" && *k != "namespace");
    if is_ref {
        let mut out = Vec::with_capacity(keys.len());
        for f in OBJECT_REFERENCE_FIELDS {
            if let Some((k, _)) = map.get_key_value(*f) {
                out.push(k);
            }
        }
        return out;
    }
    keys
}

/// Render a JSON value the way Go's `encoding/json` (HTML escaping off) renders the
/// equivalent Go value.
pub fn json_value_into(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number_json(n)),
        Value::String(s) => {
            out.push('"');
            json_escape_into(out, s);
            out.push('"');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_value_into(out, e);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, k) in go_key_order(m).into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('"');
                json_escape_into(out, k);
                out.push_str("\":");
                json_value_into(out, &m[k]);
            }
            out.push('}');
        }
    }
}

fn number_json(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        format_float_json(n.as_f64().unwrap_or(0.0))
    }
}

/// Render a top-level number as zap does (`zap.Int*` / `zap.Float64`).
pub fn number_zap(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        let f = n.as_f64().unwrap_or(0.0);
        if f.is_nan() {
            "\"NaN\"".into()
        } else if f.is_infinite() {
            if f > 0.0 {
                "\"+Inf\"".into()
            } else {
                "\"-Inf\"".into()
            }
        } else {
            format_float_f(f)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats() {
        // Values checked against Go's fmt.Println / strconv.
        let v = [
            (1704067200.123456, "1.704067200123456e+09"),
            (1e6, "1e+06"),
            (123456789.0, "1.23456789e+08"),
            (100000.0, "100000"),
            (1704067200123.456, "1.704067200123456e+12"),
            (0.00001, "1e-05"),
            (1e20, "1e+20"),
            (1e21, "1e+21"),
            (1.5, "1.5"),
            (0.0, "0"),
        ];
        for (f, want) in v {
            assert_eq!(format_float_v(f), want, "{f}");
        }
        assert_eq!(format_float_f(1704067200.5), "1704067200.5");
        assert_eq!(format_float_f(1e21), "1000000000000000000000");
        assert_eq!(format_float_f(0.000001), "0.000001");
        assert_eq!(format_float_f(-2.5), "-2.5");
        assert_eq!(format_float_json(1e21), "1e+21");
        assert_eq!(format_float_json(1e-7), "1e-7");
        assert_eq!(format_float_json(0.1), "0.1");
        assert_eq!(format_float_json(100.0), "100");
    }
}
