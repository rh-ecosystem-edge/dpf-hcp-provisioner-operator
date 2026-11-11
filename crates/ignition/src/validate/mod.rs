//! Port of `github.com/coreos/ignition/v2/config/validate.ValidateWithContext` and the parts
//! of `github.com/coreos/vcontext` (path, report, tree, validate, json) it uses.

pub(crate) mod dataurl;
pub(crate) mod gonet;
pub(crate) mod gopath;
pub(crate) mod rules;
pub(crate) mod systemd;

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::gotypes::{GoVal, SRef, StructDef, Ty, call_key};

/// One element of a `path.ContextPath`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathElem {
    /// A struct field (JSON name) or map key.
    Field(String),
    /// A slice index.
    Index(usize),
    /// A `tree.Key` (refers to the key itself rather than its value).
    Key(String),
}

impl fmt::Display for PathElem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathElem::Field(s) | PathElem::Key(s) => f.write_str(s),
            PathElem::Index(i) => write!(f, "{i}"),
        }
    }
}

impl From<&str> for PathElem {
    fn from(s: &str) -> Self {
        PathElem::Field(s.to_string())
    }
}

impl From<usize> for PathElem {
    fn from(i: usize) -> Self {
        PathElem::Index(i)
    }
}

/// `path.ContextPath` (tag is always `json`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextPath(pub Vec<PathElem>);

impl ContextPath {
    /// `c.Append(e)`.
    pub fn append(&self, e: impl Into<PathElem>) -> ContextPath {
        let mut p = self.0.clone();
        p.push(e.into());
        ContextPath(p)
    }
    fn append_all(&self, es: Vec<PathElem>) -> ContextPath {
        let mut p = self.0.clone();
        p.extend(es);
        ContextPath(p)
    }
    fn pop(&self) -> ContextPath {
        let mut p = self.0.clone();
        p.pop();
        ContextPath(p)
    }
}

/// Builds a [`ContextPath`] by appending several elements: `cp!(c, "storage", "files", i)`.
macro_rules! cp {
    ($c:expr $(, $e:expr)*) => {
        $c.append_all(vec![$(crate::validate::PathElem::from($e)),*])
    };
}
pub(crate) use cp;

impl fmt::Display for ContextPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("$")?;
        for e in &self.0 {
            write!(f, ".{e}")?;
        }
        Ok(())
    }
}

/// `report.Kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Error,
    Warn,
    Info,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Error => "error",
            Kind::Warn => "warning",
            Kind::Info => "info",
        })
    }
}

/// `report.Entry`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub message: String,
    pub context: ContextPath,
    /// Start (line, column) of the correlated JSON node, if any.
    pub marker: Option<(i64, i64)>,
}

impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = match (self.marker, self.context.0.is_empty()) {
            (Some((l, c)), false) => format!(" at {}, line {l} col {c}", self.context),
            (Some((l, c)), true) => format!(" at line {l} col {c}"),
            (None, false) => format!(" at {}", self.context),
            (None, true) => String::new(),
        };
        write!(f, "{}{}: {}", self.kind, at, self.message)
    }
}

/// `report.Report`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub entries: Vec<Entry>,
}

impl Report {
    pub(crate) fn merge(&mut self, child: Report) {
        self.entries.extend(child.entries);
    }

    fn add_on(&mut self, c: &ContextPath, err: Option<String>, kind: Kind) {
        if let Some(message) = err {
            self.entries.push(Entry {
                kind,
                message,
                context: c.clone(),
                marker: None,
            });
        }
    }

    /// `r.AddOnError(c, err)` (`None` = nil error).
    pub(crate) fn add_on_error(&mut self, c: &ContextPath, err: Option<String>) {
        self.add_on(c, err, Kind::Error);
    }

    /// `r.AddOnWarn(c, err)`.
    pub(crate) fn add_on_warn(&mut self, c: &ContextPath, err: Option<String>) {
        self.add_on(c, err, Kind::Warn);
    }

    /// `r.IsFatal()`.
    pub fn is_fatal(&self) -> bool {
        self.entries.iter().any(|e| e.kind == Kind::Error)
    }

    fn correlate(&mut self, n: &Node, line_col: &HashMap<usize, (i64, i64)>) {
        for e in &mut self.entries {
            let mut c = e.context.clone();
            let node = loop {
                if let Some(n) = n.get(&c.0) {
                    break n;
                }
                c = c.pop();
            };
            e.marker = node
                .start
                .and_then(|i| line_col.get(&i).copied())
                .or(Some((0, 0)));
        }
    }
}

impl fmt::Display for Report {
    /// `r.String()`: every entry followed by a newline.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for e in &self.entries {
            writeln!(f, "{e}")?;
        }
        Ok(())
    }
}

// -------------------------------------------------------------------------------------------
// vcontext tree (go-json node positions)
// -------------------------------------------------------------------------------------------

