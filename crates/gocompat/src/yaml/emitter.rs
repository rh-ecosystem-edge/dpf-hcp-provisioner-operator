//! Port of the parts of `go.yaml.in/yaml/v2` emitterc.go / encode.go used when marshalling
//! the `interface{}` trees produced by decoding JSON (maps, sequences, scalars; no anchors,
//! tags or explicit document markers).

use super::Node;
use super::resolve::{Resolved, is_base60_float, resolve};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Style {
    Plain,
    SingleQuoted,
    DoubleQuoted,
    Literal,
}

#[derive(Default)]
struct ScalarData {
    multiline: bool,
    flow_plain_allowed: bool,
    block_plain_allowed: bool,
    single_quoted_allowed: bool,
    block_allowed: bool,
}

pub(crate) struct Emitter {
    pub(crate) out: Vec<u8>,
    best_indent: i64,
    best_width: i64,
    indent: i64,
    indents: Vec<i64>,
    flow_level: i64,
    root_context: bool,
    sequence_context: bool,
    mapping_context: bool,
    simple_key_context: bool,
    column: i64,
    whitespace: bool,
    indention: bool,
    open_ended: bool,
    scalar: ScalarData,
}

fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

fn width(b: u8) -> usize {
    if b & 0x80 == 0x00 {
        1
    } else if b & 0xE0 == 0xC0 {
        2
    } else if b & 0xF0 == 0xE0 {
        3
    } else if b & 0xF8 == 0xF0 {
        4
    } else {
        0
    }
}

fn is_printable(b: &[u8], i: usize) -> bool {
    let c = at(b, i);
    let c1 = at(b, i + 1);
    let c2 = at(b, i + 2);
    c == 0x0A
        || (0x20..=0x7E).contains(&c)
        || (c == 0xC2 && c1 >= 0xA0)
        || (c > 0xC2 && c < 0xED)
        || (c == 0xED && c1 < 0xA0)
        || c == 0xEE
        || (c == 0xEF && !(c1 == 0xBB && c2 == 0xBF) && !(c1 == 0xBF && (c2 == 0xBE || c2 == 0xBF)))
}

fn is_bom(b: &[u8], _i: usize) -> bool {
    at(b, 0) == 0xEF && at(b, 1) == 0xBB && at(b, 2) == 0xBF
}

fn is_space(b: &[u8], i: usize) -> bool {
    at(b, i) == b' '
}

fn is_blank(b: &[u8], i: usize) -> bool {
    let c = at(b, i);
    c == b' ' || c == b'\t'
}

fn is_break(b: &[u8], i: usize) -> bool {
    let c = at(b, i);
    c == b'\r'
        || c == b'\n'
        || (c == 0xC2 && at(b, i + 1) == 0x85)
        || (c == 0xE2 && at(b, i + 1) == 0x80 && at(b, i + 2) == 0xA8)
        || (c == 0xE2 && at(b, i + 1) == 0x80 && at(b, i + 2) == 0xA9)
}

fn is_blankz(b: &[u8], i: usize) -> bool {
    is_blank(b, i) || is_break(b, i) || at(b, i) == 0
}

impl Emitter {
    pub(crate) fn new() -> Self {
        Emitter {
            out: Vec::new(),
            best_indent: 2,
            best_width: 80,
            indent: -1,
            indents: Vec::new(),
            flow_level: 0,
            root_context: false,
            sequence_context: false,
            mapping_context: false,
            simple_key_context: false,
            column: 0,
            whitespace: true,
            indention: true,
            open_ended: false,
            scalar: ScalarData::default(),
        }
    }

    fn put(&mut self, v: u8) {
        self.out.push(v);
        self.column += 1;
    }

    fn put_break(&mut self) {
        self.out.push(b'\n');
        self.column = 0;
    }

    fn write(&mut self, s: &[u8], i: &mut usize) {
        let w = width(s[*i]).max(1);
        let end = (*i + w).min(s.len());
        self.out.extend_from_slice(&s[*i..end]);
        self.column += 1;
        *i += w;
    }

    fn write_all(&mut self, s: &[u8]) {
        let mut i = 0;
        while i < s.len() {
            self.write(s, &mut i);
        }
    }

    fn write_break(&mut self, s: &[u8], i: &mut usize) {
        if s[*i] == b'\n' {
            self.put_break();
            *i += 1;
        } else {
            self.write(s, i);
            self.column = 0;
        }
    }

