//! Port of the `golang.org/x/crypto/cryptobyte` ASN.1 readers used by `crypto/x509`.

use num_bigint::BigUint;

use super::certreq::BigInt;

/// `cryptobyte.String`
#[derive(Clone, Copy, Debug)]
pub struct Reader<'a> {
    s: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(s: &'a [u8]) -> Self {
        Self { s }
    }

    /// `Empty()`
    pub fn is_empty(&self) -> bool {
        self.s.is_empty()
    }

    /// `readASN1(out, outTag, skipHeader=true)`: returns (tag, contents).
    fn read(&mut self) -> Option<(u8, &'a [u8])> {
        let s = self.s;
        if s.len() < 2 {
            return None;
        }
        let (tag, len_byte) = (s[0], s[1]);
        if tag & 0x1f == 0x1f {
            // Only low-tag-number form is supported.
            return None;
        }
        let (length, header_len): (u64, u64) = if len_byte & 0x80 == 0 {
            (u64::from(len_byte) + 2, 2)
        } else {
            let len_len = (len_byte & 0x7f) as usize;
            if len_len == 0 || len_len > 4 || s.len() < 2 + len_len {
                return None;
            }
            let mut len32: u32 = 0;
            for &b in &s[2..2 + len_len] {
                len32 = (len32 << 8) | u32::from(b);
            }
            if len32 < 128 {
                return None;
            }
            if (len32 >> ((len_len - 1) * 8)) == 0 {
                return None;
            }
            let header_len = 2 + len_len as u64;
            (header_len + u64::from(len32), header_len)
        };
        if length > s.len() as u64 {
            return None;
        }
        let elem = &s[..length as usize];
        self.s = &s[length as usize..];
        Some((tag, &elem[header_len as usize..]))
    }

    /// `ReadASN1(&out, tag)`: the contents of the next element if its tag matches.
    pub fn read_asn1(&mut self, tag: u8) -> Option<Reader<'a>> {
        let mut copy = *self;
        let (t, contents) = copy.read()?;
        // On mismatch Go has already advanced; callers bail out in that case anyway.
        *self = copy;
        if t != tag {
            return None;
        }
        Some(Reader::new(contents))
    }

    /// `ReadAnyASN1(&out, &tag)`
    pub fn read_any_asn1(&mut self) -> Option<(u8, &'a [u8])> {
        self.read()
    }

    fn read_integer_bytes(&mut self) -> Option<&'a [u8]> {
        let bytes = self.read_asn1(0x02)?.s;
        if !check_asn1_integer(bytes) {
            return None;
        }
        Some(bytes)
    }

    /// `ReadASN1Integer(*big.Int)`
    pub fn read_asn1_big_int(&mut self) -> Option<BigInt> {
        let bytes = self.read_integer_bytes()?;
        if bytes[0] & 0x80 == 0x80 {
            let neg: Vec<u8> = bytes.iter().map(|b| !b).collect();
            let magnitude = BigUint::from_bytes_be(&neg) + 1u32;
            Some(BigInt {
                sign: -1,
                magnitude,
            })
        } else {
            let magnitude = BigUint::from_bytes_be(bytes);
            let sign = if magnitude == BigUint::default() { 0 } else { 1 };
            Some(BigInt { sign, magnitude })
        }
    }

    /// `ReadASN1Integer(*int)` (64-bit int)
    pub fn read_asn1_int64(&mut self) -> Option<i64> {
        let bytes = self.read_integer_bytes()?;
        if bytes.len() > 8 {
            return None;
        }
        let mut out: i64 = 0;
        for &b in bytes {
            out = (out << 8) | i64::from(b);
        }
        let shift = 64 - (bytes.len() as u32) * 8;
        if shift < 64 {
            out = (out << shift) >> shift;
        }
        Some(out)
    }

    /// `ReadASN1ObjectIdentifier`
    pub fn read_asn1_object_identifier(&mut self) -> Option<Vec<i64>> {
        let r = self.read_asn1(0x06)?;
        let mut bytes = r.s;
        if bytes.is_empty() {
            return None;
        }
        let mut components = Vec::with_capacity(bytes.len() + 1);
        let v = read_base128_int(&mut bytes)?;
        if v < 80 {
            components.push(v / 40);
            components.push(v % 40);
        } else {
            components.push(2);
            components.push(v - 80);
        }
        while !bytes.is_empty() {
            components.push(read_base128_int(&mut bytes)?);
        }
        Some(components)
    }
}

fn check_asn1_integer(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    if bytes.len() == 1 {
        return true;
    }
    !((bytes[0] == 0 && bytes[1] & 0x80 == 0) || (bytes[0] == 0xff && bytes[1] & 0x80 == 0x80))
}

fn read_base128_int(s: &mut &[u8]) -> Option<i64> {
    let mut ret: i64 = 0;
    let mut i = 0;
    while !s.is_empty() {
        if i == 5 {
            return None;
        }
        if ret >= 1 << (31 - 7) {
            return None;
        }
        ret <<= 7;
        let b = s[0];
        *s = &s[1..];
        if i == 0 && b == 0x80 {
            return None;
        }
        ret |= i64::from(b & 0x7f);
        if b & 0x80 == 0 {
            return Some(ret);
        }
        i += 1;
    }
    None
}