enum NodeKind {
    Map {
        children: HashMap<String, Node>,
        keys: HashMap<String, Node>,
    },
    Slice(Vec<Node>),
    Leaf,
}

struct Node {
    start: Option<usize>,
    kind: NodeKind,
}

impl Node {
    fn get(&self, path: &[PathElem]) -> Option<&Node> {
        let Some(head) = path.first() else {
            return Some(self);
        };
        match (&self.kind, head) {
            (NodeKind::Map { children, .. }, PathElem::Field(k)) => {
                children.get(k)?.get(&path[1..])
            }
            (NodeKind::Map { keys, .. }, PathElem::Key(k)) => keys.get(k)?.get(&path[1..]),
            (NodeKind::Slice(items), PathElem::Index(i)) => items.get(*i)?.get(&path[1..]),
            _ => None,
        }
    }

    fn positions(&self, out: &mut Vec<usize>) {
        if let Some(s) = self.start {
            out.push(s);
        }
        match &self.kind {
            NodeKind::Map { children, keys } => {
                for n in children.values().chain(keys.values()) {
                    n.positions(out);
                }
            }
            NodeKind::Slice(items) => {
                for n in items {
                    n.positions(out);
                }
            }
            NodeKind::Leaf => {}
        }
    }
}

/// Parses valid JSON into the go-json/vcontext node tree. Only start indices are kept (the
/// report only prints start positions). Object/array starts are the index after the
/// opening bracket; literal starts are the first byte; key starts are the opening quote.
struct TreeParser<'a> {
    d: &'a [u8],
    i: usize,
}

