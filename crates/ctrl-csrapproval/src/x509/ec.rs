//! NIST curve point validation, as performed by Go's `crypto/internal/fips140/nistec`
//! `SetBytes` for uncompressed points: coordinates must be reduced modulo p and the point
//! must satisfy y² = x³ - 3x + b.

use std::sync::LazyLock;

use num_bigint::BigUint;

pub struct Curve {
    name: &'static str,
    byte_len: usize,
    params: LazyLock<(BigUint, BigUint)>,
}

fn dec(s: &str) -> BigUint {
    BigUint::parse_bytes(s.as_bytes(), 10).unwrap_or_default()
}
fn hex(s: &str) -> BigUint {
    BigUint::parse_bytes(s.as_bytes(), 16).unwrap_or_default()
}

// Constants from Go's crypto/elliptic/nistec.go (SP 800-186).
pub static P224: Curve = Curve {
    name: "P224",
    byte_len: 28,
    params: LazyLock::new(|| {
        (
            dec("26959946667150639794667015087019630673557916260026308143510066298881"),
            hex("b4050a850c04b3abf54132565044b0b7d7bfd8ba270b39432355ffb4"),
        )
    }),
};

pub static P256: Curve = Curve {
    name: "P256",
    byte_len: 32,
    params: LazyLock::new(|| {
        (
            dec("115792089210356248762697446949407573530086143415290314195533631308867097853951"),
            hex("5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b"),
        )
    }),
};

pub static P384: Curve = Curve {
    name: "P384",
    byte_len: 48,
    params: LazyLock::new(|| {
        (
            dec(concat!(
                "394020061963944792122790401001436138050797392704654",
                "46667948293404245721771496870329047266088258938001861606973112319"
            )),
            hex(concat!(
                "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088",
                "f5013875ac656398d8a2ed19d2a85c8edd3ec2aef"
            )),
        )
    }),
};

pub static P521: Curve = Curve {
    name: "P521",
    byte_len: 66,
    params: LazyLock::new(|| {
        (
            dec(concat!(
                "68647976601306097149819007990813932172694353001433",
                "0540939446345918554318339765605212255964066145455497729631139148",
                "0858037121987999716643812574028291115057151"
            )),
            hex(concat!(
                "0051953eb9618e1c9a1f929a21a0b68540eea2da725b99b315f3b8",
                "b489918ef109e156193951ec7e937b1652c0bd3bb1bf073573df883d2c34f1ef",
                "451fd46b503f00"
            )),
        )
    }),
};

impl Curve {
    /// `ecdsa.NewPublicKey` → `Point.SetBytes` for a `0x04 || X || Y` encoding.
    pub fn check_uncompressed_point(&self, data: &[u8]) -> Result<(), String> {
        if data.len() != 1 + 2 * self.byte_len || data[0] != 4 {
            return Err(format!("invalid {} point encoding", self.name));
        }
        let (p, b) = &*self.params;
        let x = BigUint::from_bytes_be(&data[1..1 + self.byte_len]);
        let y = BigUint::from_bytes_be(&data[1 + self.byte_len..]);
        if &x >= p || &y >= p {
            // amd64 assembly P256 vs fiat-crypto field elements for the other curves.
            return Err(if self.name == "P256" {
                "invalid P256 element encoding".to_string()
            } else {
                format!("invalid {}Element encoding", self.name)
            });
        }
        // y² = x³ - 3x + b (mod p)
        let lhs = (&y * &y) % p;
        let x3 = (&x * &x % p) * &x % p;
        let three_x = (&x * 3u32) % p;
        let rhs = ((x3 + p - three_x) + b) % p;
        if lhs != rhs {
            return Err(format!("{} point not on curve", self.name));
        }
        Ok(())
    }
}
