//! Port of Go `encoding/json` scanner.go: the state machine used by `Valid`, `checkValid`,
//! `Compact` and `Indent`, producing Go's exact `SyntaxError` messages and offsets.

use std::fmt;

/// Go `*json.SyntaxError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    /// The message (`Error()` string).
    pub msg: String,
    /// Error occurred after reading `offset` bytes.
    pub offset: i64,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for SyntaxError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Op {
    Continue,
    BeginLiteral,
    BeginObject,
    ObjectKey,
    ObjectValue,
    EndObject,
    BeginArray,
    ArrayValue,
    EndArray,
    SkipSpace,
    End,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    ObjectKey,
    ObjectValue,
    ArrayValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    BeginValue,
    BeginValueOrEmpty,
    BeginStringOrEmpty,
    BeginString,
    EndValue,
    EndTop,
    InString,
    InStringEsc,
    InStringEscU,
    InStringEscU1,
    InStringEscU12,
    InStringEscU123,
    Neg,
    S1,
    S0,
    Dot,
    Dot0,
    E,
    ESign,
    E0,
    T,
    Tr,
    Tru,
    F,
    Fa,
    Fal,
    Fals,
    N,
    Nu,
    Nul,
    Error,
}

const MAX_NESTING_DEPTH: usize = 10000;

pub(crate) struct Scanner {
    step: Step,
    end_top: bool,
    parse_state: Vec<ParseState>,
    pub(crate) err: Option<SyntaxError>,
    pub(crate) bytes: i64,
}

fn is_space(c: u8) -> bool {
    c <= b' ' && (c == b' ' || c == b'\t' || c == b'\r' || c == b'\n')
}

fn is_hex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

/// Go `quoteChar`.
fn quote_char(c: u8) -> String {
    if c == b'\'' {
        return "'\\''".to_string();
    }
    if c == b'"' {
        return "'\"'".to_string();
    }
    // string(c) converts the byte to the rune U+00XX.
    let mut tmp = Vec::new();
    crate::utf8::append_rune(&mut tmp, c as u32);
    let s = crate::strconv::quote(&tmp);
    format!("'{}'", &s[1..s.len() - 1])
}

impl Scanner {
    pub(crate) fn new() -> Self {
        Scanner {
            step: Step::BeginValue,
            end_top: false,
            parse_state: Vec::new(),
            err: None,
            bytes: 0,
        }
    }

    pub(crate) fn eof(&mut self) -> Op {
        if self.err.is_some() {
            return Op::Error;
        }
        if self.end_top {
            return Op::End;
        }
        self.step(b' ');
        if self.end_top {
            return Op::End;
        }
        if self.err.is_none() {
            self.err = Some(SyntaxError {
                msg: "unexpected end of JSON input".to_string(),
                offset: self.bytes,
            });
        }
        Op::Error
    }

    fn push_parse_state(&mut self, c: u8, new: ParseState, success: Op) -> Op {
        self.parse_state.push(new);
        if self.parse_state.len() <= MAX_NESTING_DEPTH {
            return success;
        }
        self.error(c, "exceeded max depth")
    }

    fn pop_parse_state(&mut self) {
        self.parse_state.pop();
        if self.parse_state.is_empty() {
            self.step = Step::EndTop;
            self.end_top = true;
        } else {
            self.step = Step::EndValue;
        }
    }

    fn error(&mut self, c: u8, context: &str) -> Op {
        self.step = Step::Error;
        self.err = Some(SyntaxError {
            msg: format!("invalid character {} {}", quote_char(c), context),
            offset: self.bytes,
        });
        Op::Error
    }

