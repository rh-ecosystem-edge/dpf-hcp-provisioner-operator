//! Port of `internal/common/ownership.go`.

use std::collections::BTreeMap;

use crate::constants::{LABEL_DPFHCP_PROVISIONER_NAME, LABEL_DPFHCP_PROVISIONER_NAMESPACE};

/// `IsOwnedByProvisioner` checks the provisioner name/namespace tracking labels.
pub fn is_owned_by_provisioner(
    labels: Option<&BTreeMap<String, String>>,
    provisioner_name: &str,
    provisioner_namespace: &str,
) -> bool {
    let Some(labels) = labels.filter(|l| !l.is_empty()) else {
        return false;
    };
    let get = |k: &str| labels.get(k).map(String::as_str).unwrap_or("");
    get(LABEL_DPFHCP_PROVISIONER_NAME) == provisioner_name
        && get(LABEL_DPFHCP_PROVISIONER_NAMESPACE) == provisioner_namespace
}
