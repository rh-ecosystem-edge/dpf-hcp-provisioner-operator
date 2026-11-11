//! Port of `crypto/x509.ParseCertificateRequest` (Go 1.26) restricted to the outputs the
//! CSR approver uses (subject CN/O and the SAN lists), with the same acceptance rules:
//! DER structure via [`super::asn1`], public key validation (`parsePublicKey`), subject
//! decoding (`Name.FillFromRDNSequence`), extension-request attributes
//! (`parseCSRExtensions`) and the SAN extension (`parseSANExtension`). The signature is not
//! verified (Go doesn't either).

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use num_bigint::BigUint;

use super::asn1::{self, Asn1Error, BitString, Field, Params, Ty, Val};
use super::cryptobyte::Reader;
use super::gourl;

/// `x509.ParseCertificateRequest` error.
pub type ParseError = Asn1Error;

fn err(m: impl Into<String>) -> ParseError {
    Asn1Error::Plain(m.into())
}

// --- Go type schema ------------------------------------------------------------------------

const P: Params = Params::NONE;

static ALGORITHM_IDENTIFIER: Ty = Ty::Struct {
    name: "AlgorithmIdentifier",
    raw_content: false,
    fields: &[
        Field {
            ty: &Ty::Oid,
            params: P,
        },
        Field {
            ty: &Ty::RawValue,
            params: Params {
                optional: true,
                tag: None,
                set: false,
            },
        },
    ],
};

static PUBLIC_KEY_INFO: Ty = Ty::Struct {
    name: "publicKeyInfo",
    raw_content: true,
    fields: &[
        Field {
            ty: &ALGORITHM_IDENTIFIER,
            params: P,
        },
        Field {
            ty: &Ty::BitString,
            params: P,
        },
    ],
};

static RAW_VALUE_SLICE: Ty = Ty::Slice {
    name: "",
    elem: &Ty::RawValue,
};

static TBS_CERTIFICATE_REQUEST: Ty = Ty::Struct {
    name: "tbsCertificateRequest",
    raw_content: true,
    fields: &[
        // Version int
        Field {
            ty: &Ty::Int,
            params: P,
        },
        // Subject asn1.RawValue
        Field {
            ty: &Ty::RawValue,
            params: P,
        },
        // PublicKey publicKeyInfo
        Field {
            ty: &PUBLIC_KEY_INFO,
            params: P,
        },
        // RawAttributes []asn1.RawValue `asn1:"tag:0"`
        Field {
            ty: &RAW_VALUE_SLICE,
            params: Params {
                optional: false,
                tag: Some(0),
                set: false,
            },
        },
    ],
};

static CERTIFICATE_REQUEST: Ty = Ty::Struct {
    name: "certificateRequest",
    raw_content: true,
    fields: &[
        Field {
            ty: &TBS_CERTIFICATE_REQUEST,
            params: P,
        },
        Field {
            ty: &ALGORITHM_IDENTIFIER,
            params: P,
        },
        Field {
            ty: &Ty::BitString,
            params: P,
        },
    ],
};

static ATTRIBUTE_TYPE_AND_VALUE: Ty = Ty::Struct {
    name: "AttributeTypeAndValue",
    raw_content: false,
    fields: &[
        Field {
            ty: &Ty::Oid,
            params: P,
        },
        Field {
            ty: &Ty::Any,
            params: P,
        },
    ],
};

static RDN_SET: Ty = Ty::Slice {
    name: "RelativeDistinguishedNameSET",
    elem: &ATTRIBUTE_TYPE_AND_VALUE,
};

static RDN_SEQUENCE: Ty = Ty::Slice {
    name: "RDNSequence",
    elem: &RDN_SET,
};

static PKCS10_ATTRIBUTE: Ty = Ty::Struct {
    name: "pkcs10Attribute",
    raw_content: false,
    fields: &[
        Field {
            ty: &Ty::Oid,
            params: P,
        },
        Field {
            ty: &RAW_VALUE_SLICE,
            params: Params {
                optional: false,
                tag: None,
                set: true,
            },
        },
    ],
};