impl TreeParser<'_> {
    fn ws(&mut self) {
        while self.i < self.d.len() && matches!(self.d[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn literal_end(&mut self) {
        let d = self.d;
        match d[self.i] {
            b'"' => {
                self.i += 1;
                while self.i < d.len() {
                    match d[self.i] {
                        b'\\' => self.i += 2,
                        b'"' => {
                            self.i += 1;
                            return;
                        }
                        _ => self.i += 1,
                    }
                }
            }
            b't' | b'n' => self.i += 4,
            b'f' => self.i += 5,
            _ => {
                while self.i < d.len()
                    && matches!(d[self.i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    self.i += 1;
                }
            }
        }
    }

    fn value(&mut self) -> Node {
        match self.d[self.i] {
            b'{' => {
                self.i += 1;
                let start = self.i;
                let mut children = HashMap::new();
                let mut keys = HashMap::new();
                loop {
                    self.ws();
                    if self.d[self.i] == b'}' {
                        self.i += 1;
                        break;
                    }
                    let ks = self.i;
                    self.literal_end();
                    let key = gocompat::json::unquote(&self.d[ks..self.i]).unwrap_or_default();
                    self.ws();
                    self.i += 1; // :
                    self.ws();
                    let v = self.value();
                    keys.insert(
                        key.clone(),
                        Node {
                            start: Some(ks),
                            kind: NodeKind::Leaf,
                        },
                    );
                    children.insert(key, v);
                    self.ws();
                    if self.d[self.i] == b',' {
                        self.i += 1;
                    }
                }
                Node {
                    start: Some(start),
                    kind: NodeKind::Map { children, keys },
                }
            }
            b'[' => {
                self.i += 1;
                let start = self.i;
                let mut items = Vec::new();
                loop {
                    self.ws();
                    if self.d[self.i] == b']' {
                        self.i += 1;
                        break;
                    }
                    items.push(self.value());
                    self.ws();
                    if self.d[self.i] == b',' {
                        self.i += 1;
                    }
                }
                Node {
                    start: Some(start),
                    kind: NodeKind::Slice(items),
                }
            }
            _ => {
                let start = self.i;
                self.literal_end();
                Node {
                    start: Some(start),
                    kind: NodeKind::Leaf,
                }
            }
        }
    }
}

/// `json.UnmarshalToContext` + `tree.FixLineColumn`: the tree and the line/column of every
/// node start index.
type LineCols = HashMap<usize, (i64, i64)>;

fn unmarshal_to_context(raw: &[u8]) -> Option<(Node, LineCols)> {
    if gocompat::json::check_valid(raw).is_err() {
        return None;
    }
    let mut p = TreeParser { d: raw, i: 0 };
    p.ws();
    let root = p.value();
    let mut pos = Vec::new();
    root.positions(&mut pos);
    let wanted: HashSet<usize> = pos.into_iter().collect();
    let mut lc = HashMap::new();
    let (mut line, mut col) = (1i64, 1i64);
    for (i, &c) in raw.iter().enumerate() {
        if wanted.contains(&i) {
            lc.insert(i, (line, col));
        }
        col += 1;
        if c == b'\n' {
            line += 1;
            col = 1;
        }
    }
    Some((root, lc))
}

// -------------------------------------------------------------------------------------------
// vcontext/validate walker
// -------------------------------------------------------------------------------------------

/// A flattened field (`validate.GetFields`).
struct FlatField<'a> {
    json: &'static str,
    ty: Ty,
    value: &'a GoVal,
}

fn get_fields<'a>(
    def: &'static StructDef,
    v: &'a GoVal,
    out: &mut Vec<FlatField<'a>>,
    go_names: &mut Vec<&'static str>,
) {
    for (f, fv) in def.fields.iter().zip(v.fields()) {
        if f.embedded {
            let Ty::Struct(ed) = f.ty else { unreachable!() };
            get_fields(ed, fv, out, go_names);
        } else {
            go_names.push(f.name);
            out.push(FlatField {
                json: f.json,
                ty: f.ty,
                value: fv,
            });
        }
    }
}

type CustomValidator<'a> = &'a dyn Fn(Ty, &GoVal, &ContextPath) -> Report;

fn walk(c: &ContextPath, ty: Ty, v: &GoVal, f: CustomValidator<'_>) -> Report {
    let mut r = Report::default();
    r.merge(f(ty, v, c));
    match (ty, v) {
        (Ty::Struct(d), _) => {
            let mut fields = Vec::new();
            get_fields(d, v, &mut fields, &mut Vec::new());
            for field in fields {
                r.merge(walk(&c.append(field.json), field.ty, field.value, f));
            }
        }
        (Ty::Slice(d), _) => {
            for (i, e) in v.as_slice().iter().enumerate() {
                r.merge(walk(&c.append(i), d.elem, e, f));
            }
        }
        // Pointer elements are primitives without Validate methods.
        _ => {}
    }
    r
}

/// `validate.DefaultValidator`: calls the value-receiver `Validate` method if the type has one.
fn default_validator(ty: Ty, v: &GoVal, c: &ContextPath) -> Report {
    match ty {
        Ty::Struct(d) => match d.validate {
            Some(f) => f(&SRef::new(d, v), c),
            None => Report::default(),
        },
        Ty::Str("types.Device") => rules::device(v.as_str(), c),
        _ => Report::default(),
    }
}

/// `validate.ValidateDups`.
fn validate_dups(ty: Ty, v: &GoVal, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let Ty::Struct(d) = ty else { return r };
    let mut dups_lists: HashMap<&'static str, HashSet<String>> = HashMap::new();
    let mut fields = Vec::new();
    let mut go_names = Vec::new();
    get_fields(d, v, &mut fields, &mut go_names);
    for (field, go_name) in fields.iter().zip(go_names) {
        let Ty::Slice(sd) = field.ty else { continue };
        if d.ignore_dups.contains(&go_name) {
            continue;
        }
        let list_name = d
            .merged_keys
            .iter()
            .find(|(k, _)| *k == go_name)
            .map(|(_, v)| *v)
            .unwrap_or(go_name);
        let dup_list = dups_lists.entry(list_name).or_default();
        for (i, item) in field.value.as_slice().iter().enumerate() {
            let key = call_key(sd.elem, item);
            if dup_list.contains(&key) {
                r.add_on_error(&cp!(c, field.json, i), Some(rules::ERR_DUPLICATE.into()));
            }
            dup_list.insert(key);
        }
    }
    r
}

/// `validate.ValidateUnusedKeys`.
fn validate_unused_keys(ty: Ty, v: &GoVal, c: &ContextPath, root: &Node) -> Report {
    let mut r = Report::default();
    let Ty::Struct(d) = ty else { return r };
    let Some(node) = root.get(&c.0) else { return r };
    let NodeKind::Map { keys, .. } = &node.kind else {
        r.add_on_warn(
            c,
            Some(format!(
                "context tree does not match content tree at {c}. Line and column reporting may be inconsistent. Unused keys may not be reported"
            )),
        );
        return r;
    };
    let mut fields = Vec::new();
    get_fields(d, v, &mut fields, &mut Vec::new());
    let names: HashSet<&str> = fields.iter().map(|f| f.json).collect();
    let mut unused: Vec<&String> = keys
        .keys()
        .filter(|k| !names.contains(k.as_str()))
        .collect();
    // Go iterates a map here (random order); sort for determinism.
    unused.sort();
    for k in unused {
        r.add_on_warn(
            &c.append(PathElem::Key(k.clone())),
            Some(format!("unused key {k}")),
        );
    }
    r
}

/// `validate.ValidateWithContext(cfg, raw)` for a v3_4 `types.Config` value.
pub(crate) fn validate_with_context(cfg: &GoVal, raw: Option<&[u8]>) -> Report {
    let root = ContextPath::default();
    let ty = crate::gotypes::CONFIG_TY;
    let mut r = walk(&root, ty, cfg, &default_validator);
    r.merge(walk(&root, ty, cfg, &validate_dups));
    let Some(raw) = raw else { return r };
    if let Some((tree, lc)) = unmarshal_to_context(raw) {
        let unused = |t: Ty, v: &GoVal, c: &ContextPath| validate_unused_keys(t, v, c, &tree);
        r.merge(walk(&root, ty, cfg, &unused));
        r.correlate(&tree, &lc);
    }
    r
}
