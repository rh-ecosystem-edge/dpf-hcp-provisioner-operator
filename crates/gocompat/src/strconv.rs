//! Go `strconv` behaviour: quoting, integer/float parsing (with Go's error messages) and
//! float formatting.

use std::fmt;

use crate::unicode::is_print;
use crate::utf8::{self, RUNE_ERROR};

const LOWERHEX: &[u8; 16] = b"0123456789abcdef";

/// Go `strconv.Quote` over a Go string (arbitrary bytes).
pub fn quote(s: impl AsRef<[u8]>) -> String {
    let mut buf = Vec::new();
    append_quoted_with(&mut buf, s.as_ref(), b'"', false);
    // Output is valid UTF-8: printable runes are copied from valid input, everything else is
    // escaped as ASCII.
    String::from_utf8(buf).unwrap_or_default()
}

/// Go `strconv.QuoteToASCII`.
pub fn quote_to_ascii(s: impl AsRef<[u8]>) -> String {
    let mut buf = Vec::new();
    append_quoted_with(&mut buf, s.as_ref(), b'"', true);
    String::from_utf8(buf).unwrap_or_default()
}

/// Go `strconv.QuoteRune`.
pub fn quote_rune(r: u32) -> String {
    let mut buf = vec![b'\''];
    let r = if utf8::valid_rune(r as i64) { r } else { RUNE_ERROR };
    append_escaped_rune(&mut buf, r, b'\'', false);
    buf.push(b'\'');
    String::from_utf8(buf).unwrap_or_default()
}

fn append_quoted_with(buf: &mut Vec<u8>, mut s: &[u8], quote: u8, ascii_only: bool) {
    buf.push(quote);
    while !s.is_empty() {
        let (r, width) = utf8::decode_rune(s);
        if width == 1 && r == RUNE_ERROR {
            buf.extend_from_slice(b"\\x");
            buf.push(LOWERHEX[(s[0] >> 4) as usize]);
            buf.push(LOWERHEX[(s[0] & 0xF) as usize]);
            s = &s[1..];
            continue;
        }
        append_escaped_rune(buf, r, quote, ascii_only);
        s = &s[width..];
    }
    buf.push(quote);
}

fn append_escaped_rune(buf: &mut Vec<u8>, r: u32, quote: u8, ascii_only: bool) {
    if r == quote as u32 || r == '\\' as u32 {
        buf.push(b'\\');
        buf.push(r as u8);
        return;
    }
    if ascii_only {
        if r < 0x80 && is_print(r) {
            buf.push(r as u8);
            return;
        }
    } else if is_print(r) {
        utf8::append_rune(buf, r);
        return;
    }
    match r {
        0x07 => buf.extend_from_slice(b"\\a"),
        0x08 => buf.extend_from_slice(b"\\b"),
        0x0C => buf.extend_from_slice(b"\\f"),
        0x0A => buf.extend_from_slice(b"\\n"),
        0x0D => buf.extend_from_slice(b"\\r"),
        0x09 => buf.extend_from_slice(b"\\t"),
        0x0B => buf.extend_from_slice(b"\\v"),
        _ => {
            if r < b' ' as u32 || r == 0x7f {
                buf.extend_from_slice(b"\\x");
                buf.push(LOWERHEX[((r as u8) >> 4) as usize]);
                buf.push(LOWERHEX[((r as u8) & 0xF) as usize]);
            } else {
                let r = if utf8::valid_rune(r as i64) { r } else { 0xFFFD };
                if r < 0x10000 {
                    buf.extend_from_slice(b"\\u");
                    for s in (0..=12).rev().step_by(4) {
                        buf.push(LOWERHEX[((r >> s) & 0xF) as usize]);
                    }
                } else {
                    buf.extend_from_slice(b"\\U");
                    for s in (0..=28).rev().step_by(4) {
                        buf.push(LOWERHEX[((r >> s) & 0xF) as usize]);
                    }
                }
            }
        }
    }
}

/// The reason a conversion failed (`strconv.ErrSyntax`, `strconv.ErrRange`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumErrorKind {
    /// `strconv.ErrSyntax` ("invalid syntax").
    Syntax,
    /// `strconv.ErrRange` ("value out of range").
    Range,
    /// "invalid base N".
    Base(i64),
    /// "invalid bit size N".
    BitSize(i64),
}

