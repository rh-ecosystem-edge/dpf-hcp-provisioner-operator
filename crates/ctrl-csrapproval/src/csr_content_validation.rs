//! Port of `internal/controller/csrapproval/csr_content_validation.go`.

use ctrlrt::error::{Result, go_quote};
use ctrlrt::wrapf;
use k8s_openapi::api::certificates::v1::CertificateSigningRequest;

use crate::hostname::SYSTEM_NODE_PREFIX;
use crate::x509::{self, CertificateRequest, Ip, fmt_ip_list, fmt_string_list};

/// BootstrapperUsername is the expected username for bootstrap CSRs
pub const BOOTSTRAPPER_USERNAME: &str =
    "system:serviceaccount:openshift-machine-config-operator:node-bootstrapper";

// Expected groups for bootstrap CSRs (from service account)
pub(crate) const GROUP_SERVICE_ACCOUNTS: &str = "system:serviceaccounts";
pub(crate) const GROUP_SERVICE_ACCOUNTS_MACHINE_CONFIG: &str =
    "system:serviceaccounts:openshift-machine-config-operator";

// Expected groups for serving CSRs (from authenticated nodes)
pub(crate) const GROUP_NODES: &str = "system:nodes";

// Common group required for both bootstrap and serving CSRs
pub(crate) const GROUP_AUTHENTICATED: &str = "system:authenticated";

// Expected organization field in certificate request (both bootstrap and serving)
const EXPECTED_ORGANIZATION: &str = "system:nodes";

/// `certv1.UsageDigitalSignature`
pub const USAGE_DIGITAL_SIGNATURE: &str = "digital signature";
/// `certv1.UsageClientAuth`
pub const USAGE_CLIENT_AUTH: &str = "client auth";
/// `certv1.UsageServerAuth`
pub const USAGE_SERVER_AUTH: &str = "server auth";

fn groups(csr: &CertificateSigningRequest) -> &[String] {
    csr.spec.groups.as_deref().unwrap_or(&[])
}

fn username(csr: &CertificateSigningRequest) -> &str {
    csr.spec.username.as_deref().unwrap_or("")
}

/// ValidateBootstrapCSR performs comprehensive validation of a bootstrap CSR.
pub fn validate_bootstrap_csr(csr: &CertificateSigningRequest, hostname: &str) -> Result<()> {
    // 1. Validate username and groups (must come from node-bootstrapper)
    validate_bootstrapper_identity(csr)
        .map_err(|err| wrapf!(err, "bootstrapper identity validation failed: {}", err))?;

    // 2. Validate key usages
    validate_bootstrap_usages(csr)
        .map_err(|err| wrapf!(err, "key usage validation failed: {}", err))?;

    // 3. Parse and validate certificate request
    let cert_req =
        parse_csr_request(csr).map_err(|err| wrapf!(err, "failed to parse CSR: {}", err))?;

    // 4. Validate organization field
    validate_organization(&cert_req)
        .map_err(|err| wrapf!(err, "organization validation failed: {}", err))?;

    // 5. Validate CN matches hostname
    validate_cn(&cert_req, hostname).map_err(|err| wrapf!(err, "CN validation failed: {}", err))?;

    // 6. Bootstrap CSRs must not contain any SANs (client-auth certificates)
    if !cert_req.dns_names.is_empty()
        || !cert_req.ip_addresses.is_empty()
        || !cert_req.email_addresses.is_empty()
        || !cert_req.uris.is_empty()
    {
        return Err(anyhow::anyhow!(
            "bootstrap CSR must not contain SANs (DNS={}, IP={}, email={}, URI={})",
            fmt_string_list(&cert_req.dns_names),
            fmt_ip_list(&cert_req.ip_addresses),
            fmt_string_list(&cert_req.email_addresses),
            fmt_string_list(&cert_req.uris)
        ));
    }

    Ok(())
}