static EXTENSION: Ty = Ty::Struct {
    name: "Extension",
    raw_content: false,
    fields: &[
        Field {
            ty: &Ty::Oid,
            params: P,
        },
        Field {
            ty: &Ty::Bool,
            params: Params {
                optional: true,
                tag: None,
                set: false,
            },
        },
        Field {
            ty: &Ty::Bytes,
            params: P,
        },
    ],
};

static EXTENSION_SLICE: Ty = Ty::Slice {
    name: "",
    elem: &EXTENSION,
};

// --- OIDs ----------------------------------------------------------------------------------

const OID_PUBLIC_KEY_RSA: &[i64] = &[1, 2, 840, 113549, 1, 1, 1];
const OID_PUBLIC_KEY_DSA: &[i64] = &[1, 2, 840, 10040, 4, 1];
const OID_PUBLIC_KEY_ECDSA: &[i64] = &[1, 2, 840, 10045, 2, 1];
const OID_PUBLIC_KEY_ED25519: &[i64] = &[1, 3, 101, 112];
const OID_NAMED_CURVE_P224: &[i64] = &[1, 3, 132, 0, 33];
const OID_NAMED_CURVE_P256: &[i64] = &[1, 2, 840, 10045, 3, 1, 7];
const OID_NAMED_CURVE_P384: &[i64] = &[1, 3, 132, 0, 34];
const OID_NAMED_CURVE_P521: &[i64] = &[1, 3, 132, 0, 35];
const OID_EXTENSION_REQUEST: &[i64] = &[1, 2, 840, 113549, 1, 9, 14];
const OID_EXTENSION_SUBJECT_ALT_NAME: &[i64] = &[2, 5, 29, 17];

// --- Output --------------------------------------------------------------------------------

/// `pkix.Name` subset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Name {
    pub common_name: String,
    pub organization: Vec<String>,
}

/// A `net.IP` from a SAN (4 or 16 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ip(pub Vec<u8>);

impl Ip {
    /// `ip.To4()`
    fn to4(&self) -> Option<[u8; 4]> {
        match self.0.len() {
            4 => Some([self.0[0], self.0[1], self.0[2], self.0[3]]),
            16 if self.0[..10].iter().all(|&b| b == 0)
                && self.0[10] == 0xff
                && self.0[11] == 0xff =>
            {
                Some([self.0[12], self.0[13], self.0[14], self.0[15]])
            }
            _ => None,
        }
    }

    /// `ip.IsUnspecified()`
    pub fn is_unspecified(&self) -> bool {
        match self.to4() {
            Some(v4) => v4 == [0, 0, 0, 0],
            None => self.0.len() == 16 && self.0.iter().all(|&b| b == 0),
        }
    }

    /// `ip.IsLoopback()`
    pub fn is_loopback(&self) -> bool {
        match self.to4() {
            Some(v4) => v4[0] == 127,
            None => self.0.len() == 16 && self.0[..15].iter().all(|&b| b == 0) && self.0[15] == 1,
        }
    }

    /// `ip.IsMulticast()`
    pub fn is_multicast(&self) -> bool {
        match self.to4() {
            Some(v4) => v4[0] & 0xf0 == 0xe0,
            None => self.0.len() == 16 && self.0[0] == 0xff,
        }
    }
}

impl fmt::Display for Ip {
    /// `net.IP.String()`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("<nil>");
        }
        if let Some(v4) = self.to4() {
            return write!(f, "{}", Ipv4Addr::from(v4));
        }
        if self.0.len() == 16 {
            let mut a = [0u8; 16];
            a.copy_from_slice(&self.0);
            return write!(f, "{}", Ipv6Addr::from(a));
        }
        write!(f, "?")?;
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// `x509.CertificateRequest` subset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CertificateRequest {
    pub version: i64,
    pub subject: Name,
    pub dns_names: Vec<String>,
    pub email_addresses: Vec<String>,
    pub ip_addresses: Vec<Ip>,
    /// URIs as given in the SAN (Go keeps parsed `*url.URL`s).
    pub uris: Vec<String>,
}

