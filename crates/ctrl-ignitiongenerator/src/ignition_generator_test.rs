//! Port of internal/controller/ignitiongenerator/ignition_generator_test.go

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::dpf::operator::v1alpha1 as operatorv1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::hypershift::v1beta1 as hyperv1;
use apis::k8s::core::v1::ObjectReference;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::k8s_openapi::ByteString;
use ctrlrt::k8s_openapi::api::core::v1::{ConfigMap, Secret};
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use ctrlrt::{Client, Ctx, FakeRecorder, ObjectKey, Recorder, meta};
use ctrlrt_fake::{FakeClientBuilder, InitObject, obj, obj_mut};
use ignition::types::{Config, File, Resource, User};

use crate::ignition_generator::*;
use crate::testutil::{fake_recorder, new_test_scheme};

/// Helper to build a standard test CR
pub(crate) fn new_test_provisioner() -> provisioningv1alpha1::DPFHCPProvisioner {
    provisioningv1alpha1::DPFHCPProvisioner {
        metadata: ObjectMeta {
            name: Some("doca".into()),
            namespace: Some("clusters".into()),
            generation: Some(1),
            ..Default::default()
        },
        spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
            dpu_cluster_ref: provisioningv1alpha1::DPUClusterReference {
                name: "dpucluster-1".into(),
                namespace: "dpf-operator-system".into(),
                ..Default::default()
            },
            dpu_deployment_ref: Some(provisioningv1alpha1::DPUDeploymentReference {
                name: "my-deployment".into(),
                namespace: "dpf-operator-system".into(),
                ..Default::default()
            }),
            machine_osurl: "https://example.com/rhcos-image".into(),
            ..Default::default()
        },
        status: provisioningv1alpha1::DPFHCPProvisionerStatus {
            hosted_cluster_ref: Some(ObjectReference {
                name: "doca".into(),
                namespace: "clusters".into(),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Helper to build a DPUDeployment matching the test provisioner's DPUDeploymentRef
pub(crate) fn new_test_dpu_deployment(
    cr: &provisioningv1alpha1::DPFHCPProvisioner,
) -> dpuservicev1alpha1::DPUDeployment {
    let r = cr.spec.dpu_deployment_ref.as_ref().unwrap();
    dpuservicev1alpha1::DPUDeployment {
        metadata: ObjectMeta {
            name: Some(r.name.clone()),
            namespace: Some(r.namespace.clone()),
            ..Default::default()
        },
        spec: dpuservicev1alpha1::DPUDeploymentSpec {
            dpus: dpuservicev1alpha1::DPUs {
                bfb: Some("test-bfb".into()),
                flavor: Some("test-flavor".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

// controlPlaneNamespace for the test CR
pub(crate) const TEST_CP_NAMESPACE: &str = "clusters-doca";
pub(crate) const TEST_IGNITION_VERSION: &str = "3.4.0";

/// `&IgnitionGenerator{}` (no client needed).
fn bare_generator() -> IgnitionGenerator {
    IgnitionGenerator::new(
        FakeClientBuilder::new().build(),
        Arc::new(FakeRecorder::discarding()) as Recorder,
    )
}

/// `&IgnitionGenerator{Client: fakeClient}`
fn generator(c: &Client) -> IgnitionGenerator {
    IgnitionGenerator::new(c.clone(), Arc::new(FakeRecorder::discarding()) as Recorder)
}

fn fake_client(objs: Vec<InitObject>) -> Client {
    FakeClientBuilder::new()
        .with_scheme(new_test_scheme())
        .with_objects(objs)
        .build()
}

fn secret(name: &str, ns: &str, data: &[(&str, &[u8])]) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(name.into()),
            namespace: Some(ns.into()),
            ..Default::default()
        },
        data: Some(
            data.iter()
                .map(|(k, v)| (k.to_string(), ByteString(v.to_vec())))
                .collect(),
        ),
        ..Default::default()
    }
}

fn deployment_with_flavor(flavor: &str) -> dpuservicev1alpha1::DPUDeployment {
    dpuservicev1alpha1::DPUDeployment {
        metadata: ObjectMeta {
            name: Some("my-deployment".into()),
            namespace: Some("dpf-operator-system".into()),
            ..Default::default()
        },
        spec: dpuservicev1alpha1::DPUDeploymentSpec {
            dpus: dpuservicev1alpha1::DPUs {
                flavor: Some(flavor.into()),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

fn flavor_with_ovs(script: &str) -> dpuprovisioningv1alpha1::DPUFlavor {
    let mut f = dpuprovisioningv1alpha1::DPUFlavor::default();
    f.spec.ovs.raw_config_script = script.into();
    f
}

fn zt_flavor_with_ovs(script: &str) -> dpuprovisioningv1alpha1::DPUFlavor {
    let mut f = flavor_with_ovs(script);
    f.spec.dpu_mode = dpuprovisioningv1alpha1::ZERO_TRUST_MODE.into();
    f
}

// ---------------------------------------------------------------------------------------------
// Describe("controlPlaneNamespace")
// ---------------------------------------------------------------------------------------------

#[test]
fn control_plane_namespace_should_return_namespace_name_format() {
    let cr = new_test_provisioner();
    let ig = bare_generator();
    assert_eq!(ig.control_plane_namespace(&cr).unwrap(), "clusters-doca");
}

// ---------------------------------------------------------------------------------------------
// Describe("getIgnitionCACert")
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn get_ignition_ca_cert_should_return_ca_cert_when_secret_exists_with_tls_crt_key() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let s = secret(
        IGNITION_SECRET_NAME,
        TEST_CP_NAMESPACE,
        &[("tls.crt", b"fake-ca-cert")],
    );
    let ig = generator(&fake_client(vec![obj(&s)]));

    let cert = ig.get_ignition_ca_cert(&ctx, &cr).await.unwrap();
    assert_eq!(cert, b"fake-ca-cert".to_vec());
}

#[tokio::test]
async fn get_ignition_ca_cert_should_return_error_when_secret_is_not_found() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let ig = generator(&fake_client(vec![]));

    let err = ig.get_ignition_ca_cert(&ctx, &cr).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("failed to get ignition CA cert secret")
    );
}

#[tokio::test]
async fn get_ignition_ca_cert_should_return_error_when_tls_crt_key_is_missing() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let s = secret(
        IGNITION_SECRET_NAME,
        TEST_CP_NAMESPACE,
        &[("wrong-key", b"data")],
    );
    let ig = generator(&fake_client(vec![obj(&s)]));

    let err = ig.get_ignition_ca_cert(&ctx, &cr).await.unwrap_err();
    assert!(err.to_string().contains("tls.crt not found"));
}

// ---------------------------------------------------------------------------------------------
// Describe("getIgnitionToken")
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn get_ignition_token_should_find_token_secret_by_prefix_and_return_base64_encoded_token() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let raw_token = b"my-secret-token";
    let s = secret(
        "token-doca-a82ec4af",
        TEST_CP_NAMESPACE,
        &[(IGNITION_TOKEN_KEY, raw_token)],
    );
    let ig = generator(&fake_client(vec![obj(&s)]));

    let tokens = ig.get_ignition_tokens(&ctx, &cr).await.unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0], gocompat::base64::encode_to_string(raw_token));
}

#[tokio::test]
async fn get_ignition_token_should_return_error_when_no_matching_secret_exists() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    // Secret with wrong prefix
    let s = secret(
        "other-secret",
        TEST_CP_NAMESPACE,
        &[(IGNITION_TOKEN_KEY, b"token")],
    );
    let ig = generator(&fake_client(vec![obj(&s)]));

    let err = ig.get_ignition_tokens(&ctx, &cr).await.unwrap_err();
    assert!(err.to_string().contains("no token secret with prefix"));
}

#[tokio::test]
async fn get_ignition_token_should_return_error_when_token_key_is_missing_from_all_matching_secrets()
 {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let s = secret(
        "token-doca-abc123",
        TEST_CP_NAMESPACE,
        &[("wrong-key", b"data")],
    );
    let ig = generator(&fake_client(vec![obj(&s)]));

    let err = ig.get_ignition_tokens(&ctx, &cr).await.unwrap_err();
    assert!(err.to_string().contains("no usable token data"));
}

#[tokio::test]
async fn get_ignition_token_should_return_all_matching_tokens_when_multiple_secrets_exist() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let secret1 = secret(
        "token-doca-first",
        TEST_CP_NAMESPACE,
        &[(IGNITION_TOKEN_KEY, b"first-token")],
    );
    let secret2 = secret(
        "token-doca-second",
        TEST_CP_NAMESPACE,
        &[(IGNITION_TOKEN_KEY, b"second-token")],
    );
    let ig = generator(&fake_client(vec![obj(&secret1), obj(&secret2)]));

    let tokens = ig.get_ignition_tokens(&ctx, &cr).await.unwrap();
    assert_eq!(tokens.len(), 2);
    // Both tokens should be present (base64-encoded)
    let mut all_decoded: Vec<String> = tokens
        .iter()
        .map(|t| String::from_utf8(gocompat::base64::decode_string(t.as_bytes()).unwrap()).unwrap())
        .collect();
    all_decoded.sort();
    assert_eq!(all_decoded, vec!["first-token", "second-token"]);
}

// ---------------------------------------------------------------------------------------------
// Describe("getIgnitionEndpoint")
// ---------------------------------------------------------------------------------------------

fn hosted_cluster(endpoint: &str) -> hyperv1::HostedCluster {
    let mut hc = hyperv1::HostedCluster {
        metadata: ObjectMeta {
            name: Some("doca".into()),
            namespace: Some("clusters".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    hc.status.ignition_endpoint = endpoint.into();
    hc
}

#[tokio::test]
async fn get_ignition_endpoint_should_return_the_ignition_endpoint_from_hosted_cluster_status() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let hc = hosted_cluster("ignition.example.com:443");
    let ig = generator(&fake_client(vec![obj(&hc)]));

    let endpoint = ig.get_ignition_endpoint(&ctx, &cr).await.unwrap();
    assert_eq!(endpoint, "ignition.example.com:443");
}

#[tokio::test]
async fn get_ignition_endpoint_should_return_error_when_hosted_cluster_is_not_found() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let ig = generator(&fake_client(vec![]));

    let err = ig.get_ignition_endpoint(&ctx, &cr).await.unwrap_err();
    assert!(err.to_string().contains("failed to get HostedCluster"));
}

#[tokio::test]
async fn get_ignition_endpoint_should_return_error_when_ignition_endpoint_is_empty() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let hc = hosted_cluster("");
    let ig = generator(&fake_client(vec![obj(&hc)]));

    let err = ig.get_ignition_endpoint(&ctx, &cr).await.unwrap_err();
    assert!(err.to_string().contains("ignition endpoint not available"));
}

// ---------------------------------------------------------------------------------------------
// Describe("retrieveDPUFlavor")
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn retrieve_dpu_flavor_should_return_dpu_flavor_when_dpu_deployment_and_dpu_flavor_exist() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let deployment = deployment_with_flavor("test-flavor");
    let mut flavor = flavor_with_ovs("#!/bin/bash\novs-vsctl add-br br0");
    flavor.metadata = ObjectMeta {
        name: Some("test-flavor".into()),
        namespace: Some("dpf-operator-system".into()),
        ..Default::default()
    };
    let ig = generator(&fake_client(vec![obj(&deployment), obj(&flavor)]));

    let result = ig.retrieve_dpu_flavor(&ctx, &cr).await.unwrap();
    assert_eq!(
        result.spec.ovs.raw_config_script,
        "#!/bin/bash\novs-vsctl add-br br0"
    );
}