    fn increase_indent(&mut self, flow: bool, indentless: bool) {
        self.indents.push(self.indent);
        if self.indent < 0 {
            self.indent = if flow { self.best_indent } else { 0 };
        } else if !indentless {
            self.indent += self.best_indent;
        }
    }

    fn pop_indent(&mut self) {
        self.indent = self.indents.pop().unwrap_or(-1);
    }

    fn write_indent(&mut self) {
        let indent = self.indent.max(0);
        if !self.indention || self.column > indent || (self.column == indent && !self.whitespace) {
            self.put_break();
        }
        while self.column < indent {
            self.put(b' ');
        }
        self.whitespace = true;
        self.indention = true;
    }

    fn write_indicator(&mut self, indicator: &[u8], need_whitespace: bool, is_whitespace: bool, is_indention: bool) {
        if need_whitespace && !self.whitespace {
            self.put(b' ');
        }
        self.write_all(indicator);
        self.whitespace = is_whitespace;
        self.indention = self.indention && is_indention;
        self.open_ended = false;
    }

    fn analyze_scalar(&mut self, value: &[u8]) {
        let mut block_indicators = false;
        let mut flow_indicators = false;
        let mut line_breaks = false;
        let mut special_characters = false;
        let mut leading_space = false;
        let mut leading_break = false;
        let mut trailing_space = false;
        let mut trailing_break = false;
        let mut break_space = false;
        let mut space_break = false;
        let mut previous_space = false;
        let mut previous_break = false;

        if value.is_empty() {
            self.scalar = ScalarData {
                multiline: false,
                flow_plain_allowed: false,
                block_plain_allowed: true,
                single_quoted_allowed: true,
                block_allowed: false,
            };
            return;
        }
        if value.len() >= 3 && (value.starts_with(b"---") || value.starts_with(b"...")) {
            block_indicators = true;
            flow_indicators = true;
        }
        let mut preceded_by_whitespace = true;
        let mut i = 0;
        while i < value.len() {
            let w = width(value[i]).max(1);
            let followed_by_whitespace = i + w >= value.len() || is_blank(value, i + w);
            if i == 0 {
                match value[i] {
                    b'#' | b',' | b'[' | b']' | b'{' | b'}' | b'&' | b'*' | b'!' | b'|' | b'>' | b'\'' | b'"'
                    | b'%' | b'@' | b'`' => {
                        flow_indicators = true;
                        block_indicators = true;
                    }
                    b'?' | b':' => {
                        flow_indicators = true;
                        if followed_by_whitespace {
                            block_indicators = true;
                        }
                    }
                    b'-' if followed_by_whitespace => {
                        flow_indicators = true;
                        block_indicators = true;
                    }
                    _ => {}
                }
            } else {
                match value[i] {
                    b',' | b'?' | b'[' | b']' | b'{' | b'}' => flow_indicators = true,
                    b':' => {
                        flow_indicators = true;
                        if followed_by_whitespace {
                            block_indicators = true;
                        }
                    }
                    b'#' if preceded_by_whitespace => {
                        flow_indicators = true;
                        block_indicators = true;
                    }
                    _ => {}
                }
            }
            // emitter.unicode is true
            if !is_printable(value, i) {
                special_characters = true;
            }
            if is_space(value, i) {
                if i == 0 {
                    leading_space = true;
                }
                if i + w == value.len() {
                    trailing_space = true;
                }
                if previous_break {
                    break_space = true;
                }
                previous_space = true;
                previous_break = false;
            } else if is_break(value, i) {
                line_breaks = true;
                if i == 0 {
                    leading_break = true;
                }
                if i + w == value.len() {
                    trailing_break = true;
                }
                if previous_space {
                    space_break = true;
                }
                previous_space = false;
                previous_break = true;
            } else {
                previous_space = false;
                previous_break = false;
            }
            preceded_by_whitespace = is_blankz(value, i);
            i += w;
        }

        let mut d = ScalarData {
            multiline: line_breaks,
            flow_plain_allowed: true,
            block_plain_allowed: true,
            single_quoted_allowed: true,
            block_allowed: true,
        };
        if leading_space || leading_break || trailing_space || trailing_break {
            d.flow_plain_allowed = false;
            d.block_plain_allowed = false;
        }
        if trailing_space {
            d.block_allowed = false;
        }
        if break_space {
            d.flow_plain_allowed = false;
            d.block_plain_allowed = false;
            d.single_quoted_allowed = false;
        }
        if space_break || special_characters {
            d.flow_plain_allowed = false;
            d.block_plain_allowed = false;
            d.single_quoted_allowed = false;
            d.block_allowed = false;
        }
        if line_breaks {
            d.flow_plain_allowed = false;
            d.block_plain_allowed = false;
        }
        if flow_indicators {
            d.flow_plain_allowed = false;
        }
        if block_indicators {
            d.block_plain_allowed = false;
        }
        self.scalar = d;
    }

