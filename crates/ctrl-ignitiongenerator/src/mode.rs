//! Port of internal/controller/ignitiongenerator/mode.go

use apis::dpf::operator::v1alpha1 as operatorv1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use ctrlrt::wrapf;
use dpuprovisioningv1alpha1::DpuModeType;

/// dpuModeFromFlavor reads DPUFlavor.spec.dpuMode (26.4 GA path).
pub(crate) fn dpu_mode_from_flavor(
    dpu_flavor: Option<&dpuprovisioningv1alpha1::DPUFlavor>,
) -> anyhow::Result<DpuModeType> {
    let Some(dpu_flavor) = dpu_flavor else {
        return Err(anyhow::anyhow!("DPUFlavor is nil"));
    };

    let mode = dpu_flavor.spec.dpu_mode.as_str();

    match mode {
        dpuprovisioningv1alpha1::ZERO_TRUST_MODE => {
            Ok(dpuprovisioningv1alpha1::ZERO_TRUST_MODE.to_string())
        }
        dpuprovisioningv1alpha1::DPU_MODE => Ok(dpuprovisioningv1alpha1::DPU_MODE.to_string()),
        dpuprovisioningv1alpha1::NIC_MODE => Err(anyhow::anyhow!("NIC mode is not supported")),
        "" => Err(anyhow::anyhow!(
            "DPU mode is empty: set DPFOperatorConfig.spec.deploymentMode or DPUFlavor.spec.dpuMode"
        )),
        _ => Err(anyhow::anyhow!(
            "invalid DPU mode: {} (expected one of: zero-trust, dpu)",
            mode
        )),
    }
}

/// detectDPUMode resolves the effective DPU mode for ignition.
/// DPUFlavor.spec.dpuMode is checked first so a newer HCP operator works with older DPF (26.4 GA).
/// When flavor dpuMode is unset (26.8+), DPFOperatorConfig.spec.deploymentMode and legacy Redfish apply.
pub(crate) fn detect_dpu_mode(
    cfg: Option<&operatorv1alpha1::DPFOperatorConfig>,
    flavor: Option<&dpuprovisioningv1alpha1::DPUFlavor>,
) -> anyhow::Result<DpuModeType> {
    let Some(cfg) = cfg else {
        return Err(anyhow::anyhow!("DPFOperatorConfig is nil"));
    };

    if let Some(flavor) = flavor
        && !flavor.spec.dpu_mode.is_empty()
    {
        return dpu_mode_from_flavor(Some(flavor))
            .map_err(|e| wrapf!(e, "failed to detect DPU mode from flavor: {}", e));
    }

    match cfg.spec.deployment_mode.as_str() {
        operatorv1alpha1::DEPLOYMENT_MODE_ZERO_TRUST => {
            Ok(dpuprovisioningv1alpha1::ZERO_TRUST_MODE.to_string())
        }
        operatorv1alpha1::DEPLOYMENT_MODE_HOST_TRUSTED => {
            Ok(dpuprovisioningv1alpha1::DPU_MODE.to_string())
        }
        "" => {
            if cfg
                .spec
                .provisioning_controller
                .as_ref()
                .and_then(|pc| pc.install_interface.as_ref())
                .is_some_and(|ii| ii.install_via_redfish.is_some())
            {
                return Ok(dpuprovisioningv1alpha1::ZERO_TRUST_MODE.to_string());
            }
            Err(anyhow::anyhow!(
                "DPU mode is empty: set DPFOperatorConfig.spec.deploymentMode or DPUFlavor.spec.dpuMode"
            ))
        }
        other => Err(anyhow::anyhow!(
            "unsupported deploymentMode {}",
            gocompat::strconv::quote(other)
        )),
    }
}

/// isZeroTrustMode reports whether ignition should use zero-trust resources for this cluster.
pub(crate) fn is_zero_trust_mode(
    cfg: Option<&operatorv1alpha1::DPFOperatorConfig>,
    flavor: Option<&dpuprovisioningv1alpha1::DPUFlavor>,
) -> anyhow::Result<(bool, DpuModeType)> {
    let mode = detect_dpu_mode(cfg, flavor)?;
    Ok((mode == dpuprovisioningv1alpha1::ZERO_TRUST_MODE, mode))
}

pub(crate) fn dpu_mode_description(mode: &str) -> String {
    match mode {
        dpuprovisioningv1alpha1::ZERO_TRUST_MODE => {
            "zero-trust (host untrusted, hardware-isolated)".to_string()
        }
        dpuprovisioningv1alpha1::DPU_MODE => "dpu (host-trusted, standard mode)".to_string(),
        _ => format!("unknown ({mode})"),
    }
}
