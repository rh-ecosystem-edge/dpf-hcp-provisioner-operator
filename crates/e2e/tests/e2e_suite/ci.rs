//! Port of `test/e2e/e2e_ci_test.go`: the `DPFHCPProvisioner E2E` ordered container
//! (label `ocp-required`).

use std::collections::BTreeMap;
use std::time::Duration;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1;
use apis::hypershift::v1beta1 as hyperv1;
use apis::k8s::core::v1::LocalObjectReference;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::ObjectKey;
use ctrlrt::error::is_not_found;
use ctrlrt_runtime::RestConfig;
use e2e::ginkgo::{
    Container, NIL_POINTER_DEREFERENCE, NodeResult, SpecReport, Tree, by, go_panic, skip,
};
use e2e::gomega::{self as g, consistently, eventually};
use e2e::gw;
use k8s_openapi::api::core::v1::{Namespace, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use serde_json::{Map, Value};

use crate::Env;
use crate::ci_helpers::*;

const MINUTE: Duration = Duration::from_secs(60);

const TEST_DPU_HOSTNAME: &str = "e2e-test-dpu-node";
const ERROR_TEST_NAME: &str = "e2e-error-test";

/// Nodes of the `DPFHCPProvisioner E2E` container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node {
    BeforeAll,
    AfterAll,
    AfterEach,
    // HostedCluster Lifecycle
    CreateAndReachReady,
    ValidIgnition,
    DpuFlavorConfigFiles,
    SelfHealIgnitionConfigMap,
    RegenerateOnFlavorChange,
    BfbAnnotationUpdate,
    NoRegenerationWhileDeploymentMissing,
    KubeconfigInjected,
    StatusConditions,
    // HostedCluster Upgrade
    Upgrade,
    // CSR Auto-Approval
    CsrBeforeEach,
    CsrAfterEach,
    CsrBootstrap,
    CsrServing,
    CsrUnknownHostname,
    // Cleanup Verification
    CleanupAfterEach,
    CleanupOnDeletion,
    // Validation Error Cases
    ValidationAfterEach,
    MissingDpuCluster,
    InvalidSecrets,
}

/// `Describe("DPFHCPProvisioner E2E", Ordered, Label("ocp-required"), ...)`
pub struct DpfhcpProvisionerE2e {
    env: Env,
    kubeconfig_file: String,
    hc_config: Option<RestConfig>,
}

impl DpfhcpProvisionerE2e {
    pub fn new(env: Env) -> Self {
        Self {
            env,
            kubeconfig_file: String::new(),
            hc_config: None,
        }
    }

    pub fn spec_tree() -> Tree<Node> {
        Tree::describe("DPFHCPProvisioner E2E")
            .ordered()
            .label("ocp-required")
            .before_all(Node::BeforeAll)
            .after_all(Node::AfterAll)
            .after_each(Node::AfterEach)
            .context(
                Tree::describe("HostedCluster Lifecycle")
                    .it("should create DPFHCPProvisioner and reach Ready state", Node::CreateAndReachReady)
                    .it("should have generated valid ignition", Node::ValidIgnition)
                    .it(
                        "should include DPUFlavor config files in target ignition with correct merge behavior",
                        Node::DpuFlavorConfigFiles,
                    )
                    .it("should self-heal when ignition ConfigMap is deleted", Node::SelfHealIgnitionConfigMap)
                    .it(
                        "should regenerate ignition when DPUDeployment flavor changes",
                        Node::RegenerateOnFlavorChange,
                    )
                    .it(
                        "should update ConfigMap BFB annotation when DPUDeployment BFB changes",
                        Node::BfbAnnotationUpdate,
                    )
                    .it(
                        "should not regenerate ignition while DPUDeployment is missing",
                        Node::NoRegenerationWhileDeploymentMissing,
                    )
                    .it("should have injected kubeconfig into DPUCluster namespace", Node::KubeconfigInjected)
                    .it("should have correct status conditions", Node::StatusConditions),
            )
            .context(
                Tree::describe("HostedCluster Upgrade")
                    .it("should upgrade HostedCluster when ocpReleaseImage is updated", Node::Upgrade),
            )
            .context(
                Tree::describe("CSR Auto-Approval")
                    .before_each(Node::CsrBeforeEach)
                    .after_each(Node::CsrAfterEach)
                    .it("should approve bootstrap CSRs for valid DPU nodes", Node::CsrBootstrap)
                    .it("should approve serving CSRs for valid DPU nodes", Node::CsrServing)
                    .it("should not approve CSRs for unknown hostnames", Node::CsrUnknownHostname),
            )
            .context(
                Tree::describe("Cleanup Verification")
                    .after_each(Node::CleanupAfterEach)
                    .it("should clean up all resources on CR deletion", Node::CleanupOnDeletion),
            )
            .context(
                Tree::describe("Validation Error Cases")
                    .after_each(Node::ValidationAfterEach)
                    .it("should fail with missing DPUCluster", Node::MissingDpuCluster)
                    .it("should fail with invalid secrets", Node::InvalidSecrets),
            )
    }

    fn hc(&self) -> NodeResult<&RestConfig> {
        self.hc_config
            .as_ref()
            .ok_or_else(|| go_panic(NIL_POINTER_DEREFERENCE))
    }

    async fn ensure_hosted_cluster_api(&mut self) -> NodeResult {
        if self.hc_config.is_some() {
            return Ok(());
        }

        by("getting HostedCluster kubeconfig");
        let b64_kubeconfig =
            get_hosted_cluster_kubeconfig(&self.env, CI_NAMESPACE, PROVISIONER_NAME).await?;
        self.kubeconfig_file = write_kubeconfig_to_file(&b64_kubeconfig)?;
        let hc_config = load_hc_config(&self.kubeconfig_file).await?;
        self.hc_config = Some(hc_config.clone());

        by("waiting for HostedCluster API to be reachable");
        wait_for_hosted_cluster_api_reachable(&hc_config, 5 * MINUTE).await
    }