#[tokio::test]
async fn retrieve_dpu_flavor_should_return_error_when_dpu_deployment_is_not_found() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let ig = generator(&fake_client(vec![]));

    let err = ig.retrieve_dpu_flavor(&ctx, &cr).await.unwrap_err();
    assert!(err.to_string().contains("failed to get DPUDeployment"));
}

#[tokio::test]
async fn retrieve_dpu_flavor_should_return_error_when_dpu_deployment_has_empty_flavor() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let deployment = deployment_with_flavor("");
    let ig = generator(&fake_client(vec![obj(&deployment)]));

    let err = ig.retrieve_dpu_flavor(&ctx, &cr).await.unwrap_err();
    assert!(err.to_string().contains("Flavor is empty"));
}

#[tokio::test]
async fn retrieve_dpu_flavor_should_return_error_when_dpu_flavor_is_not_found() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let deployment = deployment_with_flavor("nonexistent-flavor");
    let ig = generator(&fake_client(vec![obj(&deployment)]));

    let err = ig.retrieve_dpu_flavor(&ctx, &cr).await.unwrap_err();
    assert!(err.to_string().contains("failed to get DPUFlavor"));
}

// ---------------------------------------------------------------------------------------------
// Describe("GetSingletonDPFOperatorConfig")
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn get_singleton_dpf_operator_config_should_return_the_singleton_dpf_operator_config() {
    let mut config = operatorv1alpha1::DPFOperatorConfig {
        metadata: ObjectMeta {
            name: Some("dpfoperatorconfig".into()),
            namespace: Some("dpf-operator-system".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    config.spec.deployment_mode = operatorv1alpha1::DEPLOYMENT_MODE_HOST_TRUSTED.into();
    config.spec.provisioning_controller =
        Some(operatorv1alpha1::ProvisioningControllerConfiguration {
            install_interface: Some(operatorv1alpha1::ProvisioningInstallInterface {
                install_via_host_agent: Some(Default::default()),
                ..Default::default()
            }),
            ..Default::default()
        });
    config.spec.networking = Some(operatorv1alpha1::Networking {
        control_plane_mtu: Some(9000),
        ..Default::default()
    });
    let c = fake_client(vec![obj(&config)]);

    let result = common::config::get_singleton_dpf_operator_config(&c)
        .await
        .unwrap();
    assert_eq!(
        result.spec.networking.unwrap().control_plane_mtu.unwrap(),
        9000
    );
}

#[tokio::test]
async fn get_singleton_dpf_operator_config_should_return_error_when_no_dpf_operator_config_exists()
{
    let c = fake_client(vec![]);

    let err = common::config::get_singleton_dpf_operator_config(&c)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("expected exactly one DPFOperatorConfig")
    );
}

// ---------------------------------------------------------------------------------------------
// Describe("buildTargetIgnition")
// ---------------------------------------------------------------------------------------------

/// The encapsulated machine config file with `spec.osImageURL`.
pub(crate) fn machine_config_file(os_image_url: &str) -> File {
    let machine_config = serde_json::json!({"spec": {"osImageURL": os_image_url}});
    let mc_json = gocompat::json::marshal(&machine_config);
    let encoded = gocompat::url::query_escape(&mc_json);
    let source = format!("data:,{encoded}");
    let mut f = File::new("/etc/ignition-machine-config-encapsulated.json".into());
    f.contents = Some(Resource {
        source: Some(source),
        ..Default::default()
    });
    f
}

/// Helper to build minimal valid HCP ignition JSON with machine config
fn build_hcp_ignition_json_version(version: &str) -> Vec<u8> {
    let mut ign = ignition::new_empty_ignition(version);
    ignition::files_mut(&mut ign).push(machine_config_file("https://old-image.example.com"));
    ignition::marshal_config(&ign)
}

fn build_hcp_ignition_json() -> Vec<u8> {
    build_hcp_ignition_json_version("3.4.0")
}

fn has_file(cfg: &Config, path: &str) -> bool {
    ignition::files(cfg).iter().any(|f| f.path == path)
}

fn has_unit(cfg: &Config, name: &str) -> bool {
    ignition::units(cfg).iter().any(|u| u.name == name)
}

#[test]
fn build_target_ignition_should_produce_a_valid_target_ignition_with_dpf_files() {
    let ig = bare_generator();
    let hcp_json = build_hcp_ignition_json();
    let flavor = flavor_with_ovs("#!/bin/bash\necho ovs");

    let result = ig
        .build_target_ignition(
            &hcp_json,
            &flavor,
            "https://new-image.example.com",
            1500,
            false,
            "",
        )
        .unwrap();

    // Should have the original file + target files + common files + OVS script
    assert!(ignition::files(&result).len() > 1);

    // Verify OVS script was added
    assert!(has_file(&result, "/usr/local/bin/dpf-ovs-script.sh"));

    let dpu_agent_unit = ignition::units(&result)
        .iter()
        .find(|u| u.name == "dpu-agent.service")
        .expect("dpu-agent.service");
    assert!(
        dpu_agent_unit
            .contents
            .as_deref()
            .unwrap()
            .contains("--control-plane-mtu $ControlPlaneMTU")
    );
}

#[test]
fn build_target_ignition_should_preserve_hcp_ignition_version_from_upstream() {
    // HCP ignition may come with older spec versions (e.g. 3.2.0).
    // buildTargetIgnition preserves the original version — validation
    // handles version normalization internally.
    let ig = bare_generator();
    let data = build_hcp_ignition_json_version("3.2.0");
    let flavor = dpuprovisioningv1alpha1::DPUFlavor::default();

    let result = ig
        .build_target_ignition(
            &data,
            &flavor,
            "https://new-image.example.com",
            1500,
            false,
            "",
        )
        .unwrap();
    assert_eq!(result.ignition.version, "3.2.0");
}

#[test]
fn build_target_ignition_should_return_error_for_invalid_json() {
    let ig = bare_generator();
    let flavor = dpuprovisioningv1alpha1::DPUFlavor::default();
    let err = ig
        .build_target_ignition(b"not-json", &flavor, "https://example.com", 1500, false, "")
        .unwrap_err();
    assert!(err.to_string().contains("failed to parse HCP ignition"));
}

fn is_mtu_nm_connection(path: &str) -> bool {
    ["p0", "p1", "pf0hpf", "pf1hpf"]
        .iter()
        .any(|i| path == format!("/etc/NetworkManager/system-connections/{i}.nmconnection"))
}

#[test]
fn build_target_ignition_should_skip_mtu_configuration_when_mtu_is_1500() {
    let ig = bare_generator();
    let hcp_json = build_hcp_ignition_json();
    let flavor = dpuprovisioningv1alpha1::DPUFlavor::default();

    let result = ig
        .build_target_ignition(&hcp_json, &flavor, "https://example.com", 1500, false, "")
        .unwrap();

    // No p0/p1/pf0hpf/pf1hpf NM connections should be added
    for f in ignition::files(&result) {
        assert!(!is_mtu_nm_connection(&f.path), "{}", f.path);
    }
}

#[test]
fn build_target_ignition_should_apply_mtu_configuration_when_mtu_is_not_1500() {
    let ig = bare_generator();
    let hcp_json = build_hcp_ignition_json();
    let flavor = dpuprovisioningv1alpha1::DPUFlavor::default();

    let result = ig
        .build_target_ignition(&hcp_json, &flavor, "https://example.com", 9000, false, "")
        .unwrap();

    // Should have p0 NM connection with MTU
    assert!(has_file(
        &result,
        "/etc/NetworkManager/system-connections/p0.nmconnection"
    ));
}

#[test]
fn build_target_ignition_should_exclude_pf_monitor_service_from_systemd_units_in_zero_trust_mode() {
    let ig = bare_generator();
    let hcp_json = build_hcp_ignition_json();
    let flavor = zt_flavor_with_ovs("#!/bin/bash\necho ovs");

    let result = ig
        .build_target_ignition(
            &hcp_json,
            &flavor,
            "https://new-image.example.com",
            1500,
            true,
            "",
        )
        .unwrap();

    for u in ignition::units(&result) {
        assert_ne!(u.name, "pf-monitor.service");
        assert_ne!(u.name, "report-machineosurl.service");
    }
}

#[test]
fn build_target_ignition_should_exclude_pf_monitor_sh_file_in_zero_trust_mode() {
    let ig = bare_generator();
    let hcp_json = build_hcp_ignition_json();
    let flavor = zt_flavor_with_ovs("#!/bin/bash\necho ovs");

    let result = ig
        .build_target_ignition(
            &hcp_json,
            &flavor,
            "https://new-image.example.com",
            1500,
            true,
            "",
        )
        .unwrap();

    for f in ignition::files(&result) {
        assert_ne!(f.path, "/usr/local/bin/pf-monitor.sh");
        assert_ne!(f.path, "/usr/local/bin/report-machineosurl.py");
    }
}

#[test]
fn build_target_ignition_should_include_pf_monitor_in_non_zero_trust_mode() {
    let ig = bare_generator();
    let hcp_json = build_hcp_ignition_json();
    let flavor = flavor_with_ovs("#!/bin/bash\necho ovs");

    let result = ig
        .build_target_ignition(
            &hcp_json,
            &flavor,
            "https://new-image.example.com",
            1500,
            false,
            "",
        )
        .unwrap();

    assert!(has_unit(&result, "pf-monitor.service"));
    assert!(has_file(&result, "/usr/local/bin/pf-monitor.sh"));
    assert!(has_unit(&result, "report-machineosurl.service"));
    assert!(has_file(&result, "/usr/local/bin/report-machineosurl.py"));
}

#[test]
fn build_target_ignition_should_keep_other_systemd_units_in_zero_trust_mode() {
    let ig = bare_generator();
    let hcp_json = build_hcp_ignition_json();
    let flavor = zt_flavor_with_ovs("#!/bin/bash\necho ovs");

    let result = ig
        .build_target_ignition(
            &hcp_json,
            &flavor,
            "https://new-image.example.com",
            1500,
            true,
            "",
        )
        .unwrap();

    assert!(has_unit(&result, "tmfifo-agent-link.service"));
    assert!(has_unit(&result, "setup-vfs-devlink.service"));
    assert!(has_unit(&result, "dpf-ovs.service"));
}

// ---------------------------------------------------------------------------------------------
// Describe("buildLiveIgnition")
// ---------------------------------------------------------------------------------------------

#[test]
fn build_live_ignition_should_produce_live_ignition_with_embedded_target_at_var_target_ign() {
    let ig = bare_generator();
    let target_ignition = ignition::new_empty_ignition("3.4.0");
    let hcp_json = ignition::marshal_config(&ignition::new_empty_ignition("3.4.0"));
    let flavor = flavor_with_ovs("#!/bin/bash\necho ovs");
    let result = ig
        .build_live_ignition(&target_ignition, &hcp_json, &flavor, false)
        .unwrap();
    assert_eq!(result.ignition.version, TEST_IGNITION_VERSION);

    // Find /var/target.ign
    let target_file = ignition::files(&result)
        .iter()
        .find(|f| f.path == "/var/target.ign")
        .expect("/var/target.ign");
    let contents = target_file.contents.as_ref().unwrap();
    assert!(
        contents
            .source
            .as_deref()
            .unwrap()
            .starts_with("data:;base64,")
    );
    assert_eq!(contents.compression.as_deref(), Some("gzip"));

    let env_file = ignition::files(&result)
        .iter()
        .find(|f| f.path == "/etc/dpf/environment")
        .expect("/etc/dpf/environment");
    assert!(
        ignition::file_source(env_file)
            .unwrap()
            .contains("ControlPlaneMTU={{.ControlPlaneMTU}}")
    );
}

#[test]
fn build_live_ignition_should_copy_passwd_from_hcp_ignition() {
    let ig = bare_generator();
    let target_ignition = ignition::new_empty_ignition("3.4.0");
    let mut hcp_ignition = ignition::new_empty_ignition("3.4.0");
    let mut core = User::new("core".into());
    core.ssh_authorized_keys = Some(vec!["ssh-rsa AAAA...".into()]);
    hcp_ignition
        .passwd
        .get_or_insert_with(Default::default)
        .users = Some(vec![core]);
    let hcp_json = ignition::marshal_config(&hcp_ignition);

    let flavor = flavor_with_ovs("#!/bin/bash\necho ovs");
    let result = ig
        .build_live_ignition(&target_ignition, &hcp_json, &flavor, false)
        .unwrap();
    assert_eq!(ignition::users(&result).len(), 1);
    assert_eq!(ignition::users(&result)[0].name, "core");
}

#[test]
fn build_live_ignition_should_work_when_hcp_ignition_has_no_passwd() {
    let ig = bare_generator();
    let target_ignition = ignition::new_empty_ignition("3.4.0");
    let hcp_json = ignition::marshal_config(&ignition::new_empty_ignition("3.4.0"));
    let flavor = flavor_with_ovs("#!/bin/bash\necho ovs");
    let result = ig
        .build_live_ignition(&target_ignition, &hcp_json, &flavor, false)
        .unwrap();
    assert!(ignition::users(&result).is_empty());
}

#[test]
fn build_live_ignition_should_return_error_for_invalid_hcp_json() {
    let ig = bare_generator();
    let target_ignition = ignition::new_empty_ignition("3.4.0");
    let flavor = dpuprovisioningv1alpha1::DPUFlavor::default();

    let err = ig
        .build_live_ignition(&target_ignition, b"invalid", &flavor, false)
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("failed to parse HCP ignition for passwd")
    );
}

// ---------------------------------------------------------------------------------------------
// Describe("createOrUpdateConfigMap")
// ---------------------------------------------------------------------------------------------

fn annotations(cm: &ConfigMap) -> BTreeMap<String, String> {
    cm.metadata.annotations.clone().unwrap_or_default()
}

fn labels(cm: &ConfigMap) -> BTreeMap<String, String> {
    cm.metadata.labels.clone().unwrap_or_default()
}

fn assert_kv(m: &BTreeMap<String, String>, k: &str, v: &str) {
    assert_eq!(m.get(k).map(String::as_str), Some(v), "{k}");
}

fn cm_key(cr: &provisioningv1alpha1::DPFHCPProvisioner) -> ObjectKey {
    ObjectKey::new(
        cr.spec.dpu_cluster_ref.namespace.clone(),
        config_map_name(&cr.spec.dpu_cluster_ref.name),
    )
}

fn assert_full_annotations(
    cm: &ConfigMap,
    cr: &provisioningv1alpha1::DPFHCPProvisioner,
    deployment: &dpuservicev1alpha1::DPUDeployment,
) {
    let a = annotations(cm);
    let dr = cr.spec.dpu_deployment_ref.as_ref().unwrap();
    assert_kv(
        &a,
        BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION,
        &cr.spec.dpu_cluster_ref.name,
    );
    assert_kv(
        &a,
        BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION,
        &cr.spec.dpu_cluster_ref.namespace,
    );
    assert_kv(
        &a,
        BFCFG_TEMPLATE_BFB_NAME_ANNOTATION,
        deployment.spec.dpus.bfb.as_deref().unwrap(),
    );
    assert_kv(&a, BFCFG_TEMPLATE_BFB_NAMESPACE_ANNOTATION, &dr.namespace);
    assert_kv(
        &a,
        BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION,
        deployment.spec.dpus.flavor.as_deref().unwrap(),
    );
    assert_kv(
        &a,
        BFCFG_TEMPLATE_DPU_FLAVOR_NAMESPACE_ANNOTATION,
        &dr.namespace,
    );
    assert_kv(
        &a,
        BFCFG_TEMPLATE_MACHINE_OS_URL_ANNOTATION,
        &cr.spec.machine_osurl,
    );
}

#[tokio::test]
async fn create_or_update_config_map_should_create_a_new_config_map_when_it_does_not_exist() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let deployment = new_test_dpu_deployment(&cr);
    let c = fake_client(vec![obj(&deployment)]);
    let (_, recorder) = fake_recorder();
    let ig = IgnitionGenerator::new(c.clone(), recorder);

    let live_ignition = ignition::new_empty_ignition(TEST_IGNITION_VERSION);
    ig.create_or_update_config_map(&ctx, &cr, &live_ignition, &cr.spec.machine_osurl)
        .await
        .unwrap();

    // Verify ConfigMap was created
    let cm: ConfigMap = c.get(&cm_key(&cr)).await.unwrap();
    assert!(cm.data.as_ref().unwrap().contains_key(CONFIG_MAP_KEY_NAME));

    // Verify labels
    assert_kv(&labels(&cm), BFCFG_TEMPLATE_LABEL, "true");

    // Verify annotations
    assert_full_annotations(&cm, &cr, &deployment);
}

