//! YAML reading for `JSONToYAML` / `YAMLToJSON` / `Unmarshal`.
//!
//! * [`parse_json_as_yaml`]: how yaml.v2 reads a JSON document (flow YAML), keeping number
//!   literals' exact text for the resolver and reproducing the reader/scanner errors yaml.v2
//!   reports for JSON (control characters, implicit keys over 1024 characters, depth).
//! * [`parse_yaml`]: a YAML 1.1 reader for the block/flow subset the yaml.v2 emitter produces
//!   (block mappings/sequences incl. indentless sequences and `?` complex keys, plain /
//!   single-quoted / double-quoted scalars with line folding, literal and folded block
//!   scalars, flow collections), following libyaml's scanning rules.

use super::resolve::{Resolved, resolve};
use super::{Error, MAX_FLOW_LEVEL, Node, number_node, reader_allowed};

// ---------------------------------------------------------------------------------------------
// JSON read as YAML
// ---------------------------------------------------------------------------------------------

struct JsonReader<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
    /// 0-based YAML line of the read position.
    line: usize,
    /// Byte offset and message of the first error yaml.v2's reader reports for the input.
    reader_err: Option<(usize, &'static str)>,
}

/// yaml.v2's reader checks (readerc.go) over the whole input: the first offending byte
/// offset and message.
fn reader_check(s: &[u8]) -> Option<(usize, &'static str)> {
    let mut i = 0;
    if s.starts_with(b"\xEF\xBB\xBF") {
        i = 3;
    }
    while i < s.len() {
        let octet = s[i];
        let width = if octet & 0x80 == 0 {
            1
        } else if octet & 0xE0 == 0xC0 {
            2
        } else if octet & 0xF0 == 0xE0 {
            3
        } else if octet & 0xF8 == 0xF0 {
            4
        } else {
            return Some((i, "invalid leading UTF-8 octet"));
        };
        if width > s.len() - i {
            return Some((i, "incomplete UTF-8 octet sequence"));
        }
        let mut value: u32 = match width {
            1 => (octet & 0x7F) as u32,
            2 => (octet & 0x1F) as u32,
            3 => (octet & 0x0F) as u32,
            _ => (octet & 0x07) as u32,
        };
        for k in 1..width {
            let o = s[i + k];
            if o & 0xC0 != 0x80 {
                return Some((i, "invalid trailing UTF-8 octet"));
            }
            value = (value << 6) + (o & 0x3F) as u32;
        }
        if !(width == 1
            || (width == 2 && value >= 0x80)
            || (width == 3 && value >= 0x800)
            || (width == 4 && value >= 0x10000))
        {
            return Some((i, "invalid length of a UTF-8 sequence"));
        }
        if (0xD800..=0xDFFF).contains(&value) || value > 0x10FFFF {
            return Some((i, "invalid Unicode character"));
        }
        if !char::from_u32(value).is_some_and(reader_allowed) {
            return Some((i, "control characters are not allowed"));
        }
        i += width;
    }
    None
}

fn yerr(msg: &str) -> Error {
    Error(format!("yaml: {msg}"))
}

/// Scans a YAML double-quoted scalar (`lit` includes the quotes) the way yaml.v2 does;
/// `line` is the 0-based line the literal starts on (for error messages).
pub(crate) fn scan_double_quoted(lit: &str, line: usize) -> Result<String, Error> {
    let mut p = P {
        c: lit.chars().collect(),
        pos: 0,
        line,
        line_start: 0,
        flow_level: 1,
        pending_plain_merge: false,
        decode_err: None,
    };
    // The literal follows other tokens on its line, so its first line is never at column 0.
    p.parse_quoted_at(true)
}

