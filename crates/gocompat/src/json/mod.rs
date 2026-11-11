//! Go `encoding/json` behaviour.
//!
//! * [`marshal`] / [`marshal_indent`]: `json.Marshal` / `json.MarshalIndent` of the Go `any`
//!   value equivalent to a [`serde_json::Value`] (`map[string]any` keys sorted bytewise,
//!   `float64` numbers formatted like Go, `int64`/`uint64` numbers printed as integers,
//!   HTML-safe string escaping).
//! * [`to_go_json`]: `json.Marshal` of a typed Go struct mirrored by a Rust `Serialize` type
//!   (field order is the serde order; strings/floats use Go's encoding).
//! * [`unmarshal_any`] / [`unmarshal_object`]: `json.Unmarshal` into `any` /
//!   `map[string]any`, including Go's syntax and type error messages.
//! * [`indent`], [`compact`], [`html_escape`], [`valid`], [`check_valid`].

mod scanner;

use std::fmt;
use std::io;

use serde::Serialize;
use serde_json::{Map, Number, Value};

use scanner::{Op, Scanner};
pub use scanner::{SyntaxError, check_valid, valid};

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Bytes Go's encoder may copy through unchanged when HTML escaping is on (`htmlSafeSet`).
fn html_safe(b: u8) -> bool {
    b >= 0x20 && b != b'"' && b != b'\\' && b != b'<' && b != b'>' && b != b'&' && b < 0x80
}

/// `safeSet` (used when HTML escaping is off).
fn safe(b: u8) -> bool {
    b >= 0x20 && b != b'"' && b != b'\\' && b < 0x80
}

/// Go `appendString`: appends the JSON string encoding of the Go string `src`.
pub fn append_string(dst: &mut Vec<u8>, src: &[u8], escape_html: bool) {
    dst.push(b'"');
    let mut start = 0;
    let mut i = 0;
    while i < src.len() {
        let b = src[i];
        if b < 0x80 {
            if html_safe(b) || (!escape_html && safe(b)) {
                i += 1;
                continue;
            }
            dst.extend_from_slice(&src[start..i]);
            match b {
                b'\\' | b'"' => {
                    dst.push(b'\\');
                    dst.push(b);
                }
                0x08 => dst.extend_from_slice(b"\\b"),
                0x0C => dst.extend_from_slice(b"\\f"),
                b'\n' => dst.extend_from_slice(b"\\n"),
                b'\r' => dst.extend_from_slice(b"\\r"),
                b'\t' => dst.extend_from_slice(b"\\t"),
                _ => {
                    dst.extend_from_slice(b"\\u00");
                    dst.push(HEX[(b >> 4) as usize]);
                    dst.push(HEX[(b & 0xF) as usize]);
                }
            }
            i += 1;
            start = i;
            continue;
        }
        let (c, size) = crate::utf8::decode_rune(&src[i..]);
        if c == crate::utf8::RUNE_ERROR && size == 1 {
            dst.extend_from_slice(&src[start..i]);
            dst.extend_from_slice(b"\\ufffd");
            i += size;
            start = i;
            continue;
        }
        if c == 0x2028 || c == 0x2029 {
            dst.extend_from_slice(&src[start..i]);
            dst.extend_from_slice(b"\\u202");
            dst.push(HEX[(c & 0xF) as usize]);
            i += size;
            start = i;
            continue;
        }
        i += size;
    }
    dst.extend_from_slice(&src[start..]);
    dst.push(b'"');
}

/// Go `floatEncoder` (bits 32 or 64). The caller must ensure `f` is finite (Go returns an
/// `UnsupportedValueError` otherwise).
// Kept as written in Go (`abs < 1e-6 || abs >= 1e21`); the range form differs for NaN.
#[allow(clippy::manual_range_contains)]
pub fn append_float(dst: &mut Vec<u8>, f: f64, bits: i64) {
    let abs = f.abs();
    let mut fmt = b'f';
    if abs != 0.0
        && (bits == 64 && (abs < 1e-6 || abs >= 1e21)
            || bits == 32 && ((abs as f32) < 1e-6 || (abs as f32) >= 1e21))
    {
        fmt = b'e';
    }
    let start = dst.len();
    crate::strconv::append_float(dst, f, fmt, -1, bits);
    if fmt == b'e' {
        // clean up e-09 to e-9
        let n = dst.len();
        if n - start >= 4 && dst[n - 4] == b'e' && dst[n - 3] == b'-' && dst[n - 2] == b'0' {
            dst[n - 2] = dst[n - 1];
            dst.truncate(n - 1);
        }
    }
}

