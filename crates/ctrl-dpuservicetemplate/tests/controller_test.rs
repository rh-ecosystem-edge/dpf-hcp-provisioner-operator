//! Additional tests (no Go counterpart): the `SetupWithManager` wiring of controller.go —
//! watched kinds, predicates and map functions.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrl_dpuservicetemplate::*;
use ctrlrt::k8s_openapi::api::apps::v1::DaemonSet;
use ctrlrt::k8s_openapi::api::core::v1::ConfigMap;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use ctrlrt::reconcile::RawEvent;
use ctrlrt::{
    Client, ControllerSpec, Ctx, ManagerHandle, Object, ObjectKey, Predicate, Recorder, Result,
};
use ctrlrt_fake::{FakeClientBuilder, Scheme, obj};

fn meta(name: &str, namespace: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.into()),
        namespace: Some(namespace.into()),
        ..Default::default()
    }
}

fn provisioner(
    name: &str,
    dpu_cluster_ns: &str,
    dd: Option<(&str, &str)>,
) -> provisioningv1alpha1::DPFHCPProvisioner {
    let mut p = provisioningv1alpha1::DPFHCPProvisioner {
        metadata: meta(name, "default"),
        ..Default::default()
    };
    p.spec.dpu_cluster_ref.namespace = dpu_cluster_ns.into();
    p.spec.dpu_deployment_ref = dd.map(|(n, ns)| provisioningv1alpha1::DPUDeploymentReference {
        name: n.into(),
        namespace: ns.into(),
        ..Default::default()
    });
    p
}

fn reconciler(c: &Client) -> Arc<DPUServiceTemplateReconciler> {
    Arc::new(DPUServiceTemplateReconciler::new(
        c.clone(),
        Arc::new(DPUServiceTemplateManager::new(
            c.clone(),
            c.clone(),
            Arc::new(RemoteReleaseImageReader),
            "operator-ns",
        )),
    ))
}

fn client() -> Client {
    FakeClientBuilder::new()
        .with_scheme(Scheme::new().register::<provisioningv1alpha1::DPFHCPProvisioner>())
        .with_objects([
            obj(&provisioner("a", "ns1", Some(("dd", "dd-ns")))),
            obj(&provisioner("b", "ns2", Some(("other", "dd-ns")))),
            obj(&provisioner("c", "ns1", Some(("dd", "dd-ns")))),
            obj(&provisioner("d", "ns3", None)),
            obj(&provisioner("e", "ns4", Some(("dd", "dd-ns")))),
        ])
        .build()
}

fn req(ns: &str) -> ObjectKey {
    ObjectKey::new("", ns)
}

#[test]
fn ovn_daemon_set_predicate_matches_only_the_ovn_daemon_set() {
    let p = ovn_daemon_set_predicate();
    let ovn = DaemonSet {
        metadata: meta("ovnkube-node-dpu-host", "openshift-ovn-kubernetes"),
        ..Default::default()
    };
    let other = DaemonSet {
        metadata: meta("ovnkube-node", "openshift-ovn-kubernetes"),
        ..Default::default()
    };
    assert!(p.create(&ovn));
    assert!(!p.create(&other));
    assert!(p.update(&other, &ovn));
    assert!(!p.update(&ovn, &other));
    assert!(!p.delete(&ovn));
    // GenericFunc unset: predicate.Funcs returns true.
    assert!(p.generic(&other));
}

#[test]
fn overrides_config_map_predicate_matches_only_the_overrides_config_map() {
    let p = overrides_config_map_predicate("operator-ns".into());
    let cm = ConfigMap {
        metadata: meta("dpuservicetemplate-overrides", "operator-ns"),
        ..Default::default()
    };
    let wrong_ns = ConfigMap {
        metadata: meta("dpuservicetemplate-overrides", "other"),
        ..Default::default()
    };
    assert!(p.create(&cm));
    assert!(!p.create(&wrong_ns));
    assert!(p.update(&wrong_ns, &cm));
    assert!(!p.update(&cm, &wrong_ns));
    assert!(p.delete(&cm));
    assert!(!p.delete(&wrong_ns));
    assert!(p.generic(&wrong_ns));
}

#[tokio::test]
async fn map_functions() {
    let c = client();
    let r = reconciler(&c);
    let ctx = Ctx::background();

    assert_eq!(
        r.provisioner_to_namespace(&provisioner("x", "ns9", None)),
        vec![req("ns9")]
    );

    let mut all = r.all_provisioner_dpu_cluster_namespaces(&ctx).await;
    all.sort();
    assert_eq!(all, vec![req("ns1"), req("ns2"), req("ns3"), req("ns4")]);

    let dd = dpuservicev1alpha1::DPUDeployment {
        metadata: meta("dd", "dd-ns"),
        ..Default::default()
    };
    let mut reqs = r.dpu_deployment_to_namespace(&ctx, &dd).await;
    reqs.sort();
    assert_eq!(reqs, vec![req("ns1"), req("ns4")]);

    let t = dpuservicev1alpha1::DPUServiceTemplate {
        metadata: meta("ovn", "tmpl-ns"),
        ..Default::default()
    };
    assert_eq!(template_to_namespace(&t), vec![req("tmpl-ns")]);
}

