//! `k8s.io/apimachinery/pkg/api/resource.Quantity` string semantics.
//!
//! [`canonical`] returns exactly what Go's `resource.ParseQuantity(s)` followed by
//! `Quantity.String()` (or `MarshalJSON`) produces, including Go's quirks:
//! * strings already in canonical form are kept verbatim (e.g. `"+1"`, `"01"`, `"1.500"` stay
//!   as they are, because Go caches the input when its fast-path canonical check passes);
//! * values are rounded up (away from zero) to nano precision;
//! * BinarySI values are capped at 2^63-1;
//! * DecimalSI exponents with no SI suffix (> 18) print without a suffix.
//!
//! Arithmetic uses `i128` instead of Go's arbitrary precision `inf.Dec`; inputs whose value
//! does not fit (more than ~38 significant digits) are returned unchanged.

use std::fmt;

const SPLIT_RE: &str = "^([+-]?[0-9.]+)([eEinumkKMGTP]*[-+]?[0-9]*)$";

/// Errors of `resource.ParseQuantity` (same messages as Go).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuantityError {
    FormatWrong,
    Numeric,
    Suffix,
}

impl fmt::Display for QuantityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QuantityError::FormatWrong => write!(
                f,
                "quantities must match the regular expression '{SPLIT_RE}'"
            ),
            QuantityError::Numeric => f.write_str("unable to parse numeric part of quantity"),
            QuantityError::Suffix => f.write_str("unable to parse quantity's suffix"),
        }
    }
}

impl std::error::Error for QuantityError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    DecimalExponent,
    BinarySI,
    DecimalSI,
}

struct Parsed<'a> {
    positive: bool,
    num: &'a str,
    denom: &'a str,
    suffix: &'a str,
}

fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

/// `parseQuantityString`
fn parse_quantity_string(str_: &str) -> Result<Parsed<'_>, QuantityError> {
    let s = str_.as_bytes();
    let end = s.len();
    let mut positive = true;
    let mut pos = 0;
    if pos < end {
        match s[0] {
            b'-' => {
                positive = false;
                pos += 1;
            }
            b'+' => pos += 1,
            _ => {}
        }
    }
    // strip leading zeros
    loop {
        if pos >= end {
            return Ok(Parsed {
                positive,
                num: "0",
                denom: "",
                suffix: "",
            });
        }
        if s[pos] == b'0' {
            pos += 1;
        } else {
            break;
        }
    }
    // numerator
    let start = pos;
    let mut i = pos;
    loop {
        if i >= end {
            return Ok(Parsed {
                positive,
                num: &str_[start..end],
                denom: "",
                suffix: "",
            });
        }
        if !is_digit(s[i]) {
            break;
        }
        i += 1;
    }
    let mut num = &str_[start..i];
    pos = i;
    if num.is_empty() {
        num = "0";
    }
    let mut denom = "";
    if pos < end && s[pos] == b'.' {
        pos += 1;
        let start = pos;
        let mut i = pos;
        loop {
            if i >= end {
                return Ok(Parsed {
                    positive,
                    num,
                    denom: &str_[start..end],
                    suffix: "",
                });
            }
            if !is_digit(s[i]) {
                break;
            }
            i += 1;
        }
        denom = &str_[start..i];
        pos = i;
    }
    let suffix_start = pos;
    let mut i = pos;
    loop {
        if i >= end {
            return Ok(Parsed {
                positive,
                num,
                denom,
                suffix: &str_[suffix_start..end],
            });
        }
        if !b"eEinumkKMGTP".contains(&s[i]) {
            pos = i;
            break;
        }
        i += 1;
    }
    if pos < end && (s[pos] == b'-' || s[pos] == b'+') {
        pos += 1;
    }
    let mut i = pos;
    loop {
        if i >= end {
            return Ok(Parsed {
                positive,
                num,
                denom,
                suffix: &str_[suffix_start..end],
            });
        }
        if !is_digit(s[i]) {
            break;
        }
        i += 1;
    }
    Err(QuantityError::FormatWrong)
}

