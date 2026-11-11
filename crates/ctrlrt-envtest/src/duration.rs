//! Go `time.ParseDuration` (used for `KUBEBUILDER_CONTROLPLANE_{START,STOP}_TIMEOUT`).

use std::time::Duration;

fn quote(s: &str) -> String {
    ctrlrt::error::go_quote(s)
}

/// Parses a Go duration string such as `"300ms"`, `"1.5h"` or `"2h45m"`.
///
/// Errors mirror Go's messages (`time: invalid duration "x"`, ...). Negative durations are
/// accepted by Go; they are clamped to zero here (a negative timeout fires immediately in Go).
pub fn parse_go_duration(orig: &str) -> anyhow::Result<Duration> {
    let invalid = || anyhow::anyhow!("time: invalid duration {}", quote(orig));
    let mut s = orig;
    let mut neg = false;
    if let Some(c) = s.chars().next()
        && (c == '-' || c == '+')
    {
        neg = c == '-';
        s = &s[1..];
    }
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    if s.is_empty() {
        return Err(invalid());
    }
    let mut total_nanos: f64 = 0.0;
    while !s.is_empty() {
        // integer part
        let int_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        let int_part = &s[..int_end];
        s = &s[int_end..];
        let mut frac_part = "";
        if let Some(rest) = s.strip_prefix('.') {
            let frac_end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            frac_part = &rest[..frac_end];
            s = &rest[frac_end..];
            if int_part.is_empty() && frac_part.is_empty() {
                return Err(invalid());
            }
        } else if int_part.is_empty() {
            return Err(invalid());
        }
        let unit_end = s
            .find(|c: char| c == '.' || c.is_ascii_digit())
            .unwrap_or(s.len());
        let unit = &s[..unit_end];
        s = &s[unit_end..];
        if unit.is_empty() {
            return Err(anyhow::anyhow!(
                "time: missing unit in duration {}",
                quote(orig)
            ));
        }
        let scale: f64 = match unit {
            "ns" => 1.0,
            "us" | "\u{b5}s" | "\u{3bc}s" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60e9,
            "h" => 3600e9,
            _ => {
                return Err(anyhow::anyhow!(
                    "time: unknown unit {} in duration {}",
                    quote(unit),
                    quote(orig)
                ));
            }
        };
        let number: f64 = format!(
            "{}.{}",
            if int_part.is_empty() { "0" } else { int_part },
            if frac_part.is_empty() { "0" } else { frac_part }
        )
        .parse()
        .map_err(|_| invalid())?;
        total_nanos += number * scale;
    }
    if total_nanos > (i64::MAX as f64) {
        return Err(invalid());
    }
    if neg {
        return Ok(Duration::ZERO);
    }
    Ok(Duration::from_nanos(total_nanos.round() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_go_durations() {
        assert_eq!(parse_go_duration("20s").unwrap(), Duration::from_secs(20));
        assert_eq!(parse_go_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(
            parse_go_duration("1.5h").unwrap(),
            Duration::from_secs(5400)
        );
        assert_eq!(
            parse_go_duration("300ms").unwrap(),
            Duration::from_millis(300)
        );
        assert_eq!(parse_go_duration("0").unwrap(), Duration::ZERO);
        assert_eq!(
            parse_go_duration("xyz").unwrap_err().to_string(),
            "time: invalid duration \"xyz\""
        );
        assert_eq!(
            parse_go_duration("10").unwrap_err().to_string(),
            "time: missing unit in duration \"10\""
        );
        assert_eq!(
            parse_go_duration("10x").unwrap_err().to_string(),
            "time: unknown unit \"x\" in duration \"10x\""
        );
    }
}
