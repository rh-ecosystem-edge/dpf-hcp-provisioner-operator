//! Go `encoding/json` decoding of JSON into the small set of Go struct types this package
//! unmarshals (`dpfdefaults.DPUServiceTemplateValues` and `imageReferences`), reproducing:
//!
//! * merge semantics (`json.Decoder.Decode` into an already populated struct only overwrites
//!   the fields present in the input; `null` leaves strings/ints/structs untouched),
//! * field matching (exact JSON name first, then Go's case-insensitive `foldName`),
//! * `DisallowUnknownFields`,
//! * Go's error messages (`*json.SyntaxError`, `*json.UnmarshalTypeError` with the
//!   struct/field context, `json: unknown field "x"`), reporting the first error in document
//!   order like Go's `saveError`.
//!
//! The decoded value is held as a `serde_json::Value` shaped like the Go struct and converted
//! to the Rust mirror type with serde afterwards.

use serde_json::{Map, Value};

/// A parsed JSON value keeping what Go's decoder sees: number literals verbatim and duplicate
/// object keys in document order.
#[derive(Debug, Clone)]
pub(crate) enum Raw {
    Null,
    Bool,
    Number(String),
    String(String),
    Array(Vec<Raw>),
    Object(Vec<(String, Raw)>),
}

/// Parse `data` (validated first with Go's scanner, so syntax errors are Go's).
pub(crate) fn parse(data: &[u8]) -> Result<Raw, String> {
    gocompat::json::check_valid(data).map_err(|e| e.to_string())?;
    let mut p = Parser { data, pos: 0 };
    Ok(p.value())
}

struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.data.len()
            && matches!(self.data[self.pos], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> u8 {
        self.skip_ws();
        self.data.get(self.pos).copied().unwrap_or(0)
    }

    /// The input is valid JSON, so the grammar can be followed without error handling.
    fn value(&mut self) -> Raw {
        match self.peek() {
            b'{' => {
                self.pos += 1;
                let mut members = Vec::new();
                if self.peek() == b'}' {
                    self.pos += 1;
                    return Raw::Object(members);
                }
                loop {
                    self.skip_ws();
                    let key = self.string();
                    self.peek(); // ':'
                    self.pos += 1;
                    let v = self.value();
                    members.push((key, v));
                    match self.peek() {
                        b',' => self.pos += 1,
                        _ => {
                            self.pos += 1; // '}'
                            return Raw::Object(members);
                        }
                    }
                }
            }
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                if self.peek() == b']' {
                    self.pos += 1;
                    return Raw::Array(items);
                }
                loop {
                    items.push(self.value());
                    match self.peek() {
                        b',' => self.pos += 1,
                        _ => {
                            self.pos += 1; // ']'
                            return Raw::Array(items);
                        }
                    }
                }
            }
            b'"' => Raw::String(self.string()),
            b't' => {
                self.pos += 4;
                Raw::Bool
            }
            b'f' => {
                self.pos += 5;
                Raw::Bool
            }
            b'n' => {
                self.pos += 4;
                Raw::Null
            }
            _ => {
                let start = self.pos;
                while self.pos < self.data.len()
                    && matches!(
                        self.data[self.pos],
                        b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'
                    )
                {
                    self.pos += 1;
                }
                Raw::Number(String::from_utf8_lossy(&self.data[start..self.pos]).into_owned())
            }
        }
    }

    /// A string literal starting at the current position (decoded like Go's `unquoteBytes`).
    fn string(&mut self) -> String {
        let start = self.pos;
        let mut i = start + 1;
        while i < self.data.len() {
            match self.data[i] {
                b'\\' => i += 2,
                b'"' => {
                    i += 1;
                    break;
                }
                _ => i += 1,
            }
        }
        self.pos = i;
        gocompat::json::unquote(&self.data[start..i]).unwrap_or_default()
    }
}

/// The Go types a value can be decoded into.
#[derive(Debug)]
pub(crate) enum GoType {
    String,
    Int32,
    /// A struct: `name` is `reflect.Type.Name()` ("" for anonymous structs), `type_string` is
    /// `reflect.Type.String()`.
    Struct {
        name: &'static str,
        type_string: &'static str,
        fields: &'static [(&'static str, GoType)],
    },
    /// A slice (`type_string` is `reflect.Type.String()`).
    Slice {
        type_string: &'static str,
        elem: &'static GoType,
    },
}