/// `quantitySuffixer.interpret`: (base, exponent, format)
fn interpret(suffix: &str) -> Option<(i32, i32, Format)> {
    Some(match suffix {
        "" => (10, 0, Format::DecimalSI),
        "n" => (10, -9, Format::DecimalSI),
        "u" => (10, -6, Format::DecimalSI),
        "m" => (10, -3, Format::DecimalSI),
        "k" => (10, 3, Format::DecimalSI),
        "M" => (10, 6, Format::DecimalSI),
        "G" => (10, 9, Format::DecimalSI),
        "T" => (10, 12, Format::DecimalSI),
        "P" => (10, 15, Format::DecimalSI),
        "E" => (10, 18, Format::DecimalSI),
        "Ki" => (2, 10, Format::BinarySI),
        "Mi" => (2, 20, Format::BinarySI),
        "Gi" => (2, 30, Format::BinarySI),
        "Ti" => (2, 40, Format::BinarySI),
        "Pi" => (2, 50, Format::BinarySI),
        "Ei" => (2, 60, Format::BinarySI),
        _ => {
            let b = suffix.as_bytes();
            if b.len() > 1 && (b[0] == b'e' || b[0] == b'E') {
                let e = go_parse_int64(&suffix[1..])?;
                return Some((10, e as i32, Format::DecimalExponent));
            }
            return None;
        }
    })
}

/// `strconv.ParseInt(s, 10, 64)` (accepts a leading sign).
fn go_parse_int64(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok()
}

fn decimal_suffix(exponent: i32) -> &'static str {
    match exponent {
        -9 => "n",
        -6 => "u",
        -3 => "m",
        3 => "k",
        6 => "M",
        9 => "G",
        12 => "T",
        15 => "P",
        18 => "E",
        // 0 and exponents with no SI suffix (Go's constructBytes returns nothing)
        _ => "",
    }
}

fn binary_suffix(exponent: i32) -> &'static str {
    match exponent {
        10 => "Ki",
        20 => "Mi",
        30 => "Gi",
        40 => "Ti",
        50 => "Pi",
        60 => "Ei",
        _ => "",
    }
}

fn pow10(n: u32) -> Option<i128> {
    10i128.checked_pow(n)
}

/// Exact decimal value `unscaled * 10^exp`.
#[derive(Clone, Copy, Debug)]
struct Dec {
    unscaled: i128,
    exp: i32,
}

/// Parse a quantity the way Go does and return its canonical string (`Quantity.String()`).
pub fn canonical(s: &str) -> Result<String, QuantityError> {
    if s.is_empty() {
        return Err(QuantityError::FormatWrong);
    }
    if s == "0" {
        return Ok("0".to_string());
    }
    let p = parse_quantity_string(s)?;
    let (base, exponent, format) = interpret(p.suffix).ok_or(QuantityError::Suffix)?;

    let precision: i32;
    let mut scale: i32 = 0;
    let mut mantissa: i64 = 1;
    match format {
        Format::DecimalExponent | Format::DecimalSI => {
            scale = exponent;
            precision = 18 - (p.num.len() + p.denom.len()) as i32;
        }
        Format::BinarySI => {
            if exponent >= 0 && p.denom.is_empty() {
                mantissa <<= exponent as u32;
                precision = 15 - p.num.len() as i32 - ((exponent as f32) * 3.0 / 10.0) as i32 - 1;
            } else {
                precision = -1;
            }
        }
    }

    // Fast path (int64Amount).
    if precision >= 0 {
        scale -= p.denom.len() as i32;
        if scale >= -9 {
            let shifted = format!("{}{}", p.num, p.denom);
            let value: i64 = shifted.parse().map_err(|_| QuantityError::Numeric)?;
            if let Some(mut result) = value.checked_mul(mantissa) {
                if !p.positive {
                    result = -result;
                }
                let keep = match format {
                    Format::BinarySI => exponent % 10 == 0 && (value & 0x07) != 0,
                    _ => {
                        scale % 3 == 0 && !shifted.ends_with("000") && shifted.as_bytes()[0] != b'0'
                    }
                };
                if keep {
                    return Ok(s.to_string());
                }
                return Ok(format_canonical(
                    Dec {
                        unscaled: result as i128,
                        exp: scale,
                    },
                    format,
                )
                .unwrap_or_else(|| s.to_string()));
            }
        }
    }

    // Slow path (inf.Dec).
    let digits = format!("{}{}", p.num, p.denom);
    let Ok(mut unscaled) = digits.parse::<i128>() else {
        return Ok(s.to_string());
    };
    let mut exp = -(p.denom.len() as i32);
    if base == 10 {
        exp += exponent;
    } else {
        let Some(m) = 1i128
            .checked_shl(exponent as u32)
            .and_then(|f| unscaled.checked_mul(f))
        else {
            return Ok(s.to_string());
        };
        unscaled = m;
    }
    // Round up (away from zero) to nano precision.
    if unscaled != 0 && exp < -9 {
        let Some(div) = pow10((-9 - exp) as u32) else {
            // Value is below 1n: rounds up to 1n.
            unscaled = 1;
            exp = -9;
            return Ok(finish_slow(Dec { unscaled, exp }, p.positive, format)
                .unwrap_or_else(|| s.to_string()));
        };
        let q = unscaled / div;
        let r = unscaled % div;
        unscaled = if r != 0 { q + 1 } else { q };
        exp = -9;
    }
    Ok(finish_slow(Dec { unscaled, exp }, p.positive, format).unwrap_or_else(|| s.to_string()))
}

