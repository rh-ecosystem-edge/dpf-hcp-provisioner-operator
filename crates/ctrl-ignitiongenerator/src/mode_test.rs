//! Port of internal/controller/ignitiongenerator/mode_test.go

use apis::dpf::operator::v1alpha1 as operatorv1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::mode::{dpu_mode_from_flavor, is_zero_trust_mode};

fn flavor_with_mode(mode: &str) -> dpuprovisioningv1alpha1::DPUFlavor {
    let mut f = dpuprovisioningv1alpha1::DPUFlavor::default();
    f.spec.dpu_mode = mode.into();
    f
}

// ---------------------------------------------------------------------------------------------
// TestDPUModeFromFlavor
// ---------------------------------------------------------------------------------------------

struct ModeCase {
    dpu_flavor: Option<dpuprovisioningv1alpha1::DPUFlavor>,
    expected_mode: &'static str,
    expected_error: bool,
}

fn run_dpu_mode_from_flavor(tt: ModeCase) {
    let res = dpu_mode_from_flavor(tt.dpu_flavor.as_ref());
    if tt.expected_error {
        assert!(res.is_err(), "expected error");
        return;
    }
    let mode = res.unwrap_or_else(|e| panic!("unexpected error: {e}"));
    assert_eq!(
        mode, tt.expected_mode,
        "expected mode {}, got {}",
        tt.expected_mode, mode
    );
}

#[test]
fn test_dpu_mode_from_flavor_zero_trust_mode() {
    run_dpu_mode_from_flavor(ModeCase {
        dpu_flavor: Some(flavor_with_mode(dpuprovisioningv1alpha1::ZERO_TRUST_MODE)),
        expected_mode: dpuprovisioningv1alpha1::ZERO_TRUST_MODE,
        expected_error: false,
    });
}

#[test]
fn test_dpu_mode_from_flavor_dpu_mode() {
    run_dpu_mode_from_flavor(ModeCase {
        dpu_flavor: Some(flavor_with_mode(dpuprovisioningv1alpha1::DPU_MODE)),
        expected_mode: dpuprovisioningv1alpha1::DPU_MODE,
        expected_error: false,
    });
}

#[test]
fn test_dpu_mode_from_flavor_empty_mode_returns_error() {
    run_dpu_mode_from_flavor(ModeCase {
        dpu_flavor: Some(flavor_with_mode("")),
        expected_mode: "",
        expected_error: true,
    });
}

#[test]
fn test_dpu_mode_from_flavor_nic_mode_returns_error() {
    run_dpu_mode_from_flavor(ModeCase {
        dpu_flavor: Some(flavor_with_mode(dpuprovisioningv1alpha1::NIC_MODE)),
        expected_mode: "",
        expected_error: true,
    });
}

#[test]
fn test_dpu_mode_from_flavor_invalid_mode_returns_error() {
    run_dpu_mode_from_flavor(ModeCase {
        dpu_flavor: Some(flavor_with_mode("invalid-mode")),
        expected_mode: "",
        expected_error: true,
    });
}

#[test]
fn test_dpu_mode_from_flavor_nil_dpu_flavor_returns_error() {
    run_dpu_mode_from_flavor(ModeCase {
        dpu_flavor: None,
        expected_mode: "",
        expected_error: true,
    });
}

// ---------------------------------------------------------------------------------------------
// TestIsZeroTrustModeResolution
// ---------------------------------------------------------------------------------------------

fn operator_config(
    spec: operatorv1alpha1::DPFOperatorConfigSpec,
) -> operatorv1alpha1::DPFOperatorConfig {
    operatorv1alpha1::DPFOperatorConfig {
        spec,
        ..Default::default()
    }
}

fn host_trusted_operator() -> operatorv1alpha1::DPFOperatorConfig {
    operator_config(operatorv1alpha1::DPFOperatorConfigSpec {
        deployment_mode: operatorv1alpha1::DEPLOYMENT_MODE_HOST_TRUSTED.into(),
        ..Default::default()
    })
}

fn zt_operator() -> operatorv1alpha1::DPFOperatorConfig {
    operator_config(operatorv1alpha1::DPFOperatorConfigSpec {
        deployment_mode: operatorv1alpha1::DEPLOYMENT_MODE_ZERO_TRUST.into(),
        ..Default::default()
    })
}

