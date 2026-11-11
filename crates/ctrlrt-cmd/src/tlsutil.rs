//! Go `crypto/tls.X509KeyPair` and `encoding/pem.Decode` equivalents producing a rustls
//! [`CertifiedKey`], plus the Go-style rendering of file errors (`open <path>: no such file
//! or directory`).

use std::sync::Arc;

use base64::Engine;
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
};
use rustls::sign::CertifiedKey;

/// One PEM block (`pem.Block`, headers ignored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PemBlock {
    pub type_: String,
    pub bytes: Vec<u8>,
}

/// `pem.Decode` loop: all decodable blocks in order (undecodable ones are skipped like Go).
pub fn pem_decode_all(data: &[u8]) -> Vec<PemBlock> {
    let text = String::from_utf8_lossy(data);
    let mut out = Vec::new();
    let mut rest: &str = &text;
    loop {
        // Find "-----BEGIN " at the start of data or of a line.
        let idx = if rest.starts_with("-----BEGIN ") {
            Some(0)
        } else {
            rest.find("\n-----BEGIN ").map(|i| i + 1)
        };
        let Some(idx) = idx else { break };
        let after = &rest[idx + "-----BEGIN ".len()..];
        let Some(eol) = after.find('\n').or(Some(after.len())) else { break };
        let line = after[..eol].trim_end_matches('\r').trim_end();
        let Some(type_) = line.strip_suffix("-----") else {
            rest = after;
            continue;
        };
        let type_ = type_.to_string();
        let body_start = &after[(eol + 1).min(after.len())..];
        let end_marker = format!("-----END {type_}-----");
        let Some(end) = body_start.find(&end_marker) else {
            rest = after;
            continue;
        };
        // Body: skip optional "Key: value" headers (until an empty line), then base64.
        let body = &body_start[..end];
        let lines: Vec<&str> = body.lines().collect();
        let mut b64 = String::new();
        let has_headers = lines.first().is_some_and(|l| l.contains(':'));
        let mut in_headers = has_headers;
        for l in lines {
            if in_headers {
                if l.trim().is_empty() {
                    in_headers = false;
                }
                continue;
            }
            b64.extend(l.chars().filter(|c| !c.is_whitespace()));
        }
        let next = &body_start[end + end_marker.len()..];
        match base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
            Ok(bytes) => out.push(PemBlock { type_, bytes }),
            Err(_) => {
                rest = after;
                continue;
            }
        }
        rest = next;
    }
    out
}

/// Minimal DER reader.
struct Der<'a> {
    data: &'a [u8],
}

impl<'a> Der<'a> {
    /// Read one TLV, returning (tag, full TLV bytes, content).
    fn read(&mut self) -> Option<(u8, &'a [u8], &'a [u8])> {
        let d = self.data;
        if d.len() < 2 {
            return None;
        }
        let tag = d[0];
        let (len, hdr) = if d[1] & 0x80 == 0 {
            (d[1] as usize, 2)
        } else {
            let n = (d[1] & 0x7f) as usize;
            if n == 0 || n > 4 || d.len() < 2 + n {
                return None;
            }
            let mut l = 0usize;
            for b in &d[2..2 + n] {
                l = (l << 8) | *b as usize;
            }
            (l, 2 + n)
        };
        if d.len() < hdr + len {
            return None;
        }
        let full = &d[..hdr + len];
        let content = &d[hdr..hdr + len];
        self.data = &d[hdr + len..];
        Some((tag, full, content))
    }
}

/// SubjectPublicKeyInfo of a certificate: (algorithm OID content, full SPKI TLV).
fn cert_spki(der: &[u8]) -> Option<(&[u8], &[u8])> {
    let (tag, _, cert) = Der { data: der }.read()?;
    if tag != 0x30 {
        return None;
    }
    let (tag, _, tbs) = Der { data: cert }.read()?;
    if tag != 0x30 {
        return None;
    }
    let mut r = Der { data: tbs };
    let (mut tag, mut full, mut content) = r.read()?;
    if tag == 0xa0 {
        // [0] version
        (tag, full, content) = r.read()?;
    }
    let _ = (full, content);
    if tag != 0x02 {
        return None; // serialNumber
    }
    for _ in 0..4 {
        // signature, issuer, validity, subject
        let (t, _, _) = r.read()?;
        if t != 0x30 {
            return None;
        }
    }
    let (t, spki_full, spki) = r.read()?;
    if t != 0x30 {
        return None;
    }
    let oid = spki_oid(spki)?;
    Some((oid, spki_full))
}