fn finish_slow(mut d: Dec, positive: bool, mut format: Format) -> Option<String> {
    // Cap BinarySI at 2^63-1.
    if format == Format::BinarySI {
        let max = Dec {
            unscaled: i64::MAX as i128,
            exp: 0,
        };
        if cmp(d, max)? == std::cmp::Ordering::Greater {
            d = max;
        }
        let one = Dec {
            unscaled: 1,
            exp: 0,
        };
        if d.unscaled > 0 && cmp(d, one)? == std::cmp::Ordering::Less {
            format = Format::DecimalSI;
        }
    }
    if !positive {
        d.unscaled = -d.unscaled;
    }
    format_canonical(d, format)
}

fn cmp(a: Dec, b: Dec) -> Option<std::cmp::Ordering> {
    let e = a.exp.min(b.exp);
    let av = a.unscaled.checked_mul(pow10((a.exp - e) as u32)?)?;
    let bv = b.unscaled.checked_mul(pow10((b.exp - e) as u32)?)?;
    Some(av.cmp(&bv))
}

/// `Quantity.CanonicalizeBytes`
fn format_canonical(d: Dec, format: Format) -> Option<String> {
    if d.unscaled == 0 {
        return Some("0".to_string());
    }
    let mut format = format;
    let mut integer: Option<i128> = None;
    if format == Format::BinarySI {
        let lo = Dec {
            unscaled: -1024,
            exp: 0,
        };
        let hi = Dec {
            unscaled: 1024,
            exp: 0,
        };
        if cmp(d, lo)? == std::cmp::Ordering::Greater && cmp(d, hi)? == std::cmp::Ordering::Less {
            format = Format::DecimalSI;
        } else if d.exp >= 0 {
            integer = Some(d.unscaled.checked_mul(pow10(d.exp as u32)?)?);
        } else {
            let div = pow10((-d.exp) as u32)?;
            if d.unscaled % div == 0 {
                integer = Some(d.unscaled / div);
            } else {
                format = Format::DecimalSI;
            }
        }
    }
    match format {
        Format::BinarySI => {
            let mut v = integer?;
            let neg = v < 0;
            if neg {
                v = -v;
            }
            let mut e = 0;
            while v >= 1024 && v % 1024 == 0 {
                v /= 1024;
                e += 1;
            }
            if neg {
                v = -v;
            }
            Some(format!("{v}{}", binary_suffix(e * 10)))
        }
        Format::DecimalSI | Format::DecimalExponent => {
            let mut v = d.unscaled;
            let mut e = d.exp;
            while v % 10 == 0 {
                v /= 10;
                e += 1;
            }
            while e % 3 != 0 {
                v = v.checked_mul(10)?;
                e -= 1;
            }
            let suffix = if format == Format::DecimalSI {
                decimal_suffix(e).to_string()
            } else if e == 0 {
                String::new()
            } else {
                format!("e{e}")
            };
            Some(format!("{v}{suffix}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_examples() {
        let cases = [
            ("1", "1"),
            ("1000m", "1"),
            ("1500m", "1500m"),
            ("1.5", "1500m"),
            ("1024Mi", "1Gi"),
            ("1Gi", "1Gi"),
            ("0.5Gi", "512Mi"),
            ("100Mi", "100Mi"),
            ("2Mi", "2Mi"),
            ("1000", "1k"),
            ("1e3", "1e3"),
            ("+1", "+1"),
            ("01", "01"),
            ("1.500", "1.500"),
            ("-0", "0"),
            ("0.0001m", "100n"),
            ("0.0000000001", "1n"),
            ("500Mi", "500Mi"),
            ("1000Ki", "1000Ki"),
            ("1024", "1024"),
            ("12e6", "12e6"),
            ("0.1", "100m"),
        ];
        for (input, want) in cases {
            assert_eq!(canonical(input).as_deref(), Ok(want), "{input}");
        }
        assert_eq!(canonical(""), Err(QuantityError::FormatWrong));
        assert_eq!(canonical("1.2.3"), Err(QuantityError::FormatWrong));
        assert_eq!(canonical("1Q"), Err(QuantityError::FormatWrong));
        assert_eq!(canonical("1Ki5"), Err(QuantityError::Suffix));
    }
}
