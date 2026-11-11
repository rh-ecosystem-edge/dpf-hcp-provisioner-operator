//! Rust-only behavioural tests for `ProcessCSRs`, `Reconcile` and `SetupWithManager` (the
//! Go package has no unit tests for them). They pin condition reasons/messages, event
//! strings, requeue durations and client-cache invalidation.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::k8s::core::v1::LocalObjectReference;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::meta::find_status_condition;
use ctrlrt::{
    Client, ControllerSpec, Ctx, FakeRecorder, ManagerHandle, ObjectKey, ReconcileResult,
    Reconciler, Recorder,
};
use ctrlrt_fake::{FakeClientBuilder, InitObject, Reaction, Scheme, obj};
use hostedclient::{ClientManager, HostedClient};
use k8s_openapi::ByteString;
use k8s_openapi::api::certificates::v1::{
    CertificateSigningRequest, CertificateSigningRequestSpec,
};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::controller::CSRApprovalReconciler;
use crate::csr_content_validation::*;
use crate::csrapproval::{CSR_POLLING_INTERVAL, CSRApprover};
use crate::hostname::*;
use crate::test_util::*;

const NS: &str = "clusters";
const NAME: &str = "doca";
const DPU_NS: &str = "dpu-system";
const HOST: &str = "dpu-worker-01";

fn provisioner(with_kubeconfig_ref: bool) -> provisioningv1alpha1::DPFHCPProvisioner {
    let mut p = provisioningv1alpha1::DPFHCPProvisioner {
        metadata: ObjectMeta {
            name: Some(NAME.into()),
            namespace: Some(NS.into()),
            ..Default::default()
        },
        ..Default::default()
    };
    p.spec.dpu_cluster_ref.namespace = DPU_NS.into();
    p.spec.dpu_cluster_ref.name = "dpu-cluster".into();
    if with_kubeconfig_ref {
        p.status.kube_config_secret_ref = Some(LocalObjectReference {
            name: format!("{NAME}-admin-kubeconfig"),
            ..Default::default()
        });
    }
    p
}

fn kubeconfig_secret() -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(format!("{NAME}-admin-kubeconfig")),
            namespace: Some(NS.into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            "kubeconfig".to_string(),
            ByteString(b"apiVersion: v1\nkind: Config\n".to_vec()),
        )])),
        ..Default::default()
    }
}

