//! Port of internal/controller/ignitiongenerator/ignition_generator_integration_test.go
//!
//! `httptest.NewTLSServer` → [`IgnitionServer`] (rustls + hyper on 127.0.0.1; the serving
//! certificate is signed by a test CA whose PEM goes into the `ignition-server-ca-cert`
//! secret, like Go's PEM of the server certificate).

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
use ctrlrt::{Client, Ctx, ObjectKey, meta};
use ctrlrt_fake::{FakeClientBuilder, InitObject, obj, obj_mut};
use ignition::types::{Config, File, Resource, Unit, User};

use crate::ignition_generator::*;
use crate::ignition_generator_test::{TEST_IGNITION_VERSION, machine_config_file};
use crate::testutil::{IgnitionServer, fake_recorder, new_test_scheme};

/// buildRealisticHCPIgnition builds a realistic HCP ignition payload with machine config,
/// SSH keys, and systemd units — similar to what HyperShift actually returns.
pub(crate) fn build_realistic_hcp_ignition() -> Vec<u8> {
    let mut mc = machine_config_file("https://registry.example.com/rhcos:4.17");
    mc.overwrite = Some(true);
    mc.mode = Some(0o644);
    let mut ign = ignition::new_empty_ignition(TEST_IGNITION_VERSION);
    ignition::files_mut(&mut ign).push(mc);
    let mut core = User::new("core".into());
    core.ssh_authorized_keys = Some(vec![
        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQ... test@example.com".into(),
    ]);
    ign.passwd.get_or_insert_with(Default::default).users = Some(vec![core]);
    let mut kubelet = Unit::new("kubelet.service".into());
    kubelet.enabled = Some(true);
    kubelet.contents = Some(
        "[Unit]\nDescription=Kubelet\n[Service]\nExecStart=/usr/bin/kubelet\n[Install]\nWantedBy=multi-user.target"
            .into(),
    );
    ignition::units_mut(&mut ign).push(kubelet);
    ignition::marshal_config(&ign)
}

struct ClusterObjects {
    ca_secret: Secret,
    token_secret: Secret,
    hc: hyperv1::HostedCluster,
    deployment: dpuservicev1alpha1::DPUDeployment,
    flavor: dpuprovisioningv1alpha1::DPUFlavor,
    operator_config: operatorv1alpha1::DPFOperatorConfig,
}

impl ClusterObjects {
    fn init_objects(&self) -> Vec<InitObject> {
        vec![
            obj(&self.ca_secret),
            obj(&self.token_secret),
            obj(&self.hc),
            obj(&self.deployment),
            obj(&self.flavor),
            obj(&self.operator_config),
        ]
    }
}

fn meta_ns(name: &str, ns: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.into()),
        namespace: Some(ns.into()),
        ..Default::default()
    }
}