impl fmt::Display for NumErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumErrorKind::Syntax => f.write_str("invalid syntax"),
            NumErrorKind::Range => f.write_str("value out of range"),
            NumErrorKind::Base(b) => write!(f, "invalid base {b}"),
            NumErrorKind::BitSize(b) => write!(f, "invalid bit size {b}"),
        }
    }
}

/// Go `*strconv.NumError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumError {
    /// The failing function (`ParseInt`, `ParseUint`, `Atoi`, `ParseFloat`).
    pub func: &'static str,
    /// The input (a Go string; may be invalid UTF-8).
    pub num: Vec<u8>,
    /// The reason.
    pub err: NumErrorKind,
}

impl fmt::Display for NumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "strconv.{}: parsing {}: {}", self.func, quote(&self.num), self.err)
    }
}

impl std::error::Error for NumError {}

fn lower(c: u8) -> u8 {
    c | (b'x' - b'X')
}

/// Go `strconv.IntSize` on the supported (64-bit) platforms.
pub const INT_SIZE: i64 = 64;

fn parse_uint_inner(s: &[u8], base: i64, bit_size: i64) -> Result<u64, (u64, NumErrorKind)> {
    if s.is_empty() {
        return Err((0, NumErrorKind::Syntax));
    }
    let base0 = base == 0;
    let s0 = s;
    let mut s = s;
    let mut base = base;
    if (2..=36).contains(&base) {
        // valid base; nothing to do
    } else if base == 0 {
        base = 10;
        if s[0] == b'0' {
            if s.len() >= 3 && lower(s[1]) == b'b' {
                base = 2;
                s = &s[2..];
            } else if s.len() >= 3 && lower(s[1]) == b'o' {
                base = 8;
                s = &s[2..];
            } else if s.len() >= 3 && lower(s[1]) == b'x' {
                base = 16;
                s = &s[2..];
            } else {
                base = 8;
                s = &s[1..];
            }
        }
    } else {
        return Err((0, NumErrorKind::Base(base)));
    }

    let bit_size = if bit_size == 0 {
        INT_SIZE
    } else if !(0..=64).contains(&bit_size) {
        return Err((0, NumErrorKind::BitSize(bit_size)));
    } else {
        bit_size
    };

    let cutoff = u64::MAX / base as u64 + 1;
    let max_val: u64 = if bit_size == 64 { u64::MAX } else { (1u64 << bit_size) - 1 };

    let mut underscores = false;
    let mut n: u64 = 0;
    for &c in s {
        let d = if c == b'_' && base0 {
            underscores = true;
            continue;
        } else if c.is_ascii_digit() {
            c - b'0'
        } else if lower(c).is_ascii_lowercase() {
            lower(c) - b'a' + 10
        } else {
            return Err((0, NumErrorKind::Syntax));
        };
        if d as i64 >= base {
            return Err((0, NumErrorKind::Syntax));
        }
        if n >= cutoff {
            return Err((max_val, NumErrorKind::Range));
        }
        n *= base as u64;
        let n1 = n.wrapping_add(d as u64);
        if n1 < n || n1 > max_val {
            return Err((max_val, NumErrorKind::Range));
        }
        n = n1;
    }
    if underscores && !underscore_ok(s0) {
        return Err((0, NumErrorKind::Syntax));
    }
    Ok(n)
}

fn parse_int_inner(s: &[u8], base: i64, bit_size: i64) -> Result<i64, (i64, NumErrorKind)> {
    if s.is_empty() {
        return Err((0, NumErrorKind::Syntax));
    }
    let mut neg = false;
    let mut t = s;
    match s[0] {
        b'+' => t = &s[1..],
        b'-' => {
            t = &s[1..];
            neg = true;
        }
        _ => {}
    }
    let un = match parse_uint_inner(t, base, bit_size) {
        Ok(v) => v,
        Err((v, NumErrorKind::Range)) => v,
        Err((_, e)) => return Err((0, e)),
    };
    let bit_size = if bit_size == 0 { INT_SIZE } else { bit_size };
    let cutoff: u64 = 1u64 << (bit_size - 1);
    if !neg && un >= cutoff {
        return Err(((cutoff - 1) as i64, NumErrorKind::Range));
    }
    if neg && un > cutoff {
        return Err(((cutoff as i64).wrapping_neg(), NumErrorKind::Range));
    }
    let n = un as i64;
    Ok(if neg { n.wrapping_neg() } else { n })
}

/// Go `strconv.ParseUint`.
pub fn parse_uint(s: impl AsRef<[u8]>, base: i64, bit_size: i64) -> Result<u64, NumError> {
    let s = s.as_ref();
    parse_uint_inner(s, base, bit_size).map_err(|(_, err)| NumError {
        func: "ParseUint",
        num: s.to_vec(),
        err,
    })
}

