//! Port of internal/controller/hostedcluster/secrets.go

use std::collections::BTreeMap;

use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::k8s_openapi::ByteString;
use ctrlrt::k8s_openapi::api::core::v1::Secret;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use ctrlrt::meta;
use ctrlrt::{Client, Ctx, ListOptions, Object, ObjectKey, ReconcileResult, kv, wrapf};
use sha2::{Digest, Sha256};

const CREDENTIAL_SECRET_COPY_LABEL: &str = "provisioning.dpu.hcp.io/credential-secret-copy";
const PULL_SECRET_COPY_TYPE: &str = "pull-secret";
const SSH_KEY_COPY_TYPE: &str = "ssh-key";

/// `corev1.SecretTypeDockerConfigJson`
const SECRET_TYPE_DOCKER_CONFIG_JSON: &str = "kubernetes.io/dockerconfigjson";
/// `corev1.SecretTypeOpaque`
const SECRET_TYPE_OPAQUE: &str = "Opaque";

/// PullSecretCopyName returns the name of the pull-secret copy for a given DPFHCPProvisioner.
///
/// The name encodes the source secret reference so that changing PullSecretRef.Name
/// produces a different copy name. This is critical: HyperShift's NodePool controller
/// includes HostedCluster.Spec.PullSecret.Name in its rollout config hash, which determines
/// the ignition-server token secret name. When the name changes, a new token secret is
/// created with the correct pull-secret-hash, avoiding the stale-hash mismatch that occurs
/// with in-place data updates.
pub fn pull_secret_copy_name(cr: &provisioningv1alpha1::DPFHCPProvisioner) -> String {
    format!(
        "{}-pull-secret-{}",
        cr.name(),
        short_hash(&cr.spec.pull_secret_ref.name)
    )
}

/// SSHKeyCopyName returns the name of the ssh-key copy for a given DPFHCPProvisioner.
/// The name encodes the source secret reference for the same reason as PullSecretCopyName.
pub fn ssh_key_copy_name(cr: &provisioningv1alpha1::DPFHCPProvisioner) -> String {
    format!(
        "{}-ssh-key-{}",
        cr.name(),
        short_hash(&cr.spec.ssh_key_secret_ref.name)
    )
}

