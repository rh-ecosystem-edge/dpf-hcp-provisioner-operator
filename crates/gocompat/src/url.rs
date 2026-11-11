//! Go `net/url` escaping: `QueryEscape`, `QueryUnescape`, `PathEscape`, `PathUnescape`.

use std::fmt;

const UPPERHEX: &[u8; 16] = b"0123456789ABCDEF";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    PathSegment,
    QueryComponent,
}

/// Port of `shouldEscape` (see `net/url/gen_encoding_table.go`) for the supported modes.
fn should_escape(c: u8, mode: Mode) -> bool {
    if c.is_ascii_alphanumeric() {
        return false;
    }
    match c {
        b'-' | b'_' | b'.' | b'~' => false,
        b'$' | b'&' | b'+' | b',' | b'/' | b':' | b';' | b'=' | b'?' | b'@' => match mode {
            Mode::PathSegment => c == b'/' || c == b';' || c == b',' || c == b'?',
            Mode::QueryComponent => true,
        },
        _ => true,
    }
}

fn ishex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

fn unhex(c: u8) -> u8 {
    9 * (c >> 6) + (c & 15)
}

fn escape(s: &[u8], mode: Mode) -> String {
    let mut out = String::with_capacity(s.len());
    for &c in s {
        if c == b' ' && mode == Mode::QueryComponent {
            out.push('+');
        } else if should_escape(c, mode) {
            out.push('%');
            out.push(UPPERHEX[(c >> 4) as usize] as char);
            out.push(UPPERHEX[(c & 15) as usize] as char);
        } else {
            out.push(c as char);
        }
    }
    out
}

/// Go `url.QueryEscape`.
pub fn query_escape(s: impl AsRef<[u8]>) -> String {
    escape(s.as_ref(), Mode::QueryComponent)
}

/// Go `url.PathEscape`.
pub fn path_escape(s: impl AsRef<[u8]>) -> String {
    escape(s.as_ref(), Mode::PathSegment)
}

/// Go `url.EscapeError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscapeError(pub Vec<u8>);

impl fmt::Display for EscapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid URL escape {}", crate::strconv::quote(&self.0))
    }
}

impl std::error::Error for EscapeError {}

fn unescape(s: &[u8], mode: Mode) -> Result<Vec<u8>, EscapeError> {
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' {
            if i + 2 >= s.len() || !ishex(s[i + 1]) || !ishex(s[i + 2]) {
                let t = &s[i..];
                let t = if t.len() > 3 { &t[..3] } else { t };
                return Err(EscapeError(t.to_vec()));
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    let plus = if mode == Mode::QueryComponent {
        b' '
    } else {
        b'+'
    };
    let mut t = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' => {
                t.push(unhex(s[i + 1]) << 4 | unhex(s[i + 2]));
                i += 3;
            }
            b'+' => {
                t.push(plus);
                i += 1;
            }
            c => {
                t.push(c);
                i += 1;
            }
        }
    }
    Ok(t)
}

/// Go `url.QueryUnescape`. Returns the raw bytes of the Go result string.
pub fn query_unescape(s: impl AsRef<[u8]>) -> Result<Vec<u8>, EscapeError> {
    unescape(s.as_ref(), Mode::QueryComponent)
}

/// Go `url.PathUnescape`. Returns the raw bytes of the Go result string.
pub fn path_unescape(s: impl AsRef<[u8]>) -> Result<Vec<u8>, EscapeError> {
    unescape(s.as_ref(), Mode::PathSegment)
}