/// Go `strconv.ParseInt`.
pub fn parse_int(s: impl AsRef<[u8]>, base: i64, bit_size: i64) -> Result<i64, NumError> {
    let s = s.as_ref();
    parse_int_inner(s, base, bit_size).map_err(|(_, err)| NumError {
        func: "ParseInt",
        num: s.to_vec(),
        err,
    })
}

/// Go `strconv.Atoi` (64-bit `int`).
pub fn atoi(s: impl AsRef<[u8]>) -> Result<i64, NumError> {
    let s0 = s.as_ref();
    let err = |err| NumError {
        func: "Atoi",
        num: s0.to_vec(),
        err,
    };
    if !s0.is_empty() && s0.len() < 19 {
        let mut s = s0;
        if s[0] == b'-' || s[0] == b'+' {
            s = &s[1..];
            if s.is_empty() {
                return Err(err(NumErrorKind::Syntax));
            }
        }
        let mut n: i64 = 0;
        for &ch in s {
            let d = ch.wrapping_sub(b'0');
            if d > 9 {
                return Err(err(NumErrorKind::Syntax));
            }
            n = n * 10 + d as i64;
        }
        if s0[0] == b'-' {
            n = -n;
        }
        return Ok(n);
    }
    parse_int_inner(s0, 10, 0).map_err(|(_, e)| err(e))
}

fn underscore_ok(s: &[u8]) -> bool {
    let mut saw = b'^';
    let mut i = 0;
    let mut s = s;
    if !s.is_empty() && (s[0] == b'-' || s[0] == b'+') {
        s = &s[1..];
    }
    let mut hex = false;
    if s.len() >= 2 && s[0] == b'0' && matches!(lower(s[1]), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0';
        hex = lower(s[1]) == b'x';
    }
    while i < s.len() {
        if s[i].is_ascii_digit() || hex && (b'a'..=b'f').contains(&lower(s[i])) {
            saw = b'0';
            i += 1;
            continue;
        }
        if s[i] == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            i += 1;
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
        i += 1;
    }
    saw != b'_'
}

// ---------------------------------------------------------------------------------------------
// ParseFloat
// ---------------------------------------------------------------------------------------------

fn special(s: &[u8]) -> Option<(f64, usize)> {
    if s.is_empty() {
        return None;
    }
    let mut sign = 1.0;
    let mut nsign = 0;
    let mut t = s;
    match s[0] {
        b'+' | b'-' => {
            if s[0] == b'-' {
                sign = -1.0;
            }
            nsign = 1;
            t = &s[1..];
        }
        _ => {}
    }
    let prefix_fold = |s: &[u8], prefix: &[u8]| -> usize {
        let mut n = 0;
        while n < prefix.len() {
            if n >= s.len() {
                return n;
            }
            let mut c = s[n];
            if c.is_ascii_uppercase() {
                c += b'a' - b'A';
            }
            if c != prefix[n] {
                return n;
            }
            n += 1;
        }
        n
    };
    let first = if nsign == 1 { b'i' } else { t.first().copied().unwrap_or(0) };
    match Some(first) {
        Some(b'i') | Some(b'I') => {
            let mut n = prefix_fold(t, b"infinity");
            if 3 < n && n < 8 {
                n = 3;
            }
            if n == 3 || n == 8 {
                return Some((sign * f64::INFINITY, nsign + n));
            }
        }
        Some(b'n') | Some(b'N') if prefix_fold(t, b"nan") == 3 => {
            return Some((f64::NAN, 3));
        }
        _ => {}
    }
    None
}

struct ReadFloat {
    mantissa: u64,
    exp: i64,
    neg: bool,
    trunc: bool,
    hex: bool,
    n: usize,
    /// Cleaned decimal literal (underscores removed) for exact decimal conversion.
    cleaned: String,
}