/// Go's encoding of a JSON number held in a `serde_json::Number`: integers print as Go
/// `int64`/`uint64`, everything else as Go `float64`.
pub fn append_number(dst: &mut Vec<u8>, n: &Number) {
    if let Some(i) = n.as_i64() {
        dst.extend_from_slice(i.to_string().as_bytes());
    } else if let Some(u) = n.as_u64() {
        dst.extend_from_slice(u.to_string().as_bytes());
    } else if let Some(f) = n.as_f64() {
        append_float(dst, f, 64);
    } else {
        // arbitrary-precision numbers (not produced by this crate): copy verbatim
        dst.extend_from_slice(n.to_string().as_bytes());
    }
}

fn encode_value(dst: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Null => dst.extend_from_slice(b"null"),
        Value::Bool(true) => dst.extend_from_slice(b"true"),
        Value::Bool(false) => dst.extend_from_slice(b"false"),
        Value::Number(n) => append_number(dst, n),
        Value::String(s) => append_string(dst, s.as_bytes(), true),
        Value::Array(a) => {
            dst.push(b'[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    dst.push(b',');
                }
                encode_value(dst, e);
            }
            dst.push(b']');
        }
        Value::Object(m) => {
            dst.push(b'{');
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    dst.push(b',');
                }
                append_string(dst, k.as_bytes(), true);
                dst.push(b':');
                encode_value(dst, &m[k]);
            }
            dst.push(b'}');
        }
    }
}

/// Go `json.Marshal` of the `any` value equivalent to `v`.
pub fn marshal(v: &Value) -> Vec<u8> {
    let mut dst = Vec::new();
    encode_value(&mut dst, v);
    dst
}

/// Go `json.MarshalIndent` (fails, like Go, only for nesting deeper than 10000).
pub fn marshal_indent(v: &Value, prefix: &str, indent_str: &str) -> Result<Vec<u8>, SyntaxError> {
    indent(&marshal(v), prefix, indent_str)
}

fn append_newline(dst: &mut Vec<u8>, prefix: &str, indent: &str, depth: usize) {
    dst.push(b'\n');
    dst.extend_from_slice(prefix.as_bytes());
    for _ in 0..depth {
        dst.extend_from_slice(indent.as_bytes());
    }
}

/// Go `json.Indent`.
pub fn indent(src: &[u8], prefix: &str, indent: &str) -> Result<Vec<u8>, SyntaxError> {
    let mut dst = Vec::with_capacity(2 * src.len());
    let mut scan = Scanner::new();
    let mut need_indent = false;
    let mut depth: usize = 0;
    for &c in src {
        scan.bytes += 1;
        let v = scan.step(c);
        if v == Op::SkipSpace {
            continue;
        }
        if v == Op::Error {
            break;
        }
        if need_indent && v != Op::EndObject && v != Op::EndArray {
            need_indent = false;
            depth += 1;
            append_newline(&mut dst, prefix, indent, depth);
        }
        if v == Op::Continue {
            dst.push(c);
            continue;
        }
        match c {
            b'{' | b'[' => {
                need_indent = true;
                dst.push(c);
            }
            b',' => {
                dst.push(c);
                append_newline(&mut dst, prefix, indent, depth);
            }
            b':' => {
                dst.push(c);
                dst.push(b' ');
            }
            b'}' | b']' => {
                if need_indent {
                    need_indent = false;
                } else {
                    depth = depth.saturating_sub(1);
                    append_newline(&mut dst, prefix, indent, depth);
                }
                dst.push(c);
            }
            _ => dst.push(c),
        }
    }
    if scan.eof() == Op::Error {
        return Err(scan.err.take().unwrap_or(SyntaxError {
            msg: String::new(),
            offset: 0,
        }));
    }
    Ok(dst)
}