#[tokio::test]
async fn create_or_update_config_map_should_update_an_existing_config_map() {
    let ctx = Ctx::background();
    let cr = new_test_provisioner();
    let cm_name = config_map_name(&cr.spec.dpu_cluster_ref.name);
    let existing_cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(cm_name.clone()),
            namespace: Some(cr.spec.dpu_cluster_ref.namespace.clone()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            CONFIG_MAP_KEY_NAME.to_string(),
            "old-data".to_string(),
        )])),
        ..Default::default()
    };
    let deployment = new_test_dpu_deployment(&cr);
    let c = fake_client(vec![obj(&existing_cm), obj(&deployment)]);
    let (_, recorder) = fake_recorder();
    let ig = IgnitionGenerator::new(c.clone(), recorder);

    let live_ignition = ignition::new_empty_ignition(TEST_IGNITION_VERSION);
    ig.create_or_update_config_map(&ctx, &cr, &live_ignition, &cr.spec.machine_osurl)
        .await
        .unwrap();

    // Verify data was updated
    let cm: ConfigMap = c.get(&cm_key(&cr)).await.unwrap();
    assert_ne!(cm.data.as_ref().unwrap()[CONFIG_MAP_KEY_NAME], "old-data");

    // Verify labels and annotations are set on update
    assert_kv(&labels(&cm), BFCFG_TEMPLATE_LABEL, "true");
    assert_full_annotations(&cm, &cr, &deployment);
}