/// Go `%v` of a `[]string`: `[a b c]`.
pub fn fmt_string_list(v: &[String]) -> String {
    format!("[{}]", v.join(" "))
}

/// Go `%v` of a `[]net.IP`: `[1.2.3.4 ::1]`.
pub fn fmt_ip_list(v: &[Ip]) -> String {
    format!(
        "[{}]",
        v.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    )
}

// --- Parsing -------------------------------------------------------------------------------

/// `x509.ParseCertificateRequest(asn1Data)`
pub fn parse_certificate_request(asn1_data: &[u8]) -> Result<CertificateRequest, ParseError> {
    let (csr, rest) = asn1::unmarshal(asn1_data, &CERTIFICATE_REQUEST)?;
    if !rest.is_empty() {
        return Err(Asn1Error::Syntax("trailing data".into()));
    }
    parse_certificate_request_inner(&csr)
}

fn parse_certificate_request_inner(csr: &Val) -> Result<CertificateRequest, ParseError> {
    // certificateRequest{Raw, TBSCSR, SignatureAlgorithm, SignatureValue}
    let tbs = &csr.fields()[1];
    // tbsCertificateRequest{Raw, Version, Subject, PublicKey, RawAttributes}
    let tf = tbs.fields();
    let version = match &tf[1] {
        Val::Int(v) => *v,
        _ => 0,
    };
    let subject_raw = tf[2].raw();
    // publicKeyInfo{Raw, Algorithm, PublicKey}
    let pki = tf[3].fields();
    let algorithm = pki[1].fields();
    let alg_oid = algorithm[0].oid();
    let alg_params = algorithm[1].raw();
    let public_key = pki[2].bits();
    let raw_attributes: Vec<asn1::RawValue> = tf[4].items().iter().map(Val::raw).collect();

    let known_algorithm = [
        OID_PUBLIC_KEY_RSA,
        OID_PUBLIC_KEY_DSA,
        OID_PUBLIC_KEY_ECDSA,
        OID_PUBLIC_KEY_ED25519,
    ]
    .contains(&alg_oid.as_slice());
    if known_algorithm {
        parse_public_key(&alg_oid, &alg_params, &public_key)?;
    }

    let (subject, rest) = asn1::unmarshal(&subject_raw.full_bytes, &RDN_SEQUENCE)?;
    if !rest.is_empty() {
        return Err(err("x509: trailing data after X.509 Subject"));
    }

    let mut out = CertificateRequest {
        version,
        subject: fill_from_rdn_sequence(&subject),
        ..Default::default()
    };

    for (id, value) in parse_csr_extensions(&raw_attributes)? {
        if id == OID_EXTENSION_SUBJECT_ALT_NAME {
            parse_san_extension(&value, &mut out)?;
        }
    }

    Ok(out)
}

/// `Name.FillFromRDNSequence` (CommonName and Organization).
fn fill_from_rdn_sequence(rdns: &Val) -> Name {
    let mut n = Name::default();
    for rdn in rdns.items() {
        for atv in rdn.items() {
            let f = atv.fields();
            let Some(value) = f[1].any_string() else {
                continue;
            };
            let t = f[0].oid();
            if t.len() == 4 && t[0] == 2 && t[1] == 5 && t[2] == 4 {
                match t[3] {
                    3 => n.common_name = value.to_string(),
                    10 => n.organization.push(value.to_string()),
                    _ => {}
                }
            }
        }
    }
    n
}

/// A requested extension: (id, value).
type RequestedExtension = (Vec<i64>, Vec<u8>);

