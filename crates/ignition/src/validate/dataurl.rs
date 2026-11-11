//! Port of `github.com/vincent-petithory/dataurl.DecodeString` (lexer + parser), returning
//! the decoded data or the library's error message.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ItemType {
    Error,
    Eof,
    DataPrefix,
    MediaType,
    MediaSep,
    MediaSubType,
    ParamSemicolon,
    ParamAttr,
    ParamEqual,
    LeftStringQuote,
    RightStringQuote,
    ParamVal,
    Base64Enc,
    DataComma,
    Data,
}

struct Item {
    t: ItemType,
    val: String,
}

const EOF: i32 = -1;

struct Lexer<'a> {
    input: &'a str,
    start: usize,
    pos: usize,
    width: usize,
    seen_base64: bool,
    items: Vec<Item>,
}

#[derive(Clone, Copy)]
enum State {
    BeforeDataPrefix,
    DataPrefix,
    AfterDataPrefix,
    XTokenMediaType,
    InDiscreteMediaType,
    MediaType,
    MediaSep,
    AfterMediaSep,
    MediaSubType,
    AfterMediaSubType,
    ParamSemicolon,
    AfterParamSemicolon,
    Base64Enc,
    InParamAttr,
    ParamAttr,
    ParamEqual,
    AfterParamEqual,
    InQuotedStringParamVal,
    EscapedChar,
    InParamVal,
    QuotedStringParamVal,
    ParamVal,
    AfterParamVal,
    DataComma,
    Data,
    Base64Data,
}

fn is_tspecial(r: i32) -> bool {
    matches!(
        u8::try_from(r).map(char::from),
        Ok('('
            | ')'
            | '<'
            | '>'
            | '@'
            | ','
            | ';'
            | ':'
            | '\\'
            | '"'
            | '/'
            | '['
            | ']'
            | '?'
            | '=')
    )
}

/// Go `unicode.IsControl` / `IsSpace` restricted to ASCII (callers check `r <= 127`; eof
/// (-1) is neither, as in Go).
fn is_token_rune(r: i32) -> bool {
    if r > 127 {
        return false;
    }
    let is_control = (0..=0x1f).contains(&r) || r == 0x7f;
    let is_space = matches!(r, 0x09..=0x0d | 0x20);
    !is_control && !is_space && !is_tspecial(r)
}

fn is_url_char_rune(r: i32) -> bool {
    r <= 127
        && !(0..=0x1f).contains(&r)
        && r != 0x7f
        && !matches!(
            u8::try_from(r).map(char::from),
            Ok(' ' | '<' | '>' | '#' | '"' | '{' | '}' | '|' | '\\' | '^' | '[' | ']' | '`')
        )
}

fn is_base64_rune(r: i32) -> bool {
    matches!(
        u8::try_from(r).map(char::from),
        Ok('a'..='z' | 'A'..='Z' | '0'..='9' | '+' | '/' | '=' | '\n')
    )
}

fn is_discrete_type(s: &str) -> bool {
    ["text", "image", "audio", "video", "application"]
        .iter()
        .any(|p| s.starts_with(p))
}

fn is_composite_type(s: &str) -> bool {
    s.starts_with("message") || s.starts_with("multipart")
}