    pub(crate) fn step(&mut self, c: u8) -> Op {
        match self.step {
            Step::BeginValueOrEmpty => {
                if is_space(c) {
                    return Op::SkipSpace;
                }
                if c == b']' {
                    return self.state_end_value(c);
                }
                self.state_begin_value(c)
            }
            Step::BeginValue => self.state_begin_value(c),
            Step::BeginStringOrEmpty => {
                if is_space(c) {
                    return Op::SkipSpace;
                }
                if c == b'}' {
                    if let Some(last) = self.parse_state.last_mut() {
                        *last = ParseState::ObjectValue;
                    }
                    return self.state_end_value(c);
                }
                self.state_begin_string(c)
            }
            Step::BeginString => self.state_begin_string(c),
            Step::EndValue => self.state_end_value(c),
            Step::EndTop => self.state_end_top(c),
            Step::InString => {
                if c == b'"' {
                    self.step = Step::EndValue;
                    return Op::Continue;
                }
                if c == b'\\' {
                    self.step = Step::InStringEsc;
                    return Op::Continue;
                }
                if c < 0x20 {
                    return self.error(c, "in string literal");
                }
                Op::Continue
            }
            Step::InStringEsc => match c {
                b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => {
                    self.step = Step::InString;
                    Op::Continue
                }
                b'u' => {
                    self.step = Step::InStringEscU;
                    Op::Continue
                }
                _ => self.error(c, "in string escape code"),
            },
            Step::InStringEscU | Step::InStringEscU1 | Step::InStringEscU12 | Step::InStringEscU123 => {
                if is_hex(c) {
                    self.step = match self.step {
                        Step::InStringEscU => Step::InStringEscU1,
                        Step::InStringEscU1 => Step::InStringEscU12,
                        Step::InStringEscU12 => Step::InStringEscU123,
                        _ => Step::InString,
                    };
                    return Op::Continue;
                }
                self.error(c, "in \\u hexadecimal character escape")
            }
            Step::Neg => {
                if c == b'0' {
                    self.step = Step::S0;
                    return Op::Continue;
                }
                if (b'1'..=b'9').contains(&c) {
                    self.step = Step::S1;
                    return Op::Continue;
                }
                self.error(c, "in numeric literal")
            }
            Step::S1 => {
                if c.is_ascii_digit() {
                    self.step = Step::S1;
                    return Op::Continue;
                }
                self.state0(c)
            }
            Step::S0 => self.state0(c),
            Step::Dot => {
                if c.is_ascii_digit() {
                    self.step = Step::Dot0;
                    return Op::Continue;
                }
                self.error(c, "after decimal point in numeric literal")
            }
            Step::Dot0 => {
                if c.is_ascii_digit() {
                    return Op::Continue;
                }
                if c == b'e' || c == b'E' {
                    self.step = Step::E;
                    return Op::Continue;
                }
                self.state_end_value(c)
            }
            Step::E => {
                if c == b'+' || c == b'-' {
                    self.step = Step::ESign;
                    return Op::Continue;
                }
                self.state_e_sign(c)
            }
            Step::ESign => self.state_e_sign(c),
            Step::E0 => {
                if c.is_ascii_digit() {
                    return Op::Continue;
                }
                self.state_end_value(c)
            }
            Step::T => self.lit(c, b'r', Step::Tr, "in literal true (expecting 'r')"),
            Step::Tr => self.lit(c, b'u', Step::Tru, "in literal true (expecting 'u')"),
            Step::Tru => self.lit(c, b'e', Step::EndValue, "in literal true (expecting 'e')"),
            Step::F => self.lit(c, b'a', Step::Fa, "in literal false (expecting 'a')"),
            Step::Fa => self.lit(c, b'l', Step::Fal, "in literal false (expecting 'l')"),
            Step::Fal => self.lit(c, b's', Step::Fals, "in literal false (expecting 's')"),
            Step::Fals => self.lit(c, b'e', Step::EndValue, "in literal false (expecting 'e')"),
            Step::N => self.lit(c, b'u', Step::Nu, "in literal null (expecting 'u')"),
            Step::Nu => self.lit(c, b'l', Step::Nul, "in literal null (expecting 'l')"),
            Step::Nul => self.lit(c, b'l', Step::EndValue, "in literal null (expecting 'l')"),
            Step::Error => Op::Error,
        }
    }

