//! The two `k8s.io/apimachinery/pkg/api/resource` operations `hugepagesQuantity` uses:
//!
//! * [`must_parse_value`] — `resource.MustParse(s).Value()`: Go's `ParseQuantity` (fast
//!   `int64Amount` path and slow `inf.Dec` path, rounding up to nano precision, BinarySI cap
//!   at 2^63-1) followed by `ScaledValue(0)` with each path's own rounding (`ceil` for
//!   positive fractions; the two paths differ for negative fractions exactly as in Go).
//!   Parse errors are Go's (`cannot parse '<s>': <err>`, the `MustParse` panic message).
//! * [`new_binary_si_string`] — `resource.NewQuantity(v, resource.BinarySI).String()`.
//!
//! The slow path uses `i128` instead of `big.Int`; values whose nano-scaled magnitude does
//! not fit (beyond ~1.7e29) saturate. The CRD restricts the size to `1Gi`, `2Mi`, `32Mi` and
//! `64Ki`, so such inputs never reach the operator.

use apis::resource::{QuantityError, canonical};

/// `parseQuantityString`: (positive, value, num, denom, suffix) for a string that
/// `ParseQuantity` accepted.
struct Parts<'a> {
    positive: bool,
    value: &'a str,
    num: &'a str,
    denom: &'a str,
    suffix: &'a str,
}

fn parse_quantity_string(s: &str) -> Parts<'_> {
    let b = s.as_bytes();
    let end = b.len();
    let mut positive = true;
    let mut pos = 0;
    if pos < end {
        match b[0] {
            b'-' => {
                positive = false;
                pos += 1;
            }
            b'+' => pos += 1,
            _ => {}
        }
    }
    // strip leading zeros
    while pos < end && b[pos] == b'0' {
        pos += 1;
    }
    if pos >= end {
        return Parts {
            positive,
            value: "0",
            num: "0",
            denom: "",
            suffix: "",
        };
    }
    // numerator
    let num_start = pos;
    while pos < end && b[pos].is_ascii_digit() {
        pos += 1;
    }
    let mut num = &s[num_start..pos];
    if pos >= end {
        return Parts {
            positive,
            value: s,
            num,
            denom: "",
            suffix: "",
        };
    }
    if num.is_empty() {
        num = "0";
    }
    let mut denom = "";
    if b[pos] == b'.' {
        pos += 1;
        let d_start = pos;
        while pos < end && b[pos].is_ascii_digit() {
            pos += 1;
        }
        denom = &s[d_start..pos];
        if pos >= end {
            return Parts {
                positive,
                value: s,
                num,
                denom,
                suffix: "",
            };
        }
    }
    Parts {
        positive,
        value: &s[..pos],
        num,
        denom,
        suffix: &s[pos..],
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    DecimalExponent,
    DecimalSI,
    BinarySI,
}

/// `quantitySuffixer.interpret`: (base, exponent, format).
fn interpret(suffix: &str) -> Option<(i32, i32, Format)> {
    let dec = match suffix {
        "n" => Some(-9),
        "u" => Some(-6),
        "m" => Some(-3),
        "" => Some(0),
        "k" => Some(3),
        "M" => Some(6),
        "G" => Some(9),
        "T" => Some(12),
        "P" => Some(15),
        "E" => Some(18),
        _ => None,
    };
    if let Some(e) = dec {
        return Some((10, e, Format::DecimalSI));
    }
    let bin = match suffix {
        "Ki" => Some(10),
        "Mi" => Some(20),
        "Gi" => Some(30),
        "Ti" => Some(40),
        "Pi" => Some(50),
        "Ei" => Some(60),
        _ => None,
    };
    if let Some(e) = bin {
        return Some((2, e, Format::BinarySI));
    }
    if suffix.len() > 1 && (suffix.starts_with('E') || suffix.starts_with('e')) {
        let parsed: i64 = suffix[1..].parse().ok()?;
        return Some((10, parsed as i32, Format::DecimalExponent));
    }
    None
}

/// `int64Multiply`
fn int64_multiply(a: i64, b: i64) -> Option<i64> {
    if a == 0 || b == 0 || a == 1 || b == 1 {
        return Some(a.wrapping_mul(b));
    }
    if a == i64::MIN || b == i64::MIN {
        return None;
    }
    a.checked_mul(b)
}