impl JsonReader<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            if self.s[self.i] == b'\n'
                || (self.s[self.i] == b'\r' && self.s.get(self.i + 1) != Some(&b'\n'))
            {
                self.line += 1;
            }
            self.i += 1;
        }
    }

    /// Reads a string literal; returns the decoded string, its raw char length (including
    /// quotes) and whether it contained a line break.
    fn string(&mut self) -> Result<(String, usize), Error> {
        let start = self.i;
        self.i += 1;
        while self.i < self.s.len() {
            match self.s[self.i] {
                b'\\' => self.i += 2,
                b'"' => {
                    self.i += 1;
                    break;
                }
                _ => self.i += 1,
            }
        }
        let raw = &self.s[start..self.i];
        let raw_str = String::from_utf8_lossy(raw).into_owned();
        let decoded =
            scan_double_quoted(&raw_str, self.line).map_err(|e| self.structural(start, e))?;
        self.line += raw_str
            .chars()
            .filter(|&c| matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}'))
            .count();
        Ok((decoded, raw_str.chars().count()))
    }

    fn depth_err(&self) -> Error {
        let line = if self.line == 0 { 0 } else { self.line + 1 };
        self.structural(
            self.i,
            Error(format!(
                "yaml: {}exceeded max depth of {MAX_FLOW_LEVEL}",
                super::line_prefix(line)
            )),
        )
    }

    /// yaml.v2's reader decodes the input in 512-byte chunks ahead of the scanner, so a
    /// reader error in a chunk the scanner already needed wins over an error found while
    /// scanning/parsing at byte offset `at`.
    fn structural(&self, at: usize, e: Error) -> Error {
        match self.reader_err {
            Some((r, msg)) if (r / 512) * 512 <= at + 3 => yerr(msg),
            _ => e,
        }
    }

    fn chars_between(&self, a: usize, b: usize) -> usize {
        std::str::from_utf8(&self.s[a..b])
            .map(|s| s.chars().count())
            .unwrap_or(b - a)
    }

    fn value(&mut self) -> Result<Node, Error> {
        self.ws();
        let c = self.s.get(self.i).copied().unwrap_or(0);
        match c {
            b'{' => {
                self.depth += 1;
                if self.depth > MAX_FLOW_LEVEL {
                    return Err(self.depth_err());
                }
                self.i += 1;
                let mut entries: Vec<(Node, Node)> = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.ws();
                        let key_start = self.i;
                        let start_line = self.line;
                        let (k, _) = self.string()?;
                        self.ws();
                        let colon = self.i;
                        if self.chars_between(key_start, colon) > 1024 || self.line != start_line {
                            return Err(self.structural(
                                colon,
                                Error(format!(
                                    "yaml: {}did not find expected ',' or '}}'",
                                    super::line_prefix(self.line)
                                )),
                            ));
                        }
                        self.i += 1; // ':'
                        let v = self.value()?;
                        if let Some(pos) = entries
                            .iter()
                            .position(|(ek, _)| *ek == Node::Str(k.clone()))
                        {
                            entries[pos].1 = v;
                        } else {
                            entries.push((Node::Str(k), v));
                        }
                        self.ws();
                        if self.s.get(self.i) == Some(&b',') {
                            self.i += 1;
                            continue;
                        }
                        self.i += 1; // '}'
                        break;
                    }
                }
                self.depth -= 1;
                Ok(Node::Map(entries))
            }
            b'[' => {
                self.depth += 1;
                if self.depth > MAX_FLOW_LEVEL {
                    return Err(self.depth_err());
                }
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                } else {
                    loop {
                        items.push(self.value()?);
                        self.ws();
                        if self.s.get(self.i) == Some(&b',') {
                            self.i += 1;
                            continue;
                        }
                        self.i += 1; // ']'
                        break;
                    }
                }
                self.depth -= 1;
                Ok(Node::Seq(items))
            }
            b'"' => Ok(Node::Str(self.string()?.0)),
            _ => {
                let start = self.i;
                while self.i < self.s.len()
                    && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9' | b'a'..=b'z')
                {
                    self.i += 1;
                }
                let text = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                Ok(number_node(&text))
            }
        }
    }
}

/// yaml.v2's reading of a JSON document (falls back to [`parse_yaml`] for non-JSON input).
pub(crate) fn parse_json_as_yaml(j: &[u8]) -> Result<Node, Error> {
    if crate::json::check_valid(j).is_err() {
        return parse_yaml(j);
    }
    let mut r = JsonReader {
        s: j,
        i: 0,
        depth: 0,
        line: 0,
        reader_err: reader_check(j),
    };
    let node = r.value()?;
    if let Some((_, msg)) = r.reader_err {
        return Err(yerr(msg));
    }
    Ok(node)
}

// ---------------------------------------------------------------------------------------------
// YAML subset
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctx {
    Root,
    MapValue,
    /// The value after a complex key's `:` indicator (block collections may start on the
    /// indicator's line).
    ComplexValue,
    SeqItem,
    ComplexKey,
}

struct P {
    c: Vec<char>,
    pos: usize,
    line: usize,
    line_start: usize,
    flow_level: usize,
    /// The implicit key just scanned by `parse_node` was a plain `<<` (merge key).
    pending_plain_merge: bool,
    /// Decoding error (reported after the whole document parsed, as yaml.v2 does).
    decode_err: Option<Error>,
}

