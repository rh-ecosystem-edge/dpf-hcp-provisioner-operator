//! Port of `controller-runtime/pkg/internal/testing/certs` (`TinyCA`), using rcgen.
//!
//! All keys are ECDSA P-256 (like Go's `elliptic.P256()`), private keys are written as
//! PKCS#8 `PRIVATE KEY` PEM blocks and certificates as `CERTIFICATE` PEM blocks.

use std::net::{IpAddr, ToSocketAddrs};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, SanType, SerialNumber,
};
use time::{Duration, OffsetDateTime};

/// A certificate and its private key (`certs.CertPair`).
pub struct CertPair {
    cert_pem: String,
    key_pem: String,
}

impl CertPair {
    /// `CertBytes()` — the PEM encoded certificate.
    pub fn cert_bytes(&self) -> Vec<u8> {
        self.cert_pem.as_bytes().to_vec()
    }

    /// `AsBytes()` — PEM encoded certificate and PKCS#8 private key.
    pub fn as_bytes(&self) -> (Vec<u8>, Vec<u8>) {
        (self.cert_bytes(), self.key_pem.as_bytes().to_vec())
    }
}

/// `certs.TinyCA` — a tiny certificate authority for test certificates.
pub struct TinyCA {
    /// The CA certificate and key.
    pub ca: CertPair,
    issuer: Issuer<'static, KeyPair>,
    org_name: String,
    next_serial: u64,
}

/// `certs.ClientInfo`
#[derive(Clone, Debug, Default)]
pub struct ClientInfo {
    pub name: String,
    pub groups: Vec<String>,
}

fn new_private_key() -> anyhow::Result<KeyPair> {
    Ok(KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?)
}

fn random_serial() -> anyhow::Result<u64> {
    // Go: uniform random value in [1, MaxInt64).
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|err| anyhow::anyhow!("{}", err))?;
    Ok(u64::from_be_bytes(bytes) % (i64::MAX as u64 - 1) + 1)
}

impl TinyCA {
    /// `certs.NewTinyCA()`
    pub fn new() -> anyhow::Result<Self> {
        let ca_key = new_private_key()
            .map_err(|err| ctrlrt::wrapf!(err, "unable to generate private key for CA: {}", err))?;
        let build = || -> anyhow::Result<(CertificateParams, String)> {
            // certutil.NewSelfSignedCACert
            let now = OffsetDateTime::now_utc();
            let mut params = CertificateParams::new(vec!["envtest-environment".to_string()])?;
            params.serial_number = Some(SerialNumber::from(random_serial()?));
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "envtest-environment");
            dn.push(DnType::OrganizationName, "envtest");
            params.distinguished_name = dn;
            params.not_before = now;
            params.not_after = now + Duration::days(365 * 10);
            params.key_usages = vec![
                KeyUsagePurpose::DigitalSignature,
                KeyUsagePurpose::KeyEncipherment,
                KeyUsagePurpose::KeyCertSign,
            ];
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            let cert = params.self_signed(&ca_key)?;
            Ok((params, cert.pem()))
        };
        let (params, cert_pem) = build()
            .map_err(|err| ctrlrt::wrapf!(err, "unable to generate certificate for CA: {}", err))?;
        let key_pem = ca_key.serialize_pem();
        Ok(Self {
            ca: CertPair { cert_pem, key_pem },
            issuer: Issuer::new(params, ca_key),
            org_name: "envtest".to_string(),
            next_serial: 1,
        })
    }

    fn make_cert(
        &mut self,
        common_name: &str,
        organization: &[String],
        sans: Vec<SanType>,
        usage: ExtendedKeyUsagePurpose,
    ) -> anyhow::Result<CertPair> {
        let now = OffsetDateTime::now_utc();
        let key = new_private_key()
            .map_err(|err| ctrlrt::wrapf!(err, "unable to create private key: {}", err))?;
        let serial = self.next_serial;
        self.next_serial += 1;

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, common_name);
        for org in organization {
            dn.push(DnType::OrganizationName, org.as_str());
        }
        params.distinguished_name = dn;
        params.subject_alt_names = sans;
        params.serial_number = Some(SerialNumber::from(serial));
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![usage];
        params.not_before = now;
        params.not_after = now + Duration::hours(168);
        params.use_authority_key_identifier_extension = true;
        let cert = params
            .signed_by(&key, &self.issuer)
            .map_err(|err| ctrlrt::wrapf!(err, "unable to create certificate: {}", err))?;
        Ok(CertPair {
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
        })
    }

    /// `NewServingCert(names...)` — a serving certificate for the given DNS names / IPs.
    pub fn new_serving_cert(&mut self, names: &[&str]) -> anyhow::Result<CertPair> {
        let names: Vec<&str> = if names.is_empty() {
            vec!["localhost"]
        } else {
            names.to_vec()
        };
        let (dns_names, ips) = resolve_names(&names)?;
        let mut sans = Vec::new();
        for dns in dns_names {
            sans.push(SanType::DnsName(dns.try_into()?));
        }
        for ip in ips {
            sans.push(SanType::IpAddress(ip));
        }
        let org = vec![self.org_name.clone()];
        self.make_cert("localhost", &org, sans, ExtendedKeyUsagePurpose::ServerAuth)
    }

    /// `NewClientCert(user)` — a client certificate (CN = user name, O = groups).
    pub fn new_client_cert(&mut self, user: &ClientInfo) -> anyhow::Result<CertPair> {
        self.make_cert(
            &user.name,
            &user.groups,
            Vec::new(),
            ExtendedKeyUsagePurpose::ClientAuth,
        )
    }
}

fn resolve_names(names: &[&str]) -> anyhow::Result<(Vec<String>, Vec<IpAddr>)> {
    let mut dns_names = Vec::new();
    let mut ips = Vec::new();
    for name in names {
        if name.is_empty() {
            continue;
        }
        match name.parse::<IpAddr>() {
            Ok(ip) => ips.push(ip),
            Err(_) => {
                dns_names.push(name.to_string());
                let addrs = (*name, 0u16)
                    .to_socket_addrs()
                    .map_err(|err| anyhow::anyhow!("lookup {}: {}", name, err))?;
                for addr in addrs {
                    if !ips.contains(&addr.ip()) {
                        ips.push(addr.ip());
                    }
                }
            }
        }
    }
    Ok((dns_names, ips))
}