impl GoType {
    fn type_string(&self) -> &'static str {
        match self {
            GoType::String => "string",
            GoType::Int32 => "int32",
            GoType::Struct { type_string, .. } | GoType::Slice { type_string, .. } => type_string,
        }
    }

    /// The Go zero value.
    pub(crate) fn zero(&self) -> Value {
        match self {
            GoType::String => Value::String(String::new()),
            GoType::Int32 => Value::from(0),
            GoType::Struct { fields, .. } => Value::Object(
                fields
                    .iter()
                    .map(|(n, t)| (n.to_string(), t.zero()))
                    .collect::<Map<String, Value>>(),
            ),
            GoType::Slice { .. } => Value::Null,
        }
    }
}

/// `decodeState.errorContext`
#[derive(Clone, Default)]
struct ErrCtx {
    /// `Struct` (`Some("")` for an anonymous struct).
    struct_name: Option<&'static str>,
    field_stack: Vec<&'static str>,
}

struct Decoder {
    disallow_unknown_fields: bool,
    saved: Option<String>,
}

impl Decoder {
    /// `saveError`: keep the first error only.
    fn save(&mut self, msg: String) {
        if self.saved.is_none() {
            self.saved = Some(msg);
        }
    }

    /// `*UnmarshalTypeError` with `addErrorContext`.
    fn type_error(&mut self, ctx: &ErrCtx, what: &str, ty: &GoType) {
        let msg = if ctx.struct_name.is_some() || !ctx.field_stack.is_empty() {
            format!(
                "json: cannot unmarshal {} into Go struct field {}.{} of type {}",
                what,
                ctx.struct_name.unwrap_or(""),
                ctx.field_stack.join("."),
                ty.type_string()
            )
        } else {
            format!(
                "json: cannot unmarshal {} into Go value of type {}",
                what,
                ty.type_string()
            )
        };
        self.save(msg);
    }

    fn decode(&mut self, raw: &Raw, ty: &GoType, target: &mut Value, ctx: &ErrCtx) {
        match raw {
            Raw::Null => {
                // null: only interfaces, pointers, maps and slices are reset.
                if matches!(ty, GoType::Slice { .. }) {
                    *target = Value::Null;
                }
            }
            Raw::Bool => self.type_error(ctx, "bool", ty),
            Raw::String(s) => match ty {
                GoType::String => *target = Value::String(s.clone()),
                _ => self.type_error(ctx, "string", ty),
            },
            Raw::Number(n) => match ty {
                GoType::Int32 => match n.parse::<i64>() {
                    Ok(v) if i32::try_from(v).is_ok() => *target = Value::from(v),
                    _ => self.type_error(ctx, &format!("number {n}"), ty),
                },
                _ => self.type_error(ctx, "number", ty),
            },
            Raw::Array(items) => match ty {
                GoType::Slice { elem, .. } => {
                    // The slices decoded here always start out nil, so every element starts
                    // from the zero value.
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        let mut v = elem.zero();
                        self.decode(item, elem, &mut v, ctx);
                        out.push(v);
                    }
                    *target = Value::Array(out);
                }
                _ => self.type_error(ctx, "array", ty),
            },
            Raw::Object(members) => match ty {
                GoType::Struct { name, fields, .. } => {
                    if !target.is_object() {
                        *target = ty.zero();
                    }
                    for (key, value) in members {
                        let field = fields.iter().find(|(n, _)| *n == key.as_str()).or_else(|| {
                            let folded = fold_name(key);
                            fields.iter().find(|(n, _)| fold_name(n) == folded)
                        });
                        match field {
                            Some((fname, ftype)) => {
                                let mut child = ctx.clone();
                                child.struct_name = Some(name);
                                child.field_stack.push(fname);
                                if let Some(slot) =
                                    target.as_object_mut().and_then(|m| m.get_mut(*fname))
                                {
                                    self.decode(value, ftype, slot, &child);
                                }
                            }
                            None => {
                                if self.disallow_unknown_fields {
                                    self.save(format!(
                                        "json: unknown field {}",
                                        gocompat::strconv::quote(key)
                                    ));
                                }
                            }
                        }
                    }
                }
                _ => self.type_error(ctx, "object", ty),
            },
        }
    }
}

/// Go `foldName`: ASCII letters upper-cased, other runes mapped through
/// `unicode.ToUpper(unicode.ToLower(r))`.
fn fold_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_ascii_uppercase()
            } else {
                let mut l = c.to_lowercase();
                let lower = match (l.next(), l.next()) {
                    (Some(x), None) => x,
                    _ => c,
                };
                let mut u = lower.to_uppercase();
                match (u.next(), u.next()) {
                    (Some(x), None) => x,
                    _ => lower,
                }
            }
        })
        .collect()
}