fn spki_oid(spki_content: &[u8]) -> Option<&[u8]> {
    let (t, _, alg) = Der { data: spki_content }.read()?;
    if t != 0x30 {
        return None;
    }
    let (t, _, oid) = Der { data: alg }.read()?;
    (t == 0x06).then_some(oid)
}

fn spki_parts(spki_full: &[u8]) -> Option<(&[u8], &[u8])> {
    let (_, _, content) = Der { data: spki_full }.read()?;
    let mut r = Der { data: content };
    let (_, _, alg) = r.read()?;
    let (t, _, oid) = Der { data: alg }.read()?;
    if t != 0x06 {
        return None;
    }
    let (_, _, bits) = r.read()?;
    Some((oid, bits))
}

const OID_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_EC: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_ED25519: &[u8] = &[0x2b, 0x65, 0x70];

/// `tls.X509KeyPair(certPEMBlock, keyPEMBlock)` returning a rustls certified key. Error
/// messages are Go's.
pub fn x509_key_pair(cert_pem: &[u8], key_pem: &[u8]) -> Result<Arc<CertifiedKey>, String> {
    let mut certs: Vec<CertificateDer<'static>> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for b in pem_decode_all(cert_pem) {
        if b.type_ == "CERTIFICATE" {
            certs.push(CertificateDer::from(b.bytes));
        } else {
            skipped.push(b.type_);
        }
    }
    if certs.is_empty() {
        if skipped.is_empty() {
            return Err("tls: failed to find any PEM data in certificate input".into());
        }
        if skipped.len() == 1 && skipped[0].ends_with("PRIVATE KEY") {
            return Err("tls: failed to find certificate PEM data in certificate input, but did find a private key; PEM inputs may have been switched".into());
        }
        return Err(format!(
            "tls: failed to find \"CERTIFICATE\" PEM block in certificate input after skipping PEM blocks of the following types: [{}]",
            skipped.join(" ")
        ));
    }

    let mut skipped: Vec<String> = Vec::new();
    let mut key_block = None;
    for b in pem_decode_all(key_pem) {
        if b.type_ == "PRIVATE KEY" || b.type_.ends_with(" PRIVATE KEY") {
            key_block = Some(b);
            break;
        }
        skipped.push(b.type_);
    }
    let Some(key_block) = key_block else {
        if skipped.is_empty() {
            return Err("tls: failed to find any PEM data in key input".into());
        }
        if skipped.len() == 1 && skipped[0] == "CERTIFICATE" {
            return Err("tls: found a certificate rather than a key in the PEM for the private key".into());
        }
        return Err(format!(
            "tls: failed to find PEM block with type ending in \"PRIVATE KEY\" in key input after skipping PEM blocks of the following types: [{}]",
            skipped.join(" ")
        ));
    };

    let Some((cert_oid, cert_spki_full)) = cert_spki(&certs[0]) else {
        return Err("x509: malformed certificate".into());
    };

    let provider = rustls::crypto::ring::default_provider();
    let der = key_block.bytes;
    let candidates = [
        PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(der.clone())),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der.clone())),
        PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(der)),
    ];
    let mut signing = None;
    for k in candidates {
        if let Ok(s) = provider.key_provider.load_private_key(k) {
            signing = Some(s);
            break;
        }
    }
    let Some(signing) = signing else {
        return Err("tls: failed to parse private key".into());
    };

    if cert_oid != OID_RSA && cert_oid != OID_EC && cert_oid != OID_ED25519 {
        return Err("tls: unknown public key algorithm".into());
    }
    match signing.public_key() {
        Some(pk) => match (spki_parts(cert_spki_full), spki_parts(pk.as_ref())) {
            (Some((c_oid, c_bits)), Some((k_oid, k_bits))) => {
                if c_oid != k_oid {
                    return Err("tls: private key type does not match public key type".into());
                }
                if c_bits != k_bits {
                    return Err("tls: private key does not match public key".into());
                }
            }
            _ => return Err("tls: private key does not match public key".into()),
        },
        None => return Err("tls: private key type does not match public key type".into()),
    }
    Ok(Arc::new(CertifiedKey::new(certs, signing)))
}

