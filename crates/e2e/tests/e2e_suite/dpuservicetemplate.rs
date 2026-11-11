//! Port of `test/e2e/e2e_dpuservicetemplate_test.go`: the `DPUServiceTemplate E2E` ordered
//! container (no labels: runs on Kind and is skipped on OCP from its `BeforeAll`).

use std::time::Duration;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use ctrlrt::{ListOptions, ObjectKey};
use e2e::ginkgo::{Container, NodeResult, SpecReport, Tree, by, skip};
use e2e::gomega::{self as g, consistently, eventually};

use crate::Env;
use crate::ci_helpers::*;
use crate::dpuservicetemplate_helpers::*;
use crate::fake_ocp_on_kind::*;

pub const MANAGED_BY_LABEL: &str = "dpfhcpprovisioner.dpu.hcp.io/managed";

const GENERAL_TEMPLATE_RECONCILE_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const GENERAL_TEMPLATE_RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

pub const TEMPLATE_NAMES: [&str; 3] = ["ovn", "doca-telemetry-service", "hbn"];

/// Nodes of the `DPUServiceTemplate E2E` container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node {
    BeforeAll,
    AfterAll,
    CreateAllTemplates,
    TemplateStructure,
    SelfHealDeleted,
    SelfHealModified,
    DeleteOnDeploymentDeletion,
    DeleteOnProvisionerDeletion,
}

/// `Describe("DPUServiceTemplate E2E", Ordered, ...)`
pub struct DpuServiceTemplateE2e {
    env: Env,
}

impl DpuServiceTemplateE2e {
    pub fn new(env: Env) -> Self {
        Self { env }
    }

    pub fn spec_tree() -> Tree<Node> {
        Tree::describe("DPUServiceTemplate E2E")
            .ordered()
            .before_all(Node::BeforeAll)
            .after_all(Node::AfterAll)
            .context(
                Tree::describe("Template Lifecycle")
                    .it(
                        "should create all three DPUServiceTemplates when provisioner exists",
                        Node::CreateAllTemplates,
                    )
                    .it(
                        "should have correct template structure",
                        Node::TemplateStructure,
                    )
                    .it(
                        "should self-heal when a template is deleted",
                        Node::SelfHealDeleted,
                    )
                    .it(
                        "should self-heal when a template is modified",
                        Node::SelfHealModified,
                    )
                    .it(
                        "should delete templates when DPUDeployment is deleted",
                        Node::DeleteOnDeploymentDeletion,
                    )
                    .it(
                        "should delete templates when provisioner is deleted",
                        Node::DeleteOnProvisionerDeletion,
                    ),
            )
    }

    async fn before_all(&mut self) -> NodeResult {
        let env = &self.env;
        if env.testing_on_ocp {
            return skip(
                "DPUServiceTemplate tests require Kind — cannot safely stub ClusterVersion status on a live OCP cluster",
            );
        }

        by("creating stub resources for DPUServiceTemplate controller");
        create_pull_secret_stub(env).await?;
        create_cluster_version_stub(env, STUB_OCP_VERSION, STUB_RELEASE_IMAGE).await?;
        create_ovn_daemon_set_stub(env, STUB_OVN_IMAGE).await?;
        create_dummy_secrets(env, CI_NAMESPACE, &[SSH_KEY_SECRET_NAME, PULL_SECRET_NAME]).await?;

        by("creating DPUCluster namespace and stub");
        create_namespace(env, DPU_CLUSTER_NS).await?;
        create_dpu_cluster_stub(env, DPU_CLUSTER_NS, DPU_CLUSTER_NAME).await?;

        by("creating DPUFlavor stub");
        create_dpu_flavor_stub(env, DPU_CLUSTER_NS, DPU_FLAVOR_NAME).await?;

        by("creating DPUDeployment stub");
        create_dpu_deployment_stub(env, DPU_CLUSTER_NS, DPU_DEPLOYMENT_NAME, DPU_FLAVOR_NAME)
            .await?;

        by("creating DPFOperatorConfig with version");
        set_dpf_operator_config_version(env, STUB_DPF_VERSION).await?;

        by("enabling DPUServiceTemplate management");
        enable_manage_dpu_service_templates(env).await?;

        by("creating DPFHCPProvisioner to trigger template creation");
        create_dpfhcp_provisioner(env, CI_NAMESPACE, PROVISIONER_NAME).await
    }