fn read_float(s: &[u8]) -> Option<ReadFloat> {
    let mut underscores = false;
    let mut i = 0;
    let mut neg = false;
    if i >= s.len() {
        return None;
    }
    if s[i] == b'+' {
        i += 1;
    } else if s[i] == b'-' {
        neg = true;
        i += 1;
    }
    let mut base: u64 = 10;
    let mut max_mant_digits = 19i64;
    let mut exp_char = b'e';
    if i + 2 < s.len() && s[i] == b'0' && lower(s[i + 1]) == b'x' {
        base = 16;
        max_mant_digits = 16;
        i += 2;
        exp_char = b'p';
    }
    let hex = base == 16;
    let mut sawdot = false;
    let mut sawdigits = false;
    let mut nd = 0i64;
    let mut nd_mant = 0i64;
    let mut dp = 0i64;
    let mut mantissa: u64 = 0;
    let mut trunc = false;
    let mut exp: i64 = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'_' {
            underscores = true;
        } else if c == b'.' {
            if sawdot {
                break;
            }
            sawdot = true;
            dp = nd;
        } else if c.is_ascii_digit() {
            sawdigits = true;
            if c == b'0' && nd == 0 {
                dp -= 1;
                i += 1;
                continue;
            }
            nd += 1;
            if nd_mant < max_mant_digits {
                mantissa = mantissa * base + (c - b'0') as u64;
                nd_mant += 1;
            } else if c != b'0' {
                trunc = true;
            }
        } else if base == 16 && (b'a'..=b'f').contains(&lower(c)) {
            sawdigits = true;
            nd += 1;
            if nd_mant < max_mant_digits {
                mantissa = mantissa * 16 + (lower(c) - b'a' + 10) as u64;
                nd_mant += 1;
            } else {
                trunc = true;
            }
        } else {
            break;
        }
        i += 1;
    }
    if !sawdigits {
        return None;
    }
    if !sawdot {
        dp = nd;
    }
    if base == 16 {
        dp *= 4;
        nd_mant *= 4;
    }
    if i < s.len() && lower(s[i]) == exp_char {
        i += 1;
        if i >= s.len() {
            return None;
        }
        let mut esign = 1;
        if s[i] == b'+' {
            i += 1;
        } else if s[i] == b'-' {
            i += 1;
            esign = -1;
        }
        if i >= s.len() || !s[i].is_ascii_digit() {
            return None;
        }
        let mut e: i64 = 0;
        while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
            if s[i] == b'_' {
                underscores = true;
                i += 1;
                continue;
            }
            if e < 10000 {
                e = e * 10 + (s[i] - b'0') as i64;
            }
            i += 1;
        }
        dp += e * esign;
    } else if base == 16 {
        return None;
    }
    if mantissa != 0 {
        exp = dp - nd_mant;
    }
    if underscores && !underscore_ok(&s[..i]) {
        return None;
    }
    let cleaned: String = s[..i]
        .iter()
        .filter(|&&c| c != b'_')
        .map(|&c| c as char)
        .collect();
    Some(ReadFloat {
        mantissa,
        exp,
        neg,
        trunc,
        hex,
        n: i,
        cleaned,
    })
}

fn atof_hex(mut mantissa: u64, mut exp: i64, neg: bool, trunc: bool, bit_size: i64) -> (f64, bool) {
    let (mantbits, expbits, bias): (i64, i64, i64) = if bit_size == 32 { (23, 8, -127) } else { (52, 11, -1023) };
    let max_exp = (1i64 << expbits) + bias - 2;
    let min_exp = bias + 1;
    exp += mantbits;

    let mut trunc = trunc;
    if mantissa != 0 {
        while mantissa >> (mantbits + 2) == 0 {
            mantissa <<= 1;
            exp -= 1;
        }
    }
    if trunc {
        mantissa |= 1;
    }
    while mantissa >> (1 + mantbits + 2) != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    while mantissa > 1 && exp < min_exp - 2 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1;
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (1 + mantbits) {
            mantissa >>= 1;
            exp += 1;
        }
    }
    if mantissa >> mantbits == 0 {
        exp = bias;
    }
    let mut overflow = false;
    if exp > max_exp {
        mantissa = 1 << mantbits;
        exp = max_exp + 1;
        overflow = true;
    }
    trunc = false;
    let _ = trunc;
    let mut bits = mantissa & ((1u64 << mantbits) - 1);
    bits |= (((exp - bias) as u64) & ((1u64 << expbits) - 1)) << mantbits;
    if neg {
        bits |= 1u64 << mantbits << expbits;
    }
    if bit_size == 32 {
        (f32::from_bits(bits as u32) as f64, overflow)
    } else {
        (f64::from_bits(bits), overflow)
    }
}