#[tokio::test]
async fn create_or_update_config_map_should_set_owner_reference_when_cr_and_config_map_are_in_the_same_namespace()
 {
    let ctx = Ctx::background();
    let mut cr = new_test_provisioner();
    // Set CR namespace to match DPUCluster namespace
    cr.metadata.namespace = Some(cr.spec.dpu_cluster_ref.namespace.clone());
    cr.metadata.uid = Some("test-uid-123".into());

    let deployment = new_test_dpu_deployment(&cr);
    let c = fake_client(vec![obj(&deployment)]);
    let (_, recorder) = fake_recorder();
    let ig = IgnitionGenerator::new(c.clone(), recorder);

    let live_ignition = ignition::new_empty_ignition(TEST_IGNITION_VERSION);
    ig.create_or_update_config_map(&ctx, &cr, &live_ignition, &cr.spec.machine_osurl)
        .await
        .unwrap();

    let cm: ConfigMap = c.get(&cm_key(&cr)).await.unwrap();
    assert!(!cm.metadata.owner_references.unwrap_or_default().is_empty());
}

#[tokio::test]
async fn create_or_update_config_map_should_not_set_owner_reference_for_cross_namespace_config_map()
{
    let ctx = Ctx::background();
    // CR namespace differs from DPUCluster namespace (default in test CR)
    let cr = new_test_provisioner();
    let deployment = new_test_dpu_deployment(&cr);
    let c = fake_client(vec![obj(&deployment)]);
    let (_, recorder) = fake_recorder();
    let ig = IgnitionGenerator::new(c.clone(), recorder);

    let live_ignition = ignition::new_empty_ignition(TEST_IGNITION_VERSION);
    ig.create_or_update_config_map(&ctx, &cr, &live_ignition, &cr.spec.machine_osurl)
        .await
        .unwrap();

    let cm: ConfigMap = c.get(&cm_key(&cr)).await.unwrap();
    assert!(cm.metadata.owner_references.unwrap_or_default().is_empty());
}

