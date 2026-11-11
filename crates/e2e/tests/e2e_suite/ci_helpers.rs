//! Port of `test/e2e/e2e_ci_helpers_test.go`.

use std::collections::BTreeMap;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1;
use apis::dpf::operator::v1alpha1 as operatorv1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1;
use apis::hypershift::v1beta1 as hyperv1;
use apis::k8s::core::v1::LocalObjectReference;
use apis::openshift::config::v1 as configv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::error::{ignore_not_found, is_already_exists, is_forbidden, is_not_found};
use ctrlrt::{Client, ListOptions, Object, ObjectKey};
use ctrlrt_runtime::RestConfig;
use ctrlrt_runtime::client::{RequestInfo, RestClient};
use ctrlrt_runtime::config::{
    KubeconfigOverrides, parse_kubeconfig, rest_config_from_kubeconfig_with,
};
use e2e::ginkgo::{Interrupt, NIL_POINTER_DEREFERENCE, NodeResult, by, fail, go_panic};
use e2e::gomega::{self as g, consistently, eventually};
use e2e::{gw, utils};
use k8s_openapi::ByteString;
use k8s_openapi::api::certificates::v1::{
    CertificateSigningRequest, CertificateSigningRequestSpec,
};
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Node, Secret};
use k8s_openapi::api::storage::v1::StorageClass;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, ObjectMeta};
use serde_json::{Map, Value};

use crate::Env;

pub const CI_NAMESPACE: &str = "dpf-hcp-provisioner-system";
pub const DPF_OPERATOR_CONFIG_NAMESPACE: &str = "dpf-operator-system";
pub const DPU_CLUSTER_NS: &str = "dpf-e2e-dpucluster";
pub const PROVISIONER_NAME: &str = "e2e-test-provisioner";
pub const DPU_CLUSTER_NAME: &str = "e2e-dpucluster";
pub const SSH_KEY_SECRET_NAME: &str = "e2e-ssh-key";
pub const PULL_SECRET_NAME: &str = "e2e-pull-secret";
pub const DPU_DEPLOYMENT_NAME: &str = "e2e-dpudeployment";
pub const DPU_FLAVOR_NAME: &str = "e2e-dpuflavor";

// Cleanup test constants
pub const CLEANUP_TEST_NAME: &str = "e2e-cleanup-test";
pub const CLEANUP_DPU_CLUSTER_NS: &str = "dpf-e2e-cleanup-dpucluster";
pub const CLEANUP_DPU_CLUSTER_NAME: &str = "cleanup-dpucluster";
pub const CLEANUP_DPU_FLAVOR_NAME: &str = "cleanup-dpuflavor";
pub const CLEANUP_DPU_DEPLOY_NAME: &str = "cleanup-dpudeploy";

pub const HOSTED_CLUSTER_READY_TIMEOUT: Duration = Duration::from_secs(25 * 60);
pub const CR_READY_TIMEOUT: Duration = Duration::from_secs(35 * 60);
pub const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const CSR_APPROVAL_TIMEOUT: Duration = Duration::from_secs(2 * 60);
pub const UPGRADE_TIMEOUT: Duration = Duration::from_secs(45 * 60);
pub const POLLING_INTERVAL: Duration = Duration::from_secs(10);

const MINUTE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------------------------
// small Go-compat helpers
// ---------------------------------------------------------------------------------------------

/// `metav1.ObjectMeta{Name: name, Namespace: ns}` (empty namespace for cluster-scoped objects).
pub fn meta(ns: &str, name: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        namespace: (!ns.is_empty()).then(|| ns.to_string()),
        ..Default::default()
    }
}

/// `time.Now().UnixNano()`
pub fn unix_nano() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// `os.ReadFile(path)` with Go's error text.
fn read_file(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("open {path}: {}", errno_text(&e)))
}

/// `os.WriteFile(path, data, perm)` with Go's error text.
fn write_file(path: &str, data: &[u8], perm: u32) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(perm)
        .open(path)
        .map_err(|e| format!("open {path}: {}", errno_text(&e)))?;
    f.write_all(data)
        .map_err(|e| format!("write {path}: {}", errno_text(&e)))
}

fn errno_text(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file or directory".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => e.to_string(),
    }
}

/// Removes files when dropped (Go's `defer os.Remove(...)`).
struct RemoveOnDrop(Vec<String>);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        for f in &self.0 {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// `os.CreateTemp("", pattern)`: creates a new file in the temp dir, `*` replaced by a random
/// number. Returns the path.
fn create_temp(pattern: &str) -> Result<(String, std::fs::File), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = std::env::temp_dir();
    let (prefix, suffix) = match pattern.rfind('*') {
        Some(i) => (&pattern[..i], &pattern[i + 1..]),
        None => (pattern, ""),
    };
    let mut last_err = String::new();
    for _ in 0..10000 {
        let name = format!("{prefix}{}{suffix}", fastrand::u32(..));
        let path = dir.join(name);
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(f) => return Ok((path.to_string_lossy().into_owned(), f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_err = format!("open {}: file exists", path.display());
            }
            Err(e) => return Err(format!("open {}: {}", path.display(), errno_text(&e))),
        }
    }
    Err(last_err)
}

/// `time.Duration.String()` for the durations used in messages.
pub fn go_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let nanos = d.subsec_nanos();
    if secs == 0 && nanos == 0 {
        return "0s".into();
    }
    if secs == 0 {
        return if nanos.is_multiple_of(1_000_000) {
            format!("{}ms", nanos / 1_000_000)
        } else {
            format!("{}ns", nanos)
        };
    }
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let mut sec = s.to_string();
    if nanos != 0 {
        let frac = format!("{nanos:09}");
        sec.push('.');
        sec.push_str(frac.trim_end_matches('0'));
    }
    if h > 0 {
        format!("{h}h{m}m{sec}s")
    } else if m > 0 {
        format!("{m}m{sec}s")
    } else {
        format!("{sec}s")
    }
}

/// Go's `%T` of a decoded JSON value.
pub fn go_type_name(v: Option<&Value>) -> &'static str {
    match v {
        None | Some(Value::Null) => "nil",
        Some(Value::Bool(_)) => "bool",
        Some(Value::Number(_)) => "float64",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "[]interface {}",
        Some(Value::Object(_)) => "map[string]interface {}",
    }
}

/// The panic message of a failed Go type assertion `v.(T)`.
fn type_assertion_panic(v: Option<&Value>, want: &str) -> String {
    format!(
        "[PANICKED] interface conversion: interface {{}} is {}, not {want}",
        go_type_name(v)
    )
}

/// `unstructured.NestedString(obj, fields...)`: `(value, found)` or the accessor error.
pub fn nested_string(obj: &Value, fields: &[&str]) -> Result<(String, bool), String> {
    let json_path = format!(".{}", fields.join("."));
    let mut val = obj;
    for (i, field) in fields.iter().enumerate() {
        if val.is_null() {
            return Ok((String::new(), false));
        }
        match val.as_object() {
            Some(m) => match m.get(*field) {
                Some(v) => val = v,
                None => return Ok((String::new(), false)),
            },
            None => {
                return Err(format!(
                    ".{} accessor error: {} is of the type {}, expected map[string]interface{{}}",
                    fields[..=i].join("."),
                    val,
                    go_type_name(Some(val))
                ));
            }
        }
    }
    if val.is_null() {
        return Ok((String::new(), false));
    }
    match val.as_str() {
        Some(s) => Ok((s.to_string(), true)),
        None => Err(format!(
            "{json_path} accessor error: {val} is of the type {}, expected string",
            go_type_name(Some(val))
        )),
    }
}

