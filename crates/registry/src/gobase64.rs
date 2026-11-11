//! Port of Go's `encoding/base64` decoder (StdEncoding / RawStdEncoding), reproducing
//! its exact `CorruptInputError` offsets and its tolerance of `\r` / `\n`.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorruptInputError(pub usize);

impl fmt::Display for CorruptInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "illegal base64 data at input byte {}", self.0)
    }
}

impl std::error::Error for CorruptInputError {}

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

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

/// `base64.StdEncoding.EncodeToString`
pub fn encode_std(src: &[u8]) -> String {
    let mut out = String::with_capacity(src.len().div_ceil(3) * 4);
    for chunk in src.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let v = (b0 << 16) | (b1 << 8) | b2;
        out.push(STD[((v >> 18) & 0x3f) as usize] as char);
        out.push(STD[((v >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(STD[((v >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(STD[(v & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Decode one quantum starting at `si`. Returns (new si, decoded bytes, error).
fn decode_quantum(
    src: &[u8],
    mut si: usize,
    padded: bool,
    dst: &mut Vec<u8>,
) -> (usize, Option<CorruptInputError>) {
    let mut dbuf = [0u8; 4];
    let mut dlen = 4usize;
    let mut err = None;
    let mut j = 0usize;
    while j < 4 {
        if src.len() == si {
            if j == 0 {
                return (si, None);
            }
            if j == 1 || padded {
                return (si, Some(CorruptInputError(si - j)));
            }
            dlen = j;
            break;
        }
        let input = src[si];
        si += 1;
        let out = decode_map(input);
        if out != 0xff {
            dbuf[j] = out;
            j += 1;
            continue;
        }
        if input == b'\n' || input == b'\r' {
            continue;
        }
        if !padded || input != b'=' {
            return (si, Some(CorruptInputError(si - 1)));
        }
        // We've reached the end and there's padding.
        match j {
            0 | 1 => return (si, Some(CorruptInputError(si - 1))),
            2 => {
                while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
                    si += 1;
                }
                if si == src.len() {
                    return (si, Some(CorruptInputError(src.len())));
                }
                if src[si] != b'=' {
                    return (si, Some(CorruptInputError(si - 1)));
                }
                si += 1;
            }
            _ => {}
        }
        while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
            si += 1;
        }
        if si < src.len() {
            err = Some(CorruptInputError(si));
        }
        dlen = j;
        break;
    }
    let val =
        (dbuf[0] as u32) << 18 | (dbuf[1] as u32) << 12 | (dbuf[2] as u32) << 6 | dbuf[3] as u32;
    let bytes = [(val >> 16) as u8, (val >> 8) as u8, val as u8];
    if dlen >= 2 {
        dst.extend_from_slice(&bytes[..dlen - 1]);
    }
    (si, err)
}

fn decode(src: &[u8], padded: bool) -> Result<Vec<u8>, CorruptInputError> {
    let mut dst = Vec::with_capacity(src.len() / 4 * 3 + 3);
    let mut si = 0;
    while si < src.len() {
        let (nsi, err) = decode_quantum(src, si, padded, &mut dst);
        if let Some(e) = err {
            return Err(e);
        }
        if nsi == si {
            break;
        }
        si = nsi;
    }
    Ok(dst)
}

/// `base64.StdEncoding.DecodeString`
pub fn decode_std(s: &str) -> Result<Vec<u8>, CorruptInputError> {
    decode(s.as_bytes(), true)
}

/// `base64.RawStdEncoding.DecodeString`
pub fn decode_raw_std(s: &str) -> Result<Vec<u8>, CorruptInputError> {
    decode(s.as_bytes(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            assert_eq!(decode_std(&encode_std(s.as_bytes())).unwrap(), s.as_bytes());
        }
        assert_eq!(encode_std(b"u:p"), "dTpw");
        assert_eq!(decode_raw_std("Zm9vYg").unwrap(), b"foob");
        assert_eq!(decode_std("Zm9vYg").unwrap_err().0, 4);
        assert_eq!(decode_std("Zm9v\nYg==").unwrap(), b"foob");
        assert_eq!(decode_std("!!!!").unwrap_err().0, 0);
        assert_eq!(decode_std("Zm9vYg=").unwrap_err().0, 7);
        assert_eq!(decode_std("Zm9vYg==x").unwrap_err().0, 8);
    }
}