fn parse_float_prefix(s: &[u8], bit_size: i64) -> Option<(f64, usize, bool)> {
    if let Some((f, n)) = special(s) {
        return Some((f, n, false));
    }
    let rf = read_float(s)?;
    if rf.hex {
        let (f, overflow) = atof_hex(rf.mantissa, rf.exp, rf.neg, rf.trunc, bit_size);
        return Some((f, rf.n, overflow));
    }
    if bit_size == 32 {
        let f: f32 = rf.cleaned.parse().ok()?;
        Some((f as f64, rf.n, f.is_infinite()))
    } else {
        let f: f64 = rf.cleaned.parse().ok()?;
        Some((f, rf.n, f.is_infinite()))
    }
}

/// Go `strconv.ParseFloat` (`bit_size` 32 or 64). On a range error Go returns ±Inf together
/// with the error; that value is not returned here.
pub fn parse_float(s: impl AsRef<[u8]>, bit_size: i64) -> Result<f64, NumError> {
    let s = s.as_ref();
    let err = |err| NumError {
        func: "ParseFloat",
        num: s.to_vec(),
        err,
    };
    match parse_float_prefix(s, bit_size) {
        Some((f, n, overflow)) => {
            if n != s.len() {
                return Err(err(NumErrorKind::Syntax));
            }
            if overflow {
                return Err(err(NumErrorKind::Range));
            }
            Ok(f)
        }
        None => Err(err(NumErrorKind::Syntax)),
    }
}

// ---------------------------------------------------------------------------------------------
// FormatFloat
// ---------------------------------------------------------------------------------------------

/// Decimal digits of a float: value = 0.d[0]d[1]... × 10^dp.
struct Digits {
    d: Vec<u8>,
    dp: i64,
}

fn parse_exp_repr(s: &str) -> Digits {
    // s looks like "1.2345e-7" or "1e21" or "0e0" (no sign).
    let (mant, exp) = s.split_once('e').unwrap_or((s, "0"));
    let exp: i64 = exp.parse().unwrap_or(0);
    let mut d: Vec<u8> = mant.bytes().filter(|&c| c != b'.').collect();
    if d.iter().all(|&c| c == b'0') {
        return Digits { d: Vec::new(), dp: 0 };
    }
    while d.last() == Some(&b'0') {
        d.pop();
    }
    Digits { d, dp: exp + 1 }
}

fn shortest_digits(f: f64, bit_size: i64) -> Digits {
    // Rust's shortest representation gives the minimal digit count; Go (Ryu) then picks the
    // closest decimal of that length, breaking exact ties to even, which is what the exact
    // (round-half-even) fixed-precision formatting yields.
    let s = if bit_size == 32 {
        format!("{:e}", (f as f32).abs())
    } else {
        format!("{:e}", f.abs())
    };
    let d = parse_exp_repr(&s);
    if d.d.len() <= 1 {
        return d;
    }
    let exact = sig_digits(f, bit_size, d.d.len());
    if exact.d.len() == d.d.len() && exact.d != d.d {
        return exact;
    }
    d
}

/// Digits rounded to `n` significant digits (n >= 1).
fn sig_digits(f: f64, bit_size: i64, n: usize) -> Digits {
    let s = if bit_size == 32 {
        format!("{:.*e}", n - 1, (f as f32).abs())
    } else {
        format!("{:.*e}", n - 1, f.abs())
    };
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i64 = exp.parse().unwrap_or(0);
    let mut d: Vec<u8> = mant.bytes().filter(|&c| c != b'.').collect();
    if d.iter().all(|&c| c == b'0') {
        return Digits { d: Vec::new(), dp: 0 };
    }
    while d.last() == Some(&b'0') {
        d.pop();
    }
    Digits { d, dp: exp + 1 }
}

/// Digits rounded to `prec` digits after the decimal point.
fn fixed_digits(f: f64, bit_size: i64, prec: usize) -> Digits {
    let s = if bit_size == 32 {
        format!("{:.*}", prec, (f as f32).abs())
    } else {
        format!("{:.*}", prec, f.abs())
    };
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut d: Vec<u8> = int.bytes().chain(frac.bytes()).collect();
    let mut dp = int.len() as i64;
    while d.first() == Some(&b'0') {
        d.remove(0);
        dp -= 1;
    }
    if d.is_empty() {
        return Digits { d, dp: 0 };
    }
    while d.last() == Some(&b'0') {
        d.pop();
    }
    Digits { d, dp }
}