fn insert_entry(entries: &mut Vec<(Node, Node)>, key: Node, value: Node) {
    if let Some(pos) = entries.iter().position(|(ek, _)| *ek == key) {
        entries[pos].1 = value;
    } else {
        entries.push((key, value));
    }
}

fn is_blank(c: Option<char>) -> bool {
    matches!(c, Some(' ') | Some('\t'))
}

fn is_break(c: Option<char>) -> bool {
    matches!(
        c,
        Some('\n') | Some('\r') | Some('\u{85}') | Some('\u{2028}') | Some('\u{2029}')
    )
}

fn is_blankz(c: Option<char>) -> bool {
    c.is_none() || is_blank(c) || is_break(c)
}

fn is_flow_indicator(c: Option<char>) -> bool {
    matches!(c, Some(',') | Some('[') | Some(']') | Some('{') | Some('}'))
}

impl P {
    fn peek(&self) -> Option<char> {
        self.c.get(self.pos).copied()
    }

    fn peek_at(&self, k: usize) -> Option<char> {
        self.c.get(self.pos + k).copied()
    }

    fn col(&self) -> usize {
        self.pos - self.line_start
    }

    /// A scanner error at the current position (yaml.v2 reports `line+1`, omitted on line 0).
    fn err(&self, msg: &str) -> Error {
        let line = if self.line == 0 { 0 } else { self.line + 1 };
        Error(format!("yaml: {}{msg}", super::line_prefix(line)))
    }

    fn advance(&mut self) {
        if self.pos < self.c.len() {
            self.pos += 1;
        }
    }

    /// Consumes a line break and returns it as libyaml's `read_line` normalizes it
    /// (CR LF, CR, LF and NEL become LF; LS and PS are kept).
    fn read_break(&mut self) -> char {
        let c = match self.peek() {
            Some('\u{2028}') => '\u{2028}',
            Some('\u{2029}') => '\u{2029}',
            _ => '\n',
        };
        self.skip_break();
        c
    }

    /// Consumes a line break (CRLF counts as one).
    fn skip_break(&mut self) {
        if self.peek() == Some('\r') && self.peek_at(1) == Some('\n') {
            self.pos += 2;
        } else if is_break(self.peek()) {
            self.pos += 1;
        } else {
            return;
        }
        self.line += 1;
        self.line_start = self.pos;
    }

    fn at_doc_marker(&self) -> bool {
        self.col() == 0
            && ((self.peek() == Some('-')
                && self.peek_at(1) == Some('-')
                && self.peek_at(2) == Some('-'))
                || (self.peek() == Some('.')
                    && self.peek_at(1) == Some('.')
                    && self.peek_at(2) == Some('.')))
            && is_blankz(self.peek_at(3))
    }

    /// Skips blanks, comments and line breaks; returns whether a line break was crossed.
    /// Tabs are only skipped where libyaml allows them (flow context, or after a token on
    /// the same line that is not an indicator).
    fn skip_to_content(&mut self) -> bool {
        self.skip_to_content_tabs(true)
    }

    fn skip_to_content_tabs(&mut self, tab_ok: bool) -> bool {
        let mut crossed = false;
        loop {
            while self.peek() == Some(' ')
                || (self.peek() == Some('\t') && (self.flow_level > 0 || (tab_ok && !crossed)))
            {
                self.advance();
            }
            if self.peek() == Some('\t') {
                return crossed;
            }
            if self.peek() == Some('#') {
                while !is_break(self.peek()) && self.peek().is_some() {
                    self.advance();
                }
            }
            if is_break(self.peek()) {
                self.skip_break();
                crossed = true;
                continue;
            }
            return crossed;
        }
    }

    fn no_tab(&self) -> Result<(), Error> {
        if self.peek() == Some('\t') {
            return Err(self.err("found character that cannot start any token"));
        }
        Ok(())
    }

    fn at_block_entry(&self) -> bool {
        self.peek() == Some('-') && is_blankz(self.peek_at(1))
    }

    fn at_complex_key(&self) -> bool {
        self.peek() == Some('?') && is_blankz(self.peek_at(1))
    }

    /// Scans the rest of the current line to decide whether a plain scalar starting here is
    /// an implicit mapping key (`text: ...`).
    fn plain_is_key(&self) -> bool {
        let mut k = self.pos;
        let mut prev_blank = false;
        while k < self.c.len() {
            let ch = self.c[k];
            if is_break(Some(ch)) {
                return false;
            }
            if ch == '#' && prev_blank {
                return false;
            }
            if ch == ':' && is_blankz(self.c.get(k + 1).copied()) {
                return true;
            }
            prev_blank = is_blank(Some(ch));
            k += 1;
        }
        false
    }