/// Decode `raw` into `target` (a value shaped like `ty`, merged in place). Returns the first
/// error Go would report.
pub(crate) fn decode_into(
    raw: &Raw,
    ty: &GoType,
    target: &mut Value,
    disallow_unknown_fields: bool,
) -> Result<(), String> {
    let mut d = Decoder {
        disallow_unknown_fields,
        saved: None,
    };
    d.decode(raw, ty, target, &ErrCtx::default());
    match d.saved {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// `json.Unmarshal(data, &v)` into a zero `ty`.
pub(crate) fn unmarshal(data: &[u8], ty: &GoType) -> Result<Value, String> {
    let raw = parse(data)?;
    let mut v = ty.zero();
    decode_into(&raw, ty, &mut v, false)?;
    Ok(v)
}

/// Go `json.Unmarshal(data, &m)` with `var m map[string]json.RawMessage`: `Ok(None)` for a
/// JSON `null` (nil map); duplicate keys keep the last value.
pub(crate) fn unmarshal_raw_message_map(data: &[u8]) -> Result<Option<Vec<(String, Raw)>>, String> {
    let raw = parse(data)?;
    let what = match raw {
        Raw::Null => return Ok(None),
        Raw::Object(members) => {
            let mut out: Vec<(String, Raw)> = Vec::new();
            for (k, v) in members {
                if let Some(slot) = out.iter_mut().find(|(ek, _)| *ek == k) {
                    slot.1 = v;
                } else {
                    out.push((k, v));
                }
            }
            return Ok(Some(out));
        }
        Raw::Bool => "bool",
        Raw::Number(_) => "number",
        Raw::String(_) => "string",
        Raw::Array(_) => "array",
    };
    Err(format!(
        "json: cannot unmarshal {what} into Go value of type map[string]json.RawMessage"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    static INNER: GoType = GoType::Struct {
        name: "Inner",
        type_string: "pkg.Inner",
        fields: &[("chartName", GoType::String), ("port", GoType::Int32)],
    };
    static OUTER: GoType = GoType::Struct {
        name: "Outer",
        type_string: "pkg.Outer",
        fields: &[(
            "ovn",
            GoType::Struct {
                name: "Inner",
                type_string: "pkg.Inner",
                fields: &[("chartName", GoType::String), ("port", GoType::Int32)],
            },
        )],
    };

    fn dec(s: &str, ty: &GoType, disallow: bool) -> Result<Value, String> {
        let raw = parse(s.as_bytes())?;
        let mut v = ty.zero();
        decode_into(&raw, ty, &mut v, disallow).map(|_| v)
    }

    #[test]
    fn go_messages() {
        assert_eq!(
            dec(r#"{"port":"x"}"#, &INNER, true).unwrap_err(),
            "json: cannot unmarshal string into Go struct field Inner.port of type int32"
        );
        assert_eq!(
            dec(r#"{"port":1.5}"#, &INNER, true).unwrap_err(),
            "json: cannot unmarshal number 1.5 into Go struct field Inner.port of type int32"
        );
        assert_eq!(
            dec(r#"{"ovn":{"chartName":{}}}"#, &OUTER, true).unwrap_err(),
            "json: cannot unmarshal object into Go struct field Inner.ovn.chartName of type string"
        );
        assert_eq!(
            dec(r#"{"ovn":5}"#, &OUTER, true).unwrap_err(),
            "json: cannot unmarshal number into Go struct field Outer.ovn of type pkg.Inner"
        );
        assert_eq!(
            dec(r#""x""#, &OUTER, true).unwrap_err(),
            "json: cannot unmarshal string into Go value of type pkg.Outer"
        );
        assert_eq!(
            dec(r#"{"bogus":1,"ovn":5}"#, &OUTER, true).unwrap_err(),
            "json: unknown field \"bogus\""
        );
        assert!(dec(r#"{"bogus":1}"#, &OUTER, false).is_ok());
        assert_eq!(
            dec(r#"{"OVN":{"CHARTNAME":"a","port":null}}"#, &OUTER, true).unwrap(),
            serde_json::json!({"ovn": {"chartName": "a", "port": 0}})
        );
        assert_eq!(
            dec("{", &OUTER, true).unwrap_err(),
            "unexpected end of JSON input"
        );
    }
}
