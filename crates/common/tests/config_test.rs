//! Behavioural tests for internal/common/config.go (no Go test file exists; these pin the
//! Go semantics used by callers).

use apis::dpf::operator::v1alpha1::DPFOperatorConfig;
use apis::provisioning::v1alpha1::DPFHCPProvisionerConfig;
use common::*;
use ctrlrt::Ctx;
use ctrlrt_fake::{FakeClientBuilder, Scheme, obj};

fn scheme() -> Scheme {
    Scheme::client_go()
        .register::<DPFHCPProvisionerConfig>()
        .register::<DPFOperatorConfig>()
}

#[tokio::test]
async fn load_operator_config_missing_returns_none() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    assert_eq!(
        load_operator_config_from_cr(&Ctx::background(), &c)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn load_operator_config_reads_spec() {
    let mut cr = DPFHCPProvisionerConfig::default();
    cr.metadata.name = Some("default".into());
    cr.spec.blue_field_ocp_layer_repo = "quay.io/x/y".into();
    cr.spec.disable_metal_lb = true;
    cr.spec.ovs_hugepages_size = "2Mi".into();
    cr.spec.ovs_hugepages_amount = 250;
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([obj(&cr)])
        .build();
    let cfg = load_operator_config_from_cr(&Ctx::background(), &c)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cfg.blue_field_ocp_layer_repo, "quay.io/x/y");
    assert!(cfg.disable_metal_lb);
    assert_eq!(cfg.ovs_hugepages_size, "2Mi");
    assert_eq!(cfg.ovs_hugepages_amount, 250);
}

#[tokio::test]
async fn dpf_version_requires_exactly_one_config() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let err = get_dpf_version(&c).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "expected exactly one DPFOperatorConfig, found 0"
    );

    let mut a = DPFOperatorConfig::default();
    a.metadata.name = Some("a".into());
    a.metadata.namespace = Some("dpf-operator-system".into());
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([obj(&a)])
        .build();
    let err = get_dpf_version(&c).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "DPFOperatorConfig.status.version is not set"
    );

    a.status.version = Some("v26.4.1".into());
    let mut b = a.clone();
    b.metadata.name = Some("b".into());
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([obj(&a)])
        .build();
    assert_eq!(get_dpf_version(&c).await.unwrap(), "v26.4.1");
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([obj(&a), obj(&b)])
        .build();
    assert_eq!(
        get_dpf_version(&c).await.unwrap_err().to_string(),
        "expected exactly one DPFOperatorConfig, found 2"
    );
}
