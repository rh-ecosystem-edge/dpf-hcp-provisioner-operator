//! `github.com/openshift/hypershift/api/util/ipnet` — `net.IPNet` with CIDR JSON encoding.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// `ipnet.IPNet` (a `net.IPNet`): `ip` is 4 bytes for IPv4 (as produced by [`IPNet::parse_cidr`])
/// or 16 bytes; `mask` is 4 or 16 bytes. The zero value (empty IP) encodes as JSON `null`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct IPNet {
    pub ip: Vec<u8>,
    pub mask: Vec<u8>,
}

/// `net.ParseError` for CIDRs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid CIDR address: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// `net.CIDRMask(ones, bits)`
fn cidr_mask(ones: usize, bits: usize) -> Vec<u8> {
    let mut m = vec![0u8; bits / 8];
    let mut n = ones;
    for b in m.iter_mut() {
        if n >= 8 {
            *b = 0xff;
            n -= 8;
        } else {
            *b = !(0xffu8 >> n);
            n = 0;
        }
    }
    m
}

/// `dtoi`: decimal digits, Go's big = 0xFFFFFF cap.
fn dtoi(s: &str) -> Option<usize> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut n: usize = 0;
    for c in s.bytes() {
        n = n * 10 + usize::from(c - b'0');
        if n >= 0xFF_FFFF {
            return None;
        }
    }
    Some(n)
}

/// IPv4-mapped / 4-byte → 4 bytes (`net.IP.To4`).
fn to4(ip: &[u8]) -> Option<[u8; 4]> {
    match ip.len() {
        4 => Some([ip[0], ip[1], ip[2], ip[3]]),
        16 if ip[..10].iter().all(|b| *b == 0) && ip[10] == 0xff && ip[11] == 0xff => {
            Some([ip[12], ip[13], ip[14], ip[15]])
        }
        _ => None,
    }
}

/// `net.IP.String()` for valid lengths.
fn ip_string(ip: &[u8]) -> String {
    if let Some(v4) = to4(ip) {
        return Ipv4Addr::from(v4).to_string();
    }
    if ip.len() != 16 {
        if ip.is_empty() {
            return "<nil>".into();
        }
        return format!("?{}", hex(ip));
    }
    // RFC 5952 like Go's netip: longest run (first on ties) of >= 2 zero groups -> "::".
    let groups: Vec<u16> = (0..8)
        .map(|i| u16::from_be_bytes([ip[2 * i], ip[2 * i + 1]]))
        .collect();
    let (mut best_start, mut best_len) = (usize::MAX, 0);
    let mut i = 0;
    while i < 8 {
        if groups[i] == 0 {
            let start = i;
            while i < 8 && groups[i] == 0 {
                i += 1;
            }
            if i - start > best_len {
                best_start = start;
                best_len = i - start;
            }
        } else {
            i += 1;
        }
    }
    if best_len < 2 {
        best_start = usize::MAX;
    }
    let mut out = String::new();
    let mut i = 0;
    while i < 8 {
        if i == best_start {
            out.push_str("::");
            i += best_len;
            continue;
        }
        if !out.is_empty() && !out.ends_with(':') {
            out.push(':');
        }
        out.push_str(&format!("{:x}", groups[i]));
        i += 1;
    }
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `simpleMaskLength`
fn simple_mask_length(mask: &[u8]) -> Option<usize> {
    let mut n = 0;
    for (i, v) in mask.iter().enumerate() {
        if *v == 0xff {
            n += 8;
            continue;
        }
        let mut v = *v;
        while v & 0x80 != 0 {
            n += 1;
            v <<= 1;
        }
        if v != 0 {
            return None;
        }
        if mask[i + 1..].iter().any(|b| *b != 0) {
            return None;
        }
        break;
    }
    Some(n)
}

impl IPNet {
    /// `ipnet.ParseCIDR` (IPv4 addresses are stored as 4 bytes; the host bits are kept).
    pub fn parse_cidr(s: &str) -> Result<IPNet, ParseError> {
        let err = || ParseError(s.to_string());
        let (addr, mask) = s.split_once('/').ok_or_else(err)?;
        let (ip16, bits): ([u8; 16], usize) = if let Ok(v4) = addr.parse::<Ipv4Addr>() {
            (v4.to_ipv6_mapped().octets(), 32)
        } else if !addr.contains('%')
            && let Ok(v6) = addr.parse::<Ipv6Addr>()
        {
            (v6.octets(), 128)
        } else {
            return Err(err());
        };
        let n = dtoi(mask).ok_or_else(err)?;
        if n > bits {
            return Err(err());
        }
        let m = cidr_mask(n, bits);
        let ip = match to4(&ip16) {
            Some(v4) => v4.to_vec(),
            None => ip16.to_vec(),
        };
        Ok(IPNet { ip, mask: m })
    }

    /// `ipnet.MustParseCIDR` — panics on invalid input (only for constant inputs, like Go).
    pub fn must_parse_cidr(s: &str) -> IPNet {
        match IPNet::parse_cidr(s) {
            Ok(n) => n,
            Err(e) => panic!("{e}"),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ip.is_empty()
    }
}

/// `(*ipnet.IPNet).String()` = `net.IPNet.String()`.
impl fmt::Display for IPNet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // networkNumberAndMask
        let ip: Vec<u8> = match to4(&self.ip) {
            Some(v4) => v4.to_vec(),
            None if self.ip.len() == 16 => self.ip.clone(),
            None => return f.write_str("<nil>"),
        };
        let mask: &[u8] = match self.mask.len() {
            4 if ip.len() == 4 => &self.mask,
            4 => return f.write_str("<nil>"),
            16 if ip.len() == 4 => &self.mask[12..],
            16 => &self.mask,
            _ => return f.write_str("<nil>"),
        };
        match simple_mask_length(mask) {
            Some(l) => write!(f, "{}/{}", ip_string(&ip), l),
            None => write!(f, "{}/{}", ip_string(&ip), hex(mask)),
        }
    }
}

