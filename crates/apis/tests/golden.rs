//! Golden differential tests against the Go types (fixtures from `hack/golden/apis`).

use std::fmt::Debug;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::dpf::operator::v1alpha1 as operatorv1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::hypershift::v1beta1 as hyperv1;
use apis::metallb::v1beta1 as metallbv1beta1;
use apis::openshift::config::v1 as configv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn check<T>(name: &str)
where
    T: Serialize + DeserializeOwned + Default + PartialEq + Debug,
{
    // (a) zero value
    assert_eq!(
        serde_json::to_string(&T::default()).unwrap(),
        fixture(&format!("{name}.zero.json")),
        "{name}: zero value encoding differs from Go"
    );
    // (b)->(c) decode the filled Go encoding, re-encode, compare bytes with Go's round trip
    let filled = fixture(&format!("{name}.filled.json"));
    let roundtrip = fixture(&format!("{name}.roundtrip.json"));
    let v: T = serde_json::from_str(&filled).unwrap_or_else(|e| panic!("{name}: decode: {e}"));
    let out = serde_json::to_string(&v).unwrap();
    if out != roundtrip {
        // Point at the first difference for easier debugging.
        let pos = out
            .bytes()
            .zip(roundtrip.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(out.len().min(roundtrip.len()));
        let lo = pos.saturating_sub(120);
        panic!(
            "{name}: round trip differs from Go at byte {pos}\n rust: ...{}\n   go: ...{}",
            &out[lo..(pos + 120).min(out.len())],
            &roundtrip[lo..(pos + 120).min(roundtrip.len())]
        );
    }
    // decode(encode(v)) == v
    let again: T = serde_json::from_str(&out).unwrap();
    assert_eq!(again, v, "{name}: decode(encode(v)) != v");

    // Unknown fields survive read-modify-write at the top level and in nested objects.
    let mut doc: Value = serde_json::from_str(&roundtrip).unwrap();
    doc["zzUnknown"] = json!({"a": [1, "b"]});
    if let Some(spec) = doc.get_mut("spec").and_then(Value::as_object_mut) {
        spec.insert("zzUnknownSpecField".into(), json!("x"));
    }
    if let Some(items) = doc.get_mut("items").and_then(Value::as_array_mut) {
        items[0]["zzUnknownItemField"] = json!(true);
    }
    let v: T = serde_json::from_value(doc.clone()).unwrap();
    let back = serde_json::to_value(&v).unwrap();
    assert_eq!(back, doc, "{name}: unknown fields not preserved");
}

macro_rules! golden {
    ($($test:ident: $ty:ty => $name:literal;)*) => {$(
        #[test]
        fn $test() {
            check::<$ty>($name);
        }
    )*};
}

golden! {
    dpfhcpprovisioner: provisioningv1alpha1::DPFHCPProvisioner => "DPFHCPProvisioner";
    dpfhcpprovisioner_list: provisioningv1alpha1::DPFHCPProvisionerList => "DPFHCPProvisionerList";
    dpfhcpprovisioner_config: provisioningv1alpha1::DPFHCPProvisionerConfig => "DPFHCPProvisionerConfig";
    dpfhcpprovisioner_config_list: provisioningv1alpha1::DPFHCPProvisionerConfigList => "DPFHCPProvisionerConfigList";
    dpu: dpuprovisioningv1alpha1::DPU => "DPU";
    dpu_list: dpuprovisioningv1alpha1::DPUList => "DPUList";
    dpu_cluster: dpuprovisioningv1alpha1::DPUCluster => "DPUCluster";
    dpu_cluster_list: dpuprovisioningv1alpha1::DPUClusterList => "DPUClusterList";
    dpu_flavor: dpuprovisioningv1alpha1::DPUFlavor => "DPUFlavor";
    dpu_flavor_list: dpuprovisioningv1alpha1::DPUFlavorList => "DPUFlavorList";
    dpu_deployment: dpuservicev1alpha1::DPUDeployment => "DPUDeployment";
    dpu_deployment_list: dpuservicev1alpha1::DPUDeploymentList => "DPUDeploymentList";
    dpu_service_template: dpuservicev1alpha1::DPUServiceTemplate => "DPUServiceTemplate";
    dpu_service_template_list: dpuservicev1alpha1::DPUServiceTemplateList => "DPUServiceTemplateList";
    dpf_operator_config: operatorv1alpha1::DPFOperatorConfig => "DPFOperatorConfig";
    dpf_operator_config_list: operatorv1alpha1::DPFOperatorConfigList => "DPFOperatorConfigList";
    hosted_cluster: hyperv1::HostedCluster => "HostedCluster";
    hosted_cluster_list: hyperv1::HostedClusterList => "HostedClusterList";
    node_pool: hyperv1::NodePool => "NodePool";
    node_pool_list: hyperv1::NodePoolList => "NodePoolList";
    ip_address_pool: metallbv1beta1::IPAddressPool => "IPAddressPool";
    ip_address_pool_list: metallbv1beta1::IPAddressPoolList => "IPAddressPoolList";
    l2_advertisement: metallbv1beta1::L2Advertisement => "L2Advertisement";
    l2_advertisement_list: metallbv1beta1::L2AdvertisementList => "L2AdvertisementList";
    cluster_version: configv1::ClusterVersion => "ClusterVersion";
    cluster_version_list: configv1::ClusterVersionList => "ClusterVersionList";
}

/// `internal/ignition/dpuflavor.go` `cleanDPUFlavor` + `json.Marshal` must be byte-identical.
#[test]
fn dpu_flavor_clean_json() {
    let f: dpuprovisioningv1alpha1::DPUFlavor =
        serde_json::from_str(&fixture("DPUFlavor.filled.json")).unwrap();
    let clean = dpuprovisioningv1alpha1::DPUFlavor {
        kind: f.kind.clone(),
        api_version: f.api_version.clone(),
        metadata: ObjectMeta {
            name: f.metadata.name.clone(),
            namespace: f.metadata.namespace.clone(),
            ..Default::default()
        },
        spec: f.spec.clone(),
        ..Default::default()
    };
    assert_eq!(
        serde_json::to_string(&clean).unwrap(),
        fixture("DPUFlavor.clean.json")
    );
}

#[test]
fn quantity_table() {
    let cases: Vec<Value> = serde_json::from_str(&fixture("quantity.json")).unwrap();
    for c in cases {
        let input = c["in"].as_str().unwrap();
        let got = apis::resource::canonical(input);
        match c.get("err").and_then(Value::as_str) {
            Some(err) => assert_eq!(
                got.as_ref().map_err(ToString::to_string),
                Err(err.to_string()),
                "quantity {input:?}"
            ),
            None => assert_eq!(
                got.as_deref(),
                Ok(c["out"].as_str().unwrap()),
                "quantity {input:?}"
            ),
        }
    }
}

#[test]
fn duration_table() {
    let cases: Vec<Value> = serde_json::from_str(&fixture("duration.json")).unwrap();
    for c in cases {
        let input = c["in"].as_str().unwrap();
        let got = apis::go::Duration::parse(input);
        match c.get("err").and_then(Value::as_str) {
            Some(err) => assert_eq!(got, Err(err.to_string()), "duration {input:?}"),
            None => {
                let d = got.unwrap_or_else(|e| panic!("duration {input:?}: {e}"));
                assert_eq!(
                    d.as_nanos(),
                    c["nanos"].as_i64().unwrap(),
                    "duration {input:?}"
                );
                assert_eq!(
                    d.to_string(),
                    c["out"].as_str().unwrap(),
                    "duration {input:?}"
                );
            }
        }
    }
}

#[test]
fn ipnet_table() {
    let cases: Vec<Value> = serde_json::from_str(&fixture("ipnet.json")).unwrap();
    for c in cases {
        let input = c["in"].as_str().unwrap();
        let got = apis::ipnet::IPNet::parse_cidr(input);
        match c.get("err").and_then(Value::as_str) {
            Some(err) => assert_eq!(
                got.map_err(|e| e.to_string()),
                Err(err.to_string()),
                "ipnet {input:?}"
            ),
            None => assert_eq!(
                serde_json::to_value(got.unwrap()).unwrap(),
                c["json"],
                "ipnet {input:?}"
            ),
        }
    }
}
