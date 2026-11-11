//! `sigs.k8s.io/yaml` v1.6.0 on top of `go.yaml.in/yaml/v2` v2.4.3.
//!
//! * [`marshal`]: `yaml.Marshal(obj)` where `json.Marshal(obj)` is [`crate::json::marshal`] of
//!   the given value (i.e. `JSONToYAML(json.Marshal(obj))`), byte-exact including key
//!   ordering (yaml.v2's natural sort), quoting/style selection, line folding at 80 columns,
//!   number formatting and the errors yaml.v2 reports while re-reading the JSON
//!   (control characters, keys longer than 1024 characters, nesting deeper than 10000).
//! * [`json_to_yaml`]: `yaml.JSONToYAML` for JSON input.
//! * [`yaml_to_json`] / [`unmarshal_any`]: `yaml.YAMLToJSON` / `yaml.Unmarshal` into `any` for
//!   the block/flow YAML subset produced by yaml.v2's emitter (and plain JSON); anchors,
//!   aliases, tags, merge keys and multi-document streams are rejected.

mod emitter;
mod parse;
mod resolve;

use std::fmt;

use serde_json::Value;

use crate::unicode::{is_digit, is_letter};

/// A yaml.v2 `interface{}` value (as produced by `yaml.Unmarshal`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Str(String),
    Seq(Vec<Node>),
    /// Map entries; keys are scalars.
    Map(Vec<(Node, Node)>),
}

/// A YAML error carrying Go's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------------------------------------
// keyList.Less (sorter.go)
// ---------------------------------------------------------------------------------------------

fn key_less_str(a: &str, b: &str) -> bool {
    let ar = crate::utf8::runes(a.as_bytes());
    let br = crate::utf8::runes(b.as_bytes());
    let mut i = 0;
    while i < ar.len() && i < br.len() {
        if ar[i] == br[i] {
            i += 1;
            continue;
        }
        let al = is_letter(ar[i]);
        let bl = is_letter(br[i]);
        if al && bl {
            return ar[i] < br[i];
        }
        if al || bl {
            return bl;
        }
        let mut an: i64 = 0;
        let mut bn: i64 = 0;
        if ar[i] == '0' as u32 || br[i] == '0' as u32 {
            let mut j = i as isize - 1;
            while j >= 0 && is_digit(ar[j as usize]) {
                if ar[j as usize] != '0' as u32 {
                    an = 1;
                    bn = 1;
                    break;
                }
                j -= 1;
            }
        }
        let mut ai = i;
        while ai < ar.len() && is_digit(ar[ai]) {
            an = an
                .wrapping_mul(10)
                .wrapping_add((ar[ai] as i64).wrapping_sub('0' as i64));
            ai += 1;
        }
        let mut bi = i;
        while bi < br.len() && is_digit(br[bi]) {
            bn = bn
                .wrapping_mul(10)
                .wrapping_add((br[bi] as i64).wrapping_sub('0' as i64));
            bi += 1;
        }
        if an != bn {
            return an < bn;
        }
        if ai != bi {
            return ai < bi;
        }
        return ar[i] < br[i];
    }
    ar.len() < br.len()
}

#[derive(PartialEq, PartialOrd)]
enum KeyKind {
    Bool = 1,
    Int = 2,
    Uint = 11,
    Float = 14,
    Other = 20,
    Str = 24,
}

fn key_kind(n: &Node) -> KeyKind {
    match n {
        Node::Bool(_) => KeyKind::Bool,
        Node::Int(_) => KeyKind::Int,
        Node::Uint(_) => KeyKind::Uint,
        Node::Float(_) => KeyKind::Float,
        Node::Str(_) => KeyKind::Str,
        _ => KeyKind::Other,
    }
}

