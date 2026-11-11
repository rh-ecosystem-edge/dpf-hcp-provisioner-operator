//! Go `time.Duration` formatting (`Duration.String()`) and parsing (`time.ParseDuration`).

use std::time::Duration;

/// `time.Duration(nanos).String()`, e.g. `1m30s`, `1.5s`, `100ms`, `0s`.
pub fn format_nanos(d: i64) -> String {
    // Port of Go's time.Duration.String / format.
    let mut buf = [0u8; 32];
    let mut w = buf.len();
    let neg = d < 0;
    let mut u = d.unsigned_abs();

    if u < 1_000_000_000 {
        // Special case: if duration is smaller than a second, use smaller units, like 1.2ms
        let prec;
        w -= 1;
        buf[w] = b's';
        w -= 1;
        if u == 0 {
            return "0s".to_string();
        } else if u < 1_000 {
            prec = 0;
            buf[w] = b'n';
        } else if u < 1_000_000 {
            prec = 3;
            // U+00B5 'µ' micro sign == 0xC2 0xB5; written backwards.
            w -= 1;
            buf[w..w + 2].copy_from_slice("µ".as_bytes());
        } else {
            prec = 6;
            buf[w] = b'm';
        }
        let (nw, nu) = fmt_frac(&mut buf[..w], u, prec);
        w = nw;
        w = fmt_int(&mut buf[..w], nu);
    } else {
        w -= 1;
        buf[w] = b's';
        let (nw, nu) = fmt_frac(&mut buf[..w], u, 9);
        w = nw;
        u = nu;
        // u is now integer seconds
        w = fmt_int(&mut buf[..w], u % 60);
        u /= 60;
        // u is now integer minutes
        if u > 0 {
            w -= 1;
            buf[w] = b'm';
            w = fmt_int(&mut buf[..w], u % 60);
            u /= 60;
            // u is now integer hours
            if u > 0 {
                w -= 1;
                buf[w] = b'h';
                w = fmt_int(&mut buf[..w], u);
            }
        }
    }
    if neg {
        w -= 1;
        buf[w] = b'-';
    }
    String::from_utf8_lossy(&buf[w..]).into_owned()
}