/// `k8sClient.DeleteAllOf(ctx, &T{}, client.InNamespace(ns))`
pub async fn delete_all_of<T: Object>(env: &Env, namespace: &str) -> ctrlrt::Result<()> {
    let m = env.backend.mapping(&T::resource_info()).await?;
    let mut path = if m.group.is_empty() {
        format!("/api/{}", m.version)
    } else {
        format!("/apis/{}/{}", m.group, m.version)
    };
    if m.namespaced && !namespace.is_empty() {
        path.push_str("/namespaces/");
        path.push_str(namespace);
    }
    path.push('/');
    path.push_str(&m.resource);
    let info = RequestInfo {
        group: m.group.clone(),
        resource: m.resource.clone(),
        name: String::new(),
    };
    env.backend
        .rest()
        .request(http::Method::DELETE, &path, None, None, None, Some(&info))
        .await
        .map(|_| ())
}

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

/// `detectOCPReleaseImage` gets the OCP release image from the management cluster.
pub async fn detect_ocp_release_image(env: &Env) -> NodeResult<String> {
    let image = std::env::var("OCP_RELEASE_IMAGE").unwrap_or_default();
    if !image.is_empty() {
        return Ok(image);
    }
    let cluster_version = env
        .client
        .backend()
        .get(&configv1::ClusterVersion::resource_info(), "", "version")
        .await;
    let cluster_version = g::ok(cluster_version, "Failed to get ClusterVersion")?;

    let (release_image, found) = g::ok(
        nested_string(&cluster_version, &["status", "desired", "image"]),
        "Failed to extract release image",
    )?;
    g::be_true(found, "Release image not found in ClusterVersion status")?;
    Ok(release_image)
}

/// `getBaseDomain` returns the base domain for the HostedCluster.
/// In CI, BASE_DOMAIN is set in the workflow config.
/// For local testing, export it manually.
pub fn get_base_domain(env: &Env) -> NodeResult<String> {
    let domain = std::env::var("BASE_DOMAIN").unwrap_or_default();
    if !domain.is_empty() {
        return Ok(domain);
    }
    if !env.testing_on_ocp {
        return Ok("e2e.example.com".into());
    }
    fail(
        "BASE_DOMAIN must be set. \
         In CI this is configured in the workflow. \
         For local testing, export it with your cluster's base domain.",
    )
}

/// `getControlPlaneAvailability` returns the control plane availability policy.
pub fn get_control_plane_availability() -> String {
    let policy = std::env::var("CONTROL_PLANE_AVAILABILITY").unwrap_or_default();
    if policy.is_empty() {
        return "SingleReplica".into();
    }
    policy
}

/// `getMachineOSURL` returns the machine OS URL if set.
pub fn get_machine_os_url() -> String {
    std::env::var("MACHINE_OS_URL").unwrap_or_default()
}