#[tokio::test]
async fn create_or_update_config_map_should_not_include_bfb_annotations_when_dpu_deployment_ref_is_nil()
 {
    let ctx = Ctx::background();
    let mut cr = new_test_provisioner();
    cr.spec.dpu_deployment_ref = None;
    let c = fake_client(vec![]);
    let (_, recorder) = fake_recorder();
    let ig = IgnitionGenerator::new(c.clone(), recorder);

    let live_ignition = ignition::new_empty_ignition(TEST_IGNITION_VERSION);
    ig.create_or_update_config_map(&ctx, &cr, &live_ignition, &cr.spec.machine_osurl)
        .await
        .unwrap();

    let cm: ConfigMap = c.get(&cm_key(&cr)).await.unwrap();

    // Label should still be present
    assert_kv(&labels(&cm), BFCFG_TEMPLATE_LABEL, "true");

    // Cluster annotations should be present
    let a = annotations(&cm);
    assert_kv(
        &a,
        BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION,
        &cr.spec.dpu_cluster_ref.name,
    );
    assert_kv(
        &a,
        BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION,
        &cr.spec.dpu_cluster_ref.namespace,
    );
    assert_kv(
        &a,
        BFCFG_TEMPLATE_MACHINE_OS_URL_ANNOTATION,
        &cr.spec.machine_osurl,
    );

    // BFB and DPUFlavor annotations should NOT be present
    assert!(!a.contains_key(BFCFG_TEMPLATE_BFB_NAME_ANNOTATION));
    assert!(!a.contains_key(BFCFG_TEMPLATE_BFB_NAMESPACE_ANNOTATION));
    assert!(!a.contains_key(BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION));
    assert!(!a.contains_key(BFCFG_TEMPLATE_DPU_FLAVOR_NAMESPACE_ANNOTATION));
}