    fn select_scalar_style(&mut self, value: &[u8], requested: Style) -> Style {
        let mut style = requested;
        if self.simple_key_context && self.scalar.multiline {
            style = Style::DoubleQuoted;
        }
        if style == Style::Plain {
            if self.flow_level > 0 && !self.scalar.flow_plain_allowed
                || self.flow_level == 0 && !self.scalar.block_plain_allowed
            {
                style = Style::SingleQuoted;
            }
            if value.is_empty() && (self.flow_level > 0 || self.simple_key_context) {
                style = Style::SingleQuoted;
            }
        }
        if style == Style::SingleQuoted && !self.scalar.single_quoted_allowed {
            style = Style::DoubleQuoted;
        }
        if style == Style::Literal && (!self.scalar.block_allowed || self.flow_level > 0 || self.simple_key_context) {
            style = Style::DoubleQuoted;
        }
        style
    }

    fn write_plain_scalar(&mut self, value: &[u8], allow_breaks: bool) {
        if !self.whitespace {
            self.put(b' ');
        }
        let mut spaces = false;
        let mut breaks = false;
        let mut i = 0;
        while i < value.len() {
            if is_space(value, i) {
                if allow_breaks && !spaces && self.column > self.best_width && !is_space(value, i + 1) {
                    self.write_indent();
                    i += width(value[i]).max(1);
                } else {
                    self.write(value, &mut i);
                }
                spaces = true;
            } else if is_break(value, i) {
                if !breaks && value[i] == b'\n' {
                    self.put_break();
                }
                self.write_break(value, &mut i);
                self.indention = true;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                self.write(value, &mut i);
                self.indention = false;
                spaces = false;
                breaks = false;
            }
        }
        self.whitespace = false;
        self.indention = false;
        if self.root_context {
            self.open_ended = true;
        }
    }