/// ValidateServingCSR performs comprehensive validation of a serving CSR.
pub fn validate_serving_csr(csr: &CertificateSigningRequest, hostname: &str) -> Result<()> {
    // 1. Validate username and groups (must come from the node itself)
    validate_node_identity(csr, hostname)
        .map_err(|err| wrapf!(err, "node identity validation failed: {}", err))?;

    // 2. Validate key usages
    validate_serving_usages(csr)
        .map_err(|err| wrapf!(err, "key usage validation failed: {}", err))?;

    // 3. Parse and validate certificate request
    let cert_req =
        parse_csr_request(csr).map_err(|err| wrapf!(err, "failed to parse CSR: {}", err))?;

    // 4. Validate organization field
    validate_organization(&cert_req)
        .map_err(|err| wrapf!(err, "organization validation failed: {}", err))?;

    // 5. Validate CN matches hostname
    validate_cn(&cert_req, hostname).map_err(|err| wrapf!(err, "CN validation failed: {}", err))?;

    // 6. Validate DNS names and IP addresses
    validate_serving_identities(&cert_req, hostname)
        .map_err(|err| wrapf!(err, "serving identities validation failed: {}", err))?;

    Ok(())
}

/// validateBootstrapperIdentity validates that bootstrap CSR comes from node-bootstrapper
/// service account.
///
/// Go iterates a map to find a missing group, so with several missing groups the reported
/// one is random; here the first missing group in declaration order is reported.
fn validate_bootstrapper_identity(csr: &CertificateSigningRequest) -> Result<()> {
    // Check username
    if username(csr) != BOOTSTRAPPER_USERNAME {
        return Err(anyhow::anyhow!(
            "invalid username: expected {}, got {}",
            BOOTSTRAPPER_USERNAME,
            username(csr)
        ));
    }

    // Check groups
    let required_groups = [
        GROUP_SERVICE_ACCOUNTS,
        GROUP_SERVICE_ACCOUNTS_MACHINE_CONFIG,
        GROUP_AUTHENTICATED,
    ];
    for group in required_groups {
        if !groups(csr).iter().any(|g| g == group) {
            return Err(anyhow::anyhow!("missing required group: {}", group));
        }
    }

    Ok(())
}

/// validateNodeIdentity validates that serving CSR comes from the node itself.
fn validate_node_identity(csr: &CertificateSigningRequest, hostname: &str) -> Result<()> {
    // Check username matches expected format
    let expected_username = format!("{SYSTEM_NODE_PREFIX}{hostname}");
    if username(csr) != expected_username {
        return Err(anyhow::anyhow!(
            "invalid username: expected {}, got {}",
            expected_username,
            username(csr)
        ));
    }

    // Check groups include system:nodes and system:authenticated
    let has_nodes_group = groups(csr).iter().any(|g| g == GROUP_NODES);
    let has_authenticated_group = groups(csr).iter().any(|g| g == GROUP_AUTHENTICATED);

    if !has_nodes_group {
        return Err(anyhow::anyhow!("missing required group: {}", GROUP_NODES));
    }
    if !has_authenticated_group {
        return Err(anyhow::anyhow!(
            "missing required group: {}",
            GROUP_AUTHENTICATED
        ));
    }

    Ok(())
}

/// validateUsages validates key usages against an allowed set.
/// Requires exact match - no extra usages allowed.
///
/// With several missing usages Go reports a random one (map iteration); here the first
/// missing usage in declaration order is reported.
fn validate_usages(csr: &CertificateSigningRequest, allowed_usages: &[&str]) -> Result<()> {
    let usages = csr.spec.usages.as_deref().unwrap_or(&[]);
    // Check each usage in CSR
    for usage in usages {
        if !allowed_usages.contains(&usage.as_str()) {
            // Disallowed usage present
            return Err(anyhow::anyhow!("disallowed usage present: {}", usage));
        }
    }

    // Verify all required usages are present
    for usage in allowed_usages {
        if !usages.iter().any(|u| u == usage) {
            return Err(anyhow::anyhow!("missing required usage: {}", usage));
        }
    }

    Ok(())
}