// ---------------------------------------------------------------------------------------------
// Describe("GenerateIgnition")
// ---------------------------------------------------------------------------------------------

fn cr_only_client(cr: &mut provisioningv1alpha1::DPFHCPProvisioner) -> Client {
    FakeClientBuilder::new()
        .with_scheme(new_test_scheme())
        .with_objects([obj_mut(cr)])
        .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
        .build()
}

#[tokio::test]
async fn generate_ignition_should_set_ignition_configured_false_and_requeue_after_on_failure() {
    let ctx = Ctx::background();
    let mut cr = new_test_provisioner();
    let (recorder, rec) = fake_recorder();
    // No resources in the fake client — generation will fail
    let c = cr_only_client(&mut cr);
    let ig = IgnitionGenerator::new(c, rec);

    let result = ig.generate_ignition(&ctx, &mut cr).await.unwrap(); // returns nil with RequeueAfter
    assert_eq!(result.requeue_after, Duration::from_secs(30));

    // Check condition was set
    let cond = meta::find_status_condition(
        &cr.status.conditions,
        provisioningv1alpha1::IGNITION_CONFIGURED,
    )
    .expect("condition");
    assert_eq!(cond.status, "False");
    assert_eq!(cond.reason, "IgnitionGenerationFailed");

    // Check event was emitted
    assert_eq!(recorder.len(), 1);
    let event = recorder.try_recv().unwrap();
    assert!(event.contains("IgnitionGenerationFailed"));
}