impl Lexer<'_> {
    fn emit(&mut self, t: ItemType) {
        self.items.push(Item {
            t,
            val: self.input[self.start..self.pos].to_string(),
        });
        self.start = self.pos;
    }

    fn next(&mut self) -> i32 {
        if self.pos >= self.input.len() {
            self.width = 0;
            return EOF;
        }
        let c = self.input[self.pos..].chars().next().unwrap_or('\u{fffd}');
        self.width = c.len_utf8();
        self.pos += self.width;
        c as i32
    }

    fn backup(&mut self) {
        self.pos -= self.width;
    }

    fn errorf(&mut self, msg: String) -> Option<State> {
        self.items.push(Item {
            t: ItemType::Error,
            val: msg,
        });
        None
    }

    fn err(&mut self, msg: &str) -> Option<State> {
        self.errorf(msg.to_string())
    }

    fn step(&mut self, s: State) -> Option<State> {
        use State::*;
        match s {
            BeforeDataPrefix => {
                if self.input[self.pos..].starts_with("data:") {
                    Some(DataPrefix)
                } else {
                    self.err("missing data prefix")
                }
            }
            DataPrefix => {
                self.pos += "data:".len();
                self.emit(ItemType::DataPrefix);
                Some(AfterDataPrefix)
            }
            AfterDataPrefix => {
                let r = self.next();
                if r == ';' as i32 {
                    self.backup();
                    Some(ParamSemicolon)
                } else if r == ',' as i32 {
                    self.backup();
                    Some(DataComma)
                } else if r == EOF {
                    self.err("missing comma before data")
                } else if r == 'x' as i32 || r == 'X' as i32 {
                    if self.next() == '-' as i32 {
                        Some(XTokenMediaType)
                    } else {
                        Some(InDiscreteMediaType)
                    }
                } else if is_token_rune(r) {
                    Some(InDiscreteMediaType)
                } else {
                    self.err("invalid character after data prefix")
                }
            }
            XTokenMediaType => loop {
                let r = self.next();
                if r == '/' as i32 {
                    self.backup();
                    return Some(MediaType);
                } else if r == EOF {
                    return self.err("missing media type slash");
                } else if is_token_rune(r) {
                } else {
                    return self.err("invalid character for media type");
                }
            },
            InDiscreteMediaType => loop {
                let r = self.next();
                if r == '/' as i32 {
                    self.backup();
                    let t = &self.input[self.start..self.pos];
                    if !is_discrete_type(t) && !is_composite_type(t) {
                        return self.err("invalid media type");
                    }
                    return Some(MediaType);
                } else if r == EOF {
                    return self.err("missing media type slash");
                } else if is_token_rune(r) {
                } else {
                    return self.err("invalid character for media type");
                }
            },
            MediaType => {
                if self.pos > self.start {
                    self.emit(ItemType::MediaType);
                }
                Some(MediaSep)
            }
            MediaSep => {
                self.next();
                self.emit(ItemType::MediaSep);
                Some(AfterMediaSep)
            }
            AfterMediaSep => loop {
                let r = self.next();
                if r == ';' as i32 || r == ',' as i32 {
                    self.backup();
                    return Some(MediaSubType);
                } else if r == EOF {
                    return self.err("incomplete media type");
                } else if is_token_rune(r) {
                } else {
                    return self.err("invalid character for media subtype");
                }
            },
            MediaSubType => {
                if self.pos > self.start {
                    self.emit(ItemType::MediaSubType);
                }
                Some(AfterMediaSubType)
            }
            AfterMediaSubType => {
                let r = self.next();
                if r == ';' as i32 {
                    self.backup();
                    Some(ParamSemicolon)
                } else if r == ',' as i32 {
                    self.backup();
                    Some(DataComma)
                } else if r == EOF {
                    self.err("missing comma before data")
                } else {
                    self.err("expected semicolon or comma")
                }
            }
            ParamSemicolon => {
                self.next();
                self.emit(ItemType::ParamSemicolon);
                Some(AfterParamSemicolon)
            }
            AfterParamSemicolon => {
                let r = self.next();
                if r == EOF || r == '=' as i32 || r == ',' as i32 {
                    self.err("unterminated parameter sequence")
                } else if is_token_rune(r) {
                    self.backup();
                    Some(InParamAttr)
                } else {
                    self.err("invalid character for parameter attribute")
                }
            }
            Base64Enc => {
                if self.pos > self.start {
                    let v = self.input[self.start..self.pos].to_string();
                    if v != "base64" {
                        return self.errorf(format!("expected base64, got {v}"));
                    }
                    self.seen_base64 = true;
                    self.emit(ItemType::Base64Enc);
                }
                Some(DataComma)
            }
            InParamAttr => loop {
                let r = self.next();
                if r == '=' as i32 {
                    self.backup();
                    return Some(ParamAttr);
                } else if r == ',' as i32 {
                    self.backup();
                    return Some(Base64Enc);
                } else if r == EOF {
                    return self.err("unterminated parameter sequence");
                } else if is_token_rune(r) {
                } else {
                    return self.err("invalid character for parameter attribute");
                }
            },
            ParamAttr => {
                if self.pos > self.start {
                    self.emit(ItemType::ParamAttr);
                }
                Some(ParamEqual)
            }
            ParamEqual => {
                self.next();
                self.emit(ItemType::ParamEqual);
                Some(AfterParamEqual)
            }
            AfterParamEqual => {
                let r = self.next();
                if r == '"' as i32 {
                    self.emit(ItemType::LeftStringQuote);
                    Some(InQuotedStringParamVal)
                } else if r == EOF {
                    self.err("missing comma before data")
                } else if is_token_rune(r) {
                    Some(InParamVal)
                } else {
                    self.err("invalid character for parameter value")
                }
            }
            InQuotedStringParamVal => loop {
                let r = self.next();
                if r == EOF {
                    return self.err("unclosed quoted string");
                } else if r == '\\' as i32 {
                    return Some(EscapedChar);
                } else if r == '"' as i32 {
                    self.backup();
                    return Some(QuotedStringParamVal);
                } else if r <= 127 {
                } else {
                    return self.err("invalid character for parameter value");
                }
            },
            EscapedChar => {
                let r = self.next();
                if r <= 127 {
                    // includes eof (-1), as in Go
                    Some(InQuotedStringParamVal)
                } else {
                    self.err("invalid escaped character")
                }
            }
            InParamVal => loop {
                let r = self.next();
                if r == ';' as i32 || r == ',' as i32 {
                    self.backup();
                    return Some(ParamVal);
                } else if r == EOF {
                    return self.err("missing comma before data");
                } else if is_token_rune(r) {
                } else {
                    return self.err("invalid character for parameter value");
                }
            },
            QuotedStringParamVal => {
                if self.pos > self.start {
                    self.emit(ItemType::ParamVal);
                }
                self.next();
                self.emit(ItemType::RightStringQuote);
                Some(AfterParamVal)
            }
            ParamVal => {
                if self.pos > self.start {
                    self.emit(ItemType::ParamVal);
                }
                Some(AfterParamVal)
            }
            AfterParamVal => {
                let r = self.next();
                if r == ';' as i32 {
                    self.backup();
                    Some(ParamSemicolon)
                } else if r == ',' as i32 {
                    self.backup();
                    Some(DataComma)
                } else if r == EOF {
                    self.err("missing comma before data")
                } else {
                    self.err("expected semicolon or comma")
                }
            }
            DataComma => {
                self.next();
                self.emit(ItemType::DataComma);
                if self.seen_base64 {
                    Some(Base64Data)
                } else {
                    Some(Data)
                }
            }
            Data | Base64Data => {
                let pred = if matches!(s, Data) {
                    is_url_char_rune
                } else {
                    is_base64_rune
                };
                loop {
                    let r = self.next();
                    if r == EOF {
                        break;
                    } else if pred(r) {
                    } else {
                        return self.err("invalid data character");
                    }
                }
                if self.pos > self.start {
                    self.emit(ItemType::Data);
                }
                self.emit(ItemType::Eof);
                None
            }
        }
    }
}