    async fn after_all(&mut self) -> NodeResult {
        by("cleaning up DPUServiceTemplate test resources");
        cleanup_template_test_resources(&self.env).await
    }

    async fn get_template(
        &self,
        name: &str,
    ) -> ctrlrt::Result<dpuservicev1alpha1::DPUServiceTemplate> {
        self.env
            .client
            .get::<dpuservicev1alpha1::DPUServiceTemplate>(&ObjectKey::new(DPU_CLUSTER_NS, name))
            .await
    }

    fn managed_label(template: &dpuservicev1alpha1::DPUServiceTemplate) -> String {
        template
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get(MANAGED_BY_LABEL))
            .cloned()
            .unwrap_or_default()
    }

    async fn list_managed_templates(
        &self,
    ) -> ctrlrt::Result<Vec<dpuservicev1alpha1::DPUServiceTemplate>> {
        self.env
            .client
            .list::<dpuservicev1alpha1::DPUServiceTemplate>(
                &ListOptions::new()
                    .in_namespace(DPU_CLUSTER_NS)
                    .matching_labels([(MANAGED_BY_LABEL, "true")]),
            )
            .await
    }

    async fn create_all_templates(&mut self) -> NodeResult {
        for name in TEMPLATE_NAMES {
            by(&format!("waiting for template {name}"));
            eventually(
                GENERAL_TEMPLATE_RECONCILE_TIMEOUT,
                GENERAL_TEMPLATE_RECONCILE_INTERVAL,
                "",
                async || {
                    let template = g::ok(
                        self.get_template(name).await,
                        &format!("Template {name} not found"),
                    )?;
                    g::equal(
                        &Self::managed_label(&template),
                        "true",
                        &format!("Template {name} should have managed-by label"),
                    )?;
                    Ok(())
                },
            )
            .await?;
        }
        Ok(())
    }

    async fn template_structure(&mut self) -> NodeResult {
        for name in TEMPLATE_NAMES {
            let template = g::ok(self.get_template(name).await, "")?;

            by(&format!("verifying {name} has chart source"));
            let source = &template.spec.helm_chart.source;
            g::not_empty(
                &source.repo_url,
                &format!("Template {name} should have repoURL"),
            )?;
            g::not_empty(
                &source.chart,
                &format!("Template {name} should have chart name"),
            )?;
            g::not_empty(
                &source.version,
                &format!("Template {name} should have chart version"),
            )?;

            by(&format!("verifying {name} has values"));
            let values = &template.spec.helm_chart.values;
            g::not_nil(values, &format!("Template {name} should have values"))?;
            let raw = values.as_ref().map(|v| v.to_bytes()).unwrap_or_default();
            g::succeed(
                &gocompat::json::unmarshal_object(&raw),
                &format!("Template {name} values should be valid JSON"),
            )?;
        }
        Ok(())
    }

    async fn self_heal_deleted(&mut self) -> NodeResult {
        let template_to_delete = "hbn";

        by(&format!("deleting the {template_to_delete} template"));
        let template = g::ok(self.get_template(template_to_delete).await, "")?;
        let original_uid = template.metadata.uid.clone().unwrap_or_default();
        g::succeed(&self.env.client.delete(&template).await, "")?;

        by("waiting for template to be recreated");
        eventually(
            GENERAL_TEMPLATE_RECONCILE_TIMEOUT,
            GENERAL_TEMPLATE_RECONCILE_INTERVAL,
            "",
            async || {
                let template = g::ok(self.get_template(template_to_delete).await, "")?;
                g::not_equal(
                    &template.metadata.uid.clone().unwrap_or_default(),
                    &original_uid,
                    "Template should be a newly recreated object",
                )?;
                g::equal(&Self::managed_label(&template), "true", "")?;
                Ok(())
            },
        )
        .await
    }

    async fn self_heal_modified(&mut self) -> NodeResult {
        let template_to_modify = "doca-telemetry-service";

        by("reading current template");
        let mut template = g::ok(self.get_template(template_to_modify).await, "")?;
        let original_version = template.spec.helm_chart.source.version.clone();

        by("modifying the template chart version");
        template.spec.helm_chart.source.version = "0.0.0-tampered".into();
        g::succeed(&self.env.client.update(&mut template).await, "")?;

        by("waiting for template to be corrected");
        eventually(
            GENERAL_TEMPLATE_RECONCILE_TIMEOUT,
            GENERAL_TEMPLATE_RECONCILE_INTERVAL,
            "",
            async || {
                let template = g::ok(self.get_template(template_to_modify).await, "")?;
                g::equal(
                    &template.spec.helm_chart.source.version,
                    &original_version,
                    "Chart version should be restored",
                )?;
                Ok(())
            },
        )
        .await
    }

    async fn delete_on_deployment_deletion(&mut self) -> NodeResult {
        by("deleting the DPUDeployment while the provisioner still exists");
        let dd = g::ok(
            self.env
                .client
                .get::<dpuservicev1alpha1::DPUDeployment>(&ObjectKey::new(
                    DPU_CLUSTER_NS,
                    DPU_DEPLOYMENT_NAME,
                ))
                .await,
            "",
        )?;
        g::succeed(&self.env.client.delete(&dd).await, "")?;

        by("verifying all managed templates are deleted");
        eventually(
            GENERAL_TEMPLATE_RECONCILE_TIMEOUT,
            GENERAL_TEMPLATE_RECONCILE_INTERVAL,
            "",
            async || {
                let list = g::ok(self.list_managed_templates().await, "")?;
                g::be_empty(&list, "All managed templates should be deleted")?;
                Ok(())
            },
        )
        .await?;

        by("verifying templates are not recreated while the provisioner still exists");
        consistently(
            Duration::from_secs(20),
            Duration::from_secs(5),
            "",
            async || {
                for name in TEMPLATE_NAMES {
                    g::have_occurred(&self.get_template(name).await, "")?;
                }
                Ok(())
            },
        )
        .await
    }

    async fn delete_on_provisioner_deletion(&mut self) -> NodeResult {
        let env = &self.env;
        by("recreating DPUDeployment so templates exist again");
        create_dpu_deployment_stub(env, DPU_CLUSTER_NS, DPU_DEPLOYMENT_NAME, DPU_FLAVOR_NAME)
            .await?;

        by("waiting for templates to be recreated");
        for name in TEMPLATE_NAMES {
            eventually(
                GENERAL_TEMPLATE_RECONCILE_TIMEOUT,
                GENERAL_TEMPLATE_RECONCILE_INTERVAL,
                "",
                async || {
                    g::succeed(
                        &self.get_template(name).await,
                        &format!("Template {name} should exist before provisioner deletion"),
                    )?;
                    Ok(())
                },
            )
            .await?;
        }

        by("deleting the DPFHCPProvisioner");
        force_delete_provisioner(env, CI_NAMESPACE, PROVISIONER_NAME).await?;

        by("verifying all templates are deleted");
        eventually(
            GENERAL_TEMPLATE_RECONCILE_TIMEOUT,
            GENERAL_TEMPLATE_RECONCILE_INTERVAL,
            "",
            async || {
                let list = g::ok(self.list_managed_templates().await, "")?;
                g::be_empty(&list, "All managed templates should be deleted")?;
                Ok(())
            },
        )
        .await
    }
}

impl Container for DpuServiceTemplateE2e {
    type Node = Node;

    fn tree(&self) -> Tree<Node> {
        Self::spec_tree()
    }

    async fn run(&mut self, node: Node, _spec: &SpecReport) -> NodeResult {
        match node {
            Node::BeforeAll => self.before_all().await,
            Node::AfterAll => self.after_all().await,
            Node::CreateAllTemplates => self.create_all_templates().await,
            Node::TemplateStructure => self.template_structure().await,
            Node::SelfHealDeleted => self.self_heal_deleted().await,
            Node::SelfHealModified => self.self_heal_modified().await,
            Node::DeleteOnDeploymentDeletion => self.delete_on_deployment_deletion().await,
            Node::DeleteOnProvisionerDeletion => self.delete_on_provisioner_deletion().await,
        }
    }
}