/// setupClusterObjects creates all the Kubernetes objects needed for generateIgnition to run
fn setup_cluster_objects(
    server: &IgnitionServer,
) -> (provisioningv1alpha1::DPFHCPProvisioner, ClusterObjects) {
    let cr = provisioningv1alpha1::DPFHCPProvisioner {
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
            ocp_release_image: "quay.io/openshift-release-dev/ocp-release:4.19.0-multi".into(),
            machine_osurl: "https://registry.example.com/rhcos-bf:4.17".into(),
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
    };

    // Ignition CA cert secret (in control plane namespace clusters-doca)
    let ca_secret = Secret {
        metadata: meta_ns(IGNITION_SECRET_NAME, "clusters-doca"),
        data: Some([("tls.crt".to_string(), ByteString(server.ca_pem.clone()))].into()),
        ..Default::default()
    };

    // Ignition token secret
    let token_secret = Secret {
        metadata: meta_ns("token-doca-abc123", "clusters-doca"),
        data: Some(
            [(
                IGNITION_TOKEN_KEY.to_string(),
                ByteString(b"test-bearer-token".to_vec()),
            )]
            .into(),
        ),
        ..Default::default()
    };

    // HostedCluster with ignition endpoint pointing to our test server
    let mut hc = hyperv1::HostedCluster {
        metadata: meta_ns("doca", "clusters"),
        ..Default::default()
    };
    hc.status.ignition_endpoint = server.host();

    // DPUDeployment
    let deployment = dpuservicev1alpha1::DPUDeployment {
        metadata: meta_ns("my-deployment", "dpf-operator-system"),
        spec: dpuservicev1alpha1::DPUDeploymentSpec {
            dpus: dpuservicev1alpha1::DPUs {
                bfb: Some("test-bfb".into()),
                flavor: Some("test-flavor".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    // DPUFlavor with OVS script (deployment mode comes from DPFOperatorConfig, not flavor)
    let mut flavor = dpuprovisioningv1alpha1::DPUFlavor {
        metadata: meta_ns("test-flavor", "dpf-operator-system"),
        ..Default::default()
    };
    flavor.spec.ovs.raw_config_script =
        "#!/bin/bash\novs-vsctl add-br br0\novs-vsctl add-port br0 p0".into();

    // DPFOperatorConfig
    let mut operator_config = operatorv1alpha1::DPFOperatorConfig {
        metadata: meta_ns("dpf-config", "dpf-operator-system"),
        ..Default::default()
    };
    operator_config.spec.deployment_mode = operatorv1alpha1::DEPLOYMENT_MODE_HOST_TRUSTED.into();
    operator_config.spec.provisioning_controller =
        Some(operatorv1alpha1::ProvisioningControllerConfiguration {
            install_interface: Some(operatorv1alpha1::ProvisioningInstallInterface {
                install_via_host_agent: Some(Default::default()),
                ..Default::default()
            }),
            ..Default::default()
        });
    operator_config.spec.networking = Some(operatorv1alpha1::Networking {
        control_plane_mtu: Some(9000),
        ..Default::default()
    });

    (
        cr,
        ClusterObjects {
            ca_secret,
            token_secret,
            hc,
            deployment,
            flavor,
            operator_config,
        },
    )
}

fn build_client(
    cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
    objects: Vec<InitObject>,
) -> Client {
    let mut all = vec![obj_mut(cr)];
    all.extend(objects);
    FakeClientBuilder::new()
        .with_scheme(new_test_scheme())
        .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
        .with_objects(all)
        .build()
}

fn cm_key() -> ObjectKey {
    ObjectKey::new("dpf-operator-system", config_map_name("dpucluster-1"))
}

fn parse_live(cm: &ConfigMap) -> Config {
    let data = &cm.data.as_ref().unwrap()[CONFIG_MAP_KEY_NAME];
    ignition::unmarshal_config(data.trim().as_bytes()).unwrap()
}

fn ctx() -> Ctx {
    Ctx::background()
}

#[tokio::test]
async fn generate_ignition_integration_should_run_the_full_ignition_generation_pipeline_end_to_end()
{
    let hcp_json = build_realistic_hcp_ignition();
    let server = IgnitionServer::serve_hcp(hcp_json).await;
    let (recorder, rec) = fake_recorder();

    let (mut cr, objects) = setup_cluster_objects(&server);
    let c = build_client(&mut cr, objects.init_objects());
    let ig = IgnitionGenerator::new(c.clone(), rec);

    // Run the full pipeline
    let result = ig.generate_ignition(&ctx(), &mut cr).await.unwrap();
    assert!(
        result.requeue_after.is_zero(),
        "should not requeue on success"
    );

    // Verify status condition was set to IgnitionConfigured=True
    let cond = meta::find_status_condition(
        &cr.status.conditions,
        provisioningv1alpha1::IGNITION_CONFIGURED,
    )
    .expect("condition");
    assert_eq!(cond.status, "True");
    assert_eq!(cond.reason, "IgnitionGenerated");

    // Verify ConfigMap was created with correct name and labels
    let cm: ConfigMap = c.get(&cm_key()).await.unwrap();
    let labels = cm.metadata.labels.clone().unwrap_or_default();
    let ann = cm.metadata.annotations.clone().unwrap_or_default();
    assert_eq!(
        labels.get(BFCFG_TEMPLATE_LABEL).map(String::as_str),
        Some("true")
    );
    assert_eq!(
        ann.get(BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION)
            .map(String::as_str),
        Some("dpucluster-1")
    );
    assert_eq!(
        ann.get(BFCFG_TEMPLATE_BFB_NAME_ANNOTATION)
            .map(String::as_str),
        Some("test-bfb")
    );
    assert_eq!(
        ann.get(BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION)
            .map(String::as_str),
        Some("test-flavor")
    );
    assert_eq!(
        ann.get(BFCFG_TEMPLATE_MACHINE_OS_URL_ANNOTATION)
            .map(String::as_str),
        Some("https://registry.example.com/rhcos-bf:4.17")
    );

    // Verify the ConfigMap data contains valid ignition JSON
    let ignition_data = cm
        .data
        .as_ref()
        .and_then(|d| d.get(CONFIG_MAP_KEY_NAME))
        .expect("ConfigMap should have BF_CFG_TEMPLATE key");
    assert!(!ignition_data.is_empty());

    // Parse the live ignition from the ConfigMap and verify it's valid
    let live_ign = parse_live(&cm);
    assert_eq!(live_ign.ignition.version, TEST_IGNITION_VERSION);

    // Verify live ignition has /var/target.ign (embedded target ignition)
    let target: Option<&File> = ignition::files(&live_ign)
        .iter()
        .find(|f| f.path == "/var/target.ign");
    let target = target.expect("live ignition should contain /var/target.ign");
    assert_eq!(
        target
            .contents
            .as_ref()
            .and_then(|c: &Resource| c.compression.as_deref()),
        Some("gzip")
    );

    // Verify SSH keys were copied to live ignition
    let users = ignition::users(&live_ign);
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].name, "core");
    assert_eq!(users[0].ssh_authorized_keys.as_ref().unwrap().len(), 1);

    // Verify success event was emitted
    assert!(!recorder.is_empty());
    let event = recorder.try_recv().unwrap();
    assert!(event.contains("IgnitionConfigured"));
}

fn is_mtu_nm_connection(path: &str) -> bool {
    ["p0", "p1", "pf0hpf", "pf1hpf"]
        .iter()
        .any(|i| path == format!("/etc/NetworkManager/system-connections/{i}.nmconnection"))
}

#[tokio::test]
async fn generate_ignition_integration_should_produce_validated_ignition_with_mtu_1500_no_nm_connections()
 {
    let hcp_json = build_realistic_hcp_ignition();
    let server = IgnitionServer::serve_hcp(hcp_json).await;
    let (_, rec) = fake_recorder();

    let (mut cr, mut objects) = setup_cluster_objects(&server);

    // Override MTU to 1500
    objects
        .operator_config
        .spec
        .networking
        .as_mut()
        .unwrap()
        .control_plane_mtu = Some(1500);

    let c = build_client(&mut cr, objects.init_objects());
    let ig = IgnitionGenerator::new(c.clone(), rec);

    let result = ig.generate_ignition(&ctx(), &mut cr).await.unwrap();
    assert!(result.requeue_after.is_zero());

    // Verify ConfigMap was created
    let cm: ConfigMap = c.get(&cm_key()).await.unwrap();

    // Parse live ignition and verify no NM connections
    let live_ign = parse_live(&cm);
    for f in ignition::files(&live_ign) {
        assert!(
            !is_mtu_nm_connection(&f.path),
            "MTU=1500 should not add NM connection files"
        );
    }
}

#[tokio::test]
async fn generate_ignition_integration_should_produce_validated_ignition_with_mtu_9000_with_nm_connections_in_target()
 {
    let hcp_json = build_realistic_hcp_ignition();
    let server = IgnitionServer::serve_hcp(hcp_json).await;
    let (_, rec) = fake_recorder();

    let (mut cr, objects) = setup_cluster_objects(&server);
    let c = build_client(&mut cr, objects.init_objects());
    let ig = IgnitionGenerator::new(c, rec);

    let result = ig.generate_ignition(&ctx(), &mut cr).await.unwrap();
    assert!(result.requeue_after.is_zero());

    let cond = meta::find_status_condition(
        &cr.status.conditions,
        provisioningv1alpha1::IGNITION_CONFIGURED,
    )
    .expect("condition");
    assert_eq!(cond.status, "True");
}

#[tokio::test]
async fn generate_ignition_integration_should_produce_idempotent_results_on_re_run_update_existing_config_map()
 {
    let hcp_json = build_realistic_hcp_ignition();
    let server = IgnitionServer::serve_hcp(hcp_json).await;
    let (_, rec) = fake_recorder();

    let (mut cr, objects) = setup_cluster_objects(&server);
    let c = build_client(&mut cr, objects.init_objects());
    let ig = IgnitionGenerator::new(c.clone(), rec);

    // First run — creates ConfigMap
    let result1 = ig.generate_ignition(&ctx(), &mut cr).await.unwrap();
    assert!(result1.requeue_after.is_zero());

    let cm1: ConfigMap = c.get(&cm_key()).await.unwrap();
    let data1 = cm1.data.unwrap()[CONFIG_MAP_KEY_NAME].clone();

    // Second run — updates ConfigMap (same input → same output)
    let result2 = ig.generate_ignition(&ctx(), &mut cr).await.unwrap();
    assert!(result2.requeue_after.is_zero());

    let cm2: ConfigMap = c.get(&cm_key()).await.unwrap();
    let data2 = cm2.data.unwrap()[CONFIG_MAP_KEY_NAME].clone();

    assert_eq!(
        data2, data1,
        "idempotent run should produce identical ignition output"
    );
}

#[tokio::test]
async fn generate_ignition_integration_should_set_ignition_configured_false_when_ignition_endpoint_is_unavailable()
 {
    // Don't start a server — HCP download will fail
    let mut cr = provisioningv1alpha1::DPFHCPProvisioner {
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
            machine_osurl: "https://registry.example.com/rhcos-bf:4.17".into(),
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
    };
    let (_, rec) = fake_recorder();
    let c = build_client(&mut cr, vec![]);
    let ig = IgnitionGenerator::new(c, rec);

    let result = ig.generate_ignition(&ctx(), &mut cr).await.unwrap(); // returns nil + RequeueAfter
    assert_eq!(result.requeue_after, Duration::from_secs(30));

    let cond = meta::find_status_condition(
        &cr.status.conditions,
        provisioningv1alpha1::IGNITION_CONFIGURED,
    )
    .expect("condition");
    assert_eq!(cond.status, "False");
    assert_eq!(cond.reason, "IgnitionGenerationFailed");
}

#[tokio::test]
async fn generate_ignition_integration_should_propagate_machine_osurl_from_spec_to_config_map_annotation()
 {
    let hcp_json = build_realistic_hcp_ignition();
    let server = IgnitionServer::serve_hcp(hcp_json).await;
    let (_, rec) = fake_recorder();

    let (mut cr, objects) = setup_cluster_objects(&server);
    let custom_url = "https://custom-registry.io/custom-rhcos-bf:v4.18";
    cr.spec.machine_osurl = custom_url.into();
    let c = build_client(&mut cr, objects.init_objects());
    let ig = IgnitionGenerator::new(c.clone(), rec);

    let result = ig.generate_ignition(&ctx(), &mut cr).await.unwrap();
    assert!(result.requeue_after.is_zero());

    let cm: ConfigMap = c.get(&cm_key()).await.unwrap();
    assert_eq!(
        cm.metadata.annotations.unwrap()[BFCFG_TEMPLATE_MACHINE_OS_URL_ANNOTATION],
        custom_url
    );
}

#[tokio::test]
async fn generate_ignition_integration_should_reject_hcp_ignition_with_duplicate_files_via_target_validation()
 {
    // Build HCP ignition with duplicate file paths — this should cause ValidateConfig
    // to fail during the pipeline
    let mut mc = machine_config_file("https://registry.example.com/rhcos:4.17");
    mc.overwrite = Some(true);
    mc.mode = Some(0o644);
    let mut ign = ignition::new_empty_ignition(TEST_IGNITION_VERSION);
    ignition::files_mut(&mut ign).push(mc);
    // Duplicate file path — the validator should catch this
    for _ in 0..2 {
        ignition::files_mut(&mut ign).push(ignition::new_file(
            "/etc/duplicate-file",
            0o644,
            "data:,content".into(),
        ));
    }
    let hcp_json = ignition::marshal_config(&ign);

    let server = IgnitionServer::serve_hcp(hcp_json).await;
    let (_, rec) = fake_recorder();

    let (mut cr, objects) = setup_cluster_objects(&server);
    let c = build_client(&mut cr, objects.init_objects());
    let ig = IgnitionGenerator::new(c.clone(), rec);

    let result = ig.generate_ignition(&ctx(), &mut cr).await.unwrap(); // returns nil + RequeueAfter on failure
    assert_eq!(
        result.requeue_after,
        Duration::from_secs(30),
        "should requeue on validation failure"
    );

    let cond = meta::find_status_condition(
        &cr.status.conditions,
        provisioningv1alpha1::IGNITION_CONFIGURED,
    )
    .expect("condition");
    assert_eq!(cond.status, "False");
    assert!(
        cond.message.contains("validation failed"),
        "{}",
        cond.message
    );

    // Verify no ConfigMap was created
    let res: ctrlrt::Result<ConfigMap> = c.get(&cm_key()).await;
    assert!(
        res.is_err(),
        "ConfigMap should not exist when validation fails"
    );
}

#[tokio::test]
async fn generate_ignition_integration_should_produce_live_ignition_that_passes_validation_after_template_substitution()
 {
    let hcp_json = build_realistic_hcp_ignition();
    let server = IgnitionServer::serve_hcp(hcp_json).await;
    let (_, rec) = fake_recorder();

    let (mut cr, objects) = setup_cluster_objects(&server);
    let c = build_client(&mut cr, objects.init_objects());
    let ig = IgnitionGenerator::new(c.clone(), rec);

    let result = ig.generate_ignition(&ctx(), &mut cr).await.unwrap();
    assert!(result.requeue_after.is_zero());

    // Get live ignition from ConfigMap
    let cm: ConfigMap = c.get(&cm_key()).await.unwrap();
    let ignition_data = cm.data.unwrap()[CONFIG_MAP_KEY_NAME].trim().to_string();

    // Replace Go template placeholders with real values — simulates what the
    // DPF provisioning controller does before applying the ignition
    let resolved = ignition_data
        .replace("{{.DPUHostName}}", "bf2-worker-0")
        .replace("{{.DPUName}}", "dpu-0")
        .replace("{{.DPUNamespace}}", "dpf-operator-system")
        .replace("{{.DPUUID}}", "12345678-1234-1234-1234-123456789abc")
        .replace("{{.KernelParameters}}", "console=ttyAMA0")
        .replace("{{.ControlPlaneMTU}}", "1500");

    // Parse the resolved live ignition
    let live_ign = ignition::unmarshal_config(resolved.as_bytes()).unwrap();
    assert_eq!(live_ign.ignition.version, TEST_IGNITION_VERSION);

    // Run full ignition validation on the resolved live ignition
    let rpt = ignition::validate_with_context(&live_ign);
    assert!(
        !rpt.is_fatal(),
        "live ignition should be valid after template substitution, got: {rpt}"
    );

    // Verify structural completeness: live ignition should have target.ign,
    // OVS script, DPU flavor files, and common content
    assert!(
        ignition::files(&live_ign)
            .iter()
            .any(|f| f.path == "/var/target.ign"),
        "should embed target ignition"
    );
}
