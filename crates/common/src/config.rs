//! Port of `internal/common/config.go`.

use apis::dpf::operator::v1alpha1::DPFOperatorConfig;
use apis::provisioning::v1alpha1::{DEFAULT_CONFIG_NAME, DPFHCPProvisionerConfig};
use ctrlrt::{Client, Ctx, ObjectKey, Result, kv, wrapf};

/// Operator configuration loaded from the DPFHCPProvisionerConfig CR.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OperatorConfig {
    pub blue_field_ocp_layer_repo: String,
    pub disable_metal_lb: bool,
    pub manage_dpu_service_templates: bool,
    pub dpu_services_image_pull_secret: String,
    pub ovs_hugepages_size: String,
    pub ovs_hugepages_amount: i32,
}

/// `LoadOperatorConfigFromCR` reads the singleton config CR. Returns `Ok(None)` when it does
/// not exist (the operator is then idle).
pub async fn load_operator_config_from_cr(ctx: &Ctx, c: &Client) -> Result<Option<OperatorConfig>> {
    let config_cr: DPFHCPProvisionerConfig = match c.get(&ObjectKey::cluster(DEFAULT_CONFIG_NAME)).await {
        Ok(v) => v,
        Err(e) if ctrlrt::error::is_not_found(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    let spec = &config_cr.spec;
    let cfg = OperatorConfig {
        blue_field_ocp_layer_repo: spec.blue_field_ocp_layer_repo.clone(),
        disable_metal_lb: spec.disable_metal_lb,
        manage_dpu_service_templates: spec.manage_dpu_service_templates,
        dpu_services_image_pull_secret: spec.dpu_services_image_pull_secret.clone(),
        ovs_hugepages_size: spec.ovs_hugepages_size.clone(),
        ovs_hugepages_amount: spec.ovs_hugepages_amount,
    };
    ctx.log.v(1).info(
        "Operator config loaded from CR",
        kv![
            "blueFieldOCPLayerRepo" => cfg.blue_field_ocp_layer_repo,
            "disableMetalLB" => cfg.disable_metal_lb,
            "manageDPUServiceTemplates" => cfg.manage_dpu_service_templates,
            "dpuServicesImagePullSecret" => cfg.dpu_services_image_pull_secret,
            "ovsHugepagesSize" => cfg.ovs_hugepages_size,
            "ovsHugepagesAmount" => cfg.ovs_hugepages_amount,
        ],
    );
    Ok(Some(cfg))
}

/// `GetSingletonDPFOperatorConfig` lists DPFOperatorConfigs and requires exactly one.
pub async fn get_singleton_dpf_operator_config(c: &Client) -> Result<DPFOperatorConfig> {
    let mut list: Vec<DPFOperatorConfig> = match c.list_all().await {
        Ok(l) => l,
        Err(e) => return Err(wrapf!(e, "listing DPFOperatorConfig: {}", e)),
    };
    if list.len() != 1 {
        return Err(anyhow::anyhow!(
            "expected exactly one DPFOperatorConfig, found {}",
            list.len()
        ));
    }
    Ok(list.remove(0))
}

/// `GetDPFVersion` returns `DPFOperatorConfig.status.version`.
pub async fn get_dpf_version(c: &Client) -> Result<String> {
    let config = get_singleton_dpf_operator_config(c).await?;
    match config.status.version.as_deref() {
        Some(v) if !v.is_empty() => Ok(v.to_string()),
        _ => Err(anyhow::anyhow!("DPFOperatorConfig.status.version is not set")),
    }
}
