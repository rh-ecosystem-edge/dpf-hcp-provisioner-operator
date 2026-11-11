//! Rust-only tests (no Go counterpart) for Go's nil-pointer panic fidelity of the reconciler:
//! a nil pointer dereference inside the ignition generator (Go: runtime panic recovered by
//! controller-runtime) must unwind out of the reconciler before any further status handling,
//! and calling into an unset dependency panics with Go's runtime error message.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::dpf::operator::v1alpha1 as operatorv1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use bytes::Bytes;
use ctrlrt::{Ctx, FakeRecorder, Recorder};
use ctrlrt_fake::{FakeClientBuilder, Scheme, obj, obj_mut};
use futures::FutureExt;
use http_body_util::Full;
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::Secret;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::DPFHCPProvisionerReconciler;
use crate::nil::NIL_DEREF_MESSAGE;
use crate::suite_test::*;

/// An HTTPS server on 127.0.0.1 serving `body` on every path (aborted on drop); returns the
/// address and the PEM of the CA that signed its certificate.
struct TlsServer {
    addr: std::net::SocketAddr,
    ca_pem: Vec<u8>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start_tls_server(body: Vec<u8>) -> TlsServer {
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        KeyUsagePurpose, SanType,
    };
    let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca_params = CertificateParams::default();
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "ignition-test-ca");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let ca_pem = ca_cert.pem().into_bytes();
    let issuer = rcgen::Issuer::new(ca_params, ca_key);

    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::new(vec!["example.com".to_string()]).unwrap();
    params.subject_alt_names.push(SanType::IpAddress(
        std::net::Ipv4Addr::new(127, 0, 0, 1).into(),
    ));
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let cert = params.signed_by(&key, &issuer).unwrap();
    let cert_der: CertificateDer<'static> = cert.der().clone();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let acceptor = acceptor.clone();
            let body = body.clone();
            tokio::spawn(async move {
                let Ok(tls_stream) = acceptor.accept(stream).await else {
                    return;
                };
                let svc = hyper::service::service_fn(
                    move |_req: hyper::Request<hyper::body::Incoming>| {
                        let body = body.clone();
                        async move {
                            Ok::<_, std::convert::Infallible>(hyper::Response::new(Full::new(
                                Bytes::from(body),
                            )))
                        }
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(tls_stream), svc)
                    .await;
            });
        }
    });
    TlsServer { addr, ca_pem, task }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else {
        String::new()
    }
}