/// `Duration.String()` for a std duration.
pub fn format(d: Duration) -> String {
    format_nanos(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

fn fmt_frac(buf: &mut [u8], mut v: u64, prec: usize) -> (usize, u64) {
    let mut w = buf.len();
    let mut print = false;
    for _ in 0..prec {
        let digit = v % 10;
        print = print || digit != 0;
        if print {
            w -= 1;
            buf[w] = digit as u8 + b'0';
        }
        v /= 10;
    }
    if print {
        w -= 1;
        buf[w] = b'.';
    }
    (w, v)
}

fn fmt_int(buf: &mut [u8], mut v: u64) -> usize {
    let mut w = buf.len();
    if v == 0 {
        w -= 1;
        buf[w] = b'0';
    } else {
        while v > 0 {
            w -= 1;
            buf[w] = (v % 10) as u8 + b'0';
            v /= 10;
        }
    }
    w
}

/// `time.ParseDuration`; returns nanoseconds. The error is Go's message
/// (`time: invalid duration "x"` etc.).
pub fn parse_nanos(s: &str) -> Result<i64, String> {
    // [-+]?([0-9]*(\.[0-9]*)?[a-z]+)+
    let orig = s;
    let quoted = || ctrlrt::error::go_quote(orig);
    let mut d: u64 = 0;
    let mut neg = false;
    let mut s = s.as_bytes();

    if !s.is_empty() {
        let c = s[0];
        if c == b'-' || c == b'+' {
            neg = c == b'-';
            s = &s[1..];
        }
    }
    if s == b"0" {
        return Ok(0);
    }
    if s.is_empty() {
        return Err(format!("time: invalid duration {}", quoted()));
    }
    while !s.is_empty() {
        let mut v: u64;
        let mut f: u64 = 0;
        let mut scale: f64 = 1.0;

        // The next character must be [0-9.]
        if !(s[0] == b'.' || s[0].is_ascii_digit()) {
            return Err(format!("time: invalid duration {}", quoted()));
        }
        // Consume [0-9]*
        let pl = s.len();
        let r = leading_int(s).map_err(|_| format!("time: invalid duration {}", quoted()))?;
        v = r.0;
        s = r.1;
        let pre = pl != s.len();

        // Consume (\.[0-9]*)?
        let mut post = false;
        if !s.is_empty() && s[0] == b'.' {
            s = &s[1..];
            let pl = s.len();
            let (ff, sc, rest) = leading_fraction(s);
            f = ff;
            scale = sc;
            s = rest;
            post = pl != s.len();
        }
        if !pre && !post {
            return Err(format!("time: invalid duration {}", quoted()));
        }

        // Consume unit.
        let mut i = 0;
        while i < s.len() {
            let c = s[i];
            if c == b'.' || c.is_ascii_digit() {
                break;
            }
            i += 1;
        }
        if i == 0 {
            return Err(format!("time: missing unit in duration {}", quoted()));
        }
        let u = &s[..i];
        s = &s[i..];
        let unit: u64 = match u {
            b"ns" => 1,
            b"us" => 1_000,
            b"\xc2\xb5s" => 1_000, // U+00B5 = micro symbol
            b"\xce\xbcs" => 1_000, // U+03BC = Greek letter mu
            b"ms" => 1_000_000,
            b"s" => 1_000_000_000,
            b"m" => 60 * 1_000_000_000,
            b"h" => 3600 * 1_000_000_000,
            _ => {
                return Err(format!(
                    "time: unknown unit {} in duration {}",
                    ctrlrt::error::go_quote(&String::from_utf8_lossy(u)),
                    quoted()
                ));
            }
        };
        if v > (1u64 << 63) / unit {
            // overflow
            return Err(format!("time: invalid duration {}", quoted()));
        }
        v *= unit;
        if f > 0 {
            // float64 is needed to be nanosecond accurate for fractions of hours.
            // v >= 0 && (f*unit/scale) <= 3.6e+12 (ns/h, h is the largest unit)
            v += (f as f64 * (unit as f64 / scale)) as u64;
            if v > 1u64 << 63 {
                // overflow
                return Err(format!("time: invalid duration {}", quoted()));
            }
        }
        d = d.wrapping_add(v);
        if d > 1u64 << 63 {
            return Err(format!("time: invalid duration {}", quoted()));
        }
    }
    if neg {
        return Ok((d as i128).wrapping_neg() as i64);
    }
    if d > (1u64 << 63) - 1 {
        return Err(format!("time: invalid duration {}", quoted()));
    }
    Ok(d as i64)
}

fn leading_int(s: &[u8]) -> Result<(u64, &[u8]), ()> {
    let mut i = 0;
    let mut x: u64 = 0;
    while i < s.len() {
        let c = s[i];
        if !c.is_ascii_digit() {
            break;
        }
        if x > (1u64 << 63) / 10 {
            return Err(());
        }
        x = x * 10 + (c - b'0') as u64;
        if x > 1u64 << 63 {
            return Err(());
        }
        i += 1;
    }
    Ok((x, &s[i..]))
}

fn leading_fraction(s: &[u8]) -> (u64, f64, &[u8]) {
    let mut i = 0;
    let mut x: u64 = 0;
    let mut scale: f64 = 1.0;
    let mut overflow = false;
    while i < s.len() {
        let c = s[i];
        if !c.is_ascii_digit() {
            break;
        }
        if overflow {
            i += 1;
            continue;
        }
        if x > ((1u64 << 63) - 1) / 10 {
            overflow = true;
            i += 1;
            continue;
        }
        let y = x * 10 + (c - b'0') as u64;
        if y > 1u64 << 63 {
            overflow = true;
            i += 1;
            continue;
        }
        x = y;
        scale *= 10.0;
        i += 1;
    }
    (x, scale, &s[i..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_go() {
        let cases: &[(i64, &str)] = &[
            (0, "0s"),
            (1, "1ns"),
            (1_100, "1.1µs"),
            (2_200_000, "2.2ms"),
            (3_300_000_000, "3.3s"),
            (60_000_000_000, "1m0s"),
            (4 * 60_000_000_000 + 5_000_000_000, "4m5s"),
            (4 * 60_000_000_000 + 5_001_000_000, "4m5.001s"),
            (5 * 3_600_000_000_000 + 6 * 60_000_000_000 + 7_001_000_000, "5h6m7.001s"),
            (8 * 60_000_000_000 + 1, "8m0.000000001s"),
            (i64::MAX, "2562047h47m16.854775807s"),
            (i64::MIN, "-2562047h47m16.854775808s"),
            (-1_500_000_000, "-1.5s"),
            (10_000_000_000, "10s"),
        ];
        for (d, want) in cases {
            assert_eq!(format_nanos(*d), *want, "{d}");
        }
    }

    #[test]
    fn parses_like_go() {
        let ok: &[(&str, i64)] = &[
            ("0", 0),
            ("5s", 5_000_000_000),
            ("30s", 30_000_000_000),
            ("1478s", 1_478_000_000_000),
            ("-5s", -5_000_000_000),
            ("+5s", 5_000_000_000),
            ("-0", 0),
            ("+0", 0),
            ("5.0s", 5_000_000_000),
            ("5.6s", 5_600_000_000),
            ("5.s", 5_000_000_000),
            (".5s", 500_000_000),
            ("1.0s", 1_000_000_000),
            ("1.00s", 1_000_000_000),
            ("1.004s", 1_004_000_000),
            ("1.0040s", 1_004_000_000),
            ("100.00100s", 100_001_000_000),
            ("10ns", 10),
            ("11us", 11_000),
            ("12µs", 12_000),
            ("12μs", 12_000),
            ("13ms", 13_000_000),
            ("14s", 14_000_000_000),
            ("15m", 900_000_000_000),
            ("16h", 57_600_000_000_000),
            ("3h30m", 12_600_000_000_000),
            ("10.5s4m", 250_500_000_000),
            ("-2m3.4s", -123_400_000_000),
            ("1h2m3s4ms5us6ns", 3_723_004_005_006),
            ("39h9m14.425s", 140_954_425_000_000),
            ("52763797000ns", 52_763_797_000),
            ("0.3333333333333333333h", 1_200_000_000_000),
            ("9007199254740993ns", 9_007_199_254_740_993),
            ("9223372036854775807ns", i64::MAX),
            ("9223372036854775.807us", i64::MAX),
            ("9223372036s854ms775us807ns", i64::MAX),
            ("-9223372036854775808ns", i64::MIN),
            ("-9223372036854775.808us", i64::MIN),
            ("-9223372036s854ms775us808ns", i64::MIN),
            ("-2562047h47m16.854775808s", i64::MIN),
            ("0.100000000000000000000h", 360_000_000_000),
            ("0.830103483285477580700h", 2_988_372_539_827),
        ];
        for (s, want) in ok {
            assert_eq!(parse_nanos(s), Ok(*want), "{s}");
        }
        let bad: &[(&str, &str)] = &[
            ("", r#"time: invalid duration """#),
            ("3", r#"time: missing unit in duration "3""#),
            ("-", r#"time: invalid duration "-""#),
            ("s", r#"time: invalid duration "s""#),
            (".", r#"time: invalid duration ".""#),
            ("-.", r#"time: invalid duration "-.""#),
            (".s", r#"time: invalid duration ".s""#),
            ("+.s", r#"time: invalid duration "+.s""#),
            ("1d", r#"time: unknown unit "d" in duration "1d""#),
            ("9223372036854775808ns", r#"time: invalid duration "9223372036854775808ns""#),
            ("9223372036854775.808us", r#"time: invalid duration "9223372036854775.808us""#),
            ("-9223372036854775809ns", r#"time: invalid duration "-9223372036854775809ns""#),
        ];
        for (s, want) in bad {
            assert_eq!(parse_nanos(s), Err(want.to_string()), "{s}");
        }
    }
}