/// Go `appendCompact(dst, src, escape)`: `json.Compact` when `escape` is false; with
/// `escape` true this is what Go's encoder applies to `MarshalJSON` / `RawMessage` output.
pub fn compact(src: &[u8], escape: bool) -> Result<Vec<u8>, SyntaxError> {
    let mut dst = Vec::with_capacity(src.len());
    let mut scan = Scanner::new();
    let mut start = 0;
    for (i, &c) in src.iter().enumerate() {
        if escape && (c == b'<' || c == b'>' || c == b'&') {
            if start < i {
                dst.extend_from_slice(&src[start..i]);
            }
            dst.extend_from_slice(b"\\u00");
            dst.push(HEX[(c >> 4) as usize]);
            dst.push(HEX[(c & 0xF) as usize]);
            start = i + 1;
        }
        if escape && c == 0xE2 && i + 2 < src.len() && src[i + 1] == 0x80 && src[i + 2] & !1 == 0xA8
        {
            if start < i {
                dst.extend_from_slice(&src[start..i]);
            }
            dst.extend_from_slice(b"\\u202");
            dst.push(HEX[(src[i + 2] & 0xF) as usize]);
            start = i + 3;
        }
        let v = scan.step(c);
        if v >= Op::SkipSpace {
            if v == Op::Error {
                break;
            }
            if start < i {
                dst.extend_from_slice(&src[start..i]);
            }
            start = i + 1;
        }
    }
    if scan.eof() == Op::Error {
        return Err(scan.err.take().unwrap_or(SyntaxError {
            msg: String::new(),
            offset: 0,
        }));
    }
    if start < src.len() {
        dst.extend_from_slice(&src[start..]);
    }
    Ok(dst)
}

/// Go `json.HTMLEscape`.
pub fn html_escape(src: &[u8]) -> Vec<u8> {
    let mut dst = Vec::with_capacity(src.len());
    let mut start = 0;
    for (i, &c) in src.iter().enumerate() {
        if c == b'<' || c == b'>' || c == b'&' {
            dst.extend_from_slice(&src[start..i]);
            dst.extend_from_slice(b"\\u00");
            dst.push(HEX[(c >> 4) as usize]);
            dst.push(HEX[(c & 0xF) as usize]);
            start = i + 1;
        }
        if c == 0xE2 && i + 2 < src.len() && src[i + 1] == 0x80 && src[i + 2] & !1 == 0xA8 {
            dst.extend_from_slice(&src[start..i]);
            dst.extend_from_slice(b"\\u202");
            dst.push(HEX[(src[i + 2] & 0xF) as usize]);
            start = i + 3;
        }
    }
    dst.extend_from_slice(&src[start..]);
    dst
}

// ---------------------------------------------------------------------------------------------
// Typed marshalling through serde
// ---------------------------------------------------------------------------------------------

/// A `serde_json` [`Formatter`](serde_json::ser::Formatter) producing Go's string and float
/// encoding (HTML escaping on, as in `json.Marshal`).
#[derive(Debug, Clone, Copy, Default)]
pub struct GoFormatter;

impl serde_json::ser::Formatter for GoFormatter {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        let mut b = Vec::new();
        append_float(&mut b, value, 64);
        writer.write_all(&b)
    }

    fn write_f32<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        let mut b = Vec::new();
        append_float(&mut b, value as f64, 32);
        writer.write_all(&b)
    }

    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        // serde_json only hands over runs without '"', '\\' and control characters; apply the
        // remaining Go escapes (<, >, &, U+2028, U+2029).
        writer.write_all(&html_escape(fragment.as_bytes()))
    }

    fn write_char_escape<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        char_escape: serde_json::ser::CharEscape,
    ) -> io::Result<()> {
        use serde_json::ser::CharEscape;
        let b: Vec<u8> = match char_escape {
            CharEscape::Quote => b"\\\"".to_vec(),
            CharEscape::ReverseSolidus => b"\\\\".to_vec(),
            CharEscape::Solidus => b"/".to_vec(),
            CharEscape::Backspace => b"\\b".to_vec(),
            CharEscape::FormFeed => b"\\f".to_vec(),
            CharEscape::LineFeed => b"\\n".to_vec(),
            CharEscape::CarriageReturn => b"\\r".to_vec(),
            CharEscape::Tab => b"\\t".to_vec(),
            CharEscape::AsciiControl(c) => {
                vec![
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[(c >> 4) as usize],
                    HEX[(c & 0xF) as usize],
                ]
            }
        };
        writer.write_all(&b)
    }
}