#[tokio::test]
async fn generate_ignition_should_use_status_blue_field_ocp_layer_image_as_fallback_when_spec_machine_osurl_is_empty()
 {
    let ctx = Ctx::background();
    let mut cr = new_test_provisioner();
    cr.spec.machine_osurl = String::new();
    cr.status.blue_field_ocp_layer_image = "https://fallback-image.example.com".into();

    // Even though the download will fail (no HCP secrets), the machineOSURL
    // resolution happens later in generateIgnition, so we verify the error
    // is NOT about MachineOSURLMissing
    let (_, rec) = fake_recorder();
    let c = cr_only_client(&mut cr);
    let ig = IgnitionGenerator::new(c, rec);

    let _ = ig.generate_ignition(&ctx, &mut cr).await;

    let cond = meta::find_status_condition(
        &cr.status.conditions,
        provisioningv1alpha1::IGNITION_CONFIGURED,
    )
    .expect("condition");
    // Should fail for a reason other than MachineOSURLMissing
    assert_ne!(cond.reason, "MachineOSURLMissing");
}

#[tokio::test]
async fn generate_ignition_should_set_machine_osurl_missing_when_both_spec_and_status_urls_are_empty()
 {
    let ctx = Ctx::background();
    let mut cr = new_test_provisioner();
    cr.spec.machine_osurl = String::new();
    cr.status.blue_field_ocp_layer_image = String::new();

    // (See the Go test: setting up resources for the earlier steps is skipped; this verifies
    // that GenerateIgnition produces a failure condition.)
    let (_, rec) = fake_recorder();
    let c = cr_only_client(&mut cr);
    let ig = IgnitionGenerator::new(c, rec);

    let _ = ig.generate_ignition(&ctx, &mut cr).await;

    let cond = meta::find_status_condition(
        &cr.status.conditions,
        provisioningv1alpha1::IGNITION_CONFIGURED,
    )
    .expect("condition");
    assert_eq!(cond.status, "False");
}