fn fmt_e(dst: &mut Vec<u8>, neg: bool, d: &Digits, prec: i64, fmt: u8) {
    if neg {
        dst.push(b'-');
    }
    let nd = d.d.len() as i64;
    dst.push(if nd != 0 { d.d[0] } else { b'0' });
    if prec > 0 {
        dst.push(b'.');
        let mut i = 1i64;
        let m = nd.min(prec + 1);
        if i < m {
            dst.extend_from_slice(&d.d[i as usize..m as usize]);
            i = m;
        }
        while i <= prec {
            dst.push(b'0');
            i += 1;
        }
    }
    dst.push(fmt);
    let mut exp = d.dp - 1;
    if nd == 0 {
        exp = 0;
    }
    if exp < 0 {
        dst.push(b'-');
        exp = -exp;
    } else {
        dst.push(b'+');
    }
    if exp < 10 {
        dst.push(b'0');
        dst.push(exp as u8 + b'0');
    } else if exp < 100 {
        dst.push((exp / 10) as u8 + b'0');
        dst.push((exp % 10) as u8 + b'0');
    } else {
        dst.push((exp / 100) as u8 + b'0');
        dst.push(((exp / 10) % 10) as u8 + b'0');
        dst.push((exp % 10) as u8 + b'0');
    }
}

fn fmt_f(dst: &mut Vec<u8>, neg: bool, d: &Digits, prec: i64) {
    if neg {
        dst.push(b'-');
    }
    let nd = d.d.len() as i64;
    if d.dp > 0 {
        let mut m = nd.min(d.dp);
        dst.extend_from_slice(&d.d[..m as usize]);
        while m < d.dp {
            dst.push(b'0');
            m += 1;
        }
    } else {
        dst.push(b'0');
    }
    if prec > 0 {
        dst.push(b'.');
        for i in 1..=prec {
            let j = d.dp + i - 1;
            let ch = if 0 <= j && j < nd { d.d[j as usize] } else { b'0' };
            dst.push(ch);
        }
    }
}

/// Go `strconv.AppendFloat` for formats `'e'`, `'E'`, `'f'`, `'g'`, `'G'` (`prec == -1` means
/// shortest representation that round-trips), `bit_size` 32 or 64.
pub fn append_float(dst: &mut Vec<u8>, f: f64, fmt: u8, prec: i64, bit_size: i64) {
    if f.is_nan() {
        dst.extend_from_slice(b"NaN");
        return;
    }
    let f = if bit_size == 32 { f as f32 as f64 } else { f };
    if f.is_infinite() {
        dst.extend_from_slice(if f > 0.0 { b"+Inf" } else { b"-Inf" });
        return;
    }
    let neg = f.is_sign_negative();
    let shortest = prec < 0;
    let mut prec = prec;
    let digs = if shortest {
        let d = shortest_digits(f, bit_size);
        let nd = d.d.len() as i64;
        prec = match fmt {
            b'e' | b'E' => (nd - 1).max(0),
            b'f' => (nd - d.dp).max(0),
            _ => nd,
        };
        d
    } else {
        match fmt {
            b'e' | b'E' => sig_digits(f, bit_size, (prec + 1) as usize),
            b'f' => fixed_digits(f, bit_size, prec as usize),
            _ => {
                if prec == 0 {
                    prec = 1;
                }
                sig_digits(f, bit_size, prec as usize)
            }
        }
    };
    match fmt {
        b'e' | b'E' => fmt_e(dst, neg, &digs, prec, fmt),
        b'f' => fmt_f(dst, neg, &digs, prec),
        b'g' | b'G' => {
            let nd = digs.d.len() as i64;
            let mut eprec = prec;
            if eprec > nd && nd >= digs.dp {
                eprec = nd;
            }
            if shortest {
                eprec = 6;
            }
            let exp = digs.dp - 1;
            if exp < -4 || exp >= eprec {
                if prec > nd {
                    prec = nd;
                }
                fmt_e(dst, neg, &digs, prec - 1, fmt + b'e' - b'g');
                return;
            }
            if prec > digs.dp {
                prec = nd;
            }
            fmt_f(dst, neg, &digs, (prec - digs.dp).max(0));
        }
        _ => {
            dst.push(b'%');
            dst.push(fmt);
        }
    }
}

/// Go `strconv.FormatFloat`.
pub fn format_float(f: f64, fmt: u8, prec: i64, bit_size: i64) -> String {
    let mut dst = Vec::new();
    append_float(&mut dst, f, fmt, prec, bit_size);
    String::from_utf8(dst).unwrap_or_default()
}

/// Go `strconv.Itoa` / `FormatInt(i, 10)`.
pub fn itoa(i: i64) -> String {
    i.to_string()
}