    /// yaml.v2 `decoder.merge` into an `interface{}` map.
    fn merge(&mut self, entries: &mut Vec<(Node, Node)>, value: Node) {
        match value {
            Node::Map(m) => m.into_iter().for_each(|(k, v)| insert_entry(entries, k, v)),
            Node::Seq(items) if items.iter().all(|i| matches!(i, Node::Map(_))) => {
                for item in items.into_iter().rev() {
                    if let Node::Map(m) = item {
                        m.into_iter().for_each(|(k, v)| insert_entry(entries, k, v));
                    }
                }
            }
            _ => {
                if self.decode_err.is_none() {
                    self.decode_err = Some(yerr(
                        "map merge requires map or sequence of maps as the value",
                    ));
                }
            }
        }
    }

    fn parse_document(&mut self) -> Result<Node, Error> {
        self.skip_to_content();
        if self.peek() == Some('%') && self.col() == 0 {
            return Err(self.err("directives are not supported"));
        }
        if self.at_doc_marker() && self.peek() == Some('-') {
            self.pos += 3;
        }
        let crossed = self.skip_to_content();
        let _ = crossed;
        if self.peek().is_none() || self.at_doc_marker() {
            return Ok(Node::Null);
        }
        let node = self.parse_node(-1, Ctx::Root, true)?;
        self.skip_to_content();
        if self.peek().is_some() && !self.at_doc_marker() {
            return Err(self.err("did not find expected <document start>"));
        }
        if let Some(e) = self.decode_err.take() {
            return Err(e);
        }
        Ok(node)
    }

    fn parse_node(&mut self, parent_indent: i64, ctx: Ctx, first: bool) -> Result<Node, Error> {
        let crossed = self.skip_to_content_tabs(false) || first;
        if self.peek() == Some('\t') {
            return Err(self.err("found character that cannot start any token"));
        }
        if self.peek().is_none() || self.at_doc_marker() {
            return Ok(Node::Null);
        }
        let c = self.col() as i64;
        let same_line_value = !crossed && ctx == Ctx::MapValue;
        if crossed && c <= parent_indent {
            if matches!(ctx, Ctx::MapValue | Ctx::ComplexValue)
                && c == parent_indent
                && self.at_block_entry()
            {
                return self.parse_block_seq(c);
            }
            return Ok(Node::Null);
        }
        match self.peek() {
            Some('-') if is_blankz(self.peek_at(1)) => {
                if same_line_value {
                    return Err(self.err("block sequence entries are not allowed in this context"));
                }
                self.parse_block_seq(c)
            }
            Some('?') if is_blankz(self.peek_at(1)) => {
                if same_line_value {
                    return Err(self.err("mapping keys are not allowed in this context"));
                }
                self.parse_block_map(c, None)
            }
            Some('|') | Some('>') => Ok(Node::Str(self.parse_block_scalar(parent_indent)?)),
            Some('[') | Some('{') => {
                let n = self.parse_flow_node()?;
                while is_blank(self.peek()) {
                    self.advance();
                }
                if self.peek() == Some(':') {
                    return Err(self.err("flow collections as mapping keys are not supported"));
                }
                Ok(n)
            }
            Some('&') | Some('*') | Some('!') => {
                Err(self.err("anchors, aliases and tags are not supported"))
            }
            Some('%') | Some('@') | Some('`') => {
                Err(self.err("found character that cannot start any token"))
            }
            Some('"') | Some('\'') => {
                let start_line = self.line;
                let s = self.parse_quoted()?;
                let save = self.pos;
                while is_blank(self.peek()) {
                    self.advance();
                }
                if self.peek() == Some(':') && is_blankz(self.peek_at(1)) {
                    if same_line_value {
                        return Err(self.err("mapping values are not allowed in this context"));
                    }
                    if self.line != start_line {
                        return Err(self.err("could not find expected ':'"));
                    }
                    return self.parse_block_map(c, Some(Node::Str(s)));
                }
                self.pos = save;
                Ok(Node::Str(s))
            }
            _ => {
                if self.plain_is_key() {
                    if same_line_value {
                        return Err(self.err("mapping values are not allowed in this context"));
                    }
                    let key = self.parse_plain(parent_indent)?;
                    self.pending_plain_merge = key == "<<";
                    return self.parse_block_map(c, Some(resolve_plain(&key)));
                }
                let s = self.parse_plain(parent_indent)?;
                Ok(resolve_plain(&s))
            }
        }
    }