fn legacy_operator(
    install_interface: operatorv1alpha1::ProvisioningInstallInterface,
) -> operatorv1alpha1::DPFOperatorConfig {
    operator_config(operatorv1alpha1::DPFOperatorConfigSpec {
        provisioning_controller: Some(operatorv1alpha1::ProvisioningControllerConfiguration {
            install_interface: Some(install_interface),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn legacy_redfish() -> operatorv1alpha1::DPFOperatorConfig {
    legacy_operator(operatorv1alpha1::ProvisioningInstallInterface {
        install_via_redfish: Some(Default::default()),
        ..Default::default()
    })
}

fn legacy_host_agent() -> operatorv1alpha1::DPFOperatorConfig {
    legacy_operator(operatorv1alpha1::ProvisioningInstallInterface {
        install_via_host_agent: Some(Default::default()),
        ..Default::default()
    })
}

fn named_flavor(name: &str, mode: &str) -> dpuprovisioningv1alpha1::DPUFlavor {
    let mut f = flavor_with_mode(mode);
    f.metadata = ObjectMeta {
        name: Some(name.into()),
        ..Default::default()
    };
    f
}

fn zt_flavor() -> dpuprovisioningv1alpha1::DPUFlavor {
    named_flavor("zt", dpuprovisioningv1alpha1::ZERO_TRUST_MODE)
}

fn dpu_flavor() -> dpuprovisioningv1alpha1::DPUFlavor {
    named_flavor("dpu", dpuprovisioningv1alpha1::DPU_MODE)
}

fn empty_flavor() -> dpuprovisioningv1alpha1::DPUFlavor {
    named_flavor("empty", "")
}

fn run_is_zero_trust(
    cfg: operatorv1alpha1::DPFOperatorConfig,
    flavor: dpuprovisioningv1alpha1::DPUFlavor,
    want_zt: bool,
    want_mode: &str,
    expect_error: bool,
) {
    let res = is_zero_trust_mode(Some(&cfg), Some(&flavor));
    if expect_error {
        assert!(res.is_err(), "expected error");
        return;
    }
    let (got_zt, got_mode) = res.unwrap_or_else(|e| panic!("unexpected error: {e}"));
    assert_eq!(got_zt, want_zt, "zeroTrust: want {want_zt}, got {got_zt}");
    assert_eq!(
        got_mode, want_mode,
        "mode: want {want_mode}, got {got_mode}"
    );
}

#[test]
fn test_is_zero_trust_mode_resolution_flavor_dpu_mode_wins_over_host_trusted_deployment_mode() {
    run_is_zero_trust(
        host_trusted_operator(),
        zt_flavor(),
        true,
        dpuprovisioningv1alpha1::ZERO_TRUST_MODE,
        false,
    );
}

#[test]
fn test_is_zero_trust_mode_resolution_26_8_deployment_mode_when_flavor_dpu_mode_unset() {
    run_is_zero_trust(
        zt_operator(),
        empty_flavor(),
        true,
        dpuprovisioningv1alpha1::ZERO_TRUST_MODE,
        false,
    );
}

#[test]
fn test_is_zero_trust_mode_resolution_legacy_redfish_when_flavor_dpu_mode_unset() {
    run_is_zero_trust(
        legacy_redfish(),
        empty_flavor(),
        true,
        dpuprovisioningv1alpha1::ZERO_TRUST_MODE,
        false,
    );
}

#[test]
fn test_is_zero_trust_mode_resolution_26_4_flavor_zero_trust_when_deployment_mode_unset() {
    run_is_zero_trust(
        legacy_host_agent(),
        zt_flavor(),
        true,
        dpuprovisioningv1alpha1::ZERO_TRUST_MODE,
        false,
    );
}

#[test]
fn test_is_zero_trust_mode_resolution_26_4_flavor_dpu_when_deployment_mode_unset() {
    run_is_zero_trust(
        legacy_host_agent(),
        dpu_flavor(),
        false,
        dpuprovisioningv1alpha1::DPU_MODE,
        false,
    );
}

// ---------------------------------------------------------------------------------------------
// Rust-only: error texts of detectDPUMode / dpuModeDescription (also covered by the golden
// fixtures through GenerateIgnition).
// ---------------------------------------------------------------------------------------------

#[test]
fn detect_dpu_mode_error_messages_match_go() {
    use crate::mode::{detect_dpu_mode, dpu_mode_description};
    assert_eq!(
        detect_dpu_mode(None, None).unwrap_err().to_string(),
        "DPFOperatorConfig is nil"
    );
    assert_eq!(
        detect_dpu_mode(Some(&legacy_host_agent()), Some(&empty_flavor()))
            .unwrap_err()
            .to_string(),
        "DPU mode is empty: set DPFOperatorConfig.spec.deploymentMode or DPUFlavor.spec.dpuMode"
    );
    let mut bad = host_trusted_operator();
    bad.spec.deployment_mode = "weird".into();
    assert_eq!(
        detect_dpu_mode(Some(&bad), None).unwrap_err().to_string(),
        "unsupported deploymentMode \"weird\""
    );
    assert_eq!(
        detect_dpu_mode(Some(&bad), Some(&flavor_with_mode("nic")))
            .unwrap_err()
            .to_string(),
        "failed to detect DPU mode from flavor: NIC mode is not supported"
    );
    assert_eq!(
        detect_dpu_mode(Some(&bad), Some(&flavor_with_mode("x")))
            .unwrap_err()
            .to_string(),
        "failed to detect DPU mode from flavor: invalid DPU mode: x (expected one of: zero-trust, dpu)"
    );
    assert_eq!(dpu_mode_description("nic"), "unknown (nic)");
    assert_eq!(
        dpu_mode_description("dpu"),
        "dpu (host-trusted, standard mode)"
    );
}
