//! Go `encoding/base64` `StdEncoding` (padded, non-strict; `\r` and `\n` are ignored when
//! decoding).

use std::fmt;

const ENCODE_STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn decode_map(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => 0xff,
    }
}

/// Go `base64.CorruptInputError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorruptInputError(pub i64);

impl fmt::Display for CorruptInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "illegal base64 data at input byte {}", self.0)
    }
}

impl std::error::Error for CorruptInputError {}

/// Go `base64.StdEncoding.EncodeToString`.
pub fn encode_to_string(src: &[u8]) -> String {
    let mut dst = Vec::with_capacity(src.len().div_ceil(3) * 4);
    let (chunks, rem) = src.as_chunks::<3>();
    for c in chunks {
        let val = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
        dst.push(ENCODE_STD[(val >> 18 & 0x3F) as usize]);
        dst.push(ENCODE_STD[(val >> 12 & 0x3F) as usize]);
        dst.push(ENCODE_STD[(val >> 6 & 0x3F) as usize]);
        dst.push(ENCODE_STD[(val & 0x3F) as usize]);
    }
    if !rem.is_empty() {
        let mut val = (rem[0] as u32) << 16;
        if rem.len() == 2 {
            val |= (rem[1] as u32) << 8;
        }
        dst.push(ENCODE_STD[(val >> 18 & 0x3F) as usize]);
        dst.push(ENCODE_STD[(val >> 12 & 0x3F) as usize]);
        if rem.len() == 2 {
            dst.push(ENCODE_STD[(val >> 6 & 0x3F) as usize]);
        } else {
            dst.push(b'=');
        }
        dst.push(b'=');
    }
    // Only ASCII bytes were written.
    String::from_utf8(dst).unwrap_or_default()
}

/// Go `decodeQuantum`; returns (new si, bytes written, error).
fn decode_quantum(
    dst: &mut Vec<u8>,
    src: &[u8],
    mut si: usize,
) -> (usize, Option<CorruptInputError>) {
    let mut dbuf = [0u8; 4];
    let mut dlen = 4;
    let mut err = None;
    let mut j: isize = 0;
    while j < 4 {
        if src.len() == si {
            if j == 0 {
                return (si, None);
            }
            // padChar != NoPadding
            return (si, Some(CorruptInputError(si as i64 - j as i64)));
        }
        let inp = src[si];
        si += 1;
        let out = decode_map(inp);
        if out != 0xff {
            dbuf[j as usize] = out;
            j += 1;
            continue;
        }
        if inp == b'\n' || inp == b'\r' {
            continue;
        }
        if inp != b'=' {
            return (si, Some(CorruptInputError(si as i64 - 1)));
        }
        match j {
            0 | 1 => return (si, Some(CorruptInputError(si as i64 - 1))),
            2 => {
                while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
                    si += 1;
                }
                if si == src.len() {
                    return (si, Some(CorruptInputError(src.len() as i64)));
                }
                if src[si] != b'=' {
                    return (si, Some(CorruptInputError(si as i64 - 1)));
                }
                si += 1;
            }
            _ => {}
        }
        while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
            si += 1;
        }
        if si < src.len() {
            err = Some(CorruptInputError(si as i64));
        }
        dlen = j as usize;
        break;
    }
    let val =
        (dbuf[0] as u32) << 18 | (dbuf[1] as u32) << 12 | (dbuf[2] as u32) << 6 | dbuf[3] as u32;
    let b = [(val >> 16) as u8, (val >> 8) as u8, val as u8];
    if dlen >= 2 {
        dst.extend_from_slice(&b[..dlen - 1]);
    }
    (si, err)
}

/// Go `base64.StdEncoding.Decode`: returns the decoded bytes produced before any error,
/// and the error.
pub fn decode(src: &[u8]) -> (Vec<u8>, Option<CorruptInputError>) {
    let mut dst = Vec::with_capacity(src.len() / 4 * 3);
    let mut si = 0;
    while si < src.len() {
        let (nsi, err) = decode_quantum(&mut dst, src, si);
        si = nsi;
        if err.is_some() {
            return (dst, err);
        }
    }
    (dst, None)
}

/// Go `base64.StdEncoding.DecodeString`.
pub fn decode_string(src: &[u8]) -> Result<Vec<u8>, CorruptInputError> {
    match decode(src) {
        (d, None) => Ok(d),
        (_, Some(e)) => Err(e),
    }
}