/// `json.Marshal` of a typed Go value mirrored by `v`: fields are emitted in serde order
/// (declare Rust fields in Go field order), maps in their iteration order (use `BTreeMap`
/// for Go maps, whose keys Go sorts), strings and floats with Go's encoding.
///
/// Differences from Go that the caller must handle in the type definition: `[]byte` must be
/// serialized as a base64 string (see [`go_bytes`]), and non-finite floats serialize as `null`
/// instead of failing.
pub fn to_go_json<T: Serialize + ?Sized>(v: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, GoFormatter);
    v.serialize(&mut ser)?;
    Ok(out)
}

/// `json.MarshalIndent` of a typed value (see [`to_go_json`]).
pub fn to_go_json_indent<T: Serialize + ?Sized>(
    v: &T,
    prefix: &str,
    indent_str: &str,
) -> Result<Vec<u8>, serde_json::Error> {
    let b = to_go_json(v)?;
    indent(&b, prefix, indent_str).map_err(serde::ser::Error::custom)
}

/// Serde helpers encoding `Vec<u8>` like Go encodes `[]byte`: a standard base64 string
/// (`null` for `None` when used through `Option`). Use with `#[serde(with = "gocompat::json::go_bytes")]`.
pub mod go_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serialize bytes as a base64 string.
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::base64::encode_to_string(v))
    }

    /// Deserialize bytes from a base64 string (`null` decodes as empty).
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        match s {
            None => Ok(Vec::new()),
            Some(s) => crate::base64::decode_string(s.as_bytes()).map_err(serde::de::Error::custom),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Unmarshal
// ---------------------------------------------------------------------------------------------

/// Go `*json.UnmarshalTypeError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmarshalTypeError {
    /// Description of the JSON value: "bool", "array", "number -5", ...
    pub value: String,
    /// The Go type name, e.g. `float64`, `map[string]interface {}`.
    pub type_name: String,
    /// Byte offset.
    pub offset: i64,
    /// Struct name (empty when not decoding into a struct field).
    pub struct_name: String,
    /// Full path of the struct field.
    pub field: String,
}

impl fmt::Display for UnmarshalTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.struct_name.is_empty() || !self.field.is_empty() {
            return write!(
                f,
                "json: cannot unmarshal {} into Go struct field {}.{} of type {}",
                self.value, self.struct_name, self.field, self.type_name
            );
        }
        write!(
            f,
            "json: cannot unmarshal {} into Go value of type {}",
            self.value, self.type_name
        )
    }
}

impl std::error::Error for UnmarshalTypeError {}

