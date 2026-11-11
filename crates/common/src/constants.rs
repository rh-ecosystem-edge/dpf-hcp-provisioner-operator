//! Port of `internal/common/constants.go`.

/// Resource kind name used in logs, metrics, events, etc.
pub const DPFHCP_PROVISIONER_NAME: &str = "dpfhcpprovisioner";
/// Name for the main provisioner controller event recorder.
pub const PROVISIONER_CONTROLLER_NAME: &str = "dpfhcpprovisioner-controller";
/// Name for the CSR approval controller event recorder.
pub const CSR_APPROVAL_CONTROLLER_NAME: &str = "csr-approval-controller";

pub const ANNOTATION_PREFIX: &str = "dpfhcpprovisioner.dpu.hcp.io/";

/// Label key for the DPFHCPProvisioner name.
pub const LABEL_DPFHCP_PROVISIONER_NAME: &str = "dpfhcpprovisioner.dpu.hcp.io/name";
/// Label key for the DPFHCPProvisioner namespace.
pub const LABEL_DPFHCP_PROVISIONER_NAMESPACE: &str = "dpfhcpprovisioner.dpu.hcp.io/namespace";
/// Marks resources created and owned by this operator.
pub const LABEL_MANAGED_BY: &str = "dpfhcpprovisioner.dpu.hcp.io/managed";

/// Namespace for OpenShift operator resources (MetalLB resources etc.).
pub const OPENSHIFT_OPERATORS_NAMESPACE: &str = "openshift-operators";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn label_keys_derive_from_prefix() {
        assert_eq!(
            LABEL_DPFHCP_PROVISIONER_NAME,
            format!("{ANNOTATION_PREFIX}name")
        );
        assert_eq!(
            LABEL_DPFHCP_PROVISIONER_NAMESPACE,
            format!("{ANNOTATION_PREFIX}namespace")
        );
        assert_eq!(LABEL_MANAGED_BY, format!("{ANNOTATION_PREFIX}managed"));
    }
}
