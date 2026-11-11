//! Golden differential test against the Go implementation: `tests/fixtures/csr_corpus.json`
//! is produced by `go run -mod=vendor ./hack/golden/csrapproval crates/ctrl-csrapproval/tests/fixtures`
//! and holds CSR request bytes plus Go's `pem.Decode`, `x509.ParseCertificateRequest`,
//! `ExtractHostname`, `ValidateBootstrapCSR` and `ValidateServingCSR` results.
//!
//! Error messages must match exactly, with two normalisations:
//! * `encoding/asn1` prints the `tag *int` pointer of field parameters as an address;
//!   addresses are replaced by a placeholder on both sides.
//! * Go's `time.Parse` / round-trip messages for invalid UTCTime/GeneralizedTime subject
//!   values are not reproduced; for those only the failure (and the message prefix) must
//!   match.

use base64::Engine;
use k8s_openapi::ByteString;
use k8s_openapi::api::certificates::v1::{CertificateSigningRequest, CertificateSigningRequestSpec};
use serde::Deserialize;

use crate::csr_content_validation::*;
use crate::hostname::*;
use crate::x509;

const HOSTNAME: &str = "dpu-worker-01";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    request: Option<String>,
    pem_found: bool,
    pem_type: String,
    parse_error: String,
    cn: String,
    org: Vec<String>,
    dns: Vec<String>,
    emails: Vec<String>,
    ips: Vec<String>,
    uris: Vec<String>,
    extract_hostname: String,
    extract_error: String,
    bootstrap_error: String,
    serving_error: String,
}

/// Replace `tag:0x<hex>` addresses with a placeholder.
fn normalize(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(i) = rest.find("tag:0x") {
        out.push_str(&rest[..i]);
        out.push_str("tag:PTR");
        let tail = &rest[i + "tag:0x".len()..];
        let end = tail
            .find(|c: char| !c.is_ascii_hexdigit())
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn assert_msg(case: &str, what: &str, go: &str, rust: &str) {
    let time_error = go.contains("parsing time ") || go.contains("did not serialize back");
    if time_error {
        assert!(!rust.is_empty(), "{case}: {what}: Rust accepted, Go: {go}");
        let prefix = go.split("parsing time ").next().unwrap_or("");
        let prefix = prefix.split("asn1: time did not").next().unwrap_or("");
        assert!(
            rust.starts_with(prefix),
            "{case}: {what}: prefix mismatch\n  go:   {go}\n  rust: {rust}"
        );
        return;
    }
    assert_eq!(
        normalize(rust),
        normalize(go),
        "{case}: {what} mismatch"
    );
}

fn err_string<T>(r: ctrlrt::Result<T>) -> String {
    match r {
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    }
}

fn csr(signer: &str, username: &str, groups: &[&str], usages: &[&str], request: &[u8]) -> CertificateSigningRequest {
    CertificateSigningRequest {
        spec: CertificateSigningRequestSpec {
            signer_name: signer.into(),
            username: Some(username.into()),
            groups: Some(groups.iter().map(|s| s.to_string()).collect()),
            usages: Some(usages.iter().map(|s| s.to_string()).collect()),
            request: ByteString(request.to_vec()),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn csr_corpus_matches_go() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../tests/fixtures/csr_corpus.json")).unwrap();
    assert!(cases.len() > 100);

    for c in &cases {
        let request = base64::engine::general_purpose::STANDARD
            .decode(c.request.as_deref().unwrap_or(""))
            .unwrap();

        // encoding/pem.Decode
        let (block, _) = x509::pem::decode(&request);
        assert_eq!(block.is_some(), c.pem_found, "{}: pem found", c.name);
        if let Some(block) = block {
            assert_eq!(block.block_type, c.pem_type, "{}: pem type", c.name);

            // x509.ParseCertificateRequest
            match x509::parse_certificate_request(&block.bytes) {
                Err(e) => assert_msg(&c.name, "parse", &c.parse_error, &e.to_string()),
                Ok(cr) => {
                    assert_eq!(c.parse_error, "", "{}: Rust parsed, Go failed", c.name);
                    assert_eq!(cr.subject.common_name, c.cn, "{}: CN", c.name);
                    assert_eq!(cr.subject.organization, c.org, "{}: O", c.name);
                    assert_eq!(cr.dns_names, c.dns, "{}: DNS", c.name);
                    assert_eq!(cr.email_addresses, c.emails, "{}: emails", c.name);
                    let ips: Vec<String> = cr.ip_addresses.iter().map(|i| i.to_string()).collect();
                    assert_eq!(ips, c.ips, "{}: IPs", c.name);
                    assert_eq!(cr.uris, c.uris, "{}: URIs", c.name);
                }
            }
        }

        let bootstrap = csr(
            SIGNER_BOOTSTRAP,
            BOOTSTRAPPER_USERNAME,
            &[
                "system:serviceaccounts",
                "system:serviceaccounts:openshift-machine-config-operator",
                "system:authenticated",
            ],
            &[USAGE_DIGITAL_SIGNATURE, USAGE_CLIENT_AUTH],
            &request,
        );
        match extract_hostname(Some(&bootstrap)) {
            Ok(h) => {
                assert_eq!(c.extract_error, "", "{}: Rust extracted, Go failed", c.name);
                assert_eq!(h, c.extract_hostname, "{}: extracted hostname", c.name);
            }
            Err(e) => assert_msg(&c.name, "extract", &c.extract_error, &e.to_string()),
        }
        assert_msg(
            &c.name,
            "bootstrap validation",
            &c.bootstrap_error,
            &err_string(validate_bootstrap_csr(&bootstrap, HOSTNAME)),
        );

        let serving = csr(
            SIGNER_SERVING,
            &format!("system:node:{HOSTNAME}"),
            &["system:nodes", "system:authenticated"],
            &[USAGE_DIGITAL_SIGNATURE, USAGE_SERVER_AUTH],
            &request,
        );
        assert_msg(
            &c.name,
            "serving validation",
            &c.serving_error,
            &err_string(validate_serving_csr(&serving, HOSTNAME)),
        );
    }
}
