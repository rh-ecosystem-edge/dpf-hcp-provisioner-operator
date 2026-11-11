//! Port of `internal/controller/csrapproval/hostname.go`.

use ctrlrt::error::Result;
use ctrlrt::wrapf;
use k8s_openapi::api::certificates::v1::CertificateSigningRequest;

use crate::x509;

/// SignerBootstrap is the signer name for bootstrap CSRs
pub const SIGNER_BOOTSTRAP: &str = "kubernetes.io/kube-apiserver-client-kubelet";
/// SignerServing is the signer name for serving CSRs
pub const SIGNER_SERVING: &str = "kubernetes.io/kubelet-serving";
/// SystemNodePrefix is the prefix used in CSR CN and username
pub const SYSTEM_NODE_PREFIX: &str = "system:node:";

/// `certv1.CertificateApproved`
pub const CERTIFICATE_APPROVED: &str = "Approved";
/// `certv1.CertificateDenied`
pub const CERTIFICATE_DENIED: &str = "Denied";

/// `strings.TrimPrefix`
fn trim_prefix<'a>(s: &'a str, prefix: &str) -> &'a str {
    s.strip_prefix(prefix).unwrap_or(s)
}

/// ExtractHostname extracts the hostname from a CSR based on its signer type.
/// For bootstrap CSRs: extracts from CN in the certificate request.
/// For serving CSRs: extracts from username field.
///
/// `None` mirrors a nil CSR.
pub fn extract_hostname(csr: Option<&CertificateSigningRequest>) -> Result<String> {
    let Some(csr) = csr else {
        return Err(anyhow::anyhow!("CSR is nil"));
    };

    match csr.spec.signer_name.as_str() {
        SIGNER_BOOTSTRAP => extract_hostname_from_bootstrap_csr(csr),
        SIGNER_SERVING => extract_hostname_from_serving_csr(csr),
        other => Err(anyhow::anyhow!("unsupported signer name: {}", other)),
    }
}

/// extractHostnameFromBootstrapCSR extracts hostname from bootstrap CSR's certificate
/// request CN.
fn extract_hostname_from_bootstrap_csr(csr: &CertificateSigningRequest) -> Result<String> {
    // csr.Spec.Request is already a []byte containing the PEM-encoded certificate request
    let (pem_block, _) = x509::pem::decode(&csr.spec.request.0);
    let Some(pem_block) = pem_block else {
        return Err(anyhow::anyhow!(
            "failed to decode PEM block from CSR request"
        ));
    };

    // Parse X509 certificate request
    let cert_req = x509::parse_certificate_request(&pem_block.bytes).map_err(|err| {
        wrapf!(
            err.clone(),
            "failed to parse X509 certificate request: {}",
            err
        )
    })?;

    // Extract hostname from CN (format: "system:node:<hostname>")
    let cn = cert_req.subject.common_name.as_str();
    let hostname = trim_prefix(cn, SYSTEM_NODE_PREFIX);
    if hostname.is_empty() || hostname == cn {
        return Err(anyhow::anyhow!(
            "invalid CN format, expected 'system:node:<hostname>', got: {}",
            cn
        ));
    }

    Ok(hostname.to_string())
}

/// extractHostnameFromServingCSR extracts hostname from serving CSR's username field.
fn extract_hostname_from_serving_csr(csr: &CertificateSigningRequest) -> Result<String> {
    // Username format: "system:node:<hostname>"
    let username = csr.spec.username.as_deref().unwrap_or("");
    let hostname = trim_prefix(username, SYSTEM_NODE_PREFIX);
    if hostname.is_empty() || hostname == username {
        return Err(anyhow::anyhow!(
            "invalid username format, expected 'system:node:<hostname>', got: {}",
            username
        ));
    }

    Ok(hostname.to_string())
}

/// IsPending checks if a CSR is in pending state (no Approved or Denied condition).
///
/// `None` mirrors a nil CSR.
pub fn is_pending(csr: Option<&CertificateSigningRequest>) -> bool {
    let Some(csr) = csr else {
        return false;
    };
    let conditions = csr
        .status
        .as_ref()
        .and_then(|s| s.conditions.as_deref())
        .unwrap_or(&[]);
    !conditions
        .iter()
        .any(|c| c.type_ == CERTIFICATE_APPROVED || c.type_ == CERTIFICATE_DENIED)
}

/// FilterBySignerName filters CSRs to only include bootstrap and serving CSR types.
pub fn filter_by_signer_name(csrs: &[CertificateSigningRequest]) -> Vec<CertificateSigningRequest> {
    csrs.iter()
        .filter(|csr| {
            csr.spec.signer_name == SIGNER_BOOTSTRAP || csr.spec.signer_name == SIGNER_SERVING
        })
        .cloned()
        .collect()
}