    fn parse_block_seq(&mut self, c: i64) -> Result<Node, Error> {
        let mut items = Vec::new();
        loop {
            // at '-'
            self.advance();
            items.push(self.parse_node(c, Ctx::SeqItem, false)?);
            self.skip_to_content();
            self.no_tab()?;
            if self.peek().is_none() || self.at_doc_marker() {
                break;
            }
            let col = self.col() as i64;
            if col < c {
                break;
            }
            if col == c {
                if self.at_block_entry() {
                    continue;
                }
                break;
            }
            return Err(self.err("did not find expected '-' indicator"));
        }
        Ok(Node::Seq(items))
    }

    fn parse_block_map(&mut self, c: i64, first_key: Option<Node>) -> Result<Node, Error> {
        let mut entries: Vec<(Node, Node)> = Vec::new();
        let mut first = first_key;
        loop {
            let entry_line = self.line;
            let (key, value);
            let mut is_merge = false;
            if let Some(k) = first.take() {
                is_merge = self.pending_plain_merge;
                self.pending_plain_merge = false;
                // positioned at ':' (possibly after blanks)
                while is_blank(self.peek()) {
                    self.advance();
                }
                self.advance(); // ':'
                key = k;
                value = self.parse_node(c, Ctx::MapValue, false)?;
            } else if self.at_complex_key() {
                self.advance();
                key = self.parse_node(c, Ctx::ComplexKey, false)?;
                self.skip_to_content();
                self.no_tab()?;
                if self.col() as i64 == c && self.peek() == Some(':') && is_blankz(self.peek_at(1))
                {
                    self.advance();
                    value = self.parse_node(c, Ctx::ComplexValue, false)?;
                } else {
                    value = Node::Null;
                }
            } else {
                match self.peek() {
                    Some('"') | Some('\'') => {
                        let s = self.parse_quoted()?;
                        key = Node::Str(s);
                    }
                    _ => {
                        if !self.plain_is_key() {
                            return Err(self.err("could not find expected ':'"));
                        }
                        let s = self.parse_plain(c)?;
                        is_merge = s == "<<";
                        key = resolve_plain(&s);
                    }
                }
                while is_blank(self.peek()) {
                    self.advance();
                }
                if self.peek() != Some(':') || !is_blankz(self.peek_at(1)) {
                    return Err(self.err("could not find expected ':'"));
                }
                self.advance();
                value = self.parse_node(c, Ctx::MapValue, false)?;
            }
            if is_merge {
                self.merge(&mut entries, value);
            } else {
                insert_entry(&mut entries, key, value);
            }
            self.skip_to_content();
            self.no_tab()?;
            let crossed = self.line != entry_line;
            if self.peek().is_none() || self.at_doc_marker() {
                break;
            }
            let col = self.col() as i64;
            if !crossed {
                return Err(self.err("did not find expected key"));
            }
            if col < c {
                break;
            }
            if col > c || self.at_block_entry() {
                return Err(self.err("did not find expected key"));
            }
        }
        Ok(Node::Map(entries))
    }

    /// libyaml `yaml_parser_scan_plain_scalar`.
    fn parse_plain(&mut self, parent_indent: i64) -> Result<String, Error> {
        let indent = parent_indent + 1;
        let mut s = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        let mut whitespaces = String::new();
        let mut leading_blanks = false;
        loop {
            if self.at_doc_marker() {
                break;
            }
            if self.peek() == Some('#') {
                break;
            }
            while !is_blankz(self.peek()) {
                let ch = self.peek();
                if self.flow_level > 0
                    && ch == Some(':')
                    && !is_blankz(self.peek_at(1))
                    && !is_flow_indicator(self.peek_at(1))
                {
                    return Err(self.err("found unexpected ':'"));
                }
                if (ch == Some(':')
                    && (is_blankz(self.peek_at(1))
                        || (self.flow_level > 0 && is_flow_indicator(self.peek_at(1)))))
                    || (self.flow_level > 0 && (is_flow_indicator(ch) || ch == Some('?')))
                {
                    break;
                }
                if leading_blanks || !whitespaces.is_empty() {
                    if leading_blanks {
                        if leading_break.starts_with('\n') {
                            if trailing_breaks.is_empty() {
                                s.push(' ');
                            } else {
                                s.push_str(&trailing_breaks);
                            }
                        } else {
                            s.push_str(&leading_break);
                            s.push_str(&trailing_breaks);
                        }
                        leading_break.clear();
                        trailing_breaks.clear();
                        leading_blanks = false;
                    } else {
                        s.push_str(&whitespaces);
                        whitespaces.clear();
                    }
                }
                if let Some(ch) = ch {
                    s.push(ch);
                }
                self.advance();
            }
            if !(is_blank(self.peek()) || is_break(self.peek())) {
                break;
            }
            while is_blank(self.peek()) || is_break(self.peek()) {
                if is_blank(self.peek()) {
                    if !leading_blanks {
                        whitespaces.push(self.peek().unwrap_or(' '));
                    }
                    self.advance();
                } else {
                    let br = self.read_break();
                    if !leading_blanks {
                        whitespaces.clear();
                        leading_break = br.to_string();
                        leading_blanks = true;
                    } else {
                        trailing_breaks.push(br);
                    }
                }
            }
            if self.flow_level == 0 && (self.col() as i64) < indent {
                break;
            }
        }
        Ok(s)
    }

