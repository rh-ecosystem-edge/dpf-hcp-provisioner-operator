//! Port of `test/e2e/e2e_dpuservicetemplate_helpers_test.go`.

use apis::dpf::operator::v1alpha1 as operatorv1;
use apis::openshift::config::v1 as configv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::ObjectKey;
use ctrlrt::error::{ignore_not_found, is_not_found};
use e2e::ginkgo::NodeResult;
use e2e::gomega as g;
use k8s_openapi::api::core::v1::Namespace;

use crate::Env;
use crate::ci_helpers::*;

pub const STUB_OCP_VERSION: &str = "4.22.6";
pub const STUB_RELEASE_IMAGE: &str = "quay.io/openshift-release-dev/ocp-release:4.22.6-multi";
pub const STUB_OVN_IMAGE: &str =
    "quay.io/openshift-release-dev/ocp-v4.0-art-dev:ovn-kubernetes-4.22.6";
pub const STUB_DPF_VERSION: &str = "v26.4.0";

/// `enableManageDPUServiceTemplates` patches the DPFHCPProvisionerConfig to
/// enable template management.
pub async fn enable_manage_dpu_service_templates(env: &Env) -> NodeResult {
    let config = env
        .client
        .get::<provisioningv1alpha1::DPFHCPProvisionerConfig>(&ObjectKey::cluster("default"))
        .await;
    if let Err(e) = &config
        && is_not_found(e)
    {
        let mut config = provisioningv1alpha1::DPFHCPProvisionerConfig {
            metadata: meta("", "default"),
            spec: provisioningv1alpha1::DPFHCPProvisionerConfigSpec {
                manage_dpu_service_templates: true,
                ..Default::default()
            },
            ..Default::default()
        };
        g::ok(
            env.client.create(&mut config).await,
            "Failed to create DPFHCPProvisionerConfig",
        )?;
        return Ok(());
    }
    let mut config = g::ok(config, "")?;
    config.spec.manage_dpu_service_templates = true;
    g::ok(
        env.client.update(&mut config).await,
        "Failed to enable ManageDPUServiceTemplates",
    )?;
    Ok(())
}

/// `setDPFOperatorConfigVersion` creates the DPFOperatorConfig singleton and sets status.version.
pub async fn set_dpf_operator_config_version(env: &Env, version: &str) -> NodeResult {
    let config = env
        .client
        .get::<operatorv1::DPFOperatorConfig>(&ObjectKey::new(
            DPF_OPERATOR_CONFIG_NAMESPACE,
            "dpfoperatorconfig",
        ))
        .await;

    let mut config = match config {
        Err(e) if is_not_found(&e) => {
            let mut config = dpf_operator_config_stub();
            g::ok(
                env.client.create(&mut config).await,
                "Failed to create DPFOperatorConfig",
            )?;
            config
        }
        other => g::ok(other, "")?,
    };

    // Patch status.version
    config.status.version = Some(version.to_string());
    g::ok(
        env.client.update_status(&mut config).await,
        "Failed to set DPFOperatorConfig version",
    )?;
    Ok(())
}

/// `cleanupTemplateTestResources` removes DPUServiceTemplate test resources.
pub async fn cleanup_template_test_resources(env: &Env) -> NodeResult {
    // Delete provisioner (force-remove finalizer if stuck)
    force_delete_provisioner(env, CI_NAMESPACE, PROVISIONER_NAME).await?;

    // Delete DPFOperatorConfig
    if let Err(e) = ignore_not_found(
        delete_all_of::<operatorv1::DPFOperatorConfig>(env, DPF_OPERATOR_CONFIG_NAMESPACE).await,
    ) {
        warn_error(&format!("failed to cleanup DPFOperatorConfig: {e}"));
    }

    // Delete ClusterVersion (only if we created the stub on Kind)
    if !env.testing_on_ocp {
        let cv = configv1::ClusterVersion {
            metadata: meta("", "version"),
            ..Default::default()
        };
        if let Err(e) = ignore_not_found(env.client.delete(&cv).await) {
            warn_error(&format!("failed to cleanup ClusterVersion: {e}"));
        }
    }

    // Delete the DPUCluster namespace (cascades DPUCluster, DPUFlavor, DPUDeployment, templates)
    let ns = Namespace {
        metadata: meta("", DPU_CLUSTER_NS),
        ..Default::default()
    };
    if let Err(e) = ignore_not_found(env.client.delete(&ns).await) {
        warn_error(&format!("failed to cleanup DPUCluster namespace: {e}"));
    }
    Ok(())
}
