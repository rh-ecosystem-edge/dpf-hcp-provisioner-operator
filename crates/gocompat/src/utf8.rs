//! Go `unicode/utf8` decoding semantics (invalid sequences decode as `RuneError` of width 1).

/// `utf8.RuneError` (U+FFFD).
pub const RUNE_ERROR: u32 = 0xFFFD;
/// `utf8.RuneSelf`.
pub const RUNE_SELF: u8 = 0x80;
/// `unicode.MaxRune`.
pub const MAX_RUNE: u32 = 0x10FFFF;

/// Go `utf8.DecodeRune`: returns the rune and its width in bytes. An empty input
/// returns `(RUNE_ERROR, 0)`; an invalid encoding returns `(RUNE_ERROR, 1)`.
pub fn decode_rune(p: &[u8]) -> (u32, usize) {
    let Some(&b0) = p.first() else {
        return (RUNE_ERROR, 0);
    };
    if b0 < 0x80 {
        return (b0 as u32, 1);
    }
    // (width, lo, hi) of the second byte, per Go's acceptRanges.
    let (n, lo, hi) = match b0 {
        0xC2..=0xDF => (2, 0x80, 0xBF),
        0xE0 => (3, 0xA0, 0xBF),
        0xE1..=0xEC | 0xEE..=0xEF => (3, 0x80, 0xBF),
        0xED => (3, 0x80, 0x9F),
        0xF0 => (4, 0x90, 0xBF),
        0xF1..=0xF3 => (4, 0x80, 0xBF),
        0xF4 => (4, 0x80, 0x8F),
        _ => return (RUNE_ERROR, 1),
    };
    if p.len() < n {
        return (RUNE_ERROR, 1);
    }
    let b1 = p[1];
    if b1 < lo || hi < b1 {
        return (RUNE_ERROR, 1);
    }
    if n == 2 {
        return ((((b0 & 0x1F) as u32) << 6) | (b1 & 0x3F) as u32, 2);
    }
    let b2 = p[2];
    if !(0x80..=0xBF).contains(&b2) {
        return (RUNE_ERROR, 1);
    }
    if n == 3 {
        return (
            (((b0 & 0x0F) as u32) << 12) | (((b1 & 0x3F) as u32) << 6) | (b2 & 0x3F) as u32,
            3,
        );
    }
    let b3 = p[3];
    if !(0x80..=0xBF).contains(&b3) {
        return (RUNE_ERROR, 1);
    }
    (
        (((b0 & 0x07) as u32) << 18)
            | (((b1 & 0x3F) as u32) << 12)
            | (((b2 & 0x3F) as u32) << 6)
            | (b3 & 0x3F) as u32,
        4,
    )
}

/// Go `utf8.ValidRune`.
pub fn valid_rune(r: i64) -> bool {
    (0..0xD800).contains(&r) || (0xE000..=MAX_RUNE as i64).contains(&r)
}

/// Go `utf8.AppendRune` (invalid runes append U+FFFD).
pub fn append_rune(buf: &mut Vec<u8>, r: u32) {
    let c = char::from_u32(r).unwrap_or('\u{FFFD}');
    let mut tmp = [0u8; 4];
    buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
}

/// Go `utf8.Valid`.
pub fn valid(p: &[u8]) -> bool {
    std::str::from_utf8(p).is_ok()
}

/// Go `[]rune(string(p))`: decodes `p` into runes, mapping each invalid byte to U+FFFD.
pub fn runes(p: &[u8]) -> Vec<u32> {
    let mut out = Vec::with_capacity(p.len());
    let mut i = 0;
    while i < p.len() {
        let (r, w) = decode_rune(&p[i..]);
        out.push(r);
        i += w;
    }
    out
}
