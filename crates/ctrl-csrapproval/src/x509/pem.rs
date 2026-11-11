//! Port of Go's `encoding/pem.Decode` (Go 1.26).

use std::collections::BTreeMap;

/// `pem.Block`
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Block {
    pub block_type: String,
    pub headers: BTreeMap<String, String>,
    pub bytes: Vec<u8>,
}

const PEM_START: &[u8] = b"\n-----BEGIN ";
const PEM_END: &[u8] = b"\n-----END ";
const PEM_END_OF_LINE: &[u8] = b"-----";

fn index(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn last_index(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

fn trim_right_space_tab(mut s: &[u8]) -> &[u8] {
    while let Some((&last, rest)) = s.split_last() {
        if last == b' ' || last == b'\t' {
            s = rest;
        } else {
            break;
        }
    }
    s
}

/// Go `bytes.TrimSpace` (ASCII whitespace; UTF-8 spaces are not expected in PEM headers).
fn trim_space(s: &[u8]) -> &[u8] {
    let is_space = |b: &u8| matches!(*b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = s.iter().position(|b| !is_space(b)).unwrap_or(s.len());
    let end = s.iter().rposition(|b| !is_space(b)).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

/// `getLine`: returns the first line (trailing `\r` and spaces/tabs removed), the rest, and
/// the number of bytes consumed.
fn get_line(data: &[u8]) -> (&[u8], &[u8], usize) {
    let (i, j) = match data.iter().position(|&b| b == b'\n') {
        None => (data.len(), data.len()),
        Some(i) => {
            let j = i + 1;
            let i = if i > 0 && data[i - 1] == b'\r' { i - 1 } else { i };
            (i, j)
        }
    };
    (trim_right_space_tab(&data[..i]), &data[j..], j)
}

fn remove_spaces_and_tabs(data: &[u8]) -> Vec<u8> {
    data.iter()
        .copied()
        .filter(|&b| b != b' ' && b != b'\t')
        .collect()
}

fn slice_from(data: &[u8], idx: isize) -> Option<&[u8]> {
    if idx < 0 || idx as usize > data.len() {
        None
    } else {
        Some(&data[idx as usize..])
    }
}

/// `pem.Decode(data)`: finds the next PEM formatted block in the input. Returns the block
/// (or `None` if no PEM data is found) and the remainder of the input.
pub fn decode(data: &[u8]) -> (Option<Block>, &[u8]) {
    let mut rest: &[u8] = data;
    let mut end_trailer_index: isize = 0;
    loop {
        if end_trailer_index < 0 || end_trailer_index as usize > rest.len() {
            return (None, data);
        }
        rest = &rest[end_trailer_index as usize..];

        let Some(end_idx) = index(rest, PEM_END) else {
            return (None, data);
        };
        let mut end_index = end_idx as isize;
        end_trailer_index = end_index + PEM_END.len() as isize;
        let begin_index = match last_index(&rest[..end_idx], &PEM_START[1..]) {
            Some(b) if b == 0 || rest[b - 1] == b'\n' => b,
            _ => continue,
        };
        let skip = begin_index + PEM_START.len() - 1;
        rest = &rest[skip..];
        end_index -= skip as isize;
        end_trailer_index -= skip as isize;

        let (type_line, after, consumed) = get_line(rest);
        rest = after;
        end_index -= consumed as isize;
        end_trailer_index -= consumed as isize;
        if !type_line.ends_with(PEM_END_OF_LINE) {
            continue;
        }
        let type_line = &type_line[..type_line.len() - PEM_END_OF_LINE.len()];

        let mut block = Block {
            block_type: String::from_utf8_lossy(type_line).into_owned(),
            headers: BTreeMap::new(),
            bytes: Vec::new(),
        };

        loop {
            if rest.is_empty() {
                return (None, data);
            }
            let (line, next, consumed) = get_line(rest);
            let Some(colon) = line.iter().position(|&b| b == b':') else {
                break;
            };
            let key = trim_space(&line[..colon]);
            let val = trim_space(&line[colon + 1..]);
            block.headers.insert(
                String::from_utf8_lossy(key).into_owned(),
                String::from_utf8_lossy(val).into_owned(),
            );
            rest = next;
            end_index -= consumed as isize;
            end_trailer_index -= consumed as isize;
        }

        if !block.headers.is_empty() && end_index < 0 {
            continue;
        }

        let Some(end_trailer) = slice_from(rest, end_trailer_index) else {
            continue;
        };
        let end_trailer_len = type_line.len() + PEM_END_OF_LINE.len();
        if end_trailer.len() < end_trailer_len {
            continue;
        }
        let rest_of_end_line = &end_trailer[end_trailer_len..];
        let end_trailer = &end_trailer[..end_trailer_len];
        if !end_trailer.starts_with(type_line) || !end_trailer.ends_with(PEM_END_OF_LINE) {
            continue;
        }

        if !get_line(rest_of_end_line).0.is_empty() {
            continue;
        }

        if end_index > 0 {
            let base64_data = remove_spaces_and_tabs(&rest[..end_index as usize]);
            match gocompat::base64::decode_string(&base64_data) {
                Ok(b) => block.bytes = b,
                Err(_) => continue,
            }
        }

        let Some(tail) = slice_from(rest, end_index + PEM_END.len() as isize - 1) else {
            continue;
        };
        let (_, rest, _) = get_line(tail);
        return (Some(block), rest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_simple_block() {
        let (b, rest) = decode(b"junk\n-----BEGIN X-----\nAQID\n-----END X-----\ntrailer");
        let b = b.unwrap();
        assert_eq!(b.block_type, "X");
        assert_eq!(b.bytes, vec![1, 2, 3]);
        assert_eq!(rest, b"trailer");
    }

    #[test]
    fn empty_block_on_adjacent_lines() {
        let (b, _) = decode(b"-----BEGIN X-----\n-----END X-----");
        let b = b.unwrap();
        assert_eq!(b.block_type, "X");
        assert!(b.bytes.is_empty());
    }

    #[test]
    fn headers_and_mismatched_end() {
        let (b, _) = decode(b"-----BEGIN X-----\nProc-Type: 4\n\nAQID\n-----END X-----\n");
        let b = b.unwrap();
        assert_eq!(b.headers.get("Proc-Type").map(String::as_str), Some("4"));
        assert_eq!(b.bytes, vec![1, 2, 3]);
        assert!(decode(b"-----BEGIN X-----\nAQID\n-----END Y-----\n").0.is_none());
        assert!(decode(b"-----BEGIN X-----\nA*ID\n-----END X-----\n").0.is_none());
        assert!(decode(b"").0.is_none());
    }
}