#[tokio::test]
async fn map_functions_return_nothing_when_listing_fails() {
    // DPFHCPProvisioner not registered in the scheme: List fails.
    let c = FakeClientBuilder::new().with_scheme(Scheme::new()).build();
    let r = reconciler(&c);
    let ctx = Ctx::background();
    assert!(
        r.all_provisioner_dpu_cluster_namespaces(&ctx)
            .await
            .is_empty()
    );
    let dd = dpuservicev1alpha1::DPUDeployment::default();
    assert!(r.dpu_deployment_to_namespace(&ctx, &dd).await.is_empty());
}

struct CaptureManager {
    client: Client,
    specs: Mutex<Vec<ControllerSpec>>,
}

impl ManagerHandle for CaptureManager {
    fn client(&self) -> Client {
        self.client.clone()
    }
    fn api_reader(&self) -> Client {
        self.client.clone()
    }
    fn event_recorder_for(&self, _name: &str) -> Recorder {
        Arc::new(ctrlrt::FakeRecorder::discarding())
    }
    fn add_controller(&self, spec: ControllerSpec) -> Result<()> {
        self.specs.lock().unwrap().push(spec);
        Ok(())
    }
}

fn raw<K: Object>(o: &K) -> Arc<serde_json::Value> {
    Arc::new(ctrlrt::client::to_value(o).unwrap())
}

#[tokio::test]
async fn setup_with_manager_registers_the_watches() {
    let c = client();
    let mgr = CaptureManager {
        client: c.clone(),
        specs: Mutex::new(vec![]),
    };
    let r = reconciler(&c);
    r.setup_with_manager(&mgr).unwrap();
    let specs = mgr.specs.lock().unwrap().clone();
    assert_eq!(specs.len(), 1);
    let spec = &specs[0];
    assert_eq!(spec.name, "dpuservicetemplate");
    assert_eq!(spec.max_concurrent_reconciles, 1);
    let kinds: Vec<String> = spec
        .watches
        .iter()
        .map(|w| w.resource.kind.clone())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "DPFHCPProvisioner",
            "DPUDeployment",
            "DaemonSet",
            "DPFHCPProvisionerConfig",
            "ConfigMap",
            "DPUServiceTemplate"
        ]
    );

    let ctx = Ctx::background();
    // DaemonSet watch: only the OVN DaemonSet enqueues every provisioner namespace.
    let ovn = DaemonSet {
        metadata: meta("ovnkube-node-dpu-host", "openshift-ovn-kubernetes"),
        ..Default::default()
    };
    let mut got = (spec.watches[2].handler)(ctx.clone(), RawEvent::Create(raw(&ovn))).await;
    got.sort();
    assert_eq!(got, vec![req("ns1"), req("ns2"), req("ns3"), req("ns4")]);
    let other = DaemonSet {
        metadata: meta("x", "openshift-ovn-kubernetes"),
        ..Default::default()
    };
    assert!(
        (spec.watches[2].handler)(ctx.clone(), RawEvent::Create(raw(&other)))
            .await
            .is_empty()
    );
    assert!(
        (spec.watches[2].handler)(ctx.clone(), RawEvent::Delete(raw(&ovn)))
            .await
            .is_empty()
    );

    // Config watch: generation changes only.
    let mut cfg = provisioningv1alpha1::DPFHCPProvisionerConfig {
        metadata: ObjectMeta {
            name: Some("default".into()),
            generation: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let old = raw(&cfg);
    assert!(
        (spec.watches[3].handler)(
            ctx.clone(),
            RawEvent::Update {
                old: old.clone(),
                new: old.clone()
            }
        )
        .await
        .is_empty()
    );
    cfg.metadata.generation = Some(2);
    assert_eq!(
        (spec.watches[3].handler)(
            ctx.clone(),
            RawEvent::Update {
                old,
                new: raw(&cfg)
            }
        )
        .await
        .len(),
        4
    );

    // ConfigMap watch: only the overrides ConfigMap in the operator namespace.
    let cm = ConfigMap {
        metadata: meta("dpuservicetemplate-overrides", "operator-ns"),
        data: Some(BTreeMap::new()),
        ..Default::default()
    };
    assert_eq!(
        (spec.watches[4].handler)(ctx.clone(), RawEvent::Create(raw(&cm)))
            .await
            .len(),
        4
    );
    let cm2 = ConfigMap {
        metadata: meta("dpuservicetemplate-overrides", "elsewhere"),
        ..Default::default()
    };
    assert!(
        (spec.watches[4].handler)(ctx.clone(), RawEvent::Create(raw(&cm2)))
            .await
            .is_empty()
    );

    // DPUServiceTemplate watch: its namespace.
    let t = dpuservicev1alpha1::DPUServiceTemplate {
        metadata: meta("hbn", "tmpl-ns"),
        ..Default::default()
    };
    assert_eq!(
        (spec.watches[5].handler)(ctx.clone(), RawEvent::Delete(raw(&t))).await,
        vec![req("tmpl-ns")]
    );
}