/// A DPFOperatorConfig without `spec.networking` makes Go's GenerateIgnition dereference a nil
/// `Networking` pointer after the HCP ignition download; the reconciler must panic with Go's
/// runtime error before touching the status again.
#[tokio::test]
async fn generate_ignition_re_raises_ignition_generator_nil_pointer_dereference_as_panic() {
    let server = start_tls_server(br#"{"ignition":{"version":"3.4.0"}}"#.to_vec()).await;

    let mut cr = provisioningv1alpha1::DPFHCPProvisioner {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            generation: Some(1),
            ..object_meta("doca", "clusters")
        },
        spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
            dpu_cluster_ref: dpu_cluster_ref("dpucluster-1", "dpf-operator-system"),
            dpu_deployment_ref: dpu_deployment_ref("my-deployment", "dpf-operator-system"),
            ocp_release_image: "quay.io/openshift-release-dev/ocp-release:4.19.0-multi".into(),
            machine_osurl: "https://registry.example.com/rhcos-bf:4.17".into(),
            ..Default::default()
        },
        status: provisioningv1alpha1::DPFHCPProvisionerStatus {
            phase: provisioningv1alpha1::PHASE_GENERATING_IGNITION.into(),
            hosted_cluster_ref: object_ref("doca", "clusters"),
            ..Default::default()
        },
        ..Default::default()
    };

    let ca_secret = Secret {
        metadata: object_meta("ignition-server-ca-cert", "clusters-doca"),
        data: Some([("tls.crt".to_string(), ByteString(server.ca_pem.clone()))].into()),
        ..Default::default()
    };
    let token_secret = Secret {
        metadata: object_meta("token-doca-abc123", "clusters-doca"),
        data: Some(
            [(
                "token".to_string(),
                ByteString(b"test-bearer-token".to_vec()),
            )]
            .into(),
        ),
        ..Default::default()
    };
    let mut hc = hyperv1::HostedCluster {
        metadata: object_meta("doca", "clusters"),
        ..Default::default()
    };
    hc.status.ignition_endpoint = server.addr.to_string();
    let deployment = dpuservicev1alpha1::DPUDeployment {
        metadata: object_meta("my-deployment", "dpf-operator-system"),
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
    let flavor = dpuprovisioningv1alpha1::DPUFlavor {
        metadata: object_meta("test-flavor", "dpf-operator-system"),
        ..Default::default()
    };
    // No spec.networking → nil pointer dereference in Go.
    let mut operator_config = operatorv1alpha1::DPFOperatorConfig {
        metadata: object_meta("dpf-config", "dpf-operator-system"),
        ..Default::default()
    };
    operator_config.spec.deployment_mode = operatorv1alpha1::DEPLOYMENT_MODE_HOST_TRUSTED.into();

    let scheme = Scheme::new()
        .with_client_go()
        .register::<provisioningv1alpha1::DPFHCPProvisioner>()
        .register::<hyperv1::HostedCluster>()
        .register::<hyperv1::NodePool>()
        .register::<dpuservicev1alpha1::DPUDeployment>()
        .register::<dpuprovisioningv1alpha1::DPUFlavor>()
        .register::<operatorv1alpha1::DPFOperatorConfig>();
    let client = FakeClientBuilder::new()
        .with_scheme(scheme)
        .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
        .with_objects([
            obj_mut(&mut cr),
            obj(&ca_secret),
            obj(&token_secret),
            obj(&hc),
            obj(&deployment),
            obj(&flavor),
            obj(&operator_config),
        ])
        .build();
    let fake_recorder = Arc::new(FakeRecorder::new(10));
    let recorder: Recorder = fake_recorder.clone();
    let r = DPFHCPProvisionerReconciler {
        client: client.clone(),
        recorder: recorder.clone(),
        ignition_generator: Some(Arc::new(ctrl_ignitiongenerator::IgnitionGenerator::new(
            client.clone(),
            recorder,
        ))),
        ..Default::default()
    };

    let before: provisioningv1alpha1::DPFHCPProvisioner =
        get(&client, "doca", "clusters").await.unwrap();
    let res = AssertUnwindSafe(r.generate_ignition(&Ctx::background(), &mut cr))
        .catch_unwind()
        .await;
    let payload = res.expect_err("must panic like Go's nil pointer dereference");
    assert_eq!(panic_message(payload.as_ref()), NIL_DEREF_MESSAGE);
    assert_eq!(
        NIL_DEREF_MESSAGE,
        "runtime error: invalid memory address or nil pointer dereference"
    );

    // Nothing was persisted and no event emitted after the panic.
    let after: provisioningv1alpha1::DPFHCPProvisioner =
        get(&client, "doca", "clusters").await.unwrap();
    assert_eq!(
        after.metadata.resource_version,
        before.metadata.resource_version
    );
    assert!(fake_recorder.is_empty(), "{:?}", fake_recorder.events());
}

/// Calling through an unset dependency (Go: nil pointer field) panics with Go's runtime error.
#[tokio::test]
async fn unset_dependency_panics_like_a_go_nil_pointer_dereference() {
    let r = DPFHCPProvisionerReconciler::default();
    let mut cr = provisioningv1alpha1::DPFHCPProvisioner {
        spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
            control_plane_availability_policy: hyperv1::HIGHLY_AVAILABLE.into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let cfg = common::OperatorConfig::default();
    let res = AssertUnwindSafe(r.configure_metal_lb(&Ctx::background(), &mut cr, &cfg))
        .catch_unwind()
        .await;
    let payload = res.expect_err("must panic");
    assert_eq!(panic_message(payload.as_ref()), NIL_DEREF_MESSAGE);

    // ...but Go's nil MetalLB manager still returns early when no LoadBalancer is needed.
    cr.spec.control_plane_availability_policy = hyperv1::SINGLE_REPLICA.into();
    let res = r
        .configure_metal_lb(&Ctx::background(), &mut cr, &cfg)
        .await
        .unwrap();
    assert!(res.is_zero());
}