/// shortHash returns the first 8 hex characters of the SHA-256 hash of s.
pub(crate) fn short_hash(s: &str) -> String {
    let h = Sha256::digest(s.as_bytes());
    h[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// setCredentialSecretCopyLabel marks a copied pull-secret or ssh-key as managed by this operator.
/// It returns whether the secret's labels changed.
fn set_credential_secret_copy_label(secret: &mut Secret, copy_type: &str) -> bool {
    let labels = secret.metadata.labels.get_or_insert_with(BTreeMap::new);
    if labels.get(CREDENTIAL_SECRET_COPY_LABEL).map(String::as_str) == Some(copy_type) {
        return false;
    }
    labels.insert(
        CREDENTIAL_SECRET_COPY_LABEL.to_string(),
        copy_type.to_string(),
    );
    true
}

/// SecretManager handles secret copying and ETCD key generation for HostedCluster
#[derive(Clone)]
pub struct SecretManager {
    client: Client,
}

/// Which credential secret a copy step handles (copyPullSecret / copySSHKey).
struct CopySpec<'a> {
    source_name: &'a str,
    copy_type: &'a str,
    secret_type: &'a str,
    /// "pull-secret" / "ssh-key" (get/create/owner errors and "exists" error)
    noun: &'a str,
    /// "pull-secret" / "SSH-key" ("failed to mark ... copy" error)
    mark_noun: &'a str,
    /// "Pull-secret" / "SSH key" (reuse log)
    reuse_noun: &'a str,
}

impl SecretManager {
    /// NewSecretManager creates a new SecretManager
    pub fn new(c: Client) -> Self {
        Self { client: c }
    }

    /// The underlying client (Go embeds `client.Client`).
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// CopySecrets copies pull-secret and ssh-key within the same namespace as DPFHCPProvisioner.
    /// Copy names encode the source secret reference (via PullSecretCopyName / SSHKeyCopyName) so
    /// that a user can rotate credentials by creating a new secret and updating the CR's
    /// PullSecretRef/SSHKeySecretRef — the new copy name propagates to the HostedCluster spec,
    /// causing HyperShift to create a new ignition-server token with the correct hash.
    /// Stale copies from a previous reference are deleted automatically.
    pub async fn copy_secrets(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;

        let pull_secret_name = pull_secret_copy_name(cr);
        if let Err(err) = self.copy_pull_secret(ctx, cr, &pull_secret_name).await {
            log.error(&err, "Failed to copy pull-secret", kv![]);
            return Err(err);
        }

        let ssh_key_name = ssh_key_copy_name(cr);
        if let Err(err) = self.copy_ssh_key(ctx, cr, &ssh_key_name).await {
            log.error(&err, "Failed to copy ssh-key", kv![]);
            return Err(err);
        }

        if let Err(err) = self
            .cleanup_stale_secret_copies(ctx, cr, &pull_secret_name, &ssh_key_name)
            .await
        {
            log.error(&err, "Failed to cleanup stale secret copies", kv![]);
            return Err(err);
        }

        log.v(1).info(
            "Successfully copied secrets",
            kv![
                "pullSecret" => pull_secret_name,
                "sshKey" => ssh_key_name,
                "namespace" => cr.namespace()
            ],
        );

        Ok(ReconcileResult::default())
    }

    /// copyPullSecret copies the pull-secret within the same namespace with proper type and labels
    async fn copy_pull_secret(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
        target_name: &str,
    ) -> ctrlrt::Result<()> {
        self.copy_secret(
            ctx,
            cr,
            target_name,
            CopySpec {
                source_name: &cr.spec.pull_secret_ref.name,
                copy_type: PULL_SECRET_COPY_TYPE,
                secret_type: SECRET_TYPE_DOCKER_CONFIG_JSON,
                noun: "pull-secret",
                mark_noun: "pull-secret",
                reuse_noun: "Pull-secret",
            },
        )
        .await
    }

    /// copySSHKey copies the ssh-key within the same namespace with proper type and labels
    async fn copy_ssh_key(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
        target_name: &str,
    ) -> ctrlrt::Result<()> {
        self.copy_secret(
            ctx,
            cr,
            target_name,
            CopySpec {
                source_name: &cr.spec.ssh_key_secret_ref.name,
                copy_type: SSH_KEY_COPY_TYPE,
                secret_type: SECRET_TYPE_OPAQUE,
                noun: "ssh-key",
                mark_noun: "SSH-key",
                reuse_noun: "SSH key",
            },
        )
        .await
    }

    /// Body shared by copyPullSecret and copySSHKey (identical apart from the strings).
    async fn copy_secret(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
        target_name: &str,
        spec: CopySpec<'_>,
    ) -> ctrlrt::Result<()> {
        let log = &ctx.log;
        let namespace = cr.namespace();

        // Get source secret from DPFHCPProvisioner namespace
        let source_secret: Secret = match self
            .client
            .get(&ObjectKey::new(namespace, spec.source_name))
            .await
        {
            Ok(s) => s,
            Err(err) => {
                return Err(wrapf!(
                    err,
                    "failed to get {} {}/{}: {}",
                    spec.noun,
                    namespace,
                    spec.source_name,
                    err
                ));
            }
        };

        // Check if target secret already exists with matching labels (idempotency)
        match self
            .client
            .get::<Secret>(&ObjectKey::new(namespace, target_name))
            .await
        {
            Ok(mut existing_secret) => {
                // Secret exists, verify ownership via OwnerReference
                if meta::is_controlled_by(&existing_secret, cr) {
                    if set_credential_secret_copy_label(&mut existing_secret, spec.copy_type)
                        && let Err(err) = self.client.update(&mut existing_secret).await
                    {
                        return Err(wrapf!(
                            err,
                            "failed to mark {} copy {}: {}",
                            spec.mark_noun,
                            target_name,
                            err
                        ));
                    }
                    log.v(1).info(
                        &format!(
                            "{} already exists and is owned by this DPFHCPProvisioner, reusing",
                            spec.reuse_noun
                        ),
                        kv!["secret" => target_name, "namespace" => namespace],
                    );
                    return Ok(());
                }
                return Err(anyhow::anyhow!(
                    "{} {} exists in {} but is owned by different DPFHCPProvisioner",
                    spec.noun,
                    target_name,
                    namespace
                ));
            }
            Err(err) => {
                if !ctrlrt::error::is_not_found(&err) {
                    return Err(wrapf!(
                        err,
                        "failed to check existing {}: {}",
                        spec.noun,
                        err
                    ));
                }
            }
        }

        // Create target secret with correct type
        let mut target_secret = Secret {
            metadata: ObjectMeta {
                name: Some(target_name.to_string()),
                namespace: Some(namespace.to_string()),
                labels: Some(BTreeMap::from([(
                    CREDENTIAL_SECRET_COPY_LABEL.to_string(),
                    spec.copy_type.to_string(),
                )])),
                ..Default::default()
            },
            type_: Some(spec.secret_type.to_string()),
            data: source_secret.data,
            ..Default::default()
        };

        // Set owner reference for automatic garbage collection
        if let Err(err) = meta::set_controller_reference(cr, &mut target_secret) {
            return Err(wrapf!(
                err,
                "failed to set owner reference on {}: {}",
                spec.noun,
                err
            ));
        }

        if let Err(err) = self.client.create(&mut target_secret).await {
            return Err(wrapf!(err, "failed to create {}: {}", spec.noun, err));
        }

        log.info(
            &format!("Created {}", spec.noun),
            kv!["secret" => target_name, "namespace" => namespace],
        );

        Ok(())
    }

    /// cleanupStaleSecretCopies deletes owned pull-secret and ssh-key copies whose names no longer
    /// match the current expected names (i.e. the source reference changed). This also handles
    /// migration from the old fixed-name scheme (<cr>-pull-secret / <cr>-ssh-key) to the
    /// reference-encoded scheme.
    async fn cleanup_stale_secret_copies(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
        current_pull_secret_name: &str,
        current_ssh_key_name: &str,
    ) -> ctrlrt::Result<()> {
        let log = &ctx.log;

        let secret_list: Vec<Secret> = match self
            .client
            .list(&ListOptions::new().in_namespace(cr.namespace()))
            .await
        {
            Ok(l) => l,
            Err(err) => {
                return Err(wrapf!(
                    err,
                    "failed to list secrets for stale copy cleanup: {}",
                    err
                ));
            }
        };

        for s in &secret_list {
            if !meta::is_controlled_by(s, cr) {
                continue;
            }
            // Match operator-labeled copies and exact legacy names. The exact names support
            // cleanup of copies created before the label was added without broad prefix matching.
            let copy_label = s
                .labels()
                .get(CREDENTIAL_SECRET_COPY_LABEL)
                .map(String::as_str);
            let is_pull_copy = copy_label == Some(PULL_SECRET_COPY_TYPE)
                || s.name() == format!("{}-pull-secret", cr.name());
            let is_ssh_copy = copy_label == Some(SSH_KEY_COPY_TYPE)
                || s.name() == format!("{}-ssh-key", cr.name());
            let is_pull_stale = is_pull_copy && s.name() != current_pull_secret_name;
            let is_ssh_stale = is_ssh_copy && s.name() != current_ssh_key_name;
            if !is_pull_stale && !is_ssh_stale {
                continue;
            }
            log.info(
                "Deleting stale secret copy",
                kv!["secret" => s.name(), "namespace" => cr.namespace()],
            );
            if let Err(err) = self.client.delete(s).await
                && !ctrlrt::error::is_not_found(&err)
            {
                return Err(wrapf!(
                    err,
                    "failed to delete stale secret copy {}: {}",
                    s.name(),
                    err
                ));
            }
        }
        Ok(())
    }

    /// GenerateETCDEncryptionKey generates a 32-byte random key for ETCD encryption
    pub async fn generate_etcd_encryption_key(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;
        let namespace = cr.namespace();

        let secret_name = format!("{}-etcd-encryption-key", cr.name());

        // Check if secret already exists (idempotency)
        match self
            .client
            .get::<Secret>(&ObjectKey::new(namespace, &secret_name))
            .await
        {
            Ok(existing_secret) => {
                // Secret exists, verify ownership via OwnerReference
                if meta::is_controlled_by(&existing_secret, cr) {
                    log.v(1).info(
                        "ETCD encryption key already exists and is owned by this DPFHCPProvisioner, reusing",
                        kv!["secret" => secret_name, "namespace" => namespace],
                    );
                    return Ok(ReconcileResult::default());
                }
                return Err(anyhow::anyhow!(
                    "etcd encryption key {} exists in {} but is owned by different DPFHCPProvisioner",
                    secret_name,
                    namespace
                ));
            }
            Err(err) => {
                if !ctrlrt::error::is_not_found(&err) {
                    return Err(wrapf!(
                        err,
                        "failed to check existing etcd encryption key: {}",
                        err
                    ));
                }
            }
        }

        // Generate 32 random bytes for ETCD encryption
        let mut key_bytes = vec![0u8; 32];
        if let Err(err) = getrandom::fill(&mut key_bytes) {
            let err = anyhow::anyhow!("{err}");
            return Err(wrapf!(
                err,
                "failed to generate random encryption key: {}",
                err
            ));
        }
        let key_length = key_bytes.len();

        // Create secret with proper type
        let mut secret = Secret {
            metadata: ObjectMeta {
                name: Some(secret_name.clone()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            type_: Some(SECRET_TYPE_OPAQUE.to_string()),
            data: Some(BTreeMap::from([(
                hyperv1::AESCBC_KEY_SECRET_KEY.to_string(),
                ByteString(key_bytes),
            )])),
            ..Default::default()
        };

        // Set owner reference for automatic garbage collection
        if let Err(err) = meta::set_controller_reference(cr, &mut secret) {
            return Err(wrapf!(
                err,
                "failed to set owner reference on etcd encryption key: {}",
                err
            ));
        }

        if let Err(err) = self.client.create(&mut secret).await {
            return Err(wrapf!(
                err,
                "failed to create etcd encryption key secret: {}",
                err
            ));
        }

        log.info(
            "Generated ETCD encryption key",
            kv![
                "secret" => secret_name,
                "namespace" => namespace,
                "keyLength" => key_length
            ],
        );

        Ok(ReconcileResult::default())
    }
}
