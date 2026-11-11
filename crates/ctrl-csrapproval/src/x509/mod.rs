//! Pure-Rust ports of the Go standard library pieces the CSR approver depends on:
//! `encoding/pem.Decode`, `crypto/x509.ParseCertificateRequest` (with the `encoding/asn1`,
//! `cryptobyte`, NIST point validation and `net/url.Parse` behaviour it relies on).

pub mod asn1;
pub mod certreq;
pub mod cryptobyte;
pub mod ec;
pub mod gourl;
pub mod pem;

pub use certreq::{
    CertificateRequest, Ip, Name, ParseError, fmt_ip_list, fmt_string_list,
    parse_certificate_request,
};

/// `k8s.io/client-go/util/cert.CertificateRequestBlockType`
pub const CERTIFICATE_REQUEST_BLOCK_TYPE: &str = "CERTIFICATE REQUEST";