    fn parse_quoted(&mut self) -> Result<String, Error> {
        self.parse_quoted_at(false)
    }

    /// libyaml `yaml_parser_scan_flow_scalar`. With `mid_line`, the opening quote is known not
    /// to be at column 0 of its line.
    fn parse_quoted_at(&mut self, mid_line: bool) -> Result<String, Error> {
        let first_line = self.line;
        let single = self.peek() == Some('\'');
        let quote = if single { '\'' } else { '"' };
        self.advance();
        let mut s = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        let mut whitespaces = String::new();
        loop {
            if self.at_doc_marker() && !(mid_line && self.line == first_line) {
                return Err(self.err("found unexpected document indicator"));
            }
            if self.peek().is_none() {
                return Err(self.err("found unexpected end of stream"));
            }
            let mut leading_blanks = false;
            while !is_blankz(self.peek()) {
                let ch = self.peek();
                if single && ch == Some('\'') && self.peek_at(1) == Some('\'') {
                    s.push('\'');
                    self.pos += 2;
                } else if ch == Some(quote) {
                    break;
                } else if !single && ch == Some('\\') && is_break(self.peek_at(1)) {
                    self.advance();
                    self.skip_break();
                    leading_blanks = true;
                    break;
                } else if !single && ch == Some('\\') {
                    let e = self.peek_at(1);
                    let mut code_len = 0;
                    match e {
                        Some('0') => s.push('\0'),
                        Some('a') => s.push('\x07'),
                        Some('b') => s.push('\x08'),
                        Some('t') | Some('\t') => s.push('\t'),
                        Some('n') => s.push('\n'),
                        Some('v') => s.push('\x0b'),
                        Some('f') => s.push('\x0c'),
                        Some('r') => s.push('\r'),
                        Some('e') => s.push('\x1b'),
                        Some(' ') => s.push(' '),
                        Some('"') => s.push('"'),
                        Some('\'') => s.push('\''),
                        Some('\\') => s.push('\\'),
                        Some('/') => s.push('/'),
                        Some('N') => s.push('\u{85}'),
                        Some('_') => s.push('\u{a0}'),
                        Some('L') => s.push('\u{2028}'),
                        Some('P') => s.push('\u{2029}'),
                        Some('x') => code_len = 2,
                        Some('u') => code_len = 4,
                        Some('U') => code_len = 8,
                        _ => return Err(self.err("found unknown escape character")),
                    }
                    self.pos += 2;
                    if code_len > 0 {
                        let mut value: u32 = 0;
                        for k in 0..code_len {
                            let Some(d) = self.peek_at(k).and_then(|c| c.to_digit(16)) else {
                                return Err(self.err("did not find expected hexdecimal number"));
                            };
                            value = (value << 4) + d;
                        }
                        let Some(ch) = char::from_u32(value) else {
                            return Err(self.err("found invalid Unicode character escape code"));
                        };
                        s.push(ch);
                        self.pos += code_len;
                    }
                } else {
                    if let Some(ch) = ch {
                        s.push(ch);
                    }
                    self.advance();
                }
            }
            if self.peek() == Some(quote) {
                break;
            }
            while is_blank(self.peek()) || is_break(self.peek()) {
                if is_blank(self.peek()) {
                    if !leading_blanks {
                        whitespaces.push(self.peek().unwrap_or(' '));
                    }
                    self.advance();
                } else {
                    let br = self.read_break();
                    if !leading_blanks {
                        whitespaces.clear();
                        leading_break = br.to_string();
                        leading_blanks = true;
                    } else {
                        trailing_breaks.push(br);
                    }
                }
            }
            if leading_blanks {
                if leading_break.starts_with('\n') {
                    if trailing_breaks.is_empty() {
                        s.push(' ');
                    } else {
                        s.push_str(&trailing_breaks);
                    }
                } else {
                    s.push_str(&leading_break);
                    s.push_str(&trailing_breaks);
                }
                leading_break.clear();
                trailing_breaks.clear();
            } else {
                s.push_str(&whitespaces);
                whitespaces.clear();
            }
        }
        self.advance(); // closing quote
        Ok(s)
    }