/// `rfc2396.go` `Unescape`.
fn rfc2396_unescape(s: &str) -> Result<Vec<u8>, String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = s[i..].chars().next().unwrap_or('\u{fffd}');
        if c.len_utf8() > 1 {
            return Err("rfc2396: non-ASCII char detected".into());
        }
        i += 1;
        if c == '%' {
            let mut hex = [0u8; 2];
            for h in &mut hex {
                if i >= b.len() {
                    return Err("rfc2396: unexpected end of unescape sequence".into());
                }
                if !b[i].is_ascii_hexdigit() {
                    return Err("rfc2396: invalid char 0x25 in unescape sequence".into());
                }
                *h = b[i];
                i += 1;
            }
            let un = |c: u8| match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                _ => c - b'A' + 10,
            };
            out.push(un(hex[1]).wrapping_add(un(hex[0]).wrapping_mul(16)));
        } else {
            out.push(c as u8);
        }
    }
    Ok(out)
}

/// Go `strconv.Unquote("\"" + val + "\"")` success check for the ASCII values the lexer
/// produces (error message `invalid syntax`).
fn unquote_ok(val: &str) -> bool {
    let b = val.as_bytes();
    // fast path: no backslash and no newline before the first quote
    let first_quote = b.iter().position(|&c| c == b'"');
    let end = first_quote.unwrap_or(b.len());
    if !b[..end].contains(&b'\\') && !b[..end].contains(&b'\n') {
        // a quote inside would end the string early -> remaining input -> ErrSyntax
        return first_quote.is_none();
    }
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' || c == b'"' {
            return false;
        }
        if c != b'\\' {
            i += 1;
            continue;
        }
        if i + 1 >= b.len() {
            return false;
        }
        let e = b[i + 1];
        i += 2;
        match e {
            b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'"' => {}
            b'0'..=b'7' => {
                if i + 2 > b.len() {
                    return false;
                }
                let mut v = u32::from(e - b'0');
                for &d in &b[i..i + 2] {
                    if !(b'0'..=b'7').contains(&d) {
                        return false;
                    }
                    v = v * 8 + u32::from(d - b'0');
                }
                if v > 255 {
                    return false;
                }
                i += 2;
            }
            b'x' | b'u' | b'U' => {
                let n = match e {
                    b'x' => 2,
                    b'u' => 4,
                    _ => 8,
                };
                if i + n > b.len() {
                    return false;
                }
                let mut v: u32 = 0;
                for &d in &b[i..i + n] {
                    let Some(x) = (d as char).to_digit(16) else {
                        return false;
                    };
                    v = v * 16 + x;
                }
                i += n;
                if e != b'x' && (v > 0x10ffff || (0xd800..0xe000).contains(&v)) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// `dataurl.DecodeString`: the decoded data, or the error message.
pub(crate) fn decode_string(s: &str) -> Result<Vec<u8>, String> {
    let mut l = Lexer {
        input: s,
        start: 0,
        pos: 0,
        width: 0,
        seen_base64: false,
        items: Vec::new(),
    };
    let mut state = Some(State::BeforeDataPrefix);
    while let Some(st) = state {
        state = l.step(st);
    }
    let mut unquote = false;
    let mut base64: Option<bool> = None;
    let mut data: Option<Vec<u8>> = None;
    for item in l.items {
        match item.t {
            ItemType::Error => return Err(item.val),
            ItemType::LeftStringQuote => unquote = true,
            ItemType::ParamVal => {
                if unquote {
                    unquote = false;
                    if !unquote_ok(&item.val) {
                        return Err("invalid syntax".into());
                    }
                } else {
                    rfc2396_unescape(&item.val)?;
                }
            }
            ItemType::Base64Enc => base64 = Some(true),
            ItemType::DataComma => {
                if base64.is_none() {
                    base64 = Some(false);
                }
            }
            ItemType::Data => {
                let d = if base64 == Some(true) {
                    gocompat::base64::decode_string(item.val.as_bytes())
                        .map_err(|e| e.to_string())?
                } else {
                    rfc2396_unescape(&item.val)?
                };
                data = Some(d);
            }
            ItemType::Eof => return Ok(data.unwrap_or_default()),
            _ => {}
        }
    }
    // Go panics ("EOF not found"); unreachable: the lexer always ends with Error or Eof.
    Err("EOF not found".into())
}

#[cfg(test)]
mod tests {
    use super::decode_string;

    #[test]
    fn decode() {
        assert_eq!(decode_string("data:,hello").unwrap(), b"hello");
        assert_eq!(decode_string("data:;base64,aGk=").unwrap(), b"hi");
        assert_eq!(
            decode_string("data:text/plain;charset=utf-8;base64,aGk=").unwrap(),
            b"hi"
        );
        assert_eq!(decode_string("data:,a%20b").unwrap(), b"a b");
        assert_eq!(decode_string("nodata").unwrap_err(), "missing data prefix");
        assert_eq!(
            decode_string("data:,{x}").unwrap_err(),
            "invalid data character"
        );
        assert_eq!(
            decode_string("data:foo/bar,x").unwrap_err(),
            "invalid media type"
        );
        assert_eq!(
            decode_string("data:;base64,!!").unwrap_err(),
            "invalid data character"
        );
        assert_eq!(
            decode_string("data:;base64,a").unwrap_err(),
            "illegal base64 data at input byte 0"
        );
        assert_eq!(
            decode_string("data:,%zz").unwrap_err(),
            "rfc2396: invalid char 0x25 in unescape sequence"
        );
    }
}
