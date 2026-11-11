//! Port of `test/e2e/e2e_suite_test.go`: the suite entry point (`TestE2E`), `BeforeSuite`,
//! `AfterSuite` and the image reference helpers.
//!
//! The suite talks to a real cluster and is `#[ignore]`d so a plain `cargo test` never runs it
//! (the Go suite is likewise excluded from `make test`). Run it with
//! `cargo test -p e2e --test e2e_suite -- --ignored --nocapture`; see `crates/e2e/README.md`.

#![allow(clippy::too_many_arguments)]

mod ci;
mod ci_helpers;
mod dpuservicetemplate;
mod dpuservicetemplate_helpers;
mod fake_ocp_on_kind;

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use ctrlrt::{Client, ListOptions};
use ctrlrt_runtime::{KubeBackend, RestConfig};
use e2e::ginkgo::{self, Config, NodeResult, Runner, by};
use e2e::gomega::{self as g, eventually};
use e2e::{gw, utils};
use k8s_openapi::api::core::v1::{Namespace, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use ci_helpers::DPF_OPERATOR_CONFIG_NAMESPACE;

/// The suite globals of the Go package (`k8sClient`, `k8sConfig`, `testingOnOCP`).
#[derive(Clone)]
pub struct Env {
    /// `k8sClient`
    pub client: Client,
    /// The backend behind `client`, for requests the typed facade lacks (`DeleteAllOf`).
    pub backend: KubeBackend,
    /// `k8sConfig`
    pub config: RestConfig,
    /// `testingOnOCP`
    pub testing_on_ocp: bool,
}

impl Env {
    /// `client.New(config, client.Options{Scheme: runtimeScheme})`
    pub fn new(config: RestConfig, testing_on_ocp: bool) -> ctrlrt::Result<Self> {
        let kc = config.build_client()?;
        let backend = KubeBackend::new(kc);
        let client = Client::new(Arc::new(backend.clone()));
        Ok(Self {
            client,
            backend,
            config,
            testing_on_ocp,
        })
    }
}

/// `imageRepository`: extracts the repository from a full image reference.
/// Supports both tag format ("quay.io/org/image:tag") and digest format ("quay.io/org/image@sha256:...")
/// e.g. "quay.io/org/image:tag" -> "quay.io/org/image"
/// e.g. "quay.io/org/image@sha256:abc" -> "quay.io/org/image"
pub fn image_repository(image: &str) -> &str {
    // Handle digest format: registry.io/repo/image@<algo>:<hash> -> registry.io/repo/image
    if let Some(idx) = image.find('@').filter(|&i| i > 0) {
        return &image[..idx];
    }

    // Handle tag format: registry.io/repo/image:tag -> registry.io/repo/image
    if let Some(i) = image.rfind(':').filter(|&i| i > 0) {
        let after_colon = &image[i + 1..];
        if !after_colon.contains('/') {
            return &image[..i];
        }
    }
    image
}

/// `imageTag`: extracts the tag from a full image reference.
/// e.g. "quay.io/org/image:tag" -> "tag"
/// Returns empty string for digest-based images.
pub fn image_tag(image: &str) -> &str {
    // Digest format: return empty (digest will be used instead)
    if image.contains('@') {
        return "";
    }

    // Tag format: extract tag
    if let Some(i) = image.rfind(':').filter(|&i| i > 0) {
        let after_colon = &image[i + 1..];
        if !after_colon.contains('/') {
            return after_colon;
        }
    }
    "latest"
}

/// `imageDigest`: extracts the digest from a full image reference.
/// e.g. "quay.io/org/image@sha256:abc123..." -> "sha256:abc123..."
/// Returns empty string for tag-based images.
pub fn image_digest(image: &str) -> &str {
    // Extract digest: registry.io/repo/image@sha256:abc -> sha256:abc
    if let Some(idx) = image.find('@').filter(|&i| i > 0) {
        return &image[idx + 1..];
    }
    ""
}

/// The helm arguments `BeforeSuite` uses to deploy the operator.
pub fn helm_deploy_args(
    operator_image: &str,
    operator_chart: &str,
    testing_on_ocp: bool,
) -> Vec<String> {
    let mut helm_args: Vec<String> = [
        "upgrade",
        "--install",
        "dpf-hcp-provisioner-operator",
        operator_chart,
        "--create-namespace",
        "--namespace",
        "dpf-hcp-provisioner-system",
        "--set",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    helm_args.push(format!(
        "image.repository={}",
        image_repository(operator_image)
    ));

    // Use digest if available (CI), otherwise use tag (local dev)
    let digest = image_digest(operator_image);
    if !digest.is_empty() {
        helm_args.push("--set".into());
        helm_args.push(format!("image.digest={digest}"));
    } else {
        helm_args.push("--set".into());
        helm_args.push(format!("image.tag={}", image_tag(operator_image)));
    }

    helm_args.extend(
        [
            "--set",
            "logLevel=debug",
            "--set",
            "provisionerConfig.disableMetalLB=true",
            "--wait",
            "--timeout",
            "5m",
        ]
        .iter()
        .map(|s| s.to_string()),
    );

    // On kind, nodes have control-plane role, not master. Remove nodeSelector.
    if !testing_on_ocp {
        helm_args.push("--set".into());
        helm_args.push("placement.target=custom".into());
    }
    helm_args
}

/// `BeforeSuite`
async fn before_suite() -> NodeResult<Env> {
    let testing_on_ocp = std::env::var("KIND_ONLY_TESTS").unwrap_or_default() != "true";

    by("initializing Kubernetes client");
    let config = g::ok(
        ctrlrt_runtime::get_config("").await,
        "Failed to get kubeconfig",
    )?;

    // The Go suite registers every API group in a runtime.Scheme here; Rust types carry their
    // GVK statically, so there is nothing to register.
    let env = g::ok(
        Env::new(config, testing_on_ocp),
        "Failed to create controller-runtime client",
    )?;

    if testing_on_ocp {
        by("deploying HyperShift operator on management cluster");
        let mut cmd = Command::new("make");
        cmd.arg("e2e-deploy-hypershift");
        g::succeed(&utils::run(cmd), "Failed to deploy HyperShift operator")?;
    }

    by("installing external CRDs");
    let mut cmd = Command::new("make");
    cmd.arg("apply-crds-for-e2e");
    g::succeed(&utils::run(cmd), "Failed to install external CRDs")?;

    // Determine operator image and chart.
    // In CI, IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI is injected automatically via the
    // workflow dependencies block. For local testing, set it manually:
    //   export IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI=quay.io/your-org/your-image:tag
    let operator_image = std::env::var("IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI").unwrap_or_default();
    g::not_empty(
        &operator_image,
        "IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI must be set. \
         In CI this is injected automatically. \
         For local testing, export it with your operator image.",
    )?;

    let mut operator_chart = std::env::var("OPERATOR_HELM_CHART").unwrap_or_default();
    if operator_chart.is_empty() {
        operator_chart = "helm/dpf-hcp-provisioner-operator".into();
    }
    gw!("Using operator image: {}\n", operator_image);
    gw!("Using helm chart: {}\n", operator_chart);

    by("uninstalling any previous operator helm release");
    // Uninstall previous helm release (if exists) to ensure clean slate.
    let mut uninstall_cmd = Command::new("helm");
    uninstall_cmd.args([
        "uninstall",
        "dpf-hcp-provisioner-operator",
        "--namespace",
        "dpf-hcp-provisioner-system",
        "--ignore-not-found",
        "--timeout=2m",
    ]);
    let _ = utils::run(uninstall_cmd); // Ignore errors - might not exist on first run

    // Wait for operator pods to be fully terminated
    by("waiting for operator pods to terminate");
    eventually(
        Duration::from_secs(30),
        Duration::from_secs(1),
        "",
        async || {
            let pod_list = env
                .client
                .list::<Pod>(
                    &ListOptions::new()
                        .in_namespace("dpf-hcp-provisioner-system")
                        .matching_labels([(
                            "app.kubernetes.io/name",
                            "dpf-hcp-provisioner-operator",
                        )]),
                )
                .await;

            // not found means cleanup completed
            if let Err(e) = &pod_list
                && ctrlrt::error::is_not_found(e)
            {
                return Ok(());
            }
            g::succeed(&pod_list, "Failed to list operator pods")?;
            g::be_empty(
                &pod_list.unwrap_or_default(),
                "Operator pods should be terminated",
            )?;
            Ok(())
        },
    )
    .await?;

    by("ensuring namespaces required by helm chart and tests exist");
    // The helm chart creates a Role in openshift-config — on OCP this namespace
    // exists, on kind we need to create it.
    for ns in ["openshift-config", DPF_OPERATOR_CONFIG_NAMESPACE] {
        let mut ns_obj = Namespace {
            metadata: ObjectMeta {
                name: Some(ns.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        if let Err(e) = env.client.create(&mut ns_obj).await
            && !ctrlrt::error::is_already_exists(&e)
        {
            g::ok(Err::<(), _>(e), &format!("Failed to create namespace {ns}"))?;
        }
    }

    by("deploying the operator via helm chart");
    let helm_args = helm_deploy_args(&operator_image, &operator_chart, testing_on_ocp);
    let mut cmd = Command::new("helm");
    cmd.args(&helm_args);
    g::succeed(&utils::run(cmd), "Failed to deploy operator via helm")?;

    Ok(env)
}

/// `AfterSuite`
async fn after_suite() -> NodeResult {
    // Don't undeploy the operator - leave resources for must-gather collection.
    // The AWS cluster will be deprovisioned anyway, so cleanup is not needed.
    // Resources are cleaned up between individual tests in AfterEach/AfterAll hooks.
    by("skipping operator cleanup to preserve resources for must-gather");
    Ok(())
}

/// `TestE2E`: `RunSpecs(t, "e2e suite")`.
///
/// Top-level containers run in declaration order (`e2e_ci_test.go`, then
/// `e2e_dpuservicetemplate_test.go`), i.e. Ginkgo's order without randomization.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "e2e suite: requires a cluster (KUBECONFIG, IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI, ...); run with --ignored"]
async fn test_e2e() {
    let config = Config::from_env().unwrap_or_else(|e| panic!("invalid e2e configuration: {e}"));
    gw!("Starting dpf-hcp-provisioner-operator e2e test suite\n");
    ginkgo::write(&format!(
        "Running Suite: e2e suite - {}\n",
        utils::get_project_dir().unwrap_or_else(|(wd, _)| wd)
    ));
    let mut runner = Runner::new("e2e suite", config);

    let ci_tree = ci::DpfhcpProvisionerE2e::spec_tree();
    let dst_tree = dpuservicetemplate::DpuServiceTemplateE2e::spec_tree();
    let any = runner.any_selected(&ci_tree) || runner.any_selected(&dst_tree);

    if any {
        match runner.run_suite_node("BeforeSuite", before_suite()).await {
            Some(env) => {
                let mut ci = ci::DpfhcpProvisionerE2e::new(env.clone());
                runner.run_container(&mut ci).await;
                let mut dst = dpuservicetemplate::DpuServiceTemplateE2e::new(env);
                runner.run_container(&mut dst).await;
            }
            None => {
                runner.skip_container(&ci_tree);
                runner.skip_container(&dst_tree);
            }
        }
        runner.run_suite_node("AfterSuite", after_suite()).await;
    } else {
        runner.skip_container(&ci_tree);
        runner.skip_container(&dst_tree);
    }

    if let Err(e) = runner.finish() {
        panic!("{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_reference_helpers() {
        let cases = [
            // (image, repository, tag, digest)
            ("quay.io/org/image:tag", "quay.io/org/image", "tag", ""),
            (
                "quay.io/org/image@sha256:abc",
                "quay.io/org/image",
                "",
                "sha256:abc",
            ),
            ("quay.io/org/image", "quay.io/org/image", "latest", ""),
            (
                "localhost:5000/org/image",
                "localhost:5000/org/image",
                "latest",
                "",
            ),
            (
                "localhost:5000/org/image:dev",
                "localhost:5000/org/image",
                "dev",
                "",
            ),
            (
                "localhost/dpf-hcp-provisioner:dev",
                "localhost/dpf-hcp-provisioner",
                "dev",
                "",
            ),
            ("@sha256:abc", "@sha256", "", ""),
            (":tag", ":tag", "latest", ""),
        ];
        for (image, repo, tag, digest) in cases {
            assert_eq!(image_repository(image), repo, "repository of {image}");
            assert_eq!(image_tag(image), tag, "tag of {image}");
            assert_eq!(image_digest(image), digest, "digest of {image}");
        }
    }

    #[test]
    fn helm_args() {
        let args = helm_deploy_args(
            "quay.io/org/op@sha256:abc",
            "helm/dpf-hcp-provisioner-operator",
            true,
        );
        assert_eq!(
            args.join(" "),
            "upgrade --install dpf-hcp-provisioner-operator helm/dpf-hcp-provisioner-operator \
             --create-namespace --namespace dpf-hcp-provisioner-system \
             --set image.repository=quay.io/org/op --set image.digest=sha256:abc \
             --set logLevel=debug --set provisionerConfig.disableMetalLB=true --wait --timeout 5m"
        );
        let args = helm_deploy_args("localhost/op:dev", "chart", false);
        assert_eq!(
            args.join(" "),
            "upgrade --install dpf-hcp-provisioner-operator chart \
             --create-namespace --namespace dpf-hcp-provisioner-system \
             --set image.repository=localhost/op --set image.tag=dev \
             --set logLevel=debug --set provisionerConfig.disableMetalLB=true --wait --timeout 5m \
             --set placement.target=custom"
        );
    }
}