/// Errors returned by [`unmarshal_any`] / [`unmarshal_object`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `*json.SyntaxError`.
    Syntax(SyntaxError),
    /// `*json.UnmarshalTypeError`.
    UnmarshalType(UnmarshalTypeError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Syntax(e) => e.fmt(f),
            Error::UnmarshalType(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<SyntaxError> for Error {
    fn from(e: SyntaxError) -> Self {
        Error::Syntax(e)
    }
}

fn getu4(s: &[u8]) -> i64 {
    if s.len() < 6 || s[0] != b'\\' || s[1] != b'u' {
        return -1;
    }
    let mut r: i64 = 0;
    for &c in &s[2..6] {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return -1,
        };
        r = r * 16 + d as i64;
    }
    r
}

/// Go `unquoteBytes`: decodes a quoted JSON string literal into a Rust string (Go coerces
/// to well-formed UTF-8, so the result is always valid).
pub fn unquote(s: &[u8]) -> Option<String> {
    if s.len() < 2 || s[0] != b'"' || s[s.len() - 1] != b'"' {
        return None;
    }
    let s = &s[1..s.len() - 1];
    let mut b: Vec<u8> = Vec::with_capacity(s.len());
    let mut r = 0;
    while r < s.len() {
        let c = s[r];
        if c == b'\\' {
            r += 1;
            if r >= s.len() {
                return None;
            }
            match s[r] {
                b'"' | b'\\' | b'/' | b'\'' => {
                    b.push(s[r]);
                    r += 1;
                }
                b'b' => {
                    b.push(0x08);
                    r += 1;
                }
                b'f' => {
                    b.push(0x0C);
                    r += 1;
                }
                b'n' => {
                    b.push(b'\n');
                    r += 1;
                }
                b'r' => {
                    b.push(b'\r');
                    r += 1;
                }
                b't' => {
                    b.push(b'\t');
                    r += 1;
                }
                b'u' => {
                    r -= 1;
                    let mut rr = getu4(&s[r..]);
                    if rr < 0 {
                        return None;
                    }
                    r += 6;
                    if (0xD800..0xE000).contains(&rr) {
                        let rr1 = getu4(&s[r..]);
                        if (0xD800..0xDC00).contains(&rr) && (0xDC00..0xE000).contains(&rr1) {
                            let dec = (((rr - 0xD800) << 10) | (rr1 - 0xDC00)) + 0x10000;
                            r += 6;
                            crate::utf8::append_rune(&mut b, dec as u32);
                            continue;
                        }
                        rr = 0xFFFD;
                    }
                    crate::utf8::append_rune(&mut b, rr as u32);
                }
                _ => return None,
            }
        } else if c == b'"' || c < b' ' {
            return None;
        } else if c < 0x80 {
            b.push(c);
            r += 1;
        } else {
            let (rr, size) = crate::utf8::decode_rune(&s[r..]);
            r += size;
            crate::utf8::append_rune(&mut b, rr);
        }
    }
    String::from_utf8(b).ok()
}

enum Frame {
    Array(Vec<Value>),
    Object(Map<String, Value>, Option<String>),
}

/// Decodes `data` (already validated) into a Value; returns the first saved error.
fn decode_any(data: &[u8]) -> Result<Value, Error> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut saved: Option<Error> = None;
    let mut i = 0;
    let n = data.len();
    let skip_ws = |i: &mut usize| {
        while *i < n && matches!(data[*i], b' ' | b'\t' | b'\r' | b'\n') {
            *i += 1;
        }
    };
    loop {
        skip_ws(&mut i);
        // Parse a value start.
        let value: Value;
        if i >= n {
            break;
        }
        match data[i] {
            b'{' => {
                i += 1;
                skip_ws(&mut i);
                if i < n && data[i] == b'}' {
                    i += 1;
                    value = Value::Object(Map::new());
                } else {
                    // read key
                    let (key, ni) = read_string(data, i);
                    i = ni;
                    skip_ws(&mut i);
                    i += 1; // ':'
                    stack.push(Frame::Object(Map::new(), Some(key)));
                    continue;
                }
            }
            b'[' => {
                i += 1;
                skip_ws(&mut i);
                if i < n && data[i] == b']' {
                    i += 1;
                    value = Value::Array(Vec::new());
                } else {
                    stack.push(Frame::Array(Vec::new()));
                    continue;
                }
            }
            b'"' => {
                let (s, ni) = read_string(data, i);
                i = ni;
                value = Value::String(s);
            }
            b't' => {
                i += 4;
                value = Value::Bool(true);
            }
            b'f' => {
                i += 5;
                value = Value::Bool(false);
            }
            b'n' => {
                i += 4;
                value = Value::Null;
            }
            _ => {
                let start = i;
                while i < n && matches!(data[i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    i += 1;
                }
                let lit = &data[start..i];
                match crate::strconv::parse_float(lit, 64) {
                    Ok(f) => {
                        value = Number::from_f64(f)
                            .map(Value::Number)
                            .unwrap_or(Value::Null)
                    }
                    Err(_) => {
                        if saved.is_none() {
                            saved = Some(Error::UnmarshalType(UnmarshalTypeError {
                                value: format!("number {}", String::from_utf8_lossy(lit)),
                                type_name: "float64".to_string(),
                                offset: i as i64,
                                struct_name: String::new(),
                                field: String::new(),
                            }));
                        }
                        value = Value::Null;
                    }
                }
            }
        }
        // Attach value to the parent, closing containers as needed.
        let mut value = value;
        loop {
            match stack.last_mut() {
                None => {
                    if let Some(e) = saved {
                        return Err(e);
                    }
                    return Ok(value);
                }
                Some(Frame::Array(a)) => {
                    a.push(value);
                    skip_ws(&mut i);
                    if i < n && data[i] == b',' {
                        i += 1;
                        break;
                    }
                    i += 1; // ']'
                    let Some(Frame::Array(a)) = stack.pop() else {
                        unreachable!()
                    };
                    value = Value::Array(a);
                }
                Some(Frame::Object(m, key)) => {
                    let k = key.take().unwrap_or_default();
                    m.insert(k, value);
                    skip_ws(&mut i);
                    if i < n && data[i] == b',' {
                        i += 1;
                        skip_ws(&mut i);
                        let (k, ni) = read_string(data, i);
                        i = ni;
                        skip_ws(&mut i);
                        i += 1; // ':'
                        *key = Some(k);
                        break;
                    }
                    i += 1; // '}'
                    let Some(Frame::Object(m, _)) = stack.pop() else {
                        unreachable!()
                    };
                    value = Value::Object(m);
                }
            }
        }
    }
    Ok(Value::Null)
}

fn read_string(data: &[u8], start: usize) -> (String, usize) {
    // data[start] == '"'; the literal is valid.
    let mut i = start + 1;
    while i < data.len() {
        match data[i] {
            b'\\' => i += 2,
            b'"' => {
                i += 1;
                break;
            }
            _ => i += 1,
        }
    }
    (unquote(&data[start..i]).unwrap_or_default(), i)
}

/// Go `json.Unmarshal(data, &v)` with `var v any`: numbers decode as `float64`.
pub fn unmarshal_any(data: &[u8]) -> Result<Value, Error> {
    check_valid(data)?;
    decode_any(data)
}

fn kind_name(data: &[u8]) -> &'static str {
    let mut i = 0;
    while i < data.len() && matches!(data[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    match data.get(i) {
        Some(b'[') => "array",
        Some(b'"') => "string",
        Some(b't') | Some(b'f') => "bool",
        Some(b'{') => "object",
        _ => "number",
    }
}

/// Go `json.Unmarshal(data, &m)` with `var m map[string]any`. `Ok(None)` is a nil map (JSON
/// `null`). Non-object JSON yields Go's `UnmarshalTypeError`.
pub fn unmarshal_object(data: &[u8]) -> Result<Option<Map<String, Value>>, Error> {
    check_valid(data)?;
    let v = decode_any(data);
    match kind_name(data) {
        "object" => match v? {
            Value::Object(m) => Ok(Some(m)),
            _ => Ok(None),
        },
        _ => {
            if let Ok(Value::Null) = v {
                return Ok(None);
            }
            let mut i = 0;
            while i < data.len() && matches!(data[i], b' ' | b'\t' | b'\r' | b'\n') {
                i += 1;
            }
            let kind = kind_name(data);
            // Go reports the offset after the literal for scalars and after '[' for arrays.
            let offset = if kind == "array" {
                (i + 1) as i64
            } else {
                let mut j = i;
                while j < data.len() && !matches!(data[j], b' ' | b'\t' | b'\r' | b'\n') {
                    j += 1;
                }
                j as i64
            };
            Err(Error::UnmarshalType(UnmarshalTypeError {
                value: kind.to_string(),
                type_name: "map[string]interface {}".to_string(),
                offset,
                struct_name: String::new(),
                field: String::new(),
            }))
        }
    }
}