    async fn before_all(&mut self) -> NodeResult {
        let env = &self.env;
        by("cleaning up any stale resources from previous runs");
        cleanup_stale_resources(env).await?;

        by("creating DPUCluster namespace");
        create_namespace(env, DPU_CLUSTER_NS).await?;

        by("creating DPUCluster stub CR");
        create_dpu_cluster_stub(env, DPU_CLUSTER_NS, DPU_CLUSTER_NAME).await?;

        by("creating DPFOperatorConfig singleton");
        create_dpf_operator_config(env).await?;

        by("creating DPUFlavor stub");
        create_dpu_flavor_stub(env, DPU_CLUSTER_NS, DPU_FLAVOR_NAME).await?;

        by("creating DPUDeployment stub");
        create_dpu_deployment_stub(env, DPU_CLUSTER_NS, DPU_DEPLOYMENT_NAME, DPU_FLAVOR_NAME)
            .await?;

        by("generating SSH key secret");
        generate_ssh_key_secret(env, CI_NAMESPACE, SSH_KEY_SECRET_NAME).await?;

        by("copying pull secret from cluster");
        copy_pull_secret(env, CI_NAMESPACE, PULL_SECRET_NAME).await
    }

    fn after_all(&mut self) -> NodeResult {
        // Skip cleanup to preserve resources for must-gather collection.
        // The AWS cluster will be deprovisioned anyway, cleaning up all resources.
        // Only clean up temporary files from the local filesystem.
        by("skipping resource cleanup to preserve state for must-gather");

        if !self.kubeconfig_file.is_empty() {
            let _ = std::fs::remove_file(&self.kubeconfig_file);
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // HostedCluster Lifecycle
    // ---------------------------------------------------------------------------------------

    async fn create_and_reach_ready(&mut self) -> NodeResult {
        let env = &self.env;
        by("creating the DPFHCPProvisioner CR");
        create_dpfhcp_provisioner(env, CI_NAMESPACE, PROVISIONER_NAME).await?;

        by("waiting for CR to pass validation and begin provisioning");
        wait_for_cr_phase(env, PROVISIONER_NAME, "WaitingForControlPlane", 5 * MINUTE).await?;

        by("verifying HostedCluster was created");
        let hc_name = eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let provisioner = g::ok(
                env.client
                    .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(
                        CI_NAMESPACE,
                        PROVISIONER_NAME,
                    ))
                    .await,
                "",
            )?;
            let hc_name = provisioner
                .status
                .hosted_cluster_ref
                .as_ref()
                .ok_or_else(|| go_panic(NIL_POINTER_DEREFERENCE))?
                .name
                .clone();
            g::not_empty(&hc_name, "HostedCluster not yet created")?;
            Ok(hc_name)
        })
        .await?;

        by("waiting for HostedCluster to become available");
        eventually(
            HOSTED_CLUSTER_READY_TIMEOUT,
            POLLING_INTERVAL,
            "",
            async || {
                let hc = g::ok(
                    env.client
                        .get::<hyperv1::HostedCluster>(&ObjectKey::new(CI_NAMESPACE, &hc_name))
                        .await,
                    "",
                )?;

                let available =
                    hc.status.conditions.iter().any(|c| {
                        c.type_ == hyperv1::HOSTED_CLUSTER_AVAILABLE && c.status == "True"
                    });
                g::be_true(available, "HostedCluster not yet available")?;
                Ok(())
            },
        )
        .await?;

        by("waiting for CR to reach Ready state");
        wait_for_cr_phase(env, PROVISIONER_NAME, "Ready", CR_READY_TIMEOUT).await?;