/// `int64Amount.AsScaledInt64(0)` (result only, as `ScaledValue` ignores `ok`).
fn int64_amount_value(value: i64, scale: i32) -> i64 {
    if scale < 0 {
        // negativeScaleInt64(value, -scale)
        let base = value;
        let mut v = value;
        let mut fraction = false;
        for _ in 0..(-scale) {
            if !fraction && v % 10 != 0 {
                fraction = true;
            }
            v /= 10;
            if v == 0 {
                if fraction {
                    return if base > 0 { 1 } else { -1 };
                }
                return 0;
            }
        }
        if fraction {
            if base > 0 {
                v += 1;
            } else {
                v -= 1;
            }
        }
        v
    } else {
        // positiveScaleInt64(value, scale): 0 on overflow
        let mut v = value;
        for _ in 0..scale {
            if v == 0 || v == 1 {
                v *= 10;
                continue;
            }
            if v == i64::MIN {
                return 0;
            }
            match v.checked_mul(10) {
                Some(n) => v = n,
                None => return 0,
            }
        }
        v
    }
}

/// `big.Int.Int64()`: the low 64 bits of |x|, negated when x < 0.
fn big_int64(x: i128) -> i64 {
    let low = x.unsigned_abs() as u64 as i64;
    if x < 0 { low.wrapping_neg() } else { low }
}

const NANO: u32 = 9;

/// `resource.MustParse(s).Value()`, returning the `MustParse` panic message as an error.
pub(crate) fn must_parse_value(s: &str) -> Result<i64, String> {
    parse_value(s).map_err(|e| format!("cannot parse '{s}': {e}"))
}

fn parse_value(s: &str) -> Result<i64, QuantityError> {
    // Validation and Go's error values.
    canonical(s)?;
    if s == "0" {
        return Ok(0);
    }
    let p = parse_quantity_string(s);
    let (base, exponent, format) = interpret(p.suffix).ok_or(QuantityError::Suffix)?;

    let mut scale: i32 = 0;
    let mut mantissa: i64 = 1;
    let precision: i32 = match format {
        Format::DecimalExponent | Format::DecimalSI => {
            scale = exponent;
            18 - (p.num.len() + p.denom.len()) as i32
        }
        Format::BinarySI => {
            if exponent >= 0 && p.denom.is_empty() {
                mantissa <<= exponent as u32;
                15 - p.num.len() as i32 - ((exponent as f32) * 3.0 / 10.0) as i32 - 1
            } else {
                -1
            }
        }
    };

    if precision >= 0 {
        scale -= p.denom.len() as i32;
        if scale >= -9 {
            let shifted = format!("{}{}", p.num, p.denom);
            let value: i64 = shifted.parse().map_err(|_| QuantityError::Numeric)?;
            if let Some(mut result) = int64_multiply(value, mantissa) {
                if !p.positive {
                    result = result.wrapping_neg();
                }
                return Ok(int64_amount_value(result, scale));
            }
        }
    }

    // Slow path: inf.Dec arithmetic on |value| as (unscaled, scale).
    let digits = p.value.trim_start_matches(['+', '-']);
    let (int_part, frac_part) = digits.split_once('.').unwrap_or((digits, ""));
    let negative = p.value.starts_with('-');
    let mut unscaled: i128 = 0;
    for c in int_part.bytes().chain(frac_part.bytes()) {
        unscaled = unscaled
            .saturating_mul(10)
            .saturating_add(i128::from(c - b'0'));
    }
    let mut dscale: i32 = frac_part.len() as i32;
    if base == 10 {
        dscale -= exponent;
    } else {
        unscaled = unscaled
            .checked_shl(exponent as u32)
            .map_or(i128::MAX, |v| {
                if v >> exponent as u32 == unscaled {
                    v
                } else {
                    i128::MAX
                }
            });
    }

    let mut rounded = false;
    if unscaled != 0 {
        // amount.Round(amount, Nano, inf.RoundUp)
        let target = NANO as i32;
        if dscale > target {
            let div = 10i128.checked_pow((dscale - target) as u32);
            unscaled = match div {
                Some(d) => {
                    let q = unscaled / d;
                    if unscaled % d != 0 { q + 1 } else { q }
                }
                None => 1,
            };
        } else if dscale < target {
            unscaled = 10i128
                .checked_pow((target - dscale) as u32)
                .and_then(|m| unscaled.checked_mul(m))
                .unwrap_or(i128::MAX);
        }
        rounded = true;
    }

    // BinarySI cap at 2^63-1.
    let max_allowed = i128::from(i64::MAX) * 1_000_000_000;
    if format == Format::BinarySI && rounded && unscaled > max_allowed {
        unscaled = max_allowed;
    }
    if negative {
        unscaled = -unscaled;
    }

    if !rounded {
        // Zero: scaledValue of 0 is 0 at any scale.
        return Ok(0);
    }
    // scaledValue(unscaled, 9, 0)
    let divide: i128 = 1_000_000_000;
    if unscaled < i128::from(i64::MAX) {
        let u = big_int64(unscaled);
        let d = divide as i64;
        let result = u / d;
        if u % d != 0 {
            return Ok(result.wrapping_add(1));
        }
        return Ok(result);
    }
    let q = unscaled.div_euclid(divide);
    let r = unscaled.rem_euclid(divide);
    if r != 0 {
        return Ok(big_int64(q).wrapping_add(1));
    }
    Ok(big_int64(q))
}