/// `parseCSRExtensions`: (extension id, value) of every requested extension.
fn parse_csr_extensions(
    raw_attributes: &[asn1::RawValue],
) -> Result<Vec<RequestedExtension>, ParseError> {
    let mut ret = Vec::new();
    let mut requested = std::collections::BTreeSet::new();
    for raw_attr in raw_attributes {
        let attr = match asn1::unmarshal(&raw_attr.full_bytes, &PKCS10_ATTRIBUTE) {
            Ok((attr, rest)) if rest.is_empty() && !attr.fields()[1].items().is_empty() => attr,
            // Ignore attributes that don't parse.
            _ => continue,
        };
        let f = attr.fields();
        if f[0].oid() != OID_EXTENSION_REQUEST {
            continue;
        }
        let first = f[1].items()[0].raw();
        let (extensions, _) = asn1::unmarshal(&first.full_bytes, &EXTENSION_SLICE)?;
        for ext in extensions.items() {
            let ef = ext.fields();
            let id = ef[0].oid();
            if !requested.insert(asn1::oid_string(&id)) {
                return Err(err(
                    "x509: certificate request contains duplicate requested extensions",
                ));
            }
            ret.push((id, ef[2].bytes()));
        }
    }
    Ok(ret)
}

/// `isIA5String` (iterating runes: any byte >= 0x80 is rejected).
fn is_ia5(data: &[u8]) -> bool {
    data.iter().all(|&b| b < 0x80)
}

/// `parseSANExtension`
fn parse_san_extension(der: &[u8], out: &mut CertificateRequest) -> Result<(), ParseError> {
    let mut der = Reader::new(der);
    let Some(mut seq) = der.read_asn1(0x30) else {
        return Err(err("x509: invalid subject alternative names"));
    };
    while !seq.is_empty() {
        let Some((tag, data)) = seq.read_any_asn1() else {
            return Err(err("x509: invalid subject alternative name"));
        };
        match tag ^ 0x80 {
            // nameTypeEmail
            1 => {
                if !is_ia5(data) {
                    return Err(err("x509: SAN rfc822Name is malformed"));
                }
                out.email_addresses
                    .push(String::from_utf8_lossy(data).into_owned());
            }
            // nameTypeDNS
            2 => {
                if !is_ia5(data) {
                    return Err(err("x509: SAN dNSName is malformed"));
                }
                out.dns_names
                    .push(String::from_utf8_lossy(data).into_owned());
            }
            // nameTypeURI
            6 => {
                if !is_ia5(data) {
                    return Err(err("x509: SAN uniformResourceIdentifier is malformed"));
                }
                let uri_str = String::from_utf8_lossy(data).into_owned();
                let host = gourl::parse(&uri_str).map_err(|e| {
                    err(format!(
                        "x509: cannot parse URI {}: {}",
                        ctrlrt::error::go_quote(&uri_str),
                        e
                    ))
                })?;
                if !host.is_empty() && !domain_name_valid(host.as_bytes(), false) {
                    return Err(err(format!(
                        "x509: cannot parse URI {}: invalid domain",
                        ctrlrt::error::go_quote(&uri_str)
                    )));
                }
                out.uris.push(uri_str);
            }
            // nameTypeIP
            7 => match data.len() {
                4 | 16 => out.ip_addresses.push(Ip(data.to_vec())),
                n => {
                    return Err(err(format!("x509: cannot parse IP address of length {n}")));
                }
            },
            _ => {}
        }
    }
    Ok(())
}

/// `domainNameValid(s, constraint)`
pub(crate) fn domain_name_valid(s: &[u8], constraint: bool) -> bool {
    if s.is_empty() {
        return true;
    }
    if s[s.len() - 1] == b'.' {
        return false;
    }
    let s = if constraint && s[0] == b'.' {
        &s[1..]
    } else {
        s
    };
    let mut last_dot: isize = -1;
    for i in 0..=s.len() {
        if i < s.len() && (s[i] < 33 || s[i] > 126) {
            return false;
        }
        if i == s.len() || s[i] == b'.' {
            let mut label_len = i as isize;
            if last_dot >= 0 {
                label_len -= last_dot + 1;
            }
            if label_len == 0 {
                return false;
            }
            last_dot = i as isize;
        }
    }
    true
}

