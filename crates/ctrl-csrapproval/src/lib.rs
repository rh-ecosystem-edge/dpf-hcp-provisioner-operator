//! Port of `internal/controller/csrapproval`: the dedicated CSR auto-approval controller.
//!
//! Go's `crypto/x509` / `encoding/pem` behaviour used to inspect CSRs is ported in the
//! [`x509`] module (pure Rust).

pub mod controller;
pub mod csr_content_validation;
pub mod csr_owner_validation;
pub mod csrapproval;
pub mod hostname;
pub mod x509;

pub use controller::{CONTROLLER_NAME, CSRApprovalReconciler};
pub use csr_content_validation::{BOOTSTRAPPER_USERNAME, validate_bootstrap_csr, validate_serving_csr};
pub use csr_owner_validation::{ValidationResult, Validator};
pub use csrapproval::{CSR_POLLING_INTERVAL, CSRApprover};
pub use hostname::{
    SIGNER_BOOTSTRAP, SIGNER_SERVING, SYSTEM_NODE_PREFIX, extract_hostname, filter_by_signer_name,
    is_pending,
};

#[cfg(test)]
mod test_util;

#[cfg(test)]
mod hostname_test;

#[cfg(test)]
mod csr_content_validation_test;

#[cfg(test)]
mod csr_owner_validation_test;

#[cfg(test)]
mod setcondition_test;

#[cfg(test)]
mod csrapproval_test;

#[cfg(test)]
mod x509_golden_test;