    fn lit(&mut self, c: u8, want: u8, next: Step, ctx: &str) -> Op {
        if c == want {
            self.step = next;
            return Op::Continue;
        }
        self.error(c, ctx)
    }

    fn state_begin_value(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::SkipSpace;
        }
        match c {
            b'{' => {
                self.step = Step::BeginStringOrEmpty;
                self.push_parse_state(c, ParseState::ObjectKey, Op::BeginObject)
            }
            b'[' => {
                self.step = Step::BeginValueOrEmpty;
                self.push_parse_state(c, ParseState::ArrayValue, Op::BeginArray)
            }
            b'"' => {
                self.step = Step::InString;
                Op::BeginLiteral
            }
            b'-' => {
                self.step = Step::Neg;
                Op::BeginLiteral
            }
            b'0' => {
                self.step = Step::S0;
                Op::BeginLiteral
            }
            b't' => {
                self.step = Step::T;
                Op::BeginLiteral
            }
            b'f' => {
                self.step = Step::F;
                Op::BeginLiteral
            }
            b'n' => {
                self.step = Step::N;
                Op::BeginLiteral
            }
            b'1'..=b'9' => {
                self.step = Step::S1;
                Op::BeginLiteral
            }
            _ => self.error(c, "looking for beginning of value"),
        }
    }

    fn state_begin_string(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::SkipSpace;
        }
        if c == b'"' {
            self.step = Step::InString;
            return Op::BeginLiteral;
        }
        self.error(c, "looking for beginning of object key string")
    }

    fn state_end_value(&mut self, c: u8) -> Op {
        let n = self.parse_state.len();
        if n == 0 {
            self.step = Step::EndTop;
            self.end_top = true;
            return self.state_end_top(c);
        }
        if is_space(c) {
            self.step = Step::EndValue;
            return Op::SkipSpace;
        }
        match self.parse_state[n - 1] {
            ParseState::ObjectKey => {
                if c == b':' {
                    self.parse_state[n - 1] = ParseState::ObjectValue;
                    self.step = Step::BeginValue;
                    return Op::ObjectKey;
                }
                self.error(c, "after object key")
            }
            ParseState::ObjectValue => {
                if c == b',' {
                    self.parse_state[n - 1] = ParseState::ObjectKey;
                    self.step = Step::BeginString;
                    return Op::ObjectValue;
                }
                if c == b'}' {
                    self.pop_parse_state();
                    return Op::EndObject;
                }
                self.error(c, "after object key:value pair")
            }
            ParseState::ArrayValue => {
                if c == b',' {
                    self.step = Step::BeginValue;
                    return Op::ArrayValue;
                }
                if c == b']' {
                    self.pop_parse_state();
                    return Op::EndArray;
                }
                self.error(c, "after array element")
            }
        }
    }

    fn state_end_top(&mut self, c: u8) -> Op {
        if !is_space(c) {
            self.error(c, "after top-level value");
        }
        Op::End
    }

    fn state0(&mut self, c: u8) -> Op {
        if c == b'.' {
            self.step = Step::Dot;
            return Op::Continue;
        }
        if c == b'e' || c == b'E' {
            self.step = Step::E;
            return Op::Continue;
        }
        self.state_end_value(c)
    }

    fn state_e_sign(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            self.step = Step::E0;
            return Op::Continue;
        }
        self.error(c, "in exponent of numeric literal")
    }
}

/// Go `checkValid`.
pub fn check_valid(data: &[u8]) -> Result<(), SyntaxError> {
    let mut scan = Scanner::new();
    for &c in data {
        scan.bytes += 1;
        if scan.step(c) == Op::Error {
            return Err(scan.err.take().unwrap_or_else(|| SyntaxError {
                msg: String::new(),
                offset: scan.bytes,
            }));
        }
    }
    if scan.eof() == Op::Error {
        return Err(scan.err.take().unwrap_or_else(|| SyntaxError {
            msg: String::new(),
            offset: scan.bytes,
        }));
    }
    Ok(())
}

/// Go `json.Valid`.
pub fn valid(data: &[u8]) -> bool {
    check_valid(data).is_ok()
}
