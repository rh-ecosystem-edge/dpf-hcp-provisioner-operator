//! Port of internal/controller/dpuservicetemplate/dpuservicetemplatevalues.go

use ctrlrt::k8s_openapi::api::core::v1::ConfigMap;
use ctrlrt::{Client, Ctx, ObjectKey, Result, kv, wrapf};
use dpfdefaults::DpuServiceTemplateValues;

use crate::gojson::{self, GoType};

pub(crate) const OVERRIDES_CONFIG_MAP_NAME: &str = "dpuservicetemplate-overrides";

/// The Go type `dpfdefaults.DPUServiceTemplateValues` as seen by `encoding/json`
/// (`ImagePullSecret` is `json:"-"`).
static DPU_SERVICE_TEMPLATE_VALUES_TYPE: GoType = GoType::Struct {
    name: "DPUServiceTemplateValues",
    type_string: "dpfdefaults.DPUServiceTemplateValues",
    fields: &[
        (
            "ovn",
            GoType::Struct {
                name: "OVNTemplateValues",
                type_string: "dpfdefaults.OVNTemplateValues",
                fields: &[
                    ("chartRepoURL", GoType::String),
                    ("chartName", GoType::String),
                    ("chartVersion", GoType::String),
                ],
            },
        ),
        (
            "dts",
            GoType::Struct {
                name: "DTSTemplateValues",
                type_string: "dpfdefaults.DTSTemplateValues",
                fields: &[
                    ("chartRepoURL", GoType::String),
                    ("chartName", GoType::String),
                    ("chartVersion", GoType::String),
                    ("imageDTS", GoType::String),
                    ("prometheusPort", GoType::Int32),
                ],
            },
        ),
        (
            "hbn",
            GoType::Struct {
                name: "HBNTemplateValues",
                type_string: "dpfdefaults.HBNTemplateValues",
                fields: &[
                    ("chartRepoURL", GoType::String),
                    ("chartName", GoType::String),
                    ("chartVersion", GoType::String),
                    ("imageRepo", GoType::String),
                    ("imageTag", GoType::String),
                    ("bfSFCount", GoType::Int32),
                ],
            },
        ),
    ],
};

/// applyOverridesFromConfigMap reads the [`OVERRIDES_CONFIG_MAP_NAME`] ConfigMap
/// from the operator namespace and merges any matching version overrides
/// on top of the given embedded values. If the ConfigMap doesn't
/// exist, the embedded values are returned unchanged.
pub(crate) async fn apply_overrides_from_config_map(
    ctx: &Ctx,
    c: &Client,
    operator_namespace: &str,
    version: &str,
    current_dpu_service_template_values: &mut DpuServiceTemplateValues,
) -> Result<()> {
    let log = &ctx.log;

    let cm: ConfigMap = match c
        .get(&ObjectKey::new(
            operator_namespace,
            OVERRIDES_CONFIG_MAP_NAME,
        ))
        .await
    {
        Ok(cm) => cm,
        Err(err) if ctrlrt::error::is_not_found(&err) => return Ok(()),
        Err(err) => return Err(wrapf!(err, "getting overrides configmap: {}", err)),
    };

    let raw_overrides_json = match cm.data.as_ref().and_then(|d| d.get("overrides.json")) {
        Some(s) if !s.is_empty() => s.clone(),
        _ => return Ok(()),
    };

    let all_version_overrides =
        match gojson::unmarshal_raw_message_map(raw_overrides_json.as_bytes()) {
            Ok(m) => m,
            Err(msg) => {
                let err = anyhow::anyhow!(msg);
                return Err(wrapf!(err, "parsing overrides configmap: {}", err));
            }
        };

    let Some(current_version_override) = all_version_overrides
        .and_then(|m| m.into_iter().find(|(k, _)| k == version).map(|(_, v)| v))
    else {
        return Ok(());
    };

    log.info(
        "Applying DPUServiceTemplate overrides from configmap",
        kv!["version" => version],
    );

    // Unmarshal the override values into the currentDPUServiceTemplateValues,
    // which will merge the overrides on top of the existing values.
    let mut target = serde_json::to_value(&*current_dpu_service_template_values)?;
    gojson::decode_into(
        &current_version_override,
        &DPU_SERVICE_TEMPLATE_VALUES_TYPE,
        &mut target,
        true,
    )
    .map_err(|msg| anyhow::anyhow!(msg))?;
    let image_pull_secret =
        std::mem::take(&mut current_dpu_service_template_values.image_pull_secret);
    *current_dpu_service_template_values = serde_json::from_value(target)?;
    current_dpu_service_template_values.image_pull_secret = image_pull_secret;
    Ok(())
}