fn key_float(n: &Node) -> Option<f64> {
    match n {
        Node::Int(i) => Some(*i as f64),
        Node::Uint(u) => Some(*u as f64),
        Node::Float(f) => Some(*f),
        Node::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn key_less(a: &Node, b: &Node) -> bool {
    let (ak, bk) = (key_kind(a), key_kind(b));
    if let (Some(af), Some(bf)) = (key_float(a), key_float(b)) {
        if af != bf {
            return af < bf;
        }
        if ak != bk {
            return ak < bk;
        }
        return match (a, b) {
            (Node::Int(x), Node::Int(y)) => x < y,
            (Node::Uint(x), Node::Uint(y)) => x < y,
            (Node::Float(x), Node::Float(y)) => x < y,
            (Node::Bool(x), Node::Bool(y)) => !x && *y,
            _ => false,
        };
    }
    if ak != KeyKind::Str || bk != KeyKind::Str {
        return ak < bk;
    }
    match (a, b) {
        (Node::Str(x), Node::Str(y)) => key_less_str(x, y),
        _ => false,
    }
}

/// Stable merge sort with a "less" predicate that never panics, even if the predicate is
/// not a strict weak order (yaml.v2's key comparison is not always one).
fn sort_entries(v: &mut Vec<(Node, Node)>) {
    if v.len() < 2 {
        return;
    }
    let mut items: Vec<(Node, Node)> = std::mem::take(v);
    let mid = items.len() / 2;
    let mut right = items.split_off(mid);
    let mut left = items;
    sort_entries(&mut left);
    sort_entries(&mut right);
    let mut out = Vec::with_capacity(left.len() + right.len());
    let mut li = left.into_iter().peekable();
    let mut ri = right.into_iter().peekable();
    loop {
        match (li.peek(), ri.peek()) {
            (Some(l), Some(r)) => {
                if key_less(&r.0, &l.0) {
                    out.extend(ri.next());
                } else {
                    out.extend(li.next());
                }
            }
            (Some(_), None) => out.extend(li.next()),
            (None, Some(_)) => out.extend(ri.next()),
            (None, None) => break,
        }
    }
    *v = out;
}

// ---------------------------------------------------------------------------------------------
// JSON -> yaml.v2 node (yaml.Unmarshal of the JSON document into interface{})
// ---------------------------------------------------------------------------------------------

const MAX_FLOW_LEVEL: usize = 10000;

/// Chars yaml.v2's reader accepts.
pub(crate) fn reader_allowed(c: char) -> bool {
    let v = c as u32;
    v == 0x09
        || v == 0x0A
        || v == 0x0D
        || (0x20..=0x7E).contains(&v)
        || v == 0x85
        || (0xA0..=0xD7FF).contains(&v)
        || (0xE000..=0xFFFD).contains(&v)
        || (0x10000..=0x10FFFF).contains(&v)
}

pub(crate) fn number_node(text: &str) -> Node {
    match resolve::resolve(text) {
        resolve::Resolved::Int(i) => Node::Int(i),
        resolve::Resolved::Uint(u) => Node::Uint(u),
        resolve::Resolved::Float(f) => Node::Float(f),
        resolve::Resolved::Null => Node::Null,
        resolve::Resolved::Bool(b) => Node::Bool(b),
        _ => Node::Str(text.to_string()),
    }
}

pub(crate) fn line_prefix(line: usize) -> String {
    if line == 0 {
        String::new()
    } else {
        format!("line {line}: ")
    }
}

fn sort_tree(n: &mut Node) {
    match n {
        Node::Seq(items) => items.iter_mut().for_each(sort_tree),
        Node::Map(entries) => {
            for (_, v) in entries.iter_mut() {
                sort_tree(v);
            }
            sort_entries(entries);
        }
        _ => {}
    }
}

fn emit(mut node: Node) -> Vec<u8> {
    sort_tree(&mut node);
    let mut e = emitter::Emitter::new();
    e.emit_document(&node);
    e.out
}

/// `sigs.k8s.io/yaml.Marshal(obj)` where `json.Marshal(obj)` produces the JSON encoding of
/// `v` (see [`crate::json::marshal`]).
pub fn marshal(v: &Value) -> Result<Vec<u8>, Error> {
    json_to_yaml(&crate::json::marshal(v))
}

/// `sigs.k8s.io/yaml.Marshal` of a typed value: `json.Marshal` is emulated with
/// [`crate::json::to_go_json`], then [`json_to_yaml`].
pub fn marshal_typed<T: serde::Serialize + ?Sized>(v: &T) -> Result<Vec<u8>, Error> {
    let j = crate::json::to_go_json(v)
        .map_err(|e| Error(format!("error marshaling into JSON: {e}")))?;
    json_to_yaml(&j)
}

/// `sigs.k8s.io/yaml.JSONToYAML` for JSON input (yaml.v2 reads the JSON as a flow-style YAML
/// document; number literals keep their exact text through yaml.v2's resolver).
pub fn json_to_yaml(j: &[u8]) -> Result<Vec<u8>, Error> {
    let node = parse::parse_json_as_yaml(j)?;
    Ok(emit(node))
}

// ---------------------------------------------------------------------------------------------
// YAML -> JSON
// ---------------------------------------------------------------------------------------------

fn float_key_string(f: f64) -> String {
    let s = crate::strconv::format_float(f, b'g', -1, 32);
    match s.as_str() {
        "+Inf" => ".inf".to_string(),
        "-Inf" => "-.inf".to_string(),
        "NaN" => ".nan".to_string(),
        _ => s,
    }
}

fn go_debug_value(n: &Node) -> String {
    match n {
        Node::Null => "<nil>".to_string(),
        Node::Bool(b) => b.to_string(),
        Node::Int(i) => i.to_string(),
        Node::Uint(u) => format!("0x{u:x}"),
        Node::Float(f) => crate::strconv::format_float(*f, b'g', -1, 64),
        Node::Str(s) => crate::strconv::quote(s),
        Node::Seq(_) => "[]interface {}{...}".to_string(),
        Node::Map(_) => "map[interface {}]interface {}{...}".to_string(),
    }
}

/// `convertToJSONableObject` + `json.Marshal`.
fn node_to_json(n: &Node, dst: &mut Vec<u8>) -> Result<(), Error> {
    match n {
        Node::Null => dst.extend_from_slice(b"null"),
        Node::Bool(b) => dst.extend_from_slice(if *b { b"true" } else { b"false" }),
        Node::Int(i) => dst.extend_from_slice(i.to_string().as_bytes()),
        Node::Uint(u) => dst.extend_from_slice(u.to_string().as_bytes()),
        Node::Float(f) => {
            if !f.is_finite() {
                return Err(Error(format!(
                    "json: unsupported value: {}",
                    crate::strconv::format_float(*f, b'g', -1, 64)
                )));
            }
            crate::json::append_float(dst, *f, 64);
        }
        Node::Str(s) => crate::json::append_string(dst, s.as_bytes(), true),
        Node::Seq(items) => {
            dst.push(b'[');
            for (i, e) in items.iter().enumerate() {
                if i > 0 {
                    dst.push(b',');
                }
                node_to_json(e, dst)?;
            }
            dst.push(b']');
        }
        Node::Map(entries) => {
            let mut m: std::collections::BTreeMap<String, &Node> =
                std::collections::BTreeMap::new();
            for (k, v) in entries {
                let ks = match k {
                    Node::Str(s) => s.clone(),
                    Node::Int(i) => i.to_string(),
                    Node::Uint(u) => u.to_string(),
                    Node::Float(f) => float_key_string(*f),
                    Node::Bool(b) => b.to_string(),
                    other => {
                        let ty = match other {
                            Node::Null => "<nil>",
                            Node::Seq(_) => "[]interface {}",
                            _ => "map[interface {}]interface {}",
                        };
                        return Err(Error(format!(
                            "unsupported map key of type: {}, key: {}, value: {}",
                            ty,
                            go_debug_value(other),
                            go_debug_value(v)
                        )));
                    }
                };
                m.insert(ks, v);
            }
            dst.push(b'{');
            for (i, (k, v)) in m.iter().enumerate() {
                if i > 0 {
                    dst.push(b',');
                }
                crate::json::append_string(dst, k.as_bytes(), true);
                dst.push(b':');
                node_to_json(v, dst)?;
            }
            dst.push(b'}');
        }
    }
    Ok(())
}

/// `sigs.k8s.io/yaml.YAMLToJSON` (supported YAML subset, see module docs).
pub fn yaml_to_json(y: &[u8]) -> Result<Vec<u8>, Error> {
    let node = parse::parse_yaml(y)?;
    let mut out = Vec::new();
    node_to_json(&node, &mut out)?;
    Ok(out)
}

/// `sigs.k8s.io/yaml.Unmarshal(y, &v)` with `var v any` (numbers become `float64`).
/// Errors carry sigs.k8s.io/yaml's wrapping (`error converting YAML to JSON: ...`).
pub fn unmarshal_any(y: &[u8]) -> Result<Value, Error> {
    let j = yaml_to_json(y).map_err(|e| Error(format!("error converting YAML to JSON: {e}")))?;
    crate::json::unmarshal_any(&j)
        .map_err(|e| Error(format!("error unmarshaling JSON: while decoding JSON: {e}")))
}
