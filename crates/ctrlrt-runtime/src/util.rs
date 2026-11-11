//! Small Go-compatibility helpers shared by the runtime modules.

use std::time::Duration;

use ctrlrt::ResourceInfo;

/// Go's `time.Duration.String()`.
pub fn go_duration(d: Duration) -> String {
    go_duration_nanos(d.as_nanos().min(i64::MAX as u128) as i64)
}

/// Go's `time.Duration.String()` for a (possibly negative) nanosecond count.
pub fn go_duration_nanos(d: i64) -> String {
    // Port of time.Duration.String (src/time/time.go).
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
            // U+00B5 'µ' micro sign == 0xC2 0xB5
            w -= 1;
            buf[w..w + 2].copy_from_slice("µ".as_bytes());
        } else {
            prec = 6;
            buf[w] = b'm';
        }
        let (nw, nu) = fmt_frac(&mut buf[..w], u, prec);
        w = nw;
        u = nu;
        w = fmt_int(&mut buf[..w], u);
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

/// Approximation of Go's `%T` for a pointer to an API type: `*<version>.<Kind>`.
///
/// Go package names of API types are the version (`corev1` is package `v1`, the HyperShift
/// API package is `v1beta1`, ...), so this matches the Go output for all types used here.
pub fn go_type_name(res: &ResourceInfo) -> String {
    format!("*{}.{}", res.version, res.kind)
}

/// `schema.GroupVersionKind.String()`: `group/version, Kind=kind`.
pub fn gvk_string(res: &ResourceInfo) -> String {
    format!("{}, Kind={}", res.api_version(), res.kind)
}

/// `labels.SelectorFromSet(set).String()`: `k1=v1,k2=v2` sorted by key.
pub fn selector_from_set<'a, I>(set: I) -> String
where
    I: IntoIterator<Item = (&'a String, &'a String)>,
{
    let mut parts: Vec<(&String, &String)> = set.into_iter().collect();
    parts.sort();
    parts
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Go's `os.Hostname()` on Linux: `/proc/sys/kernel/hostname`, falling back to `uname`.
pub fn os_hostname() -> std::io::Result<String> {
    if let Ok(s) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let s = s.trim_end_matches('\n').to_string();
        if !s.is_empty() {
            return Ok(s);
        }
    }
    hostname::get().map(|h| h.to_string_lossy().into_owned())
}

/// `uuid.NewUUID()` from apimachinery (random v4 UUID).
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_duration_formats_like_go() {
        let cases: &[(i64, &str)] = &[
            (0, "0s"),
            (1, "1ns"),
            (1_100, "1.1µs"),
            (2_200_000, "2.2ms"),
            (3_300_000_000, "3.3s"),
            (4 * 60 * 1_000_000_000 + 5_000_000_000, "4m5s"),
            (4 * 60 * 1_000_000_000 + 5_001_000_000, "4m5.001s"),
            (
                5 * 3600 * 1_000_000_000 + 6 * 60 * 1_000_000_000 + 7_001_000_000,
                "5h6m7.001s",
            ),
            (8 * 60 * 1_000_000_000 + 1, "8m0.000000001s"),
            (30_000_000_000, "30s"),
            (10_000_000_000, "10s"),
            (-1, "-1ns"),
            (3600 * 1_000_000_000, "1h0m0s"),
        ];
        for (d, want) in cases {
            assert_eq!(go_duration_nanos(*d), *want, "{d}");
        }
    }

    #[test]
    fn selector_from_set_sorts() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("b".to_string(), "2".to_string());
        m.insert("a".to_string(), "1".to_string());
        assert_eq!(selector_from_set(&m), "a=1,b=2");
    }
}
