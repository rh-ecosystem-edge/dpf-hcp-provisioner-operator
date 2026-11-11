//! Port of `api/v1alpha1/dpfhcpprovisioner_types_test.go` ("DPFHCPProvisioner API Types").

use std::collections::BTreeMap;

use apis::hypershift::v1beta1 as hyperv1;
use apis::k8s::core::v1::{LocalObjectReference, ObjectReference};
use apis::provisioning::v1alpha1::*;
use ctrlrt::Object;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

fn local_ref(name: &str) -> LocalObjectReference {
    LocalObjectReference {
        name: name.into(),
        ..Default::default()
    }
}

fn meta(name: &str, namespace: &str) -> ObjectMeta {
    ObjectMeta {
        name: (!name.is_empty()).then(|| name.to_string()),
        namespace: (!namespace.is_empty()).then(|| namespace.to_string()),
        ..Default::default()
    }
}

fn selector(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// Context("JSON Serialization")

#[test]
fn json_serialization_should_successfully_serialize_and_deserialize_a_complete_dpfhcpprovisioner() {
    let original = DPFHCPProvisioner {
        api_version: "provisioning.dpu.hcp.io/v1alpha1".into(),
        kind: "DPFHCPProvisioner".into(),
        metadata: meta("test-provisioner", "default"),
        spec: DPFHCPProvisionerSpec {
            dpu_cluster_ref: DPUClusterReference {
                name: "test-dpu".into(),
                namespace: "dpu-system".into(),
                ..Default::default()
            },
            base_domain: "clusters.example.com".into(),
            ocp_release_image: "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi".into(),
            ssh_key_secret_ref: local_ref("test-ssh-key"),
            pull_secret_ref: local_ref("test-pull-secret"),
            etcd_storage_class: "ceph-rbd-retain".into(),
            control_plane_availability_policy: hyperv1::HIGHLY_AVAILABLE.into(),
            virtual_ip: "192.168.1.100".into(),
            node_selector: selector(&[
                ("node-role.kubernetes.io/master", ""),
                ("topology.kubernetes.io/zone", "us-east-1a"),
            ]),
            ..Default::default()
        },
        status: DPFHCPProvisionerStatus {
            phase: PHASE_READY.into(),
            conditions: vec![ctrlrt::meta::condition(
                "Ready",
                ctrlrt::meta::CONDITION_TRUE,
                "ProvisionerReady",
                "All components operational",
            )]
            .into_iter()
            .map(|mut c| {
                c.last_transition_time = ctrlrt::meta::now();
                c
            })
            .collect(),
            hosted_cluster_ref: Some(ObjectReference {
                api_version: "hypershift.openshift.io/v1beta1".into(),
                kind: "HostedCluster".into(),
                name: "test-provisioner".into(),
                namespace: "default".into(),
                ..Default::default()
            }),
            kube_config_secret_ref: Some(local_ref("test-provisioner-kubeconfig")),
            ..Default::default()
        },
        ..Default::default()
    };

    // Marshal to JSON
    let json_data = serde_json::to_vec(&original).expect("marshal");
    assert!(!json_data.is_empty());

    // Unmarshal back
    let deserialized: DPFHCPProvisioner = serde_json::from_slice(&json_data).expect("unmarshal");

    // Verify critical spec fields
    assert_eq!(deserialized.spec.base_domain, original.spec.base_domain);
    assert_eq!(
        deserialized.spec.dpu_cluster_ref.name,
        original.spec.dpu_cluster_ref.name
    );
    assert_eq!(
        deserialized.spec.dpu_cluster_ref.namespace,
        original.spec.dpu_cluster_ref.namespace
    );
    assert_eq!(
        deserialized.spec.ocp_release_image,
        original.spec.ocp_release_image
    );
    assert_eq!(
        deserialized.spec.etcd_storage_class,
        original.spec.etcd_storage_class
    );
    assert_eq!(deserialized.spec.virtual_ip, original.spec.virtual_ip);
    assert_eq!(
        deserialized.spec.control_plane_availability_policy,
        original.spec.control_plane_availability_policy
    );
    assert_eq!(deserialized.spec.node_selector, original.spec.node_selector);

    // Verify status fields
    assert_eq!(deserialized.status.phase, original.status.phase);
    assert_eq!(
        deserialized.status.conditions.len(),
        original.status.conditions.len()
    );
}

// Context("DeepCopy")

#[test]
fn deep_copy_should_create_an_independent_copy_of_dpfhcpprovisioner() {
    let original = DPFHCPProvisioner {
        metadata: meta("test-provisioner", "default"),
        spec: DPFHCPProvisionerSpec {
            dpu_cluster_ref: DPUClusterReference {
                name: "test-dpu".into(),
                namespace: "dpu-system".into(),
                ..Default::default()
            },
            base_domain: "clusters.example.com".into(),
            ocp_release_image: "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi".into(),
            ssh_key_secret_ref: local_ref("test-ssh-key"),
            pull_secret_ref: local_ref("test-pull-secret"),
            etcd_storage_class: "ceph-rbd-retain".into(),
            ..Default::default()
        },
        status: DPFHCPProvisionerStatus {
            phase: PHASE_READY.into(),
            hosted_cluster_ref: Some(ObjectReference {
                name: "test-provisioner".into(),
                ..Default::default()
            }),
            kube_config_secret_ref: Some(local_ref("test-provisioner-kubeconfig")),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut copied = original.clone();

    // Verify it's a different object
    assert!(!std::ptr::eq(&copied, &original));

    // Verify fields are equal
    assert_eq!(copied.name(), original.name());
    assert_eq!(copied.namespace(), original.namespace());
    assert_eq!(copied.spec.base_domain, original.spec.base_domain);
    assert_eq!(copied.status.phase, original.status.phase);

    // Modify copy and ensure original is unchanged
    copied.spec.base_domain = "modified.example.com".into();
    assert_eq!(original.spec.base_domain, "clusters.example.com");
    assert_eq!(copied.spec.base_domain, "modified.example.com");

    // Verify pointer fields are deep copied
    assert!(!std::ptr::eq(
        copied.status.hosted_cluster_ref.as_ref().unwrap(),
        original.status.hosted_cluster_ref.as_ref().unwrap()
    ));
    assert!(!std::ptr::eq(
        copied.status.kube_config_secret_ref.as_ref().unwrap(),
        original.status.kube_config_secret_ref.as_ref().unwrap()
    ));

    // Modify pointer field in copy
    copied.status.hosted_cluster_ref.as_mut().unwrap().name = "modified-cluster".into();
    assert_eq!(
        original.status.hosted_cluster_ref.as_ref().unwrap().name,
        "test-provisioner"
    );
}

#[test]
fn deep_copy_should_create_an_independent_copy_of_dpfhcpprovisioner_list() {
    let original = DPFHCPProvisionerList {
        items: Some(vec![
            DPFHCPProvisioner {
                metadata: meta("provisioner-1", ""),
                spec: DPFHCPProvisionerSpec {
                    base_domain: "test1.example.com".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            DPFHCPProvisioner {
                metadata: meta("provisioner-2", ""),
                spec: DPFHCPProvisionerSpec {
                    base_domain: "test2.example.com".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        ]),
        ..Default::default()
    };

    let mut copied = original.clone();

    assert!(!std::ptr::eq(&copied, &original));
    assert_eq!(
        copied.items.as_ref().unwrap().len(),
        original.items.as_ref().unwrap().len()
    );

    // Modify copy and ensure original is unchanged
    copied.items.as_mut().unwrap()[0].spec.base_domain = "modified.example.com".into();
    assert_eq!(
        original.items.as_ref().unwrap()[0].spec.base_domain,
        "test1.example.com"
    );
}

// Context("Scheme Registration")

#[test]
fn scheme_registration_should_register_dpfhcpprovisioner_types_in_the_scheme() {
    let info = DPFHCPProvisioner::resource_info();
    assert_eq!(info.group, GROUP);
    assert_eq!(info.version, VERSION);
    assert_eq!(info.kind, "DPFHCPProvisioner");
    assert_eq!(info.plural, "dpfhcpprovisioners");
    assert!(info.namespaced);

    let cfg = DPFHCPProvisionerConfig::resource_info();
    assert_eq!(cfg.api_version(), "provisioning.dpu.hcp.io/v1alpha1");
    assert_eq!(cfg.kind, "DPFHCPProvisionerConfig");
    assert_eq!(cfg.plural, "dpfhcpprovisionerconfigs");
    assert!(!cfg.namespaced);
}

#[test]
fn scheme_registration_should_register_dpfhcpprovisioner_list_in_the_scheme() {
    // Lists are plain typed structs in Rust (the client returns Vec<T>); verify the list type
    // decodes a List document of the registered kind.
    let list: DPFHCPProvisionerList = serde_json::from_str(
        r#"{"kind":"DPFHCPProvisionerList","apiVersion":"provisioning.dpu.hcp.io/v1alpha1","metadata":{},"items":[{"metadata":{"name":"a"}}]}"#,
    )
    .unwrap();
    assert_eq!(list.kind, "DPFHCPProvisionerList");
    assert_eq!(list.items.unwrap()[0].name(), "a");
}

#[test]
fn scheme_registration_should_integrate_with_client_go_scheme() {
    let mut provisioner = DPFHCPProvisioner::default();
    let info = DPFHCPProvisioner::resource_info();
    provisioner.api_version = info.api_version();
    provisioner.kind = info.kind.clone();

    assert_eq!(provisioner.kind, "DPFHCPProvisioner");
    let v = ctrlrt::client::to_value(&provisioner).unwrap();
    assert_eq!(v["kind"], "DPFHCPProvisioner");
    assert_eq!(v["apiVersion"], "provisioning.dpu.hcp.io/v1alpha1");
}

// Context("Phase Enum")

#[test]
fn phase_enum_should_have_correct_phase_enum_values() {
    assert_eq!(PHASE_PENDING, "Pending");
    assert_eq!(PHASE_WAITING_FOR_CONTROL_PLANE, "WaitingForControlPlane");
    assert_eq!(PHASE_GENERATING_IGNITION, "GeneratingIgnition");
    assert_eq!(
        PHASE_CLUSTER_VERSION_PROGRESSING,
        "ClusterVersionProgressing"
    );
    assert_eq!(PHASE_READY, "Ready");
    assert_eq!(PHASE_FAILED, "Failed");
    assert_eq!(PHASE_ERROR, "Error");
    assert_eq!(PHASE_DELETING, "Deleting");
}

#[test]
fn phase_enum_should_allow_all_valid_phases_to_be_set() {
    let valid_phases: [DPFHCPProvisionerPhase; 8] = [
        PHASE_PENDING.into(),
        PHASE_WAITING_FOR_CONTROL_PLANE.into(),
        PHASE_GENERATING_IGNITION.into(),
        PHASE_CLUSTER_VERSION_PROGRESSING.into(),
        PHASE_READY.into(),
        PHASE_FAILED.into(),
        PHASE_ERROR.into(),
        PHASE_DELETING.into(),
    ];

    for phase in valid_phases {
        let provisioner = DPFHCPProvisioner {
            status: DPFHCPProvisionerStatus {
                phase: phase.clone(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(provisioner.status.phase, phase);
    }
}

// Context("NodeSelector")

#[test]
fn node_selector_should_allow_empty_nil_node_selector_optional_field() {
    let provisioner = DPFHCPProvisioner {
        spec: DPFHCPProvisionerSpec {
            dpu_cluster_ref: DPUClusterReference {
                name: "test".into(),
                namespace: "default".into(),
                ..Default::default()
            },
            base_domain: "test.example.com".into(),
            ocp_release_image: "quay.io/test:4.19.0".into(),
            ssh_key_secret_ref: local_ref("ssh"),
            pull_secret_ref: local_ref("pull"),
            ..Default::default()
        },
        ..Default::default()
    };

    assert!(provisioner.spec.node_selector.is_empty());
    // and it is omitted from the encoding (omitempty)
    let v = serde_json::to_value(&provisioner).unwrap();
    assert!(v["spec"].get("nodeSelector").is_none());
}

#[test]
fn node_selector_should_support_master_node_selector() {
    let provisioner = DPFHCPProvisioner {
        spec: DPFHCPProvisionerSpec {
            node_selector: selector(&[("node-role.kubernetes.io/master", "")]),
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(provisioner.spec.node_selector.len(), 1);
    assert_eq!(
        provisioner.spec.node_selector["node-role.kubernetes.io/master"],
        ""
    );
}

#[test]
fn node_selector_should_support_worker_node_selector() {
    let provisioner = DPFHCPProvisioner {
        spec: DPFHCPProvisionerSpec {
            node_selector: selector(&[("node-role.kubernetes.io/worker", "")]),
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(provisioner.spec.node_selector.len(), 1);
    assert_eq!(
        provisioner.spec.node_selector["node-role.kubernetes.io/worker"],
        ""
    );
}

#[test]
fn node_selector_should_support_multiple_node_selectors() {
    let provisioner = DPFHCPProvisioner {
        spec: DPFHCPProvisionerSpec {
            node_selector: selector(&[
                ("node-role.kubernetes.io/master", ""),
                ("topology.kubernetes.io/zone", "us-east-1a"),
                ("hardware", "gpu"),
            ]),
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(provisioner.spec.node_selector.len(), 3);
    assert_eq!(
        provisioner.spec.node_selector["node-role.kubernetes.io/master"],
        ""
    );
    assert_eq!(
        provisioner.spec.node_selector["topology.kubernetes.io/zone"],
        "us-east-1a"
    );
    assert_eq!(provisioner.spec.node_selector["hardware"], "gpu");
}

#[test]
fn node_selector_should_deep_copy_node_selector_map() {
    let original = DPFHCPProvisioner {
        spec: DPFHCPProvisionerSpec {
            node_selector: selector(&[
                ("node-role.kubernetes.io/master", ""),
                ("zone", "us-east-1"),
            ]),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut copied = original.clone();

    // Verify map is copied
    assert_eq!(copied.spec.node_selector, original.spec.node_selector);

    // Modify copy and ensure original is unchanged
    copied
        .spec
        .node_selector
        .insert("zone".into(), "us-west-2".into());
    copied
        .spec
        .node_selector
        .insert("new-key".into(), "new-value".into());

    assert_eq!(original.spec.node_selector["zone"], "us-east-1");
    assert!(!original.spec.node_selector.contains_key("new-key"));
    assert_eq!(copied.spec.node_selector["zone"], "us-west-2");
    assert_eq!(copied.spec.node_selector["new-key"], "new-value");
}

// Methods (dpfhcpprovisioner_types.go)

#[test]
fn should_expose_through_load_balancer_and_is_vip_required() {
    let mk = |policy: &str, vip: &str| DPFHCPProvisioner {
        spec: DPFHCPProvisionerSpec {
            control_plane_availability_policy: policy.into(),
            virtual_ip: vip.into(),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(mk(hyperv1::HIGHLY_AVAILABLE, "").should_expose_through_load_balancer());
    assert!(mk(hyperv1::HIGHLY_AVAILABLE, "1.2.3.4").should_expose_through_load_balancer());
    assert!(mk(hyperv1::SINGLE_REPLICA, "1.2.3.4").should_expose_through_load_balancer());
    assert!(!mk(hyperv1::SINGLE_REPLICA, "").should_expose_through_load_balancer());
    assert!(!mk("", "1.2.3.4").should_expose_through_load_balancer());
    assert!(mk(hyperv1::HIGHLY_AVAILABLE, "").is_vip_required());
    assert!(!mk(hyperv1::SINGLE_REPLICA, "1.2.3.4").is_vip_required());
    assert!(!mk("", "").is_vip_required());
}

#[test]
fn constants_match_go() {
    assert_eq!(DEFAULT_CONFIG_NAME, "default");
    assert_eq!(hyperv1::HIGHLY_AVAILABLE, "HighlyAvailable");
    assert_eq!(hyperv1::SINGLE_REPLICA, "SingleReplica");
    assert_eq!(REASON_KUBE_CONFIG_INJECTED, "Injected");
    assert_eq!(REASON_CSR_APPROVAL_ACTIVE, "Active");
    assert_eq!(METAL_LB_CONFIGURED, "MetalLBConfigured");
}