/// `removeInt64Factors`
fn remove_int64_factors(value: i64, base: i64) -> (i64, i32) {
    let mut times = 0;
    let negative = value < 0;
    let mut result = if negative {
        value.wrapping_neg()
    } else {
        value
    };
    while result >= base && result % base == 0 {
        times += 1;
        result /= base;
    }
    if negative {
        result = result.wrapping_neg();
    }
    (result, times)
}

/// `resource.NewQuantity(value, resource.BinarySI).String()`
pub(crate) fn new_binary_si_string(value: i64) -> String {
    if value == 0 {
        return "0".to_string();
    }
    if value > -1024 && value < 1024 {
        // DecimalSI: int64Amount.AsCanonicalBytes
        let (mut amount, times) = remove_int64_factors(value, 10);
        let mut exponent = times;
        match exponent % 3 {
            1 | -2 => {
                amount *= 10;
                exponent -= 1;
            }
            2 | -1 => {
                amount *= 100;
                exponent -= 2;
            }
            _ => {}
        }
        // |value| < 1024 leaves an exponent of 0 or 3.
        let suffix = if exponent == 3 { "k" } else { "" };
        return format!("{amount}{suffix}");
    }
    let (amount, exponent) = remove_int64_factors(value, 1024);
    let suffix = match exponent {
        0 => "",
        1 => "Ki",
        2 => "Mi",
        3 => "Gi",
        4 => "Ti",
        5 => "Pi",
        _ => "Ei",
    };
    format!("{amount}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        for (s, v) in [
            ("2Mi", 2 * 1024 * 1024),
            ("1Gi", 1 << 30),
            ("64Ki", 64 * 1024),
            ("32Mi", 32 << 20),
            ("1.5Mi", 1_572_864),
            ("500m", 1),
            ("1k", 1000),
            ("0", 0),
            ("1e3", 1000),
            ("-2Mi", -(2 << 20)),
            ("1Ei", 1 << 60),
            ("8Ei", i64::MAX),
        ] {
            assert_eq!(parse_value(s).unwrap(), v, "{s}");
        }
        assert_eq!(
            must_parse_value("foo").unwrap_err(),
            "cannot parse 'foo': quantities must match the regular expression '^([+-]?[0-9.]+)([eEinumkKMGTP]*[-+]?[0-9]*)$'"
        );
    }

    #[test]
    fn binary_si_strings() {
        for (v, s) in [
            (0, "0"),
            (500, "500"),
            (1000, "1k"),
            (1023, "1023"),
            (1024, "1Ki"),
            (1025, "1025"),
            (524_288_000, "500Mi"),
            (4 << 30, "4Gi"),
            (-(500 << 20), "-500Mi"),
            (i64::MIN, "-9223372036854775808"),
        ] {
            assert_eq!(new_binary_si_string(v), s, "{v}");
        }
    }
}