/// Go's `syscall.Errno.Error()` text for an I/O error.
pub fn go_errno_text(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(1) => "operation not permitted".into(),
        Some(2) => "no such file or directory".into(),
        Some(5) => "input/output error".into(),
        Some(13) => "permission denied".into(),
        Some(17) => "file exists".into(),
        Some(20) => "not a directory".into(),
        Some(21) => "is a directory".into(),
        Some(24) => "too many open files".into(),
        Some(28) => "no space left on device".into(),
        Some(36) => "file name too long".into(),
        Some(40) => "too many levels of symbolic links".into(),
        Some(98) => "address already in use".into(),
        Some(99) => "cannot assign requested address".into(),
        Some(104) => "connection reset by peer".into(),
        Some(111) => "connection refused".into(),
        _ => {
            let s = e.to_string();
            let s = match s.find(" (os error ") {
                Some(i) => s[..i].to_string(),
                None => s,
            };
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
                None => s,
            }
        }
    }
}

/// `os.ReadFile(path)` with Go's `*PathError` message (`open <path>: <errno>` or
/// `read <path>: <errno>`).
pub fn read_file(path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("open {path}: {}", go_errno_text(&e)))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)
        .map_err(|e| format!("read {path}: {}", go_errno_text(&e)))?;
    Ok(buf)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn gen_pair(alg: &'static rcgen::SignatureAlgorithm) -> (String, String) {
        let key = rcgen::KeyPair::generate_for(alg).unwrap();
        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let cert = params.self_signed(&key).unwrap();
        (cert.pem(), key.serialize_pem())
    }

    #[test]
    fn key_pair_ok_and_errors() {
        let (c, k) = gen_pair(&rcgen::PKCS_ECDSA_P256_SHA256);
        assert!(x509_key_pair(c.as_bytes(), k.as_bytes()).is_ok());
        let (c2, k2) = gen_pair(&rcgen::PKCS_ED25519);
        assert!(x509_key_pair(c2.as_bytes(), k2.as_bytes()).is_ok());
        let (_c3, k3) = gen_pair(&rcgen::PKCS_ECDSA_P256_SHA256);

        let err = |c: &str, k: &str| x509_key_pair(c.as_bytes(), k.as_bytes()).unwrap_err();
        assert_eq!(err("", &k), "tls: failed to find any PEM data in certificate input");
        assert_eq!(
            err(&k, &k),
            "tls: failed to find certificate PEM data in certificate input, but did find a private key; PEM inputs may have been switched"
        );
        assert_eq!(
            err("-----BEGIN FOO-----\nAAAA\n-----END FOO-----\n-----BEGIN BAR-----\nAAAA\n-----END BAR-----\n", &k),
            "tls: failed to find \"CERTIFICATE\" PEM block in certificate input after skipping PEM blocks of the following types: [FOO BAR]"
        );
        assert_eq!(err(&c, "garbage"), "tls: failed to find any PEM data in key input");
        assert_eq!(err(&c, &c), "tls: found a certificate rather than a key in the PEM for the private key");
        assert_eq!(err(&c, &k3), "tls: private key does not match public key");
        assert_eq!(err(&c, &k2), "tls: private key type does not match public key type");
        assert_eq!(
            err(&c, "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"),
            "tls: failed to parse private key"
        );
        assert_eq!(
            err("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n", &k),
            "x509: malformed certificate"
        );
    }

    #[test]
    fn read_file_errors_like_go() {
        assert_eq!(
            read_file("/nonexistent/x/tls.crt").unwrap_err(),
            "open /nonexistent/x/tls.crt: no such file or directory"
        );
        assert_eq!(read_file("/").unwrap_err(), "read /: is a directory");
    }
}