/// `getEtcdStorageClass` returns the storage class for etcd PVCs.
/// Auto-detects the default StorageClass if not set via env var.
/// Fails the test if no StorageClass is available (required for HostedCluster etcd).
pub async fn get_etcd_storage_class(env: &Env) -> NodeResult<String> {
    let sc = std::env::var("ETCD_STORAGE_CLASS").unwrap_or_default();
    if !sc.is_empty() {
        return Ok(sc);
    }
    // Try to detect default storage class
    let sc_list = env
        .client
        .backend()
        .list(&StorageClass::resource_info(), &ListOptions::default())
        .await;
    if let Ok(items) = sc_list {
        for item in items {
            let default = item
                .pointer("/metadata/annotations/storageclass.kubernetes.io~1is-default-class")
                .and_then(Value::as_str);
            if default == Some("true") {
                return Ok(item
                    .pointer("/metadata/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string());
            }
        }
    }
    if !env.testing_on_ocp {
        return Ok(String::new());
    }
    fail(
        "No default StorageClass found and ETCD_STORAGE_CLASS not set. \
         A StorageClass is required for HostedCluster etcd PVCs. \
         On AWS IPI clusters this is available by default. \
         For local clusters, deploy a storage provider first.",
    )
}

/// `createNamespace` creates a Kubernetes namespace, waiting for it to be fully deleted first if terminating.
pub async fn create_namespace(env: &Env, ns: &str) -> NodeResult {
    // Wait for namespace to be fully gone if it's terminating from a previous run
    eventually(2 * MINUTE, Duration::from_secs(5), "", async || {
        let namespace = env.client.get::<Namespace>(&ObjectKey::cluster(ns)).await;
        if let Err(e) = &namespace
            && is_not_found(e)
        {
            return Ok(()); // Namespace doesn't exist, good
        }
        let namespace = g::ok(namespace, "")?;
        let phase = namespace.status.and_then(|s| s.phase).unwrap_or_default();
        g::not_equal(
            &phase,
            "Terminating",
            &format!("Namespace {ns} is still terminating"),
        )?;
        Ok(())
    })
    .await?;

    let mut namespace = Namespace {
        metadata: meta("", ns),
        ..Default::default()
    };
    if let Err(e) = env.client.create(&mut namespace).await
        && !is_already_exists(&e)
    {
        g::ok(Err::<(), _>(e), &format!("Failed to create namespace {ns}"))?;
    }
    Ok(())
}

/// `createDPUClusterStub` creates a minimal DPUCluster CR for testing.
pub async fn create_dpu_cluster_stub(env: &Env, ns: &str, name: &str) -> NodeResult {
    let mut dpu_cluster = dpuprovisioningv1::DPUCluster {
        metadata: meta(ns, name),
        spec: dpuprovisioningv1::DPUClusterSpec {
            type_: dpuprovisioningv1::STATIC_CLUSTER.to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    if let Err(e) = env.client.create(&mut dpu_cluster).await
        && !is_already_exists(&e)
    {
        g::ok(Err::<(), _>(e), "Failed to create DPUCluster stub")?;
    }
    Ok(())
}

/// `generateSSHKeySecret` creates a secret with a generated SSH public key.
pub async fn generate_ssh_key_secret(env: &Env, ns: &str, name: &str) -> NodeResult {
    by("generating SSH key pair");
    let key_file = format!("/tmp/e2e-ssh-key-{}", unix_nano());
    let mut cmd = Command::new("ssh-keygen");
    cmd.args(["-t", "rsa", "-b", "4096", "-f", &key_file, "-N", ""]);
    g::ok(utils::run(cmd), "Failed to generate SSH key")?;
    let _cleanup = RemoveOnDrop(vec![key_file.clone(), format!("{key_file}.pub")]);

    let pub_key_data = g::ok(
        read_file(&format!("{key_file}.pub")),
        "Failed to read public key",
    )?;

    let mut secret = Secret {
        metadata: meta(ns, name),
        data: Some(BTreeMap::from([(
            "id_rsa.pub".to_string(),
            ByteString(pub_key_data),
        )])),
        ..Default::default()
    };
    g::ok(
        env.client.create(&mut secret).await,
        "Failed to create SSH key secret",
    )?;
    Ok(())
}

/// `copyPullSecret` copies the cluster pull secret to the target namespace.
pub async fn copy_pull_secret(env: &Env, ns: &str, name: &str) -> NodeResult {
    by("copying pull secret from openshift-config");

    let source_secret = g::ok(
        env.client
            .get::<Secret>(&ObjectKey::new("openshift-config", "pull-secret"))
            .await,
        "Failed to get cluster pull secret",
    )?;

    let docker_config_json = source_secret
        .data
        .as_ref()
        .and_then(|d| d.get(".dockerconfigjson"))
        .cloned();
    g::not_nil(&docker_config_json, "Pull secret missing .dockerconfigjson")?;

    let mut target_secret = Secret {
        metadata: meta(ns, name),
        type_: Some("kubernetes.io/dockerconfigjson".into()),
        data: Some(BTreeMap::from([(
            ".dockerconfigjson".to_string(),
            docker_config_json.unwrap_or_default(),
        )])),
        ..Default::default()
    };
    g::ok(
        env.client.create(&mut target_secret).await,
        "Failed to create pull secret",
    )?;
    Ok(())
}

/// `createDPUDeploymentStub` creates a minimal DPUDeployment CR that references a DPUFlavor.
pub async fn create_dpu_deployment_stub(
    env: &Env,
    ns: &str,
    name: &str,
    flavor_name: &str,
) -> NodeResult {
    let mut dpu_deployment = dpuservicev1::DPUDeployment {
        metadata: meta(ns, name),
        spec: dpuservicev1::DPUDeploymentSpec {
            dpus: dpuservicev1::DPUs {
                bfb: Some("e2e-bfb".into()),
                flavor: Some(flavor_name.into()),
                node_effect: dpuprovisioningv1::Action {
                    no_effect: Some(true),
                    ..Default::default()
                },
                // DPUSetStrategy is required - use RollingUpdate for stub
                dpu_set_strategy: dpuprovisioningv1::DPUSetStrategy {
                    type_: "RollingUpdate".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            // Services requires at least 1 service - add minimal stub service
            services: Some(BTreeMap::from([(
                "e2e-stub-service".to_string(),
                dpuservicev1::DPUDeploymentServiceConfiguration {
                    service_template: "e2e-stub-template".into(),
                    service_configuration: "e2e-stub-config".into(),
                    ..Default::default()
                },
            )])),
            service_chains: Some(dpuservicev1::ServiceChains {
                upgrade_policy: dpuservicev1::UpgradePolicy::default(),
                switches: Some(vec![dpuservicev1::DPUDeploymentSwitch {
                    ports: Some(vec![dpuservicev1::DPUDeploymentPort {
                        service_interface: Some(dpuservicev1::ServiceIfc {
                            match_labels: Some(BTreeMap::from([(
                                "e2e-test".to_string(),
                                "true".to_string(),
                            )])),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    if let Err(e) = env.client.create(&mut dpu_deployment).await
        && !is_already_exists(&e)
    {
        g::ok(Err::<(), _>(e), "Failed to create DPUDeployment stub")?;
    }
    Ok(())
}

/// `createDPUFlavorStub` creates a minimal DPUFlavor CR.
pub async fn create_dpu_flavor_stub(env: &Env, ns: &str, name: &str) -> NodeResult {
    let mut dpu_flavor = dpuprovisioningv1::DPUFlavor {
        metadata: meta(ns, name),
        spec: dpuprovisioningv1::DPUFlavorSpec {
            dpu_mode: dpuprovisioningv1::DPU_MODE.into(),
            ovs: dpuprovisioningv1::DPUFlavorOVS {
                raw_config_script: "#!/bin/bash\necho \"e2e test OVS config\"\n".into(),
                ..Default::default()
            },
            config_files: vec![
                dpuprovisioningv1::ConfigFile {
                    path: "/etc/mellanox/mlnx-bf.conf".into(),
                    operation: dpuprovisioningv1::FILE_OVERRIDE.into(),
                    permissions: "0644".into(),
                    raw: Some(
                        "ALLOW_SHARED_RQ=\"yes\"\nIPSEC_FULL_OFFLOAD=\"no\"\n\
                         ENABLE_ESWITCH_MULTIPORT=\"yes\"\nE2E_CUSTOM=\"true\"\n"
                            .into(),
                    ),
                    ..Default::default()
                },
                dpuprovisioningv1::ConfigFile {
                    path: "/etc/dpf/e2e-custom.conf".into(),
                    operation: dpuprovisioningv1::FILE_OVERRIDE.into(),
                    permissions: "0644".into(),
                    raw: Some("E2E_NEW_FILE=yes\n".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        ..Default::default()
    };
    if let Err(e) = env.client.create(&mut dpu_flavor).await
        && !is_already_exists(&e)
    {
        g::ok(Err::<(), _>(e), "Failed to create DPUFlavor stub")?;
    }
    Ok(())
}

/// The DPFOperatorConfig singleton created by the suite.
pub fn dpf_operator_config_stub() -> operatorv1::DPFOperatorConfig {
    let mtu = 1500;
    let bfb_pvc_name = "e2e-bfb-pvc".to_string();
    operatorv1::DPFOperatorConfig {
        metadata: meta(DPF_OPERATOR_CONFIG_NAMESPACE, "dpfoperatorconfig"),
        spec: operatorv1::DPFOperatorConfigSpec {
            deployment_mode: operatorv1::DEPLOYMENT_MODE_HOST_TRUSTED.into(),
            provisioning_controller: Some(operatorv1::ProvisioningControllerConfiguration {
                bfb_pvc_name: Some(bfb_pvc_name),
                install_interface: Some(operatorv1::ProvisioningInstallInterface {
                    install_via_host_agent: Some(operatorv1::InstallViaHostAgent::default()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            networking: Some(operatorv1::Networking {
                control_plane_mtu: Some(mtu),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `createDPFOperatorConfig` creates the singleton DPFOperatorConfig in dpf-operator-system.
pub async fn create_dpf_operator_config(env: &Env) -> NodeResult {
    let mut dpf_operator_config = dpf_operator_config_stub();
    g::ok(
        env.client.create(&mut dpf_operator_config).await,
        "Failed to create DPFOperatorConfig",
    )?;
    Ok(())
}

/// `createDPFHCPProvisioner` creates a DPFHCPProvisioner CR.
/// Uses the default DPUCluster from constants.
pub async fn create_dpfhcp_provisioner(env: &Env, ns: &str, name: &str) -> NodeResult {
    create_dpfhcp_provisioner_with_cluster(
        env,
        ns,
        name,
        DPU_CLUSTER_NAME,
        DPU_CLUSTER_NS,
        DPU_DEPLOYMENT_NAME,
    )
    .await
}

/// `createDPFHCPProvisionerWithCluster` creates a DPFHCPProvisioner CR
/// with a custom DPUCluster reference.
pub async fn create_dpfhcp_provisioner_with_cluster(
    env: &Env,
    ns: &str,
    name: &str,
    cluster_name: &str,
    cluster_ns: &str,
    deployment_name: &str,
) -> NodeResult {
    let release_image = detect_ocp_release_image(env).await?;
    let base_domain = get_base_domain(env)?;
    let availability = get_control_plane_availability();
    let machine_os_url = get_machine_os_url();
    let etcd_sc = get_etcd_storage_class(env).await?;

    gw!(
        "Creating DPFHCPProvisioner with:\n  releaseImage={}\n  baseDomain={}\n  availability={}\n  machineOSURL={}\n  etcdStorageClass={}\n  virtualIP=192.168.100.100 (dummy IP for AWS LoadBalancer mode)\n  dpuCluster={}/{}\n",
        release_image,
        base_domain,
        availability,
        machine_os_url,
        etcd_sc,
        cluster_ns,
        cluster_name
    );

    let mut provisioner = provisioningv1alpha1::DPFHCPProvisioner {
        metadata: meta(ns, name),
        spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
            dpu_cluster_ref: provisioningv1alpha1::DPUClusterReference {
                name: cluster_name.into(),
                namespace: cluster_ns.into(),
                ..Default::default()
            },
            base_domain,
            ocp_release_image: release_image,
            ssh_key_secret_ref: LocalObjectReference {
                name: SSH_KEY_SECRET_NAME.into(),
                ..Default::default()
            },
            pull_secret_ref: LocalObjectReference {
                name: PULL_SECRET_NAME.into(),
                ..Default::default()
            },
            control_plane_availability_policy: availability,
            // VirtualIP: Use a dummy IP to trigger LoadBalancer mode on AWS
            // AWS Cloud Controller will create an AWS NLB instead of using NodePort with internal IP
            // The actual VirtualIP value is not used on AWS (only for MetalLB on bare-metal)
            virtual_ip: "192.168.100.100".into(),
            dpu_deployment_ref: Some(provisioningv1alpha1::DPUDeploymentReference {
                name: deployment_name.into(),
                namespace: cluster_ns.into(),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    if !machine_os_url.is_empty() {
        provisioner.spec.machine_osurl = machine_os_url;
    }
    if !etcd_sc.is_empty() {
        provisioner.spec.etcd_storage_class = etcd_sc;
    }

    if let Err(e) = env.client.create(&mut provisioner).await
        && !is_already_exists(&e)
    {
        g::ok(Err::<(), _>(e), "Failed to create DPFHCPProvisioner")?;
    }
    Ok(())
}

/// `waitForCRPhase` waits for a DPFHCPProvisioner to reach a specific phase.
pub async fn wait_for_cr_phase(
    env: &Env,
    name: &str,
    phase: &str,
    timeout: Duration,
) -> NodeResult {
    eventually(
        timeout,
        POLLING_INTERVAL,
        &format!("Timed out waiting for phase {phase}"),
        async || {
            let current_phase = get_cr_phase(env, name).await;
            g::equal(
                &current_phase,
                phase,
                &format!("CR phase is {current_phase}, expected {phase}"),
            )?;
            Ok(())
        },
    )
    .await
}

/// `getCRPhase` gets the current phase of a DPFHCPProvisioner.
pub async fn get_cr_phase(env: &Env, name: &str) -> String {
    match env
        .client
        .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(CI_NAMESPACE, name))
        .await
    {
        Ok(p) => p.status.phase,
        Err(_) => String::new(),
    }
}

/// `getCRConditions` retrieves all conditions from the CR status.
pub async fn get_cr_conditions(env: &Env, ns: &str, name: &str) -> Vec<Condition> {
    match env
        .client
        .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(ns, name))
        .await
    {
        Ok(p) => p.status.conditions,
        Err(_) => Vec::new(),
    }
}

/// `getConditionStatus` returns the status of a specific condition type.
pub async fn get_condition_status(env: &Env, ns: &str, name: &str, condition_type: &str) -> String {
    let conditions = get_cr_conditions(env, ns, name).await;
    conditions
        .into_iter()
        .find(|c| c.type_ == condition_type)
        .map(|c| c.status)
        .unwrap_or_default()
}

/// `createDPUStub` creates a minimal DPU CR for CSR auto-approval testing.
pub async fn create_dpu_stub(env: &Env, ns: &str, name: &str, phase: &str) -> NodeResult {
    let mut dpu = dpuprovisioningv1::DPU {
        metadata: meta(ns, name),
        spec: dpuprovisioningv1::DPUSpec {
            dpu_node_name: name.into(),
            dpu_device_name: "bf3-device".into(),
            bfb: Some("e2e-bfb".into()),
            serial_number: "E2E0000000001".into(),
            dpu_flavor: DPU_FLAVOR_NAME.into(),
            node_effect: dpuprovisioningv1::NodeEffect {
                no_effect: Some(true),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    g::ok(
        env.client.create(&mut dpu).await,
        "Failed to create DPU stub",
    )?;

    // Re-fetch to get the latest state before updating status
    g::ok(
        env.client
            .get_into(&ObjectKey::new(ns, name), &mut dpu)
            .await,
        "Failed to get DPU after creation",
    )?;

    // Update status subresource
    dpu.status.phase = phase.into();
    g::ok(
        env.client.update_status(&mut dpu).await,
        "Failed to update DPU status",
    )?;
    Ok(())
}

/// `deleteDPUStub` deletes a DPU CR.
pub async fn delete_dpu_stub(env: &Env, ns: &str, name: &str) {
    let dpu = dpuprovisioningv1::DPU {
        metadata: meta(ns, name),
        ..Default::default()
    };
    // Equivalent to kubectl delete --ignore-not-found=true
    if let Err(e) = ignore_not_found(env.client.delete(&dpu).await) {
        warn_error(&format!("failed to delete DPU {ns}/{name}: {e}"));
    }
}

/// `getHostedClusterKubeconfig` retrieves the kubeconfig for the HostedCluster.
pub async fn get_hosted_cluster_kubeconfig(
    env: &Env,
    provisioner_ns: &str,
    provisioner_name: &str,
) -> NodeResult<String> {
    let provisioner = g::ok(
        env.client
            .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(
                provisioner_ns,
                provisioner_name,
            ))
            .await,
        "Failed to get DPFHCPProvisioner",
    )?;

    let secret_name = provisioner
        .status
        .kube_config_secret_ref
        .as_ref()
        .ok_or_else(|| go_panic(NIL_POINTER_DEREFERENCE))?
        .name
        .clone();
    g::not_empty(&secret_name, "Kubeconfig secret name is empty")?;

    // The kubeconfig secret is in the DPUCluster namespace (injected by controller)
    let secret = g::ok(
        env.client
            .get::<Secret>(&ObjectKey::new(DPU_CLUSTER_NS, &secret_name))
            .await,
        "Failed to get kubeconfig secret",
    )?;

    let b64_kubeconfig = secret
        .data
        .as_ref()
        .and_then(|d| d.get("super-admin.conf"))
        .map(|b| b.0.clone());
    g::not_nil(&b64_kubeconfig, "super-admin.conf not found in secret")?;
    Ok(gocompat::base64::encode_to_string(
        &b64_kubeconfig.unwrap_or_default(),
    ))
}

/// `writeKubeconfigToFile` writes base64-encoded kubeconfig to a temp file and returns the path.
/// The file is cleaned up in AfterAll when the entire test suite completes.
pub fn write_kubeconfig_to_file(b64_kubeconfig: &str) -> NodeResult<String> {
    use std::io::Write;
    let kubeconfig = g::ok(
        gocompat::base64::decode_string(b64_kubeconfig.as_bytes()),
        "Failed to decode kubeconfig",
    )?;

    let (name, mut f) = g::ok(
        create_temp("e2e-hc-kubeconfig-*"),
        "Failed to create temp kubeconfig file",
    )?;

    g::ok(
        f.write_all(&kubeconfig)
            .map_err(|e| format!("write {name}: {}", errno_text(&e))),
        "Failed to write kubeconfig file",
    )?;
    g::ok(
        f.sync_all()
            .map_err(|e| format!("close {name}: {}", errno_text(&e))),
        "Failed to close kubeconfig file",
    )?;
    Ok(name)
}

/// `warnError` logs a warning message for cleanup errors that are non-critical.
/// These warnings appear in test output but don't fail the test.
pub fn warn_error(message: &str) {
    gw!("Warning: {}\n", message);
}

/// `loadHCConfig` loads the rest.Config from a kubeconfig file path
/// (`clientcmd.BuildConfigFromFlags("", file)`: client-go default QPS 5 / Burst 10, no timeout).
/// This should be called once and the config reused.
pub async fn load_hc_config(kubeconfig_file: &str) -> NodeResult<RestConfig> {
    let res = async {
        let data = read_file(kubeconfig_file).map_err(|e| anyhow::anyhow!(e))?;
        let kc = parse_kubeconfig(&data)?;
        rest_config_from_kubeconfig_with(
            kc,
            &KubeconfigOverrides {
                server: None,
                timeout: Duration::ZERO,
                qps: 0.0,
                burst: 0,
            },
        )
        .await
    }
    .await;
    g::ok(res, "Failed to build config from kubeconfig file").map_err(Into::into)
}

/// `rest.Config.Host` as written in the kubeconfig.
fn host_of(config: &RestConfig) -> String {
    config.host().trim_end_matches('/').to_string()
}

/// `discoveryClient.ServerGroups()`: `/api` then `/apis` (NotFound/Forbidden tolerated).
async fn server_groups(rest: &RestClient) -> ctrlrt::Result<()> {
    for path in ["/api", "/apis"] {
        if let Err(e) = rest.get_json(path, None).await
            && !is_not_found(&e)
            && !is_forbidden(&e)
        {
            return Err(e);
        }
    }
    Ok(())
}

/// `waitForHostedClusterAPIReachable` waits for the HostedCluster kube-apiserver to be reachable.
/// This is needed because the HostedCluster can report "Available" before the kube-apiserver
/// pod is fully ready and the NodePort service is routing traffic.
pub async fn wait_for_hosted_cluster_api_reachable(
    hc_config: &RestConfig,
    timeout: Duration,
) -> NodeResult {
    let host = host_of(hc_config);
    gw!(
        "Waiting for HostedCluster API to be reachable at {}\n",
        host
    );

    // Create discovery client once - config errors won't fix themselves by retrying
    // (discovery.NewDiscoveryClientForConfig: 32s timeout, burst 300).
    let mut discovery_config = hc_config.clone();
    if discovery_config.timeout.is_none() {
        discovery_config.timeout = Some(Duration::from_secs(32));
    }
    if discovery_config.burst == 0 && discovery_config.qps < 100.0 {
        discovery_config.burst = 300;
    }
    let discovery_client = g::ok(
        discovery_config.build_client(),
        "Failed to create discovery client",
    )?;
    let rest = RestClient::new(discovery_client).with_base(host.clone());

    // Test the /api endpoint that client.Create() will actually use for resource operations
    eventually(
        timeout,
        Duration::from_secs(5),
        &format!(
            "HostedCluster API server did not become reachable after {} at {}",
            go_duration(timeout),
            host
        ),
        async || {
            g::succeed(
                &server_groups(&rest).await,
                "API server discovery not yet reachable",
            )?;
            Ok(())
        },
    )
    .await?;

    gw!("HostedCluster API is now reachable\n");
    Ok(())
}

/// `getHCClient` creates a Kubernetes client from a rest.Config.
pub fn get_hc_client(config: &RestConfig) -> NodeResult<Client> {
    let (hc_client, _) = g::ok(
        ctrlrt_runtime::client_from_rest_config(config),
        "Failed to create HC client",
    )?;
    Ok(hc_client)
}

/// `getHCClientWithImpersonation` creates a client for the HostedCluster with user impersonation.
/// This allows creating resources (like CSRs) that appear to come from a specific user/group.
pub fn get_hc_client_with_impersonation(
    base_config: &RestConfig,
    username: &str,
    groups: &[&str],
) -> NodeResult<Client> {
    // Clone the config to avoid mutating the shared instance
    let mut config = base_config.clone();

    // Set impersonation to mimic the desired user identity
    config.kube.auth_info.impersonate = Some(username.to_string());
    config.kube.auth_info.impersonate_groups = Some(groups.iter().map(|g| g.to_string()).collect());

    let (hc_client, _) = g::ok(
        ctrlrt_runtime::client_from_rest_config(&config),
        "Failed to create HC client with impersonation",
    )?;
    Ok(hc_client)
}

fn csr(
    name: &str,
    request: Vec<u8>,
    signer_name: &str,
    usages: &[&str],
) -> CertificateSigningRequest {
    CertificateSigningRequest {
        metadata: meta("", name),
        spec: CertificateSigningRequestSpec {
            request: ByteString(request),
            signer_name: signer_name.into(),
            usages: Some(usages.iter().map(|u| u.to_string()).collect()),
            // Note: Username and Groups are server-populated based on the authenticated user.
            // We use impersonation above to set the identity.
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `createBootstrapCSRInHostedCluster` creates a bootstrap CSR in the HostedCluster.
/// Bootstrap CSRs come from the node-bootstrapper service account, not from the node itself.
pub async fn create_bootstrap_csr_in_hosted_cluster(
    hc_config: &RestConfig,
    hostname: &str,
) -> NodeResult<String> {
    let csr_name = format!("e2e-bootstrap-csr-{hostname}-{}", unix_nano());

    // Generate key and CSR PEM with CN=system:node:<hostname>, O=system:nodes, no SANs
    let key_file = format!("/tmp/e2e-csr-key-{}", unix_nano());
    let csr_file = format!("/tmp/e2e-csr-{}.pem", unix_nano());
    let _cleanup = RemoveOnDrop(vec![key_file.clone(), csr_file.clone()]);

    let mut cmd = Command::new("openssl");
    cmd.args([
        "req",
        "-new",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-keyout",
        &key_file,
        "-out",
        &csr_file,
        "-subj",
        &format!("/CN=system:node:{hostname}/O=system:nodes"),
    ]);
    g::ok(utils::run(cmd), "Failed to generate bootstrap CSR")?;

    let b64_csr = base64_encode_file(&csr_file)?;
    let csr_bytes = g::ok(
        gocompat::base64::decode_string(b64_csr.as_bytes()),
        "Failed to decode CSR",
    )?;

    // Create client with impersonation to mimic node-bootstrapper service account
    let hc_client = get_hc_client_with_impersonation(
        hc_config,
        "system:serviceaccount:openshift-machine-config-operator:node-bootstrapper",
        &[
            "system:serviceaccounts",
            "system:serviceaccounts:openshift-machine-config-operator",
            "system:authenticated",
        ],
    )?;

    let mut csr = csr(
        &csr_name,
        csr_bytes,
        "kubernetes.io/kube-apiserver-client-kubelet",
        &["digital signature", "client auth"],
    );
    g::ok(
        hc_client.create(&mut csr).await,
        "Failed to create bootstrap CSR in hosted cluster",
    )?;

    Ok(csr_name)
}

/// The OpenSSL config used for serving CSRs (adds the DNS SAN).
pub fn serving_csr_openssl_config(hostname: &str) -> String {
    format!(
        "[req]
req_extensions = v3_req
distinguished_name = req_distinguished_name
prompt = no

[req_distinguished_name]
CN = system:node:{hostname}
O = system:nodes

[v3_req]
subjectAltName = DNS:{hostname}
"
    )
}

/// `createServingCSRInHostedCluster` creates a serving CSR in the HostedCluster.
/// Serving CSRs come from the node itself (system:node:<hostname>).
pub async fn create_serving_csr_in_hosted_cluster(
    hc_config: &RestConfig,
    hostname: &str,
) -> NodeResult<String> {
    let csr_name = format!("e2e-serving-csr-{hostname}-{}", unix_nano());

    // Generate key and CSR PEM with CN=system:node:<hostname>, O=system:nodes, DNS SAN=hostname
    let key_file = format!("/tmp/e2e-csr-key-{}", unix_nano());
    let csr_file = format!("/tmp/e2e-csr-{}.pem", unix_nano());
    let conf_file = format!("/tmp/e2e-csr-{}.cnf", unix_nano());
    let _cleanup = RemoveOnDrop(vec![key_file.clone(), csr_file.clone(), conf_file.clone()]);

    // OpenSSL config to add DNS SAN
    let conf = serving_csr_openssl_config(hostname);
    g::ok(
        write_file(&conf_file, conf.as_bytes(), 0o600),
        "Failed to write openssl config",
    )?;

    let mut cmd = Command::new("openssl");
    cmd.args([
        "req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", &key_file, "-out", &csr_file,
        "-config", &conf_file,
    ]);
    g::ok(utils::run(cmd), "Failed to generate serving CSR")?;

    let b64_csr = base64_encode_file(&csr_file)?;
    let csr_bytes = g::ok(
        gocompat::base64::decode_string(b64_csr.as_bytes()),
        "Failed to decode CSR",
    )?;

    // Create client with impersonation to mimic the node itself
    let hc_client = get_hc_client_with_impersonation(
        hc_config,
        &format!("system:node:{hostname}"),
        &["system:nodes", "system:authenticated"],
    )?;

    let mut csr = csr(
        &csr_name,
        csr_bytes,
        "kubernetes.io/kubelet-serving",
        &["digital signature", "server auth"],
    );
    g::ok(
        hc_client.create(&mut csr).await,
        "Failed to create serving CSR in hosted cluster",
    )?;

    Ok(csr_name)
}

/// `base64EncodeFile` reads a file and returns its base64-encoded content.
pub fn base64_encode_file(path: &str) -> NodeResult<String> {
    let data = g::ok(read_file(path), &format!("Failed to read file {path}"))?;
    Ok(gocompat::base64::encode_to_string(&data))
}

fn csr_approved(csr: &CertificateSigningRequest) -> bool {
    csr.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Approved"))
}

/// `waitForCSRApproval` waits for a CSR to be approved in the HostedCluster.
pub async fn wait_for_csr_approval(
    hc_config: &RestConfig,
    csr_name: &str,
    timeout: Duration,
) -> NodeResult {
    let hc_client = get_hc_client(hc_config)?;

    eventually(
        timeout,
        Duration::from_secs(5),
        &format!("Timed out waiting for CSR {csr_name} approval"),
        async || {
            let csr = g::ok(
                hc_client
                    .get::<CertificateSigningRequest>(&ObjectKey::cluster(csr_name))
                    .await,
                "",
            )?;
            g::be_true(csr_approved(&csr), "CSR not yet approved")?;
            Ok(())
        },
    )
    .await
}

/// `verifyCSRNotApproved` verifies a CSR stays pending (not approved) for a duration.
pub async fn verify_csr_not_approved(
    hc_config: &RestConfig,
    csr_name: &str,
    duration: Duration,
) -> NodeResult {
    let hc_client = get_hc_client(hc_config)?;

    consistently(duration, Duration::from_secs(5), "", async || {
        let csr = g::ok(
            hc_client
                .get::<CertificateSigningRequest>(&ObjectKey::cluster(csr_name))
                .await,
            "",
        )?;
        for condition in csr.status.and_then(|s| s.conditions).unwrap_or_default() {
            if condition.type_ == "Approved" {
                g::be_true(false, "CSR should not be approved")?;
            }
        }
        Ok(())
    })
    .await
}

/// `createNodeInHostedCluster` creates a Node object in the HostedCluster.
pub async fn create_node_in_hosted_cluster(hc_config: &RestConfig, hostname: &str) -> NodeResult {
    let hc_client = get_hc_client(hc_config)?;

    let mut node = Node {
        metadata: meta("", hostname),
        ..Default::default()
    };
    g::ok(
        hc_client.create(&mut node).await,
        "Failed to create Node in hosted cluster",
    )?;
    Ok(())
}

/// `deleteNodeInHostedCluster` deletes a Node object from the HostedCluster.
pub async fn delete_node_in_hosted_cluster(hc_config: &RestConfig, hostname: &str) -> NodeResult {
    let hc_client = get_hc_client(hc_config)?;

    let node = Node {
        metadata: meta("", hostname),
        ..Default::default()
    };
    if let Err(e) = ignore_not_found(hc_client.delete(&node).await) {
        warn_error(&format!("failed to delete Node {hostname}: {e}"));
    }
    Ok(())
}

/// `deleteCSRInHostedCluster` deletes a CSR from the HostedCluster.
pub async fn delete_csr_in_hosted_cluster(hc_config: &RestConfig, csr_name: &str) -> NodeResult {
    let hc_client = get_hc_client(hc_config)?;

    let csr = CertificateSigningRequest {
        metadata: meta("", csr_name),
        ..Default::default()
    };
    if let Err(e) = ignore_not_found(hc_client.delete(&csr).await) {
        warn_error(&format!("failed to delete CSR {csr_name}: {e}"));
    }
    Ok(())
}

/// `waitForDeletion` polls for a resource to be deleted, returning true if deleted before the
/// timeout (Go: context deadline) expires. Polls on a ticker (first check after one interval).
pub async fn wait_for_deletion(
    env: &Env,
    key: &ObjectKey,
    timeout: Duration,
    poll_interval: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut ticker =
        tokio::time::interval_at(tokio::time::Instant::now() + poll_interval, poll_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return false, // Timeout
            _ = ticker.tick() => {
                let res = tokio::time::timeout_at(
                    deadline,
                    env.client.get::<provisioningv1alpha1::DPFHCPProvisioner>(key),
                )
                .await;
                if let Ok(Err(e)) = res
                    && is_not_found(&e)
                {
                    return true; // Deleted
                }
            }
        }
    }
}

/// `forceDeleteProvisioner` deletes a DPFHCPProvisioner CR, removing finalizers if stuck.
pub async fn force_delete_provisioner(env: &Env, ns: &str, name: &str) -> NodeResult {
    let key = ObjectKey::new(ns, name);

    // Check if CR exists
    let provisioner = match env
        .client
        .get::<provisioningv1alpha1::DPFHCPProvisioner>(&key)
        .await
    {
        Err(e) if is_not_found(&e) => return Ok(()), // Doesn't exist
        // Go deletes the (unfilled) object on other Get errors.
        Err(_) => provisioningv1alpha1::DPFHCPProvisioner::default(),
        Ok(p) => p,
    };

    // Request deletion
    if let Err(e) = ignore_not_found(env.client.delete(&provisioner).await) {
        warn_error(&format!("failed to delete provisioner {ns}/{name}: {e}"));
        return Ok(());
    }

    // Wait for operator to naturally remove finalizer and delete the CR (up to 2 minutes)
    // Use a non-asserting check so we can fall back to force-removal if needed
    let deleted = wait_for_deletion(env, &key, 2 * MINUTE, Duration::from_secs(10)).await;
    if deleted {
        return Ok(()); // Successfully deleted by operator
    }

    // Operator didn't remove finalizer within 2 minutes - force-remove as last resort
    gw!("CR still exists after 2m, force-removing finalizers...\n");
    let mut fresh_provisioner = match env
        .client
        .get::<provisioningv1alpha1::DPFHCPProvisioner>(&key)
        .await
    {
        Ok(p) => p,
        Err(e) if is_not_found(&e) => return Ok(()), // Already gone
        Err(e) => {
            warn_error(&format!(
                "failed to re-fetch provisioner for finalizer removal: {e}"
            ));
            return Ok(());
        }
    };

    fresh_provisioner.metadata.finalizers = Some(Vec::new());
    if let Err(e) = env.client.update(&mut fresh_provisioner).await {
        warn_error(&format!("failed to remove finalizers: {e}"));
        return Ok(());
    }

    // Wait for it to be gone after force-removing finalizers
    eventually(MINUTE, Duration::from_secs(5), "", async || {
        let err = env
            .client
            .get::<provisioningv1alpha1::DPFHCPProvisioner>(&key)
            .await
            .err();
        g::be_true(err.as_ref().is_some_and(is_not_found), "")?;
        Ok(())
    })
    .await?;

    // Clean up orphaned resources
    if let Err(e) = ignore_not_found(delete_all_of::<hyperv1::HostedCluster>(env, ns).await) {
        warn_error(&format!("failed to delete orphaned HostedClusters: {e}"));
    }
    if let Err(e) = ignore_not_found(delete_all_of::<hyperv1::NodePool>(env, ns).await) {
        warn_error(&format!("failed to delete orphaned NodePools: {e}"));
    }
    Ok(())
}

/// `cleanupStaleResources` removes leftover resources from previous failed test runs.
pub async fn cleanup_stale_resources(env: &Env) -> NodeResult {
    // Delete the main test provisioner (this should cascade delete HostedCluster and NodePools)
    force_delete_provisioner(env, CI_NAMESPACE, PROVISIONER_NAME).await?;

    // Delete the cleanup test provisioner (from previous failed runs)
    force_delete_provisioner(env, CI_NAMESPACE, CLEANUP_TEST_NAME).await?;

    // Clean up any orphaned HostedClusters and NodePools (best effort)
    if let Err(e) =
        ignore_not_found(delete_all_of::<hyperv1::HostedCluster>(env, CI_NAMESPACE).await)
    {
        warn_error(&format!("failed to cleanup orphaned HostedClusters: {e}"));
    }
    if let Err(e) = ignore_not_found(delete_all_of::<hyperv1::NodePool>(env, CI_NAMESPACE).await) {
        warn_error(&format!("failed to cleanup orphaned NodePools: {e}"));
    }

    // Clean up DPU resources in main test DPUCluster namespace (best effort)
    if let Err(e) =
        ignore_not_found(delete_all_of::<dpuprovisioningv1::DPU>(env, DPU_CLUSTER_NS).await)
    {
        warn_error(&format!("failed to cleanup DPUs: {e}"));
    }
    if let Err(e) =
        ignore_not_found(delete_all_of::<dpuprovisioningv1::DPUCluster>(env, DPU_CLUSTER_NS).await)
    {
        warn_error(&format!("failed to cleanup DPUClusters: {e}"));
    }
    if let Err(e) =
        ignore_not_found(delete_all_of::<dpuprovisioningv1::DPUFlavor>(env, DPU_CLUSTER_NS).await)
    {
        warn_error(&format!("failed to cleanup DPUFlavors: {e}"));
    }
    if let Err(e) =
        ignore_not_found(delete_all_of::<dpuservicev1::DPUDeployment>(env, DPU_CLUSTER_NS).await)
    {
        warn_error(&format!("failed to cleanup DPUDeployments: {e}"));
    }
    if let Err(e) = ignore_not_found(
        delete_all_of::<operatorv1::DPFOperatorConfig>(env, DPF_OPERATOR_CONFIG_NAMESPACE).await,
    ) {
        warn_error(&format!("failed to cleanup DPFOperatorConfigs: {e}"));
    }

    // Clean up DPU resources in cleanup test namespace (best effort)
    if let Err(e) = ignore_not_found(
        delete_all_of::<dpuprovisioningv1::DPUCluster>(env, CLEANUP_DPU_CLUSTER_NS).await,
    ) {
        warn_error(&format!("failed to cleanup cleanup test DPUClusters: {e}"));
    }
    if let Err(e) = ignore_not_found(
        delete_all_of::<dpuprovisioningv1::DPUFlavor>(env, CLEANUP_DPU_CLUSTER_NS).await,
    ) {
        warn_error(&format!("failed to cleanup cleanup test DPUFlavors: {e}"));
    }
    if let Err(e) = ignore_not_found(
        delete_all_of::<dpuservicev1::DPUDeployment>(env, CLEANUP_DPU_CLUSTER_NS).await,
    ) {
        warn_error(&format!(
            "failed to cleanup cleanup test DPUDeployments: {e}"
        ));
    }
    // Clean up secrets in operator namespace (best effort)
    let secret = Secret {
        metadata: meta(CI_NAMESPACE, SSH_KEY_SECRET_NAME),
        ..Default::default()
    };
    if let Err(e) = ignore_not_found(env.client.delete(&secret).await) {
        warn_error(&format!("failed to cleanup SSH key secret: {e}"));
    }

    let secret = Secret {
        metadata: meta(CI_NAMESPACE, PULL_SECRET_NAME),
        ..Default::default()
    };
    if let Err(e) = ignore_not_found(env.client.delete(&secret).await) {
        warn_error(&format!("failed to cleanup pull secret: {e}"));
    }

    // Give resources time to clean up
    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(())
}

/// `dumpProvisionerStatus` logs the current CR status for debugging.
pub async fn dump_provisioner_status(env: &Env, ns: &str, name: &str) {
    if let Ok(provisioner) = env
        .client
        .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(ns, name))
        .await
    {
        let output = gocompat::json::to_go_json_indent(&provisioner, "", "  ").unwrap_or_default();
        gw!(
            "DPFHCPProvisioner status:\n{}\n",
            String::from_utf8_lossy(&output)
        );
    }
}

/// `getUpgradeOCPReleaseImage` returns the upgrade target OCP release image from env var.
/// Returns empty string if not set (caller should skip the test).
pub fn get_upgrade_ocp_release_image() -> String {
    std::env::var("UPGRADE_OCP_RELEASE_IMAGE").unwrap_or_default()
}

/// `updateDPFHCPProvisionerReleaseImage` patches the DPFHCPProvisioner CR with a new ocpReleaseImage.
pub async fn update_dpfhcp_provisioner_release_image(
    env: &Env,
    ns: &str,
    name: &str,
    new_image: &str,
) -> NodeResult {
    let mut provisioner = g::ok(
        env.client
            .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(ns, name))
            .await,
        "Failed to get DPFHCPProvisioner for upgrade",
    )?;

    provisioner.spec.ocp_release_image = new_image.to_string();
    g::ok(
        env.client.update(&mut provisioner).await,
        "Failed to update DPFHCPProvisioner release image",
    )?;
    Ok(())
}

/// `getHostedClusterReleaseImage` returns the current release image from the HostedCluster spec.
pub async fn get_hosted_cluster_release_image(
    env: &Env,
    ns: &str,
    name: &str,
) -> NodeResult<String> {
    let hc = g::ok(
        env.client
            .get::<hyperv1::HostedCluster>(&ObjectKey::new(ns, name))
            .await,
        "Failed to get HostedCluster",
    )?;
    Ok(hc.spec.release.image)
}

/// `getNodePoolReleaseImage` returns the current release image from the NodePool spec.
pub async fn get_node_pool_release_image(env: &Env, ns: &str, name: &str) -> NodeResult<String> {
    let np = g::ok(
        env.client
            .get::<hyperv1::NodePool>(&ObjectKey::new(ns, name))
            .await,
        "Failed to get NodePool",
    )?;
    Ok(np.spec.release.image)
}

/// The name of the ignition ConfigMap for the test DPUCluster.
pub fn ignition_config_map_name() -> String {
    format!("bfcfg-{DPU_CLUSTER_NAME}.cfg")
}

/// `getIgnitionConfigMap` returns the ignition ConfigMap for the test DPUCluster, or nil if not found.
pub async fn get_ignition_config_map(env: &Env) -> NodeResult<Option<ConfigMap>> {
    let cm_name = ignition_config_map_name();
    match env
        .client
        .get::<ConfigMap>(&ObjectKey::new(DPU_CLUSTER_NS, &cm_name))
        .await
    {
        Err(e) if is_not_found(&e) => Ok(None),
        res => Ok(Some(g::ok(res, "Failed to get ignition ConfigMap")?)),
    }
}

/// `decodeTargetIgnition` extracts and decodes the target ignition from a live
/// ignition ConfigMap. The target ignition is embedded at /var/target.ign as
/// a gzip-compressed, base64-encoded file inside the live ignition JSON.
///
/// Go panics on unexpected value types (failed type assertions); those cases return an error
/// carrying the panic message, which fails the spec just the same.
pub fn decode_target_ignition(cm_data: &str) -> Result<Map<String, Value>, String> {
    let live_ign = gocompat::json::unmarshal_object(cm_data.as_bytes())
        .map_err(|e| format!("failed to parse live ignition: {e}"))?
        .unwrap_or_default();

    let storage = live_ign
        .get("storage")
        .and_then(Value::as_object)
        .ok_or_else(|| "no storage in live ignition".to_string())?;
    let files = storage
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| "no files in live ignition storage".to_string())?;

    for f in files {
        let file = f
            .as_object()
            .ok_or_else(|| type_assertion_panic(Some(f), "map[string]interface {}"))?;
        if file.get("path").and_then(Value::as_str) != Some("/var/target.ign") {
            continue;
        }
        let contents = file
            .get("contents")
            .and_then(Value::as_object)
            .ok_or_else(|| type_assertion_panic(file.get("contents"), "map[string]interface {}"))?;
        let source = contents
            .get("source")
            .and_then(Value::as_str)
            .ok_or_else(|| type_assertion_panic(contents.get("source"), "string"))?;

        let b64_data = source.strip_prefix("data:;base64,").unwrap_or(source);
        let compressed = gocompat::base64::decode_string(b64_data.as_bytes())
            .map_err(|e| format!("failed to decode target ignition base64: {e}"))?;
        let gz_reader = gocompat::gzip::Reader::new(&compressed)
            .map_err(|e| format!("failed to create gzip reader: {e}"))?;

        let buf = gz_reader
            .read_all()
            .map_err(|e| format!("failed to decompress target ignition: {e}"))?;

        let target_ign = gocompat::json::unmarshal_object(&buf)
            .map_err(|e| format!("failed to parse target ignition: {e}"))?;
        return Ok(target_ign.unwrap_or_default());
    }
    Err("/var/target.ign not found in live ignition".into())
}

/// `getTargetIgnitionFiles` returns the files list from a decoded target ignition.
pub fn get_target_ignition_files(
    target_ign: &Map<String, Value>,
) -> NodeResult<Vec<Map<String, Value>>> {
    let Some(storage) = target_ign.get("storage").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let Some(files) = storage.get("files").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for f in files {
        match f.as_object() {
            Some(m) => result.push(m.clone()),
            None => {
                return Err(Interrupt::Fail(type_assertion_panic(
                    Some(f),
                    "map[string]interface {}",
                )));
            }
        }
    }
    Ok(result)
}

/// `decodeIgnitionFileContent` decodes the content of a file entry from the target
/// ignition, handling gzip+base64 and plain base64 data URIs.
pub fn decode_ignition_file_content(file: &Map<String, Value>) -> Result<String, String> {
    let contents = file
        .get("contents")
        .and_then(Value::as_object)
        .ok_or_else(|| "no contents in file".to_string())?;
    let source = contents
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| "no source in contents".to_string())?;

    let compression = contents
        .get("compression")
        .and_then(Value::as_str)
        .unwrap_or("");

    let b64_data = match source.find(";base64,") {
        Some(idx) => &source[idx + ";base64,".len()..],
        None => {
            let b = source.as_bytes();
            return Err(format!(
                "unsupported source format: {}",
                String::from_utf8_lossy(&b[..b.len().min(50)])
            ));
        }
    };

    let raw = gocompat::base64::decode_string(b64_data.as_bytes())
        .map_err(|e| format!("failed to decode base64: {e}"))?;

    if compression == "gzip" {
        let gz_reader = gocompat::gzip::Reader::new(&raw)
            .map_err(|e| format!("failed to create gzip reader: {e}"))?;
        let buf = gz_reader
            .read_all()
            .map_err(|e| format!("failed to decompress: {e}"))?;
        return Ok(String::from_utf8_lossy(&buf).into_owned());
    }

    Ok(String::from_utf8_lossy(&raw).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn gzip_b64(data: &[u8]) -> String {
        gocompat::base64::encode_to_string(&gocompat::gzip::compress(data, 9).unwrap())
    }

    fn live_ignition(target: &Value) -> String {
        let target = serde_json::to_vec(target).unwrap();
        json!({
            "ignition": {"version": "3.4.0"},
            "storage": {"files": [
                {"path": "/etc/other", "contents": {"source": "data:,x"}},
                {"path": "/var/target.ign", "contents": {"source": format!("data:;base64,{}", gzip_b64(&target))}}
            ]}
        })
        .to_string()
    }

    #[test]
    fn target_ignition_roundtrip() {
        let target = json!({
            "ignition": {"version": "3.4.0"},
            "storage": {"files": [
                {"path": "/etc/mellanox/mlnx-bf.conf", "contents": {"compression": "gzip", "source": format!("data:;base64,{}", gzip_b64(b"E2E_CUSTOM=\"true\"\n"))}},
                {"path": "/etc/dpf/e2e-custom.conf", "contents": {"source": format!("data:text/plain;charset=utf-8;base64,{}", gocompat::base64::encode_to_string(b"E2E_NEW_FILE=yes\n"))}},
            ]}
        });
        let decoded = decode_target_ignition(&live_ignition(&target)).unwrap();
        assert_eq!(Value::Object(decoded.clone()), target);
        let files = get_target_ignition_files(&decoded).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            decode_ignition_file_content(&files[0]).unwrap(),
            "E2E_CUSTOM=\"true\"\n"
        );
        assert_eq!(
            decode_ignition_file_content(&files[1]).unwrap().trim(),
            "E2E_NEW_FILE=yes"
        );
    }

    #[test]
    fn target_ignition_errors() {
        assert_eq!(
            decode_target_ignition("{}").unwrap_err(),
            "no storage in live ignition"
        );
        assert_eq!(
            decode_target_ignition("null").unwrap_err(),
            "no storage in live ignition"
        );
        assert_eq!(
            decode_target_ignition(r#"{"storage":{}}"#).unwrap_err(),
            "no files in live ignition storage"
        );
        assert_eq!(
            decode_target_ignition(r#"{"storage":{"files":[{"path":"/x"}]}}"#).unwrap_err(),
            "/var/target.ign not found in live ignition"
        );
        assert!(
            decode_target_ignition("[]")
                .unwrap_err()
                .starts_with("failed to parse live ignition: json: cannot unmarshal array")
        );
        assert_eq!(
            decode_target_ignition(r#"{"storage":{"files":[{"path":"/var/target.ign","contents":{"source":"data:;base64,!!"}}]}}"#)
                .unwrap_err(),
            "failed to decode target ignition base64: illegal base64 data at input byte 0"
        );
        assert_eq!(
            decode_target_ignition(r#"{"storage":{"files":[{"path":"/var/target.ign","contents":{"source":"data:;base64,AAAA"}}]}}"#)
                .unwrap_err(),
            "failed to create gzip reader: unexpected EOF"
        );
        assert_eq!(
            decode_target_ignition(r#"{"storage":{"files":[{"path":"/var/target.ign","contents":{"source":"data:;base64,AAAAAAAAAAAAAAAA"}}]}}"#)
                .unwrap_err(),
            "failed to create gzip reader: gzip: invalid header"
        );
        assert_eq!(
            decode_target_ignition(r#"{"storage":{"files":["x"]}}"#).unwrap_err(),
            "[PANICKED] interface conversion: interface {} is string, not map[string]interface {}"
        );
    }

    #[test]
    fn file_content_errors() {
        let f = |v: Value| v.as_object().unwrap().clone();
        assert_eq!(
            decode_ignition_file_content(&f(json!({}))).unwrap_err(),
            "no contents in file"
        );
        assert_eq!(
            decode_ignition_file_content(&f(json!({"contents": {}}))).unwrap_err(),
            "no source in contents"
        );
        assert_eq!(
            decode_ignition_file_content(&f(json!({"contents": {"source": "data:,plain"}})))
                .unwrap_err(),
            "unsupported source format: data:,plain"
        );
        assert_eq!(
            decode_ignition_file_content(&f(json!({"contents": {"source": "data:;base64,aGk="}})))
                .unwrap(),
            "hi"
        );
    }

    #[test]
    fn target_files_absent() {
        assert!(get_target_ignition_files(&Map::new()).unwrap().is_empty());
        let m = json!({"storage": {"files": [1]}});
        assert!(get_target_ignition_files(m.as_object().unwrap()).is_err());
    }

    #[test]
    fn nested_string_semantics() {
        let v = json!({"status": {"desired": {"image": "quay.io/x:1"}}});
        assert_eq!(
            nested_string(&v, &["status", "desired", "image"]).unwrap(),
            ("quay.io/x:1".to_string(), true)
        );
        assert_eq!(
            nested_string(&json!({"status": {}}), &["status", "desired", "image"]).unwrap(),
            (String::new(), false)
        );
        assert_eq!(
            nested_string(&json!({"status": null}), &["status", "desired", "image"]).unwrap(),
            (String::new(), false)
        );
        assert_eq!(
            nested_string(&json!({"status": "x"}), &["status", "desired", "image"]).unwrap_err(),
            ".status.desired accessor error: \"x\" is of the type string, expected map[string]interface{}"
        );
        assert_eq!(
            nested_string(
                &json!({"status": {"desired": {"image": 1}}}),
                &["status", "desired", "image"]
            )
            .unwrap_err(),
            ".status.desired.image accessor error: 1 is of the type float64, expected string"
        );
    }

    #[test]
    fn durations_and_names() {
        assert_eq!(go_duration(Duration::from_secs(300)), "5m0s");
        assert_eq!(go_duration(Duration::from_secs(3600)), "1h0m0s");
        assert_eq!(go_duration(Duration::from_secs(30)), "30s");
        assert_eq!(go_duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(ignition_config_map_name(), "bfcfg-e2e-dpucluster.cfg");
    }

    #[test]
    fn serving_csr_config() {
        assert_eq!(
            serving_csr_openssl_config("node-a"),
            "[req]\nreq_extensions = v3_req\ndistinguished_name = req_distinguished_name\nprompt = no\n\n\
             [req_distinguished_name]\nCN = system:node:node-a\nO = system:nodes\n\n[v3_req]\nsubjectAltName = DNS:node-a\n"
        );
    }

    #[test]
    fn stub_objects_serialize_like_go() {
        let cfg = dpf_operator_config_stub();
        let v = ctrlrt::client::to_value(&cfg).unwrap();
        assert_eq!(
            v["spec"],
            json!({
                "deploymentMode": "host-trusted",
                "provisioningController": {"bfbPVCName": "e2e-bfb-pvc", "installInterface": {"installViaHostAgent": {}}},
                "networking": {"controlPlaneMTU": 1500}
            })
        );
    }
}