    fn write_single_quoted_scalar(&mut self, value: &[u8], allow_breaks: bool) {
        self.write_indicator(b"'", true, false, false);
        let mut spaces = false;
        let mut breaks = false;
        let mut i = 0;
        while i < value.len() {
            if is_space(value, i) {
                if allow_breaks
                    && !spaces
                    && self.column > self.best_width
                    && i > 0
                    && i < value.len() - 1
                    && !is_space(value, i + 1)
                {
                    self.write_indent();
                    i += width(value[i]).max(1);
                } else {
                    self.write(value, &mut i);
                }
                spaces = true;
            } else if is_break(value, i) {
                if !breaks && value[i] == b'\n' {
                    self.put_break();
                }
                self.write_break(value, &mut i);
                self.indention = true;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                if value[i] == b'\'' {
                    self.put(b'\'');
                }
                self.write(value, &mut i);
                self.indention = false;
                spaces = false;
                breaks = false;
            }
        }
        self.write_indicator(b"'", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_double_quoted_scalar(&mut self, value: &[u8], allow_breaks: bool) {
        let mut spaces = false;
        self.write_indicator(b"\"", true, false, false);
        let mut i = 0;
        while i < value.len() {
            if !is_printable(value, i)
                || is_bom(value, i)
                || is_break(value, i)
                || value[i] == b'"'
                || value[i] == b'\\'
            {
                let octet = value[i];
                let (w, mut v): (usize, u32) = if octet & 0x80 == 0x00 {
                    (1, (octet & 0x7F) as u32)
                } else if octet & 0xE0 == 0xC0 {
                    (2, (octet & 0x1F) as u32)
                } else if octet & 0xF0 == 0xE0 {
                    (3, (octet & 0x0F) as u32)
                } else {
                    (4, (octet & 0x07) as u32)
                };
                for k in 1..w {
                    v = (v << 6) + (at(value, i + k) as u32 & 0x3F);
                }
                i += w;
                self.put(b'\\');
                match v {
                    0x00 => self.put(b'0'),
                    0x07 => self.put(b'a'),
                    0x08 => self.put(b'b'),
                    0x09 => self.put(b't'),
                    0x0A => self.put(b'n'),
                    0x0b => self.put(b'v'),
                    0x0c => self.put(b'f'),
                    0x0d => self.put(b'r'),
                    0x1b => self.put(b'e'),
                    0x22 => self.put(b'"'),
                    0x5c => self.put(b'\\'),
                    0x85 => self.put(b'N'),
                    0xA0 => self.put(b'_'),
                    0x2028 => self.put(b'L'),
                    0x2029 => self.put(b'P'),
                    _ => {
                        let digits = if v <= 0xFF {
                            self.put(b'x');
                            2
                        } else if v <= 0xFFFF {
                            self.put(b'u');
                            4
                        } else {
                            self.put(b'U');
                            8
                        };
                        let mut k: i32 = (digits - 1) * 4;
                        while k >= 0 {
                            let digit = ((v >> k) & 0x0F) as u8;
                            if digit < 10 {
                                self.put(digit + b'0');
                            } else {
                                self.put(digit + b'A' - 10);
                            }
                            k -= 4;
                        }
                    }
                }
                spaces = false;
            } else if is_space(value, i) {
                if allow_breaks && !spaces && self.column > self.best_width && i > 0 && i < value.len() - 1 {
                    self.write_indent();
                    if is_space(value, i + 1) {
                        self.put(b'\\');
                    }
                    i += width(value[i]).max(1);
                } else {
                    self.write(value, &mut i);
                }
                spaces = true;
            } else {
                self.write(value, &mut i);
                spaces = false;
            }
        }
        self.write_indicator(b"\"", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_block_scalar_hints(&mut self, value: &[u8]) {
        if is_space(value, 0) || is_break(value, 0) {
            let hint = [b'0' + self.best_indent as u8];
            self.write_indicator(&hint, false, false, false);
        }
        self.open_ended = false;
        let mut chomp: u8 = 0;
        if value.is_empty() {
            chomp = b'-';
        } else {
            let mut i = value.len() - 1;
            while i > 0 && value[i] & 0xC0 == 0x80 {
                i -= 1;
            }
            if !is_break(value, i) {
                chomp = b'-';
            } else if i == 0 {
                chomp = b'+';
                self.open_ended = true;
            } else {
                i -= 1;
                while i > 0 && value[i] & 0xC0 == 0x80 {
                    i -= 1;
                }
                if is_break(value, i) {
                    chomp = b'+';
                    self.open_ended = true;
                }
            }
        }
        if chomp != 0 {
            self.write_indicator(&[chomp], false, false, false);
        }
    }

    fn write_literal_scalar(&mut self, value: &[u8]) {
        self.write_indicator(b"|", true, false, false);
        self.write_block_scalar_hints(value);
        self.put_break();
        self.indention = true;
        self.whitespace = true;
        let mut breaks = true;
        let mut i = 0;
        while i < value.len() {
            if is_break(value, i) {
                self.write_break(value, &mut i);
                self.indention = true;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                self.write(value, &mut i);
                self.indention = false;
                breaks = false;
            }
        }
    }

    fn emit_scalar(&mut self, value: &[u8], requested: Style) {
        self.analyze_scalar(value);
        let style = self.select_scalar_style(value, requested);
        self.increase_indent(true, false);
        let allow_breaks = !self.simple_key_context;
        match style {
            Style::Plain => self.write_plain_scalar(value, allow_breaks),
            Style::SingleQuoted => self.write_single_quoted_scalar(value, allow_breaks),
            Style::DoubleQuoted => self.write_double_quoted_scalar(value, allow_breaks),
            Style::Literal => self.write_literal_scalar(value),
        }
        self.pop_indent();
    }

    /// Whether a key scalar can be written as a simple key (`yaml_emitter_check_simple_key`).
    fn check_simple_key(&mut self, key: &[u8]) -> bool {
        self.analyze_scalar(key);
        if self.scalar.multiline {
            return false;
        }
        key.len() <= 128
    }

    fn set_context(&mut self, root: bool, sequence: bool, mapping: bool, simple_key: bool) {
        self.root_context = root;
        self.sequence_context = sequence;
        self.mapping_context = mapping;
        self.simple_key_context = simple_key;
    }

    pub(crate) fn emit_node(&mut self, node: &Node, root: bool, sequence: bool, mapping: bool, simple_key: bool) {
        self.set_context(root, sequence, mapping, simple_key);
        match node {
            Node::Seq(items) => {
                if self.flow_level > 0 || items.is_empty() {
                    self.write_indicator(b"[", true, true, false);
                    self.increase_indent(true, false);
                    self.flow_level += 1;
                    for (n, item) in items.iter().enumerate() {
                        if n > 0 {
                            self.write_indicator(b",", false, false, false);
                        }
                        if self.column > self.best_width {
                            self.write_indent();
                        }
                        self.emit_node(item, false, true, false, false);
                    }
                    self.flow_level -= 1;
                    self.pop_indent();
                    self.write_indicator(b"]", false, false, false);
                } else {
                    let indentless = self.mapping_context && !self.indention;
                    self.increase_indent(false, indentless);
                    for item in items {
                        self.write_indent();
                        self.write_indicator(b"-", true, false, true);
                        self.emit_node(item, false, true, false, false);
                    }
                    self.pop_indent();
                }
            }
            Node::Map(entries) => {
                if self.flow_level > 0 || entries.is_empty() {
                    self.write_indicator(b"{", true, true, false);
                    self.increase_indent(true, false);
                    self.flow_level += 1;
                    for (n, (k, v)) in entries.iter().enumerate() {
                        if n > 0 {
                            self.write_indicator(b",", false, false, false);
                        }
                        if self.column > self.best_width {
                            self.write_indent();
                        }
                        let kb = scalar_text(k);
                        if self.check_simple_key(&kb) {
                            self.emit_node(k, false, false, true, true);
                            self.write_indicator(b":", false, false, false);
                        } else {
                            self.write_indicator(b"?", true, false, false);
                            self.emit_node(k, false, false, true, false);
                            if self.column > self.best_width {
                                self.write_indent();
                            }
                            self.write_indicator(b":", true, false, false);
                        }
                        self.emit_node(v, false, false, true, false);
                    }
                    self.flow_level -= 1;
                    self.pop_indent();
                    self.write_indicator(b"}", false, false, false);
                } else {
                    self.increase_indent(false, false);
                    for (k, v) in entries {
                        self.write_indent();
                        let kb = scalar_text(k);
                        if self.check_simple_key(&kb) {
                            self.emit_node(k, false, false, true, true);
                            self.write_indicator(b":", false, false, false);
                        } else {
                            self.write_indicator(b"?", true, false, true);
                            self.emit_node(k, false, false, true, false);
                            self.write_indent();
                            self.write_indicator(b":", true, false, true);
                        }
                        self.emit_node(v, false, false, true, false);
                    }
                    self.pop_indent();
                }
            }
            scalar => {
                let (text, style) = scalar_event(scalar);
                self.emit_scalar(&text, style);
            }
        }
    }

    /// Emits a whole document as `yaml.Marshal` does (stream start, implicit document,
    /// root node, implicit document end, stream end).
    pub(crate) fn emit_document(&mut self, root: &Node) {
        self.emit_node(root, true, false, false, false);
        // document end: write_indent, implicit (no "...")
        self.write_indent();
        self.open_ended = false;
    }
}

fn scalar_text(n: &Node) -> Vec<u8> {
    scalar_event(n).0
}

/// The scalar text and requested style the encoder (`encode.go`) produces for a node.
pub(crate) fn scalar_event(n: &Node) -> (Vec<u8>, Style) {
    match n {
        Node::Null => (b"null".to_vec(), Style::Plain),
        Node::Bool(true) => (b"true".to_vec(), Style::Plain),
        Node::Bool(false) => (b"false".to_vec(), Style::Plain),
        Node::Int(i) => (i.to_string().into_bytes(), Style::Plain),
        Node::Uint(u) => (u.to_string().into_bytes(), Style::Plain),
        Node::Float(f) => {
            let s = crate::strconv::format_float(*f, b'g', -1, 64);
            let s = match s.as_str() {
                "+Inf" => ".inf".to_string(),
                "-Inf" => "-.inf".to_string(),
                "NaN" => ".nan".to_string(),
                _ => s,
            };
            (s.into_bytes(), Style::Plain)
        }
        Node::Str(s) => {
            let can_use_plain = resolve(s) == Resolved::Str && !is_base60_float(s);
            let style = if s.contains('\n') {
                Style::Literal
            } else if can_use_plain {
                Style::Plain
            } else {
                Style::DoubleQuoted
            };
            (s.as_bytes().to_vec(), style)
        }
        Node::Seq(_) | Node::Map(_) => (Vec::new(), Style::Plain),
    }
}