// ---------------------------------------------------------------------------------------------
// Rust-only checks
// ---------------------------------------------------------------------------------------------

#[test]
fn constants_match_go_definitions() {
    assert_eq!(
        BFCFG_TEMPLATE_LABEL,
        format!("{DPU_PROVISIONING_PREFIX}bfcfg-template")
    );
    assert_eq!(
        BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION,
        format!("{DPU_PROVISIONING_PREFIX}bfcfg-template-cluster-name")
    );
    assert_eq!(
        BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION,
        format!("{DPU_PROVISIONING_PREFIX}bfcfg-template-cluster-namespace")
    );
    assert_eq!(
        BFCFG_TEMPLATE_BFB_NAME_ANNOTATION,
        format!("{DPU_PROVISIONING_PREFIX}bfcfg-template-bfb-name")
    );
    assert_eq!(
        BFCFG_TEMPLATE_BFB_NAMESPACE_ANNOTATION,
        format!("{DPU_PROVISIONING_PREFIX}bfcfg-template-bfb-namespace")
    );
    assert_eq!(
        BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION,
        format!("{DPU_PROVISIONING_PREFIX}bfcfg-template-dpuflavor-name")
    );
    assert_eq!(
        BFCFG_TEMPLATE_DPU_FLAVOR_NAMESPACE_ANNOTATION,
        format!("{DPU_PROVISIONING_PREFIX}bfcfg-template-dpuflavor-namespace")
    );
    assert_eq!(
        BFCFG_TEMPLATE_MACHINE_OS_URL_ANNOTATION,
        format!(
            "{}bfcfg-template-machine-os-url",
            common::constants::ANNOTATION_PREFIX
        )
    );
    assert_eq!(
        BFCFG_TEMPLATE_OCP_RELEASE_IMAGE_ANNOTATION,
        format!(
            "{}bfcfg-template-ocp-release-image",
            common::constants::ANNOTATION_PREFIX
        )
    );
    assert_eq!(config_map_name("my-cluster"), "bfcfg-my-cluster.cfg");
}

#[test]
fn hash_pull_secret_matches_hypershift_hash_simple() {
    // fnv32a("[123 125]") — HashSimple formats []byte with %v.
    assert_eq!(hash_pull_secret(b"{}"), {
        let mut h: u32 = 0x811c9dc5;
        for b in b"[123 125]" {
            h ^= u32::from(*b);
            h = h.wrapping_mul(16777619);
        }
        format!("{h:08x}")
    });
    // Go: fmt.Sprintf("%08x", fnv32a("[]")) for an empty slice.
    assert_eq!(hash_pull_secret(b"").len(), 8);
}
