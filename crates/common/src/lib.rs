//! Port of `internal/common`.

pub mod config;
pub mod constants;
pub mod keychain;
pub mod ocp;
pub mod ownership;
pub mod release;
pub mod version;

pub use config::{
    OperatorConfig, get_dpf_version, get_singleton_dpf_operator_config,
    load_operator_config_from_cr,
};
pub use constants::*;
pub use keychain::{
    DockerConfigKeychain, keychain_from_pull_secret, new_keychain_from_docker_config,
};
pub use ocp::extract_ocp_version;
pub use ownership::is_owned_by_provisioner;
pub use release::{aarch64_release_image, is_multi_arch_release_image};
pub use version::dpf_major_minor_from_version;
