//! Shared test fixtures: port of `suite_test.go` (BeforeSuite, `createFakeNode`) and the CSR
//! generators of `hostname_test.go` (`createTestCSR`, `createTestCSRWithOrganization`,
//! `createServingCSRWithDNS`).
//!
//! Go generates RSA-2048 keys with `crypto/x509`; `rcgen` (ring backend) cannot generate RSA
//! keys, so ECDSA P-256 keys are used. The key type does not affect any validation outcome.

use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use ctrlrt::Client;
use ctrlrt_fake::{FakeClientBuilder, FakeClientsetBuilder, Scheme};
use hostedclient::HostedClient;
use k8s_openapi::api::core::v1::{Node, NodeStatus};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};

/// `testHostname`
pub const TEST_HOSTNAME: &str = "dpu-worker-01";

/// BeforeSuite: scheme with the DPU types registered (`dpuprovisioningv1alpha1.AddToScheme`).
pub fn scheme() -> Scheme {
    Scheme::new()
        .register::<dpuprovisioningv1alpha1::DPU>()
        .register::<dpuprovisioningv1alpha1::DPUCluster>()
        .register::<dpuprovisioningv1alpha1::DPUFlavor>()
}

/// BeforeSuite: `k8sClient = fake.NewClientBuilder().WithScheme(scheme).Build()`.
pub fn k8s_client() -> Client {
    FakeClientBuilder::new().with_scheme(scheme()).build()
}

/// BeforeSuite: `fakeClientset = kubefake.NewSimpleClientset()`.
pub fn fake_clientset() -> HostedClient {
    HostedClient::fake(FakeClientsetBuilder::new().build())
}

/// createFakeNode creates a fake Kubernetes Node object for testing.
pub fn create_fake_node(name: &str) -> Node {
    Node {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        status: Some(NodeStatus {
            phase: Some("Running".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn csr_pem(cn: &str, org: Option<&str>, dns_names: &[&str]) -> Vec<u8> {
    let key_pair = KeyPair::generate().expect("generate key");
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, cn);
    if let Some(org) = org {
        dn.push(DnType::OrganizationName, org);
    }
    params.distinguished_name = dn;
    params.subject_alt_names = dns_names
        .iter()
        .map(|d| SanType::DnsName((*d).try_into().expect("ia5 dns name")))
        .collect();
    let csr = params.serialize_request(&key_pair).expect("serialize CSR");
    csr.pem().expect("pem").into_bytes()
}

/// createTestCSR creates a test CSR with the given CN.
pub fn create_test_csr(cn: &str) -> Vec<u8> {
    csr_pem(cn, None, &[])
}

/// createTestCSRWithOrganization creates a test CSR with the given CN and organization.
pub fn create_test_csr_with_organization(cn: &str, org: &str) -> Vec<u8> {
    csr_pem(cn, Some(org), &[])
}

/// createServingCSRWithDNS creates a test CSR with the given CN and DNS names.
pub fn create_serving_csr_with_dns(cn: &str, dns_name: &str) -> Vec<u8> {
    csr_pem(cn, Some("system:nodes"), &[dns_name])
}