    fn block_scalar_breaks(
        &mut self,
        indent: &mut i64,
        breaks: &mut String,
        parent_indent: i64,
    ) -> Result<(), Error> {
        let mut max_indent = 0i64;
        loop {
            while (*indent == 0 || (self.col() as i64) < *indent) && self.peek() == Some(' ') {
                self.advance();
            }
            if self.col() as i64 > max_indent {
                max_indent = self.col() as i64;
            }
            if (*indent == 0 || (self.col() as i64) < *indent) && self.peek() == Some('\t') {
                return Err(
                    self.err("found a tab character where an indentation space is expected")
                );
            }
            if !is_break(self.peek()) {
                break;
            }
            let br = self.read_break();
            breaks.push(br);
        }
        if *indent == 0 {
            *indent = max_indent;
            if *indent < parent_indent + 1 {
                *indent = parent_indent + 1;
            }
            if *indent < 1 {
                *indent = 1;
            }
        }
        Ok(())
    }

    /// libyaml `yaml_parser_scan_block_scalar`.
    fn parse_block_scalar(&mut self, parent_indent: i64) -> Result<String, Error> {
        let literal = self.peek() == Some('|');
        self.advance();
        let mut chomping = 0i32;
        let mut increment = 0i64;
        if matches!(self.peek(), Some('+') | Some('-')) {
            chomping = if self.peek() == Some('+') { 1 } else { -1 };
            self.advance();
            if let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
                if d == 0 {
                    return Err(self.err("found an indentation indicator equal to 0"));
                }
                increment = d as i64;
                self.advance();
            }
        } else if let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            if d == 0 {
                return Err(self.err("found an indentation indicator equal to 0"));
            }
            increment = d as i64;
            self.advance();
            if matches!(self.peek(), Some('+') | Some('-')) {
                chomping = if self.peek() == Some('+') { 1 } else { -1 };
                self.advance();
            }
        }
        while is_blank(self.peek()) {
            self.advance();
        }
        if self.peek() == Some('#') {
            while !is_break(self.peek()) && self.peek().is_some() {
                self.advance();
            }
        }
        if !is_break(self.peek()) && self.peek().is_some() {
            return Err(self.err("did not find expected comment or line break"));
        }
        self.skip_break();

        let mut indent = 0i64;
        if increment > 0 {
            indent = if parent_indent >= 0 {
                parent_indent + increment
            } else {
                increment
            };
        }
        let mut s = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        self.block_scalar_breaks(&mut indent, &mut trailing_breaks, parent_indent)?;
        let mut leading_blank = false;
        while self.col() as i64 == indent && self.peek().is_some() {
            let trailing_blank = is_blank(self.peek());
            if !literal && leading_break.starts_with('\n') && !leading_blank && !trailing_blank {
                if trailing_breaks.is_empty() {
                    s.push(' ');
                }
                leading_break.clear();
            } else {
                s.push_str(&leading_break);
                leading_break.clear();
            }
            s.push_str(&trailing_breaks);
            trailing_breaks.clear();
            leading_blank = is_blank(self.peek());
            while !is_break(self.peek()) && self.peek().is_some() {
                if let Some(ch) = self.peek() {
                    s.push(ch);
                }
                self.advance();
            }
            if is_break(self.peek()) {
                let br = self.read_break();
                leading_break.push(br);
            }
            self.block_scalar_breaks(&mut indent, &mut trailing_breaks, parent_indent)?;
        }
        if chomping != -1 {
            s.push_str(&leading_break);
        }
        if chomping == 1 {
            s.push_str(&trailing_breaks);
        }
        Ok(s)
    }

    fn skip_flow_ws(&mut self) {
        loop {
            while is_blank(self.peek()) {
                self.advance();
            }
            if self.peek() == Some('#') {
                while !is_break(self.peek()) && self.peek().is_some() {
                    self.advance();
                }
            }
            if is_break(self.peek()) {
                self.skip_break();
                continue;
            }
            return;
        }
    }

    fn parse_flow_node(&mut self) -> Result<Node, Error> {
        self.skip_flow_ws();
        match self.peek() {
            Some('[') => {
                self.flow_level += 1;
                if self.flow_level > MAX_FLOW_LEVEL {
                    return Err(Error(format!(
                        "yaml: exceeded max depth of {MAX_FLOW_LEVEL}"
                    )));
                }
                self.advance();
                let mut items = Vec::new();
                loop {
                    self.skip_flow_ws();
                    if self.peek() == Some(']') {
                        self.advance();
                        break;
                    }
                    let item = self.parse_flow_node()?;
                    self.skip_flow_ws();
                    if self.peek() == Some(':') {
                        // single-pair implicit mapping inside a flow sequence
                        self.advance();
                        self.skip_flow_ws();
                        let value = if matches!(self.peek(), Some(',') | Some(']')) {
                            Node::Null
                        } else {
                            self.parse_flow_node()?
                        };
                        items.push(Node::Map(vec![(item, value)]));
                        self.skip_flow_ws();
                    } else {
                        items.push(item);
                    }
                    match self.peek() {
                        Some(',') => self.advance(),
                        Some(']') => {}
                        _ => return Err(self.err("did not find expected ',' or ']'")),
                    }
                }
                self.flow_level -= 1;
                Ok(Node::Seq(items))
            }
            Some('{') => {
                self.flow_level += 1;
                if self.flow_level > MAX_FLOW_LEVEL {
                    return Err(Error(format!(
                        "yaml: exceeded max depth of {MAX_FLOW_LEVEL}"
                    )));
                }
                self.advance();
                let mut entries: Vec<(Node, Node)> = Vec::new();
                loop {
                    self.skip_flow_ws();
                    if self.peek() == Some('}') {
                        self.advance();
                        break;
                    }
                    let plain_key = !matches!(self.peek(), Some('"') | Some('\''));
                    let key = self.parse_flow_node()?;
                    let is_merge = plain_key && key == Node::Str("<<".to_string());
                    self.skip_flow_ws();
                    let value = if self.peek() == Some(':') {
                        self.advance();
                        self.skip_flow_ws();
                        if matches!(self.peek(), Some(',') | Some('}')) {
                            Node::Null
                        } else {
                            self.parse_flow_node()?
                        }
                    } else {
                        Node::Null
                    };
                    if is_merge {
                        self.merge(&mut entries, value);
                    } else {
                        insert_entry(&mut entries, key, value);
                    }
                    self.skip_flow_ws();
                    match self.peek() {
                        Some(',') => self.advance(),
                        Some('}') => {}
                        _ => return Err(self.err("did not find expected ',' or '}'")),
                    }
                }
                self.flow_level -= 1;
                Ok(Node::Map(entries))
            }
            Some('"') | Some('\'') => Ok(Node::Str(self.parse_quoted()?)),
            Some('&') | Some('*') | Some('!') => {
                Err(self.err("anchors, aliases and tags are not supported"))
            }
            None => Err(self.err("did not find expected node content")),
            _ => {
                let s = self.parse_plain(-1)?;
                Ok(resolve_plain(&s))
            }
        }
    }
}