        by("verifying status fields are populated");
        let provisioner = g::ok(
            env.client
                .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(
                    CI_NAMESPACE,
                    PROVISIONER_NAME,
                ))
                .await,
            "",
        )?;

        let hc_ref = provisioner
            .status
            .hosted_cluster_ref
            .as_ref()
            .ok_or_else(|| go_panic(NIL_POINTER_DEREFERENCE))?;
        g::not_empty(&hc_ref.name, "hostedClusterRef should be set")?;
        let kc_ref = provisioner
            .status
            .kube_config_secret_ref
            .as_ref()
            .ok_or_else(|| go_panic(NIL_POINTER_DEREFERENCE))?;
        g::not_empty(&kc_ref.name, "kubeConfigSecretRef should be set")?;
        Ok(())
    }

    async fn valid_ignition(&mut self) -> NodeResult {
        let env = &self.env;
        by("checking ignition ConfigMap exists in DPUCluster namespace");
        let cm = eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let cm = get_ignition_config_map(env).await?;
            g::not_nil(&cm, "Ignition ConfigMap not found")?;
            Ok(cm.unwrap_or_default())
        })
        .await?;

        by("verifying ignition content is valid JSON");
        let ignition_data = config_map_value(&cm, "BF_CFG_TEMPLATE");
        g::not_empty(&ignition_data, "Ignition ConfigMap data is empty")?;
        // Verify it's valid JSON (ignition format)
        let ignition_json = gocompat::json::unmarshal_object(ignition_data.as_bytes());
        g::succeed(&ignition_json, "Ignition data should be valid JSON")?;
        // Verify it has the ignition version field
        g::have_key(
            &ignition_json.unwrap_or_default(),
            "ignition",
            "Ignition data should contain 'ignition' key",
        )?;
        Ok(())
    }

    async fn dpu_flavor_config_files(&mut self) -> NodeResult {
        let env = &self.env;
        by("fetching the ignition ConfigMap");
        let cm = get_ignition_config_map(env).await?;
        g::not_nil(&cm, "Ignition ConfigMap not found")?;
        let cm = cm.unwrap_or_default();

        by("decoding the target ignition from live ignition");
        let target_ign = g::ok(
            decode_target_ignition(&config_map_value(&cm, "BF_CFG_TEMPLATE")),
            "Failed to decode target ignition",
        )?;

        let files = get_target_ignition_files(&target_ign)?;
        g::not_empty(&files, "Target ignition should have files")?;

        by("collecting file paths from target ignition");
        let mut path_set: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
        for f in &files {
            let path = match f.get("path") {
                Some(Value::String(p)) => p.clone(),
                other => {
                    return Err(go_panic(format!(
                        "interface conversion: interface {{}} is {}, not string",
                        go_type_name(other)
                    )));
                }
            };
            path_set.insert(path, f.clone());
        }

        by("verifying /etc/mellanox/mlnx-bf.conf was overridden by DPUFlavor (not duplicated)");
        let mlnx_bf = path_set.get("/etc/mellanox/mlnx-bf.conf");
        g::be_true(
            mlnx_bf.is_some(),
            "/etc/mellanox/mlnx-bf.conf should exist in target ignition",
        )?;

        let content = decode_ignition_file_content(&mlnx_bf.cloned().unwrap_or_default());
        g::succeed(&content, "")?;
        let content = content.unwrap_or_default();
        g::contain_substring(
            &content,
            "E2E_CUSTOM=\"true\"",
            "mlnx-bf.conf should contain the DPUFlavor override content",
        )?;
        g::contain_substring(
            &content,
            "ALLOW_SHARED_RQ=\"yes\"",
            "mlnx-bf.conf should have the flavor's value, not the default",
        )?;

        let count = files
            .iter()
            .filter(|f| f.get("path").and_then(Value::as_str) == Some("/etc/mellanox/mlnx-bf.conf"))
            .count();
        g::equal(
            &count,
            &1,
            "mlnx-bf.conf should appear exactly once (merged, not duplicated)",
        )?;

        by("verifying new file /etc/dpf/e2e-custom.conf was added");
        let custom_file = path_set.get("/etc/dpf/e2e-custom.conf");
        g::be_true(
            custom_file.is_some(),
            "/etc/dpf/e2e-custom.conf should exist",
        )?;
        let custom_content =
            decode_ignition_file_content(&custom_file.cloned().unwrap_or_default());
        g::succeed(&custom_content, "")?;
        g::equal(
            custom_content.unwrap_or_default().trim(),
            "E2E_NEW_FILE=yes",
            "",
        )?;

        by("verifying default files not in DPUFlavor are still present");
        let has_ovs = path_set.contains_key("/etc/mellanox/mlnx-ovs.conf");
        g::be_true(has_ovs, "Default mlnx-ovs.conf should still be present")?;
        let has_agent = path_set.contains_key("/usr/local/bin/install-dpu-agent.sh");
        g::be_true(
            has_agent,
            "Default install-dpu-agent.sh should still be present",
        )?;
        Ok(())
    }

    async fn self_heal_ignition_config_map(&mut self) -> NodeResult {
        let env = &self.env;
        by("verifying ignition ConfigMap exists before deletion");
        let cm = get_ignition_config_map(env).await?;
        g::not_nil(&cm, "Ignition ConfigMap should exist before test")?;

        by("deleting the ignition ConfigMap to trigger self-heal");
        g::succeed(&env.client.delete(&cm.unwrap_or_default()).await, "")?;

        by("verifying ConfigMap is actually gone");
        eventually(Duration::from_secs(30), POLLING_INTERVAL, "", async || {
            let found = get_ignition_config_map(env).await?;
            g::be_nil(&found, "ConfigMap should be deleted")?;
            Ok(())
        })
        .await?;

        by("waiting for ignition ConfigMap to be regenerated");
        eventually(5 * MINUTE, POLLING_INTERVAL, "", async || {
            let found = get_ignition_config_map(env).await?;
            g::not_nil(&found, "Ignition ConfigMap should be regenerated")?;
            g::not_empty(
                &config_map_value(&found.unwrap_or_default(), "BF_CFG_TEMPLATE"),
                "Regenerated ConfigMap should have ignition data",
            )?;
            Ok(())
        })
        .await?;

        by("waiting for CR to return to Ready state");
        wait_for_cr_phase(env, PROVISIONER_NAME, "Ready", CR_READY_TIMEOUT).await?;

        by("verifying IgnitionConfigured condition is True again");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let status =
                get_condition_status(env, CI_NAMESPACE, PROVISIONER_NAME, "IgnitionConfigured")
                    .await;
            g::equal(
                &status,
                "True",
                "IgnitionConfigured should be True after regeneration",
            )?;
            Ok(())
        })
        .await
    }

    async fn regenerate_on_flavor_change(&mut self) -> NodeResult {
        let env = &self.env;
        let new_flavor_name = "e2e-dpuflavor-v2";

        by("verifying CR is Ready and ignition is configured");
        g::equal(&get_cr_phase(env, PROVISIONER_NAME).await, "Ready", "")?;
        g::equal(
            &get_condition_status(env, CI_NAMESPACE, PROVISIONER_NAME, "IgnitionConfigured").await,
            "True",
            "",
        )?;

        by("recording the current ConfigMap resourceVersion");
        let cm = get_ignition_config_map(env).await?;
        g::not_nil(&cm, "")?;
        let cm = cm.unwrap_or_default();
        let old_resource_version = cm.metadata.resource_version.clone().unwrap_or_default();
        let old_flavor_annotation = annotation(
            &cm,
            "provisioning.dpu.nvidia.com/bfcfg-template-dpuflavor-name",
        );
        g::equal(&old_flavor_annotation, DPU_FLAVOR_NAME, "")?;

        by("creating a new DPUFlavor");
        let mut new_flavor = dpuprovisioningv1::DPUFlavor {
            metadata: meta(DPU_CLUSTER_NS, new_flavor_name),
            spec: dpuprovisioningv1::DPUFlavorSpec {
                dpu_mode: dpuprovisioningv1::DPU_MODE.into(),
                ovs: dpuprovisioningv1::DPUFlavorOVS {
                    raw_config_script: "#!/bin/bash\necho \"e2e test OVS config v2\"\n".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        g::succeed(&env.client.create(&mut new_flavor).await, "")?;

        by("updating DPUDeployment to reference the new flavor");
        let mut dpu_deployment = g::ok(
            env.client
                .get::<dpuservicev1::DPUDeployment>(&ObjectKey::new(
                    DPU_CLUSTER_NS,
                    DPU_DEPLOYMENT_NAME,
                ))
                .await,
            "",
        )?;
        dpu_deployment.spec.dpus.flavor = Some(new_flavor_name.into());
        g::succeed(&env.client.update(&mut dpu_deployment).await, "")?;

        by("waiting for ignition ConfigMap to be regenerated with new flavor");
        eventually(5 * MINUTE, POLLING_INTERVAL, "", async || {
            let cm = get_ignition_config_map(env).await?;
            g::not_nil(&cm, "")?;
            let cm = cm.unwrap_or_default();
            g::not_equal(
                &cm.metadata.resource_version.clone().unwrap_or_default(),
                &old_resource_version,
                "ConfigMap should have been updated",
            )?;
            g::equal(
                &annotation(
                    &cm,
                    "provisioning.dpu.nvidia.com/bfcfg-template-dpuflavor-name",
                ),
                new_flavor_name,
                "ConfigMap annotation should reflect the new flavor",
            )?;
            Ok(())
        })
        .await?;

        by("waiting for CR to return to Ready state");
        wait_for_cr_phase(env, PROVISIONER_NAME, "Ready", CR_READY_TIMEOUT).await?;

        by("verifying IgnitionConfigured condition is True again");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let status =
                get_condition_status(env, CI_NAMESPACE, PROVISIONER_NAME, "IgnitionConfigured")
                    .await;
            g::equal(&status, "True", "")?;
            Ok(())
        })
        .await
    }

    async fn bfb_annotation_update(&mut self) -> NodeResult {
        let env = &self.env;
        let new_bfb_name = "e2e-bfb-v2";

        by("verifying CR is Ready");
        g::equal(&get_cr_phase(env, PROVISIONER_NAME).await, "Ready", "")?;

        by("recording the current ConfigMap BFB annotation");
        let cm = get_ignition_config_map(env).await?;
        g::not_nil(&cm, "")?;
        let old_bfb = annotation(
            &cm.unwrap_or_default(),
            "provisioning.dpu.nvidia.com/bfcfg-template-bfb-name",
        );

        by("updating DPUDeployment BFB reference");
        let mut dpu_deployment = g::ok(
            env.client
                .get::<dpuservicev1::DPUDeployment>(&ObjectKey::new(
                    DPU_CLUSTER_NS,
                    DPU_DEPLOYMENT_NAME,
                ))
                .await,
            "",
        )?;
        dpu_deployment.spec.dpus.bfb = Some(new_bfb_name.into());
        g::succeed(&env.client.update(&mut dpu_deployment).await, "")?;

        by("waiting for ConfigMap BFB annotation to be updated");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let cm = get_ignition_config_map(env).await?;
            g::not_nil(&cm, "")?;
            let bfb = annotation(
                &cm.unwrap_or_default(),
                "provisioning.dpu.nvidia.com/bfcfg-template-bfb-name",
            );
            g::equal(
                &bfb,
                new_bfb_name,
                "BFB annotation should be updated to new BFB",
            )?;
            g::not_equal(&bfb, &old_bfb, "BFB annotation should differ from original")?;
            Ok(())
        })
        .await?;

        by("verifying CR stays Ready (no ignition regeneration needed)");
        g::equal(&get_cr_phase(env, PROVISIONER_NAME).await, "Ready", "")?;
        g::equal(
            &get_condition_status(env, CI_NAMESPACE, PROVISIONER_NAME, "IgnitionConfigured").await,
            "True",
            "IgnitionConfigured should stay True - BFB change doesn't require regeneration",
        )?;
        Ok(())
    }

    async fn no_regeneration_while_deployment_missing(&mut self) -> NodeResult {
        let env = &self.env;

        by("verifying CR is Ready and ignition ConfigMap exists");
        g::equal(&get_cr_phase(env, PROVISIONER_NAME).await, "Ready", "")?;
        let cm = get_ignition_config_map(env).await?;
        g::not_nil(
            &cm,
            "Ignition ConfigMap should exist before DPUDeployment deletion",
        )?;

        by("deleting the DPUDeployment");
        let dpu_deployment = g::ok(
            env.client
                .get::<dpuservicev1::DPUDeployment>(&ObjectKey::new(
                    DPU_CLUSTER_NS,
                    DPU_DEPLOYMENT_NAME,
                ))
                .await,
            "",
        )?;
        g::succeed(&env.client.delete(&dpu_deployment).await, "")?;

        by("waiting for DPUDeployment to be fully removed");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let found = env
                .client
                .get::<dpuservicev1::DPUDeployment>(&ObjectKey::new(
                    DPU_CLUSTER_NS,
                    DPU_DEPLOYMENT_NAME,
                ))
                .await;
            if let Ok(found) = &found
                && !found
                    .metadata
                    .finalizers
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty()
            {
                let mut found = found.clone();
                found.metadata.finalizers = None;
                let _ = env.client.update(&mut found).await;
            }
            g::be_true(
                found.as_ref().err().is_some_and(is_not_found),
                "DPUDeployment should be deleted",
            )?;
            Ok(())
        })
        .await?;

        by("waiting for ignition ConfigMap to be deleted");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            g::be_nil(
                &get_ignition_config_map(env).await?,
                "Ignition ConfigMap should be deleted when DPUDeployment is missing",
            )?;
            Ok(())
        })
        .await?;

        by("waiting for IgnitionConfigured=False with DependencyDeleted");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let conditions = get_cr_conditions(env, CI_NAMESPACE, PROVISIONER_NAME).await;
            let ign_cond: Option<Condition> = conditions
                .into_iter()
                .find(|c| c.type_ == "IgnitionConfigured");
            g::not_nil(&ign_cond, "IgnitionConfigured condition should exist")?;
            let Some(ign_cond) = ign_cond else {
                return Ok(());
            };
            g::equal(&ign_cond.status, "False", "")?;
            g::equal(
                &ign_cond.reason,
                provisioningv1alpha1::REASON_DEPENDENCY_DELETED,
                "should wait for DPUDeployment instead of failing ignition generation",
            )?;
            Ok(())
        })
        .await?;

        by("verifying ignition is not retried while DPUDeployment is missing");
        consistently(Duration::from_secs(45), POLLING_INTERVAL, "", async || {
            g::be_nil(
                &get_ignition_config_map(env).await?,
                "Ignition ConfigMap should stay deleted while DPUDeployment is missing",
            )?;
            let conditions = get_cr_conditions(env, CI_NAMESPACE, PROVISIONER_NAME).await;
            for c in conditions {
                if c.type_ == "IgnitionConfigured" {
                    g::equal(
                        &c.reason,
                        provisioningv1alpha1::REASON_DEPENDENCY_DELETED,
                        "IgnitionConfigured reason should stay DependencyDeleted, not IgnitionGenerationFailed",
                    )?;
                }
            }
            g::not_equal(
                &get_cr_phase(env, PROVISIONER_NAME).await,
                "Failed",
                "CR should not enter Failed while waiting for DPUDeployment",
            )?;
            Ok(())
        })
        .await?;

        by("recreating the DPUDeployment");
        create_dpu_deployment_stub(env, DPU_CLUSTER_NS, DPU_DEPLOYMENT_NAME, DPU_FLAVOR_NAME)
            .await?;

        by("waiting for ignition ConfigMap to be regenerated");
        eventually(5 * MINUTE, POLLING_INTERVAL, "", async || {
            let found = get_ignition_config_map(env).await?;
            g::not_nil(
                &found,
                "Ignition ConfigMap should be regenerated after DPUDeployment is recreated",
            )?;
            g::not_empty(
                &config_map_value(&found.unwrap_or_default(), "BF_CFG_TEMPLATE"),
                "Regenerated ConfigMap should have ignition data",
            )?;
            Ok(())
        })
        .await?;

        by("waiting for CR to return to Ready state");
        wait_for_cr_phase(env, PROVISIONER_NAME, "Ready", CR_READY_TIMEOUT).await?;

        by("verifying IgnitionConfigured condition is True again");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let status =
                get_condition_status(env, CI_NAMESPACE, PROVISIONER_NAME, "IgnitionConfigured")
                    .await;
            g::equal(&status, "True", "")?;
            Ok(())
        })
        .await
    }

    async fn kubeconfig_injected(&mut self) -> NodeResult {
        let env = &self.env;
        let kubeconfig_secret_name = format!("{PROVISIONER_NAME}-admin-kubeconfig");

        by("verifying kubeconfig secret exists in DPUCluster namespace");
        let secret = g::ok(
            env.client
                .get::<Secret>(&ObjectKey::new(DPU_CLUSTER_NS, &kubeconfig_secret_name))
                .await,
            "",
        )?;
        g::not_empty(
            secret.metadata.name.as_deref().unwrap_or_default(),
            "Kubeconfig secret not found in DPUCluster namespace",
        )?;

        by("verifying DPUCluster spec.kubeconfig is updated");
        let dpu_cluster = g::ok(
            env.client
                .get::<dpuprovisioningv1::DPUCluster>(&ObjectKey::new(
                    DPU_CLUSTER_NS,
                    DPU_CLUSTER_NAME,
                ))
                .await,
            "",
        )?;

        g::not_empty(
            &dpu_cluster.spec.kubeconfig,
            "DPUCluster spec.kubeconfig should be set",
        )?;
        Ok(())
    }

    async fn status_conditions(&mut self) -> NodeResult {
        let conditions = get_cr_conditions(&self.env, CI_NAMESPACE, PROVISIONER_NAME).await;
        g::not_empty(&conditions, "No conditions found")?;

        let cond_map = condition_map(&conditions);
        let get = |t: &str| cond_map.get(t).cloned().unwrap_or_default();

        g::equal(
            &get("HostedClusterAvailable"),
            "True",
            "HostedClusterAvailable should be True",
        )?;
        g::equal(
            &get("KubeConfigInjected"),
            "True",
            "KubeConfigInjected should be True",
        )?;
        g::equal(&get("SecretsValid"), "True", "SecretsValid should be True")?;
        g::equal(
            &get("ClusterTypeValid"),
            "True",
            "ClusterTypeValid should be True",
        )?;
        g::equal(&get("Ready"), "True", "Ready should be True")?;
        g::equal(
            &get("DPUClusterMissing"),
            "False",
            "DPUClusterMissing should be False",
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // HostedCluster Upgrade
    // ---------------------------------------------------------------------------------------

    async fn upgrade(&mut self) -> NodeResult {
        let env = &self.env;
        let upgrade_image = get_upgrade_ocp_release_image();
        if upgrade_image.is_empty() {
            return skip("Skipping upgrade test - UPGRADE_OCP_RELEASE_IMAGE not set");
        }

        if get_cr_phase(env, PROVISIONER_NAME).await != "Ready" {
            return skip("Skipping upgrade test - CR not in Ready state");
        }

        by("recording the original release image");
        let provisioner = g::ok(
            env.client
                .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(
                    CI_NAMESPACE,
                    PROVISIONER_NAME,
                ))
                .await,
            "",
        )?;
        let original_image = provisioner.spec.ocp_release_image.clone();
        let hc_name = provisioner
            .status
            .hosted_cluster_ref
            .as_ref()
            .ok_or_else(|| go_panic(NIL_POINTER_DEREFERENCE))?
            .name
            .clone();

        gw!("Upgrading from {} to {}\n", original_image, upgrade_image);

        by("updating DPFHCPProvisioner ocpReleaseImage");
        update_dpfhcp_provisioner_release_image(
            env,
            CI_NAMESPACE,
            PROVISIONER_NAME,
            &upgrade_image,
        )
        .await?;

        by("verifying HostedClusterUpgrading condition is set to True");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let status = get_condition_status(
                env,
                CI_NAMESPACE,
                PROVISIONER_NAME,
                "HostedClusterUpgrading",
            )
            .await;
            g::equal(&status, "True", "HostedClusterUpgrading should be True")?;
            Ok(())
        })
        .await?;

        by(
            "verifying phase reflects upgrade in progress (WaitingForControlPlane or GeneratingIgnition)",
        );
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let phase = get_cr_phase(env, PROVISIONER_NAME).await;
            g::equal_any(
                &phase,
                &[
                    "WaitingForControlPlane",
                    "GeneratingIgnition",
                    "ClusterVersionProgressing",
                ],
                "Phase should reflect upgrade progress",
            )?;
            Ok(())
        })
        .await?;

        by("verifying ignition ConfigMap was deleted");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let cm = get_ignition_config_map(env).await?;
            g::be_nil(&cm, "Ignition ConfigMap should be deleted during upgrade")?;
            Ok(())
        })
        .await?;

        by("verifying HostedCluster release image is updated");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let image = get_hosted_cluster_release_image(env, CI_NAMESPACE, &hc_name).await?;
            g::equal(
                &image,
                &upgrade_image,
                "HostedCluster release image should be updated",
            )?;
            Ok(())
        })
        .await?;

        by("verifying NodePool release image is updated");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let image = get_node_pool_release_image(env, CI_NAMESPACE, &hc_name).await?;
            g::equal(
                &image,
                &upgrade_image,
                "NodePool release image should be updated",
            )?;
            Ok(())
        })
        .await?;

        by("waiting for CR to return to Ready state");
        wait_for_cr_phase(env, PROVISIONER_NAME, "Ready", UPGRADE_TIMEOUT).await?;

        by("verifying HostedClusterUpgrading is False after upgrade");
        eventually(2 * MINUTE, POLLING_INTERVAL, "", async || {
            let status = get_condition_status(
                env,
                CI_NAMESPACE,
                PROVISIONER_NAME,
                "HostedClusterUpgrading",
            )
            .await;
            g::equal(
                &status,
                "False",
                "HostedClusterUpgrading should be False after upgrade",
            )?;
            Ok(())
        })
        .await?;

        by("verifying ignition ConfigMap was regenerated with version suffix");
        eventually(5 * MINUTE, POLLING_INTERVAL, "", async || {
            let cm = get_ignition_config_map(env).await?;
            g::not_nil(&cm, "Ignition ConfigMap should exist after upgrade")?;
            g::not_empty(
                &config_map_value(&cm.unwrap_or_default(), "BF_CFG_TEMPLATE"),
                "Ignition ConfigMap should have data after upgrade",
            )?;
            Ok(())
        })
        .await?;

        by("verifying final status conditions are healthy");
        let conditions = get_cr_conditions(env, CI_NAMESPACE, PROVISIONER_NAME).await;
        g::not_empty(&conditions, "No conditions found")?;

        let cond_map = condition_map(&conditions);
        let get = |t: &str| cond_map.get(t).cloned().unwrap_or_default();
        g::equal(
            &get("HostedClusterAvailable"),
            "True",
            "HostedClusterAvailable should be True after upgrade",
        )?;
        g::equal(&get("Ready"), "True", "Ready should be True after upgrade")?;
        g::equal(
            &get("IgnitionConfigured"),
            "True",
            "IgnitionConfigured should be True after upgrade",
        )?;
        g::equal(
            &get("HostedClusterUpgrading"),
            "False",
            "HostedClusterUpgrading should be False after upgrade",
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // CSR Auto-Approval
    // ---------------------------------------------------------------------------------------

    async fn csr_before_each(&mut self) -> NodeResult {
        if get_cr_phase(&self.env, PROVISIONER_NAME).await != "Ready" {
            return skip("Skipping CSR tests - CR not in Ready state");
        }
        self.ensure_hosted_cluster_api().await
    }

    async fn csr_after_each(&mut self) -> NodeResult {
        // Clean up DPU stubs and CSRs after each test
        delete_dpu_stub(&self.env, DPU_CLUSTER_NS, TEST_DPU_HOSTNAME).await;
        if let Some(hc_config) = &self.hc_config {
            delete_node_in_hosted_cluster(hc_config, TEST_DPU_HOSTNAME).await?;
        }
        Ok(())
    }

    async fn csr_bootstrap(&mut self) -> NodeResult {
        by("creating DPU stub with phase 'DPU Cluster Config'");
        create_dpu_stub(
            &self.env,
            DPU_CLUSTER_NS,
            TEST_DPU_HOSTNAME,
            "DPU Cluster Config",
        )
        .await?;

        let hc = self.hc()?;
        by("creating bootstrap CSR in HostedCluster");
        let csr_name = create_bootstrap_csr_in_hosted_cluster(hc, TEST_DPU_HOSTNAME).await?;

        by("waiting for CSR to be approved");
        wait_for_csr_approval(hc, &csr_name, CSR_APPROVAL_TIMEOUT).await?;

        by("cleaning up CSR");
        delete_csr_in_hosted_cluster(hc, &csr_name).await
    }

    async fn csr_serving(&mut self) -> NodeResult {
        by("creating DPU stub with phase 'Ready'");
        create_dpu_stub(&self.env, DPU_CLUSTER_NS, TEST_DPU_HOSTNAME, "Ready").await?;

        let hc = self.hc()?;
        by("creating Node in HostedCluster matching DPU hostname");
        create_node_in_hosted_cluster(hc, TEST_DPU_HOSTNAME).await?;

        by("creating serving CSR in HostedCluster");
        let csr_name = create_serving_csr_in_hosted_cluster(hc, TEST_DPU_HOSTNAME).await?;

        by("waiting for CSR to be approved");
        wait_for_csr_approval(hc, &csr_name, CSR_APPROVAL_TIMEOUT).await?;

        by("cleaning up");
        delete_csr_in_hosted_cluster(hc, &csr_name).await?;
        delete_node_in_hosted_cluster(hc, TEST_DPU_HOSTNAME).await
    }

    async fn csr_unknown_hostname(&mut self) -> NodeResult {
        let unknown_hostname = "unknown-dpu-node";
        let hc = self.hc()?;

        by("creating bootstrap CSR with unknown hostname");
        let csr_name = create_bootstrap_csr_in_hosted_cluster(hc, unknown_hostname).await?;

        by("verifying CSR stays pending for 60 seconds");
        verify_csr_not_approved(hc, &csr_name, Duration::from_secs(60)).await?;

        by("cleaning up CSR");
        delete_csr_in_hosted_cluster(hc, &csr_name).await
    }

    // ---------------------------------------------------------------------------------------
    // Cleanup Verification
    // ---------------------------------------------------------------------------------------

    async fn cleanup_after_each(&mut self, spec: &SpecReport) -> NodeResult {
        let env = &self.env;
        // Dump status on failure (preserve all resources for debugging)
        if spec.failed() {
            dump_provisioner_status(env, CI_NAMESPACE, CLEANUP_TEST_NAME).await;
            return Ok(());
        }

        // On success: Clean up DPF-specific namespace
        // (Provisioner CR should already be deleted - verified by the test)
        if let Ok(ns) = env
            .client
            .get::<Namespace>(&ObjectKey::cluster(CLEANUP_DPU_CLUSTER_NS))
            .await
        {
            let _ = env.client.delete(&ns).await;
        }
        Ok(())
    }

    async fn cleanup_on_deletion(&mut self) -> NodeResult {
        let env = &self.env;

        by("creating cleanup test DPUCluster namespace");
        create_namespace(env, CLEANUP_DPU_CLUSTER_NS).await?;

        by("creating cleanup test DPUCluster stub");
        create_dpu_cluster_stub(env, CLEANUP_DPU_CLUSTER_NS, CLEANUP_DPU_CLUSTER_NAME).await?;

        by("creating cleanup test DPUFlavor stub");
        create_dpu_flavor_stub(env, CLEANUP_DPU_CLUSTER_NS, CLEANUP_DPU_FLAVOR_NAME).await?;

        by("creating cleanup test DPUDeployment stub");
        create_dpu_deployment_stub(
            env,
            CLEANUP_DPU_CLUSTER_NS,
            CLEANUP_DPU_DEPLOY_NAME,
            CLEANUP_DPU_FLAVOR_NAME,
        )
        .await?;

        by("creating a DPFHCPProvisioner CR for cleanup testing");
        create_dpfhcp_provisioner_with_cluster(
            env,
            CI_NAMESPACE,
            CLEANUP_TEST_NAME,
            CLEANUP_DPU_CLUSTER_NAME,
            CLEANUP_DPU_CLUSTER_NS,
            CLEANUP_DPU_DEPLOY_NAME,
        )
        .await?;

        by("waiting for cleanup test CR to reach Ready state");
        wait_for_cr_phase(env, CLEANUP_TEST_NAME, "Ready", CR_READY_TIMEOUT).await?;

        by("getting the HostedCluster name for cleanup test CR");
        let provisioner = g::ok(
            env.client
                .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(
                    CI_NAMESPACE,
                    CLEANUP_TEST_NAME,
                ))
                .await,
            "",
        )?;
        let cleanup_hc_name = provisioner
            .status
            .hosted_cluster_ref
            .as_ref()
            .ok_or_else(|| go_panic(NIL_POINTER_DEREFERENCE))?
            .name
            .clone();

        by("deleting the cleanup test DPFHCPProvisioner CR");
        g::succeed(
            &env.client.delete(&provisioner).await,
            "Failed to delete DPFHCPProvisioner",
        )?;

        by("verifying cleanup test CR is fully deleted");
        eventually(CLEANUP_TIMEOUT, POLLING_INTERVAL, "", async || {
            let err = env
                .client
                .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(
                    CI_NAMESPACE,
                    CLEANUP_TEST_NAME,
                ))
                .await
                .err();
            g::be_true(
                err.as_ref().is_some_and(is_not_found),
                "CR should be deleted",
            )?;
            Ok(())
        })
        .await?;

        by("verifying cleanup test HostedCluster is deleted");
        eventually(CLEANUP_TIMEOUT, POLLING_INTERVAL, "", async || {
            let err = env
                .client
                .get::<hyperv1::HostedCluster>(&ObjectKey::new(CI_NAMESPACE, &cleanup_hc_name))
                .await
                .err();
            g::be_true(
                err.as_ref().is_some_and(is_not_found),
                "HostedCluster should be deleted",
            )?;
            Ok(())
        })
        .await
    }

    // ---------------------------------------------------------------------------------------
    // Validation Error Cases
    // ---------------------------------------------------------------------------------------

    fn error_test_provisioner(
        release_image: String,
        cluster: (&str, &str),
        ssh_key: &str,
        pull_secret: &str,
        deployment: (&str, &str),
    ) -> provisioningv1alpha1::DPFHCPProvisioner {
        provisioningv1alpha1::DPFHCPProvisioner {
            metadata: meta(CI_NAMESPACE, ERROR_TEST_NAME),
            spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
                dpu_cluster_ref: provisioningv1alpha1::DPUClusterReference {
                    name: cluster.0.into(),
                    namespace: cluster.1.into(),
                    ..Default::default()
                },
                base_domain: "test.example.com".into(),
                ocp_release_image: release_image,
                ssh_key_secret_ref: LocalObjectReference {
                    name: ssh_key.into(),
                    ..Default::default()
                },
                pull_secret_ref: LocalObjectReference {
                    name: pull_secret.into(),
                    ..Default::default()
                },
                control_plane_availability_policy: hyperv1::SINGLE_REPLICA.into(),
                dpu_deployment_ref: Some(provisioningv1alpha1::DPUDeploymentReference {
                    name: deployment.0.into(),
                    namespace: deployment.1.into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    async fn missing_dpu_cluster(&mut self) -> NodeResult {
        let env = &self.env;
        by("creating CR referencing non-existent DPUCluster");
        let release_image = detect_ocp_release_image(env).await?;

        let mut provisioner = Self::error_test_provisioner(
            release_image,
            ("non-existent-cluster", "non-existent-ns"),
            SSH_KEY_SECRET_NAME,
            PULL_SECRET_NAME,
            ("non-existent-deployment", "non-existent-ns"),
        );
        g::succeed(&env.client.create(&mut provisioner).await, "")?;

        by("verifying CR reaches Failed phase");
        wait_for_cr_phase(env, ERROR_TEST_NAME, "Failed", 2 * MINUTE).await?;

        by("verifying DPUClusterMissing condition");
        eventually(
            Duration::from_secs(30),
            Duration::from_secs(5),
            "",
            async || {
                let status =
                    get_condition_status(env, CI_NAMESPACE, ERROR_TEST_NAME, "DPUClusterMissing")
                        .await;
                g::equal(&status, "True", "DPUClusterMissing should be True")?;
                Ok(())
            },
        )
        .await
    }

    async fn invalid_secrets(&mut self) -> NodeResult {
        let env = &self.env;
        by("creating CR with non-existent secret references");
        let release_image = detect_ocp_release_image(env).await?;

        let mut provisioner = Self::error_test_provisioner(
            release_image,
            (DPU_CLUSTER_NAME, DPU_CLUSTER_NS),
            "non-existent-ssh-key",
            "non-existent-pull-secret",
            (DPU_DEPLOYMENT_NAME, DPU_CLUSTER_NS),
        );
        g::succeed(&env.client.create(&mut provisioner).await, "")?;

        by("verifying CR reaches Failed phase");
        wait_for_cr_phase(env, ERROR_TEST_NAME, "Failed", 2 * MINUTE).await?;

        by("verifying SecretsValid condition is False");
        eventually(
            Duration::from_secs(30),
            Duration::from_secs(5),
            "",
            async || {
                let status =
                    get_condition_status(env, CI_NAMESPACE, ERROR_TEST_NAME, "SecretsValid").await;
                g::equal(&status, "False", "SecretsValid should be False")?;
                Ok(())
            },
        )
        .await
    }
}

/// `cm.Data[key]` (empty when absent).
fn config_map_value(cm: &k8s_openapi::api::core::v1::ConfigMap, key: &str) -> String {
    cm.data
        .as_ref()
        .and_then(|d| d.get(key))
        .cloned()
        .unwrap_or_default()
}

/// `obj.Annotations[key]` (empty when absent).
fn annotation(cm: &k8s_openapi::api::core::v1::ConfigMap, key: &str) -> String {
    cm.metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(key))
        .cloned()
        .unwrap_or_default()
}

/// `condMap[c.Type] = c.Status`
fn condition_map(conditions: &[Condition]) -> BTreeMap<String, String> {
    conditions
        .iter()
        .map(|c| (c.type_.clone(), c.status.clone()))
        .collect()
}

impl Container for DpfhcpProvisionerE2e {
    type Node = Node;

    fn tree(&self) -> Tree<Node> {
        Self::spec_tree()
    }

    async fn run(&mut self, node: Node, spec: &SpecReport) -> NodeResult {
        match node {
            Node::BeforeAll => self.before_all().await,
            Node::AfterAll => self.after_all(),
            Node::AfterEach => {
                if spec.failed() {
                    dump_provisioner_status(&self.env, CI_NAMESPACE, PROVISIONER_NAME).await;
                }
                Ok(())
            }
            Node::CreateAndReachReady => self.create_and_reach_ready().await,
            Node::ValidIgnition => self.valid_ignition().await,
            Node::DpuFlavorConfigFiles => self.dpu_flavor_config_files().await,
            Node::SelfHealIgnitionConfigMap => self.self_heal_ignition_config_map().await,
            Node::RegenerateOnFlavorChange => self.regenerate_on_flavor_change().await,
            Node::BfbAnnotationUpdate => self.bfb_annotation_update().await,
            Node::NoRegenerationWhileDeploymentMissing => {
                self.no_regeneration_while_deployment_missing().await
            }
            Node::KubeconfigInjected => self.kubeconfig_injected().await,
            Node::StatusConditions => self.status_conditions().await,
            Node::Upgrade => self.upgrade().await,
            Node::CsrBeforeEach => self.csr_before_each().await,
            Node::CsrAfterEach => self.csr_after_each().await,
            Node::CsrBootstrap => self.csr_bootstrap().await,
            Node::CsrServing => self.csr_serving().await,
            Node::CsrUnknownHostname => self.csr_unknown_hostname().await,
            Node::CleanupAfterEach => self.cleanup_after_each(spec).await,
            Node::CleanupOnDeletion => self.cleanup_on_deletion().await,
            Node::ValidationAfterEach => {
                // Use forceDeleteProvisioner to ensure it's fully deleted before next test
                force_delete_provisioner(&self.env, CI_NAMESPACE, ERROR_TEST_NAME).await
            }
            Node::MissingDpuCluster => self.missing_dpu_cluster().await,
            Node::InvalidSecrets => self.invalid_secrets().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_tree_mirrors_go_container() {
        let t = DpfhcpProvisionerE2e::spec_tree();
        assert!(t.ordered);
        assert_eq!(t.labels, vec!["ocp-required"]);
        let contexts: Vec<&str> = t
            .children
            .iter()
            .map(|c| match c {
                e2e::ginkgo::Child::Container(c) => c.text.as_str(),
                e2e::ginkgo::Child::It { text, .. } => text.as_str(),
            })
            .collect();
        assert_eq!(
            contexts,
            vec![
                "HostedCluster Lifecycle",
                "HostedCluster Upgrade",
                "CSR Auto-Approval",
                "Cleanup Verification",
                "Validation Error Cases"
            ]
        );
    }
}