fn dpu(name: &str, phase: &str) -> dpuprovisioningv1alpha1::DPU {
    dpuprovisioningv1alpha1::DPU {
        metadata: ObjectMeta {
            name: Some(name.into()),
            namespace: Some(DPU_NS.into()),
            ..Default::default()
        },
        status: dpuprovisioningv1alpha1::DPUStatus {
            phase: phase.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn bootstrap_csr(name: &str, host: &str) -> CertificateSigningRequest {
    CertificateSigningRequest {
        metadata: ObjectMeta {
            name: Some(name.into()),
            ..Default::default()
        },
        spec: CertificateSigningRequestSpec {
            signer_name: SIGNER_BOOTSTRAP.into(),
            username: Some(BOOTSTRAPPER_USERNAME.into()),
            groups: Some(vec![
                GROUP_SERVICE_ACCOUNTS.into(),
                GROUP_SERVICE_ACCOUNTS_MACHINE_CONFIG.into(),
                GROUP_AUTHENTICATED.into(),
            ]),
            usages: Some(vec![
                USAGE_DIGITAL_SIGNATURE.into(),
                USAGE_CLIENT_AUTH.into(),
            ]),
            request: ByteString(create_test_csr_with_organization(
                &format!("system:node:{host}"),
                "system:nodes",
            )),
            ..Default::default()
        },
        status: None,
    }
}

fn serving_csr(name: &str, host: &str) -> CertificateSigningRequest {
    CertificateSigningRequest {
        metadata: ObjectMeta {
            name: Some(name.into()),
            ..Default::default()
        },
        spec: CertificateSigningRequestSpec {
            signer_name: SIGNER_SERVING.into(),
            username: Some(format!("system:node:{host}")),
            groups: Some(vec![GROUP_NODES.into(), GROUP_AUTHENTICATED.into()]),
            usages: Some(vec![
                USAGE_DIGITAL_SIGNATURE.into(),
                USAGE_SERVER_AUTH.into(),
            ]),
            request: ByteString(create_serving_csr_with_dns(
                &format!("system:node:{host}"),
                host,
            )),
            ..Default::default()
        },
        status: None,
    }
}

struct Env {
    ctx: Ctx,
    mgmt: Client,
    recorder: Arc<FakeRecorder>,
    cm: Arc<ClientManager>,
    approver: Arc<CSRApprover>,
}

fn env(objs: Vec<InitObject>) -> Env {
    let scheme = Scheme::new()
        .register::<provisioningv1alpha1::DPFHCPProvisioner>()
        .register::<Secret>()
        .register::<dpuprovisioningv1alpha1::DPU>();
    let mgmt = FakeClientBuilder::new()
        .with_scheme(scheme)
        .with_objects(objs)
        .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
        .build();
    let recorder = Arc::new(FakeRecorder::new(100));
    let cm = Arc::new(ClientManager::new(mgmt.clone()));
    let rec: Recorder = recorder.clone();
    let approver = Arc::new(CSRApprover::new(mgmt.clone(), rec, cm.clone()));
    Env {
        ctx: Ctx::background(),
        mgmt,
        recorder,
        cm,
        approver,
    }
}

impl Env {
    async fn provisioner(&self) -> provisioningv1alpha1::DPFHCPProvisioner {
        self.mgmt.get(&ObjectKey::new(NS, NAME)).await.unwrap()
    }

    async fn condition(&self) -> (String, String, String) {
        let p = self.provisioner().await;
        let c = find_status_condition(
            &p.status.conditions,
            provisioningv1alpha1::CSR_AUTO_APPROVAL_ACTIVE,
        )
        .expect("condition set");
        (c.status.clone(), c.reason.clone(), c.message.clone())
    }
}

mod process_csrs {
    use super::*;

    #[tokio::test]
    async fn kubeconfig_not_available_sets_condition_and_does_not_requeue() {
        let e = env(vec![obj(&provisioner(true))]);
        let mut p = e.provisioner().await;
        let res = e.approver.process_csrs(&e.ctx, &mut p).await.unwrap();
        assert_eq!(res, ReconcileResult::done());
        assert_eq!(
            e.condition().await,
            (
                "False".into(),
                "KubeconfigNotAvailable".into(),
                "Kubeconfig secret not available for hosted cluster".into()
            )
        );
        assert_eq!(
            e.recorder.drain(),
            vec![
                "Warning KubeconfigNotAvailable Kubeconfig secret not available for hosted cluster"
            ]
        );
    }

    #[tokio::test]
    async fn client_creation_failure_reports_unreachable() {
        // The kubeconfig secret exists but holds no current context.
        let e = env(vec![obj(&provisioner(true)), obj(&kubeconfig_secret())]);
        let mut p = e.provisioner().await;
        let res = e.approver.process_csrs(&e.ctx, &mut p).await.unwrap();
        assert_eq!(res, ReconcileResult::done());
        let msg = "Cannot connect to hosted cluster: failed to replace server endpoint: kubeconfig has no current context";
        assert_eq!(
            e.condition().await,
            (
                "False".into(),
                "HostedClusterNotReachable".into(),
                msg.into()
            )
        );
        assert_eq!(
            e.recorder.drain(),
            vec![
                format!("Warning HostedClusterNotReachable {msg}"),
                format!("Warning HostedClusterUnreachable {msg}"),
            ]
        );
    }

    #[tokio::test]
    async fn connection_failure_invalidates_cached_client() {
        let e = env(vec![obj(&provisioner(true)), obj(&kubeconfig_secret())]);
        let hc = fake_clientset()
            .with_server_version(Arc::new(|| Err(anyhow::anyhow!("dial tcp: i/o timeout"))));
        e.cm.set_hosted_cluster_client(NS, NAME, hc);

        let mut p = e.provisioner().await;
        let res = e.approver.process_csrs(&e.ctx, &mut p).await.unwrap();
        assert_eq!(res, ReconcileResult::done());
        assert!(!e.cm.has_cached_client(NS, NAME));
        let msg = "Cannot connect to hosted cluster: failed to connect to hosted cluster API server: dial tcp: i/o timeout";
        assert_eq!(
            e.condition().await,
            (
                "False".into(),
                "HostedClusterNotReachable".into(),
                msg.into()
            )
        );
        assert_eq!(
            e.recorder.drain(),
            vec![
                format!("Warning HostedClusterNotReachable {msg}"),
                format!("Warning HostedClusterUnreachable {msg}"),
            ]
        );
    }

    #[tokio::test]
    async fn approves_valid_pending_csrs_and_polls() {
        let e = env(vec![
            obj(&provisioner(true)),
            obj(&kubeconfig_secret()),
            obj(&dpu(HOST, dpuprovisioningv1alpha1::DPU_CLUSTER_CONFIG)),
        ]);
        let hc = fake_clientset();
        for mut o in [
            bootstrap_csr("csr-bootstrap", HOST),
            serving_csr("csr-serving", HOST),
            // owner validation fails (no DPU) → skipped
            bootstrap_csr("csr-unknown", "other-host"),
        ] {
            hc.create(&mut o).await.unwrap();
        }
        // Other signer → skipped
        let mut other = serving_csr("csr-other", HOST);
        other.spec.signer_name = "example.com/signer".into();
        hc.create(&mut other).await.unwrap();
        let mut node = create_fake_node(HOST);
        hc.create(&mut node).await.unwrap();
        e.cm.set_hosted_cluster_client(NS, NAME, hc.clone());

        let mut p = e.provisioner().await;
        let res = e.approver.process_csrs(&e.ctx, &mut p).await.unwrap();
        assert_eq!(res, ReconcileResult::requeue_after(Duration::from_secs(30)));
        assert_eq!(res.requeue_after, CSR_POLLING_INTERVAL);

        assert_eq!(
            e.condition().await,
            (
                "True".into(),
                "Active".into(),
                "CSR auto-approval active for worker nodes".into()
            )
        );
        assert_eq!(
            e.recorder.drain(),
            vec![
                format!("Normal CSRApproved Approved bootstrap CSR for DPU worker node {HOST}"),
                format!("Normal CSRApproved Approved serving CSR for DPU worker node {HOST}"),
                "Normal Active CSR auto-approval active for worker nodes".to_string(),
            ]
        );

        for (name, kind, approved) in [
            ("csr-bootstrap", "bootstrap", true),
            ("csr-serving", "serving", true),
            ("csr-unknown", "", false),
            ("csr-other", "", false),
        ] {
            let csr: CertificateSigningRequest = hc.get_named("", name).await.unwrap();
            assert_eq!(!is_pending(Some(&csr)), approved, "{name}");
            if approved {
                let c = &csr.status.as_ref().unwrap().conditions.as_ref().unwrap()[0];
                assert_eq!(c.type_, "Approved");
                assert_eq!(c.status, "True");
                assert_eq!(c.reason.as_deref(), Some("AutoApproved"));
                let expected = format!("Auto-approved {kind} CSR for DPU worker node {HOST}");
                assert_eq!(c.message.as_deref(), Some(expected.as_str()));
                assert!(c.last_update_time.is_some());
            }
        }

        // Second pass: nothing pending to approve, condition unchanged → no events.
        let mut p = e.provisioner().await;
        let res = e.approver.process_csrs(&e.ctx, &mut p).await.unwrap();
        assert_eq!(res.requeue_after, CSR_POLLING_INTERVAL);
        assert!(e.recorder.is_empty());
    }

    #[tokio::test]
    async fn list_error_is_transient() {
        let e = env(vec![obj(&provisioner(true)), obj(&kubeconfig_secret())]);
        let fake = ctrlrt_fake::FakeClientsetBuilder::new().build_fake();
        fake.prepend_reactor("list", "certificatesigningrequests", |_| {
            Reaction::error(ctrlrt::error::ApiError::internal("etcd unavailable"))
        });
        e.cm.set_hosted_cluster_client(NS, NAME, HostedClient::fake(fake.client()));

        let mut p = e.provisioner().await;
        let res = e.approver.process_csrs(&e.ctx, &mut p).await.unwrap();
        assert_eq!(res.requeue_after, CSR_POLLING_INTERVAL);
        assert_eq!(
            e.condition().await,
            (
                "True".into(),
                "Active".into(),
                "CSR processing encountered transient error: failed to list CSRs: Internal error occurred: etcd unavailable".into()
            )
        );
    }
}

mod reconcile {
    use super::*;

    fn reconciler(e: &Env, with_approver: bool) -> CSRApprovalReconciler {
        CSRApprovalReconciler {
            client: e.mgmt.clone(),
            approver: with_approver.then(|| e.approver.clone()),
        }
    }

    #[tokio::test]
    async fn not_found_is_ignored() {
        let e = env(vec![]);
        let r = reconciler(&e, true);
        let res = r.reconcile(&e.ctx, ObjectKey::new(NS, NAME)).await.unwrap();
        assert!(res.is_zero());
    }

    #[tokio::test]
    async fn skips_without_kubeconfig_ref_or_approver() {
        let e = env(vec![obj(&provisioner(false))]);
        let res = reconciler(&e, true)
            .reconcile(&e.ctx, ObjectKey::new(NS, NAME))
            .await
            .unwrap();
        assert!(res.is_zero());
        assert!(e.recorder.is_empty());

        let e = env(vec![obj(&provisioner(true))]);
        let res = reconciler(&e, false)
            .reconcile(&e.ctx, ObjectKey::new(NS, NAME))
            .await
            .unwrap();
        assert!(res.is_zero());
        assert!(e.recorder.is_empty());
    }

    #[tokio::test]
    async fn deletion_invalidates_cached_client() {
        let mut p = provisioner(true);
        p.metadata.finalizers = Some(vec!["example.com/finalizer".into()]);
        p.metadata.deletion_timestamp = Some(ctrlrt::meta::now());
        let e = env(vec![obj(&p)]);
        e.cm.set_hosted_cluster_client(NS, NAME, fake_clientset());
        let res = reconciler(&e, true)
            .reconcile(&e.ctx, ObjectKey::new(NS, NAME))
            .await
            .unwrap();
        assert!(res.is_zero());
        assert!(!e.cm.has_cached_client(NS, NAME));
    }

    #[tokio::test]
    async fn processes_csrs_and_requeues() {
        let e = env(vec![obj(&provisioner(true)), obj(&kubeconfig_secret())]);
        e.cm.set_hosted_cluster_client(NS, NAME, fake_clientset());
        let res = reconciler(&e, true)
            .reconcile(&e.ctx, ObjectKey::new(NS, NAME))
            .await
            .unwrap();
        assert_eq!(res, ReconcileResult::requeue_after(CSR_POLLING_INTERVAL));
    }

    #[tokio::test]
    async fn process_error_requeues_after_polling_interval() {
        // Status updates fail → ProcessCSRs returns the error → requeue after 30s.
        let scheme = Scheme::new()
            .register::<provisioningv1alpha1::DPFHCPProvisioner>()
            .register::<Secret>();
        let fake = FakeClientBuilder::new()
            .with_scheme(scheme)
            .with_objects([obj(&provisioner(true))])
            .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
            .build_fake();
        fake.prepend_reactor("update", "dpfhcpprovisioners/status", |_| {
            Reaction::error(ctrlrt::error::ApiError::internal("boom"))
        });
        let mgmt = fake.client();
        let rec: Recorder = Arc::new(FakeRecorder::new(10));
        let cm = Arc::new(ClientManager::new(mgmt.clone()));
        let r = CSRApprovalReconciler {
            client: mgmt.clone(),
            approver: Some(Arc::new(CSRApprover::new(mgmt, rec, cm))),
        };
        let res = r
            .reconcile(&Ctx::background(), ObjectKey::new(NS, NAME))
            .await
            .unwrap();
        assert_eq!(res, ReconcileResult::requeue_after(CSR_POLLING_INTERVAL));
    }
}

mod setup_with_manager {
    use super::*;

    #[derive(Default)]
    struct FakeManager {
        specs: Mutex<Vec<ControllerSpec>>,
    }

    impl ManagerHandle for FakeManager {
        fn client(&self) -> Client {
            unimplemented!()
        }
        fn api_reader(&self) -> Client {
            unimplemented!()
        }
        fn event_recorder_for(&self, _name: &str) -> Recorder {
            Arc::new(FakeRecorder::new(1))
        }
        fn add_controller(&self, spec: ControllerSpec) -> ctrlrt::Result<()> {
            self.specs.lock().unwrap().push(spec);
            Ok(())
        }
    }

    #[test]
    fn registers_named_controller_for_dpfhcpprovisioner() {
        let e = env(vec![]);
        let mgr = FakeManager::default();
        let r = Arc::new(CSRApprovalReconciler {
            client: e.mgmt.clone(),
            approver: Some(e.approver.clone()),
        });
        r.setup_with_manager(&mgr).unwrap();
        let specs = mgr.specs.lock().unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "csrapproval");
        assert_eq!(
            specs[0].for_resource.as_ref().unwrap().kind,
            "DPFHCPProvisioner"
        );
        assert_eq!(
            specs[0].for_resource.as_ref().unwrap().group,
            "provisioning.dpu.hcp.io"
        );
        assert_eq!(specs[0].watches.len(), 1);
        assert_eq!(specs[0].max_concurrent_reconciles, 1);
    }
}