fn resolve_plain(s: &str) -> Node {
    match resolve(s) {
        Resolved::Null => Node::Null,
        Resolved::Bool(b) => Node::Bool(b),
        Resolved::Int(i) => Node::Int(i),
        Resolved::Uint(u) => Node::Uint(u),
        Resolved::Float(f) => Node::Float(f),
        // yaml.v2 keeps timestamps as strings when decoding into interface{}.
        Resolved::Timestamp | Resolved::Merge | Resolved::Str => Node::Str(s.to_string()),
    }
}

/// Parses the first YAML document of `y` (the supported subset) into yaml.v2's
/// `interface{}` model.
pub(crate) fn parse_yaml(y: &[u8]) -> Result<Node, Error> {
    let text = std::str::from_utf8(y)
        .map_err(|_| Error("yaml: invalid leading UTF-8 octet".to_string()))?;
    if let Some(bad) = text
        .chars()
        .find(|&c| !reader_allowed(c) && c != '\u{feff}')
    {
        let _ = bad;
        return Err(Error(
            "yaml: control characters are not allowed".to_string(),
        ));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = P {
        c: text.chars().collect(),
        pos: 0,
        line: 0,
        line_start: 0,
        flow_level: 0,
        pending_plain_merge: false,
        decode_err: None,
    };
    p.parse_document()
}
