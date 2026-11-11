//! Go `unicode` classification predicates, generated from the Go toolchain's tables
//! (see `hack/golden/gocompat-tables`).

use crate::unicode_tables::{IS_DIGIT, IS_LETTER, IS_PRINT, IS_SPACE};

fn in_table(table: &[(u32, u32)], r: u32) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < r {
                std::cmp::Ordering::Less
            } else if lo > r {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Go `strconv.IsPrint` / `unicode.IsPrint`.
pub fn is_print(r: u32) -> bool {
    in_table(IS_PRINT, r)
}

/// Go `unicode.IsLetter`.
pub fn is_letter(r: u32) -> bool {
    if r < 0x80 {
        let b = r as u8;
        return b.is_ascii_alphabetic();
    }
    in_table(IS_LETTER, r)
}

/// Go `unicode.IsDigit`.
pub fn is_digit(r: u32) -> bool {
    if r < 0x80 {
        return (b'0' as u32..=b'9' as u32).contains(&r);
    }
    in_table(IS_DIGIT, r)
}

/// Go `unicode.IsSpace`.
pub fn is_space(r: u32) -> bool {
    in_table(IS_SPACE, r)
}

/// Go `strings.TrimSpace` over a byte string.
pub fn trim_space(s: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < s.len() {
        let (r, w) = crate::utf8::decode_rune(&s[start..]);
        if s[start] < 0x80 {
            if !is_space(s[start] as u32) {
                break;
            }
            start += 1;
            continue;
        }
        if w == 1 || !is_space(r) {
            break;
        }
        start += w;
    }
    let mut stop = s.len();
    while stop > start {
        // Find the start of the last rune (Go DecodeLastRune semantics).
        let b = s[stop - 1];
        if b < 0x80 {
            if !is_space(b as u32) {
                break;
            }
            stop -= 1;
            continue;
        }
        let lim = stop.saturating_sub(4).max(start);
        let mut st = stop - 1;
        while st > lim && (s[st] & 0xC0) == 0x80 {
            st -= 1;
        }
        let (r, w) = crate::utf8::decode_rune(&s[st..stop]);
        if st + w != stop || w == 1 || !is_space(r) {
            break;
        }
        stop = st;
    }
    &s[start..stop]
}