/// validateBootstrapUsages validates key usages for bootstrap CSRs
/// (digital signature + client auth, exact match).
fn validate_bootstrap_usages(csr: &CertificateSigningRequest) -> Result<()> {
    validate_usages(csr, &[USAGE_DIGITAL_SIGNATURE, USAGE_CLIENT_AUTH])
}

/// validateServingUsages validates key usages for serving CSRs
/// (digital signature + server auth, exact match).
fn validate_serving_usages(csr: &CertificateSigningRequest) -> Result<()> {
    validate_usages(csr, &[USAGE_DIGITAL_SIGNATURE, USAGE_SERVER_AUTH])
}

/// parseCSRRequest parses the X509 certificate request from a CSR.
fn parse_csr_request(csr: &CertificateSigningRequest) -> Result<CertificateRequest> {
    // csr.Spec.Request already holds the PEM-encoded certificate request bytes (the JSON
    // base64 layer is decoded by the client).
    let (pem_block, _) = x509::pem::decode(&csr.spec.request.0);
    let Some(pem_block) = pem_block else {
        return Err(anyhow::anyhow!(
            "failed to decode PEM block from CSR request"
        ));
    };

    if pem_block.block_type != x509::CERTIFICATE_REQUEST_BLOCK_TYPE {
        return Err(anyhow::anyhow!(
            "unexpected PEM block type {}, expected {}",
            go_quote(&pem_block.block_type),
            x509::CERTIFICATE_REQUEST_BLOCK_TYPE
        ));
    }

    // Parse X509 certificate request
    x509::parse_certificate_request(&pem_block.bytes)
        .map_err(|err| wrapf!(err.clone(), "failed to parse X509 certificate request: {}", err))
}

/// validateOrganization validates that the CSR has the correct organization field.
fn validate_organization(cert_req: &CertificateRequest) -> Result<()> {
    let org = &cert_req.subject.organization;
    if org.len() != 1 {
        return Err(anyhow::anyhow!(
            "expected exactly one organization, got {}",
            org.len()
        ));
    }
    if org[0] != EXPECTED_ORGANIZATION {
        return Err(anyhow::anyhow!(
            "invalid organization: expected {}, got {}",
            EXPECTED_ORGANIZATION,
            org[0]
        ));
    }
    Ok(())
}

/// validateCN validates that the CN matches the expected hostname format.
fn validate_cn(cert_req: &CertificateRequest, hostname: &str) -> Result<()> {
    let expected_cn = format!("{SYSTEM_NODE_PREFIX}{hostname}");
    if cert_req.subject.common_name != expected_cn {
        return Err(anyhow::anyhow!(
            "invalid CN: expected {}, got {}",
            expected_cn,
            cert_req.subject.common_name
        ));
    }
    Ok(())
}

/// validateServingIdentities validates DNS names and IP addresses in serving CSR.
/// Serving CSRs must carry exactly one DNS SAN equal to the node hostname.
fn validate_serving_identities(cert_req: &CertificateRequest, hostname: &str) -> Result<()> {
    // Serving CSRs must contain exactly one DNS SAN
    if cert_req.dns_names.len() != 1 {
        return Err(anyhow::anyhow!(
            "CSR must contain exactly one DNS SAN, got {}: {}",
            cert_req.dns_names.len(),
            fmt_string_list(&cert_req.dns_names)
        ));
    }

    // The DNS SAN must match the hostname
    if cert_req.dns_names[0] != hostname {
        return Err(anyhow::anyhow!(
            "CSR DNS SAN {} does not match hostname {}",
            go_quote(&cert_req.dns_names[0]),
            go_quote(hostname)
        ));
    }

    // Validate that IP addresses are valid (if any)
    for ip in &cert_req.ip_addresses {
        if !has_valid_ip_address(Some(ip)) {
            return Err(anyhow::anyhow!("CSR contains invalid IP address: {}", ip));
        }
    }

    Ok(())
}

/// hasValidIPAddress checks if an IP address is valid for a node.
fn has_valid_ip_address(ip: Option<&Ip>) -> bool {
    let Some(ip) = ip else {
        return false;
    };
    // Reject unspecified, loopback, and multicast addresses
    !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast()
}