impl Serialize for IPNet {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.ip.is_empty() {
            s.serialize_none()
        } else {
            s.collect_str(self)
        }
    }
}

impl<'de> Deserialize<'de> for IPNet {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Some(cidr) = Option::<String>::deserialize(d)? else {
            return Ok(IPNet::default());
        };
        if cidr == "<nil>" {
            return Ok(IPNet::default());
        }
        IPNet::parse_cidr(&cidr)
            .map_err(|e| D::Error::custom(format!("could not parse cidr {cidr}: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format() {
        for (input, want) in [
            ("172.31.0.0/16", "172.31.0.0/16"),
            ("10.132.0.0/14", "10.132.0.0/14"),
            ("10.0.0.5/8", "10.0.0.5/8"),
            ("2001:db8::/64", "2001:db8::/64"),
            ("2001:db8:0:0:1:0:0:1/128", "2001:db8::1:0:0:1/128"),
            ("::ffff:10.0.0.1/104", "10.0.0.1/8"),
            ("0.0.0.0/0", "0.0.0.0/0"),
            ("1.2.3.4/08", "1.2.3.4/8"),
        ] {
            assert_eq!(
                IPNet::parse_cidr(input).unwrap().to_string(),
                want,
                "{input}"
            );
        }
        for bad in [
            "10.0.0.0",
            "10.0.0.0/33",
            "010.0.0.0/8",
            "fe80::1%eth0/64",
            "x/8",
            "1.2.3.4/",
        ] {
            assert!(IPNet::parse_cidr(bad).is_err(), "{bad}");
        }
        assert_eq!(
            IPNet::parse_cidr("10.0.0.0").unwrap_err().to_string(),
            "invalid CIDR address: 10.0.0.0"
        );
    }

    #[test]
    fn json() {
        assert_eq!(serde_json::to_string(&IPNet::default()).unwrap(), "null");
        let n = IPNet::must_parse_cidr("10.96.0.0/12");
        assert_eq!(serde_json::to_string(&n).unwrap(), "\"10.96.0.0/12\"");
        let back: IPNet = serde_json::from_str("\"10.96.0.0/12\"").unwrap();
        assert_eq!(back, n);
        let null: IPNet = serde_json::from_str("null").unwrap();
        assert!(null.is_empty());
    }
}