// --- Public keys ---------------------------------------------------------------------------

const NULL_BYTES: &[u8] = &[0x05, 0x00];

/// `parsePublicKey` (validation only).
fn parse_public_key(
    oid: &[i64],
    params: &asn1::RawValue,
    key: &BitString,
) -> Result<(), ParseError> {
    let data = key.right_align();
    if oid == OID_PUBLIC_KEY_RSA {
        // RSA public keys must have a NULL in the parameters (RFC 3279, Section 2.3.1).
        if params.full_bytes != NULL_BYTES {
            return Err(err("x509: RSA key missing NULL parameters"));
        }
        let mut der = Reader::new(&data);
        let Some(mut seq) = der.read_asn1(0x30) else {
            return Err(err("x509: invalid RSA public key"));
        };
        let Some(n) = seq.read_asn1_big_int() else {
            return Err(err("x509: invalid RSA modulus"));
        };
        let Some(e) = seq.read_asn1_int64() else {
            return Err(err("x509: invalid RSA public exponent"));
        };
        if n.sign <= 0 {
            return Err(err("x509: RSA modulus is not a positive number"));
        }
        if e <= 0 {
            return Err(err("x509: RSA public exponent is not a positive number"));
        }
        Ok(())
    } else if oid == OID_PUBLIC_KEY_ECDSA {
        let mut params_der = Reader::new(&params.full_bytes);
        let Some(curve_oid) = params_der.read_asn1_object_identifier() else {
            return Err(err("x509: invalid ECDSA parameters"));
        };
        let curve = match curve_oid.as_slice() {
            c if c == OID_NAMED_CURVE_P224 => &super::ec::P224,
            c if c == OID_NAMED_CURVE_P256 => &super::ec::P256,
            c if c == OID_NAMED_CURVE_P384 => &super::ec::P384,
            c if c == OID_NAMED_CURVE_P521 => &super::ec::P521,
            _ => return Err(err("x509: unsupported elliptic curve")),
        };
        // ecdsa.ParseUncompressedPublicKey
        if data.is_empty() || data[0] != 4 {
            return Err(err("ecdsa: invalid uncompressed public key"));
        }
        curve.check_uncompressed_point(&data).map_err(err)
    } else if oid == OID_PUBLIC_KEY_ED25519 {
        // RFC 8410, Section 3: for all of the OIDs, the parameters MUST be absent.
        if !params.full_bytes.is_empty() {
            return Err(err("x509: Ed25519 key encoded with illegal parameters"));
        }
        if data.len() != 32 {
            return Err(err("x509: wrong Ed25519 public key size"));
        }
        Ok(())
    } else if oid == OID_PUBLIC_KEY_DSA {
        let mut der = Reader::new(&data);
        let Some(y) = der.read_asn1_big_int() else {
            return Err(err("x509: invalid DSA public key"));
        };
        let mut params_der = Reader::new(&params.full_bytes);
        let parsed = params_der.read_asn1(0x30).and_then(|mut seq| {
            let p = seq.read_asn1_big_int()?;
            let q = seq.read_asn1_big_int()?;
            let g = seq.read_asn1_big_int()?;
            Some((p, q, g))
        });
        let Some((p, q, g)) = parsed else {
            return Err(err("x509: invalid DSA parameters"));
        };
        if y.sign <= 0 || p.sign <= 0 || q.sign <= 0 || g.sign <= 0 {
            return Err(err("x509: zero or negative DSA parameter"));
        }
        Ok(())
    } else {
        Err(err("x509: unknown public key algorithm"))
    }
}

/// A big integer's sign and magnitude.
pub struct BigInt {
    pub sign: i8,
    pub magnitude: BigUint,
}
