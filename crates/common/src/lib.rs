//! Port of `internal/common`.

pub mod config;
pub mod constants;
pub mod ocp;
pub mod ownership;
pub mod version;

pub use constants::*;
pub use ocp::extract_ocp_version;
pub use ownership::is_owned_by_provisioner;
pub use version::dpf_major_minor_from_version;
pub use config::{OperatorConfig, get_dpf_version, get_singleton_dpf_operator_config, load_operator_config_from_cr};
