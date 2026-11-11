//! Port of internal/controller/secrets/validator_test.go ("Secrets Validator" suite).

use std::collections::BTreeMap;
use std::sync::Arc;

use apis::k8s::core::v1::LocalObjectReference;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::k8s_openapi::ByteString;
use ctrlrt::k8s_openapi::api::core::v1::Secret;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, ObjectMeta};
use ctrlrt::meta;
use ctrlrt::{ApiError, Client, Ctx, FakeRecorder, Object, Recorder};
use ctrlrt_fake::{FakeClientBuilder, Reaction, Scheme, obj, obj_mut};

use crate::*;

/// BeforeEach: ctx, recorder (buffer 100) and a scheme with DPFHCPProvisioner + core/v1.
struct Env {
    ctx: Ctx,
    recorder: Arc<FakeRecorder>,
    scheme: Scheme,
}

fn before_each() -> Env {
    Env {
        ctx: Ctx::background(),
        recorder: Arc::new(FakeRecorder::new(100)),
        scheme: Scheme::new()
            .register::<provisioningv1alpha1::DPFHCPProvisioner>()
            .with_client_go(),
    }
}

impl Env {
    fn validator(&self, c: &Client) -> Validator {
        Validator::new(c.clone(), self.recorder.clone() as Recorder)
    }
}

fn secret(name: &str, key: &str, value: &str) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(name.into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            key.to_string(),
            ByteString(value.as_bytes().to_vec()),
        )])),
        ..Default::default()
    }
}

fn provisioner(ssh: &str, pull: &str) -> provisioningv1alpha1::DPFHCPProvisioner {
    let mut p = provisioningv1alpha1::DPFHCPProvisioner::default();
    p.metadata.name = Some("test-provisioner".into());
    p.metadata.namespace = Some("default".into());
    p.metadata.generation = Some(1);
    p.spec.ssh_key_secret_ref = LocalObjectReference {
        name: ssh.into(),
        ..Default::default()
    };
    p.spec.pull_secret_ref = LocalObjectReference {
        name: pull.into(),
        ..Default::default()
    };
    p
}

/// `Eventually(recorder.Events).Should(Receive(ContainSubstring(s)))`: receive events until
/// one matches.
fn receive_containing(recorder: &FakeRecorder, s: &str) -> bool {
    while let Some(e) = recorder.try_recv() {
        if e.contains(s) {
            return true;
        }
    }
    false
}

async fn get_updated(
    c: &Client,
    p: &provisioningv1alpha1::DPFHCPProvisioner,
) -> provisioningv1alpha1::DPFHCPProvisioner {
    c.get(&p.key()).await.expect("get updated provisioner")
}

fn secrets_valid(p: &provisioningv1alpha1::DPFHCPProvisioner) -> Condition {
    meta::find_status_condition(&p.status.conditions, provisioningv1alpha1::SECRETS_VALID)
        .cloned()
        .expect("SecretsValid condition")
}

fn build(env: &Env, objs: Vec<ctrlrt_fake::InitObject>) -> Client {
    FakeClientBuilder::new()
        .with_scheme(env.scheme.clone())
        .with_objects(objs)
        .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
        .build()
}

// Describe("ValidateSecrets") / Context("when both secrets exist and are valid")

#[tokio::test]
async fn validate_secrets_when_both_secrets_exist_and_are_valid_should_set_secrets_valid_true() {
    let env = before_each();
    let ssh = secret("ssh-key", SSH_PUBLIC_KEY_SECRET_KEY, "ssh-rsa AAAAB3...");
    let pull = secret(
        "pull-secret",
        PULL_SECRET_KEY,
        r#"{"auths":{"registry.io":{"auth":"..."}}}"#,
    );
    let mut p = provisioner("ssh-key", "pull-secret");
    let c = build(&env, vec![obj(&ssh), obj(&pull), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    let result = validator.validate_secrets(&env.ctx, &mut p).await.unwrap();
    assert!(result.requeue_after.is_zero());

    // Verify status updated
    let updated = get_updated(&c, &p).await;
    let condition = secrets_valid(&updated);
    assert_eq!(condition.status, meta::CONDITION_TRUE);
    assert_eq!(condition.reason, REASON_SECRETS_VALID);
    assert!(condition.message.contains("ssh-key"));
    assert!(condition.message.contains("pull-secret"));
    assert_eq!(condition.observed_generation, Some(1));

    // Verify event emitted
    assert!(receive_containing(&env.recorder, "SecretsValid"));
}

#[tokio::test]
async fn validate_secrets_when_both_secrets_exist_and_are_valid_should_emit_event_only_on_first_success_not_on_subsequent_reconciliations()
 {
    let env = before_each();
    let ssh = secret("ssh-key", SSH_PUBLIC_KEY_SECRET_KEY, "ssh-rsa AAAAB3...");
    let pull = secret("pull-secret", PULL_SECRET_KEY, r#"{"auths":{}}"#);
    let mut p = provisioner("ssh-key", "pull-secret");
    let c = build(&env, vec![obj(&ssh), obj(&pull), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    // First reconciliation - should emit event
    let _ = validator.validate_secrets(&env.ctx, &mut p).await;
    assert!(receive_containing(&env.recorder, "SecretsValid"));

    // Get updated provisioner for second reconciliation
    let mut updated = get_updated(&c, &p).await;

    // Second reconciliation - should NOT emit event (condition unchanged)
    let _ = validator.validate_secrets(&env.ctx, &mut updated).await;
    assert!(env.recorder.try_recv().is_none());
}

// Context("when SSH key secret is missing")

#[tokio::test]
async fn validate_secrets_when_ssh_key_secret_is_missing_should_set_secrets_valid_false_with_ssh_key_secret_missing_reason_and_not_requeue()
 {
    let env = before_each();
    let pull = secret("pull-secret", PULL_SECRET_KEY, r#"{"auths":{}}"#);
    let mut p = provisioner("missing-ssh-key", "pull-secret");
    let c = build(&env, vec![obj(&pull), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    let result = validator.validate_secrets(&env.ctx, &mut p).await.unwrap();
    assert!(result.requeue_after.is_zero()); // Permanent error - don't requeue

    let updated = get_updated(&c, &p).await;
    let condition = secrets_valid(&updated);
    assert_eq!(condition.status, meta::CONDITION_FALSE);
    assert_eq!(condition.reason, REASON_SSH_KEY_SECRET_MISSING);
    assert!(condition.message.contains("missing-ssh-key"));
    assert!(condition.message.contains("not found"));

    // Verify warning event emitted
    assert!(receive_containing(&env.recorder, "SSHKeySecretMissing"));
}

// Context("when SSH key secret is missing required key")

#[tokio::test]
async fn validate_secrets_when_ssh_key_secret_is_missing_required_key_should_set_secrets_valid_false_with_ssh_key_secret_invalid_reason_and_not_requeue()
 {
    let env = before_each();
    let ssh = secret("ssh-key", "wrong-key", "ssh-rsa AAAAB3...");
    let pull = secret("pull-secret", PULL_SECRET_KEY, r#"{"auths":{}}"#);
    let mut p = provisioner("ssh-key", "pull-secret");
    let c = build(&env, vec![obj(&ssh), obj(&pull), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    let result = validator.validate_secrets(&env.ctx, &mut p).await.unwrap();
    assert!(result.requeue_after.is_zero());

    let updated = get_updated(&c, &p).await;
    let condition = secrets_valid(&updated);
    assert_eq!(condition.status, meta::CONDITION_FALSE);
    assert_eq!(condition.reason, REASON_SSH_KEY_SECRET_INVALID);
    assert!(condition.message.contains("ssh-key"));
    assert!(condition.message.contains(SSH_PUBLIC_KEY_SECRET_KEY));

    assert!(receive_containing(&env.recorder, "SSHKeySecretInvalid"));
}

// Context("when pull secret is missing")

#[tokio::test]
async fn validate_secrets_when_pull_secret_is_missing_should_set_secrets_valid_false_with_pull_secret_missing_reason_and_not_requeue()
 {
    let env = before_each();
    let ssh = secret("ssh-key", SSH_PUBLIC_KEY_SECRET_KEY, "ssh-rsa AAAAB3...");
    let mut p = provisioner("ssh-key", "missing-pull-secret");
    let c = build(&env, vec![obj(&ssh), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    let result = validator.validate_secrets(&env.ctx, &mut p).await.unwrap();
    assert!(result.requeue_after.is_zero());

    let updated = get_updated(&c, &p).await;
    let condition = secrets_valid(&updated);
    assert_eq!(condition.status, meta::CONDITION_FALSE);
    assert_eq!(condition.reason, REASON_PULL_SECRET_MISSING);
    assert!(condition.message.contains("missing-pull-secret"));
    assert!(condition.message.contains("not found"));

    assert!(receive_containing(&env.recorder, "PullSecretMissing"));
}

// Context("when pull secret is missing required key")

#[tokio::test]
async fn validate_secrets_when_pull_secret_is_missing_required_key_should_set_secrets_valid_false_with_pull_secret_invalid_reason_and_not_requeue()
 {
    let env = before_each();
    let ssh = secret("ssh-key", SSH_PUBLIC_KEY_SECRET_KEY, "ssh-rsa AAAAB3...");
    let pull = secret("pull-secret", "wrong-key", r#"{"auths":{}}"#);
    let mut p = provisioner("ssh-key", "pull-secret");
    let c = build(&env, vec![obj(&ssh), obj(&pull), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    let result = validator.validate_secrets(&env.ctx, &mut p).await.unwrap();
    assert!(result.requeue_after.is_zero());

    let updated = get_updated(&c, &p).await;
    let condition = secrets_valid(&updated);
    assert_eq!(condition.status, meta::CONDITION_FALSE);
    assert_eq!(condition.reason, REASON_PULL_SECRET_INVALID);
    assert!(condition.message.contains("pull-secret"));
    assert!(condition.message.contains(PULL_SECRET_KEY));

    assert!(receive_containing(&env.recorder, "PullSecretInvalid"));
}

// Context("when RBAC permission is denied")

#[tokio::test]
async fn validate_secrets_when_rbac_permission_is_denied_should_set_secrets_valid_false_with_access_denied_reason_and_not_requeue()
 {
    let env = before_each();
    let mut p = provisioner("ssh-key", "pull-secret");
    // Create fake client that returns Forbidden error (forbiddenSecretsClient)
    let c = FakeClientBuilder::new()
        .with_scheme(env.scheme.clone())
        .with_objects([obj_mut(&mut p)])
        .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
        .with_reactor("get", "secrets", |a| {
            Reaction::error(ApiError::forbidden("", "secrets", &a.name, "<nil>"))
        })
        .build();
    let validator = env.validator(&c);

    let res = validator.validate_secrets(&env.ctx, &mut p).await;
    assert!(res.is_ok(), "{:?}", res.err());

    // Extra (beyond the Go test): the condition and event carry the Forbidden error.
    let updated = get_updated(&c, &p).await;
    let condition = secrets_valid(&updated);
    assert_eq!(condition.reason, REASON_SECRETS_ACCESS_DENIED);
    assert_eq!(
        condition.message,
        "Operator lacks RBAC permissions to access SSH key secret in namespace 'default': secrets \"ssh-key\" is forbidden: <nil>"
    );
    assert_eq!(
        env.recorder.drain(),
        vec![
            "Warning SecretsAccessDenied Operator lacks RBAC permissions to access SSH key secret in namespace 'default': secrets \"ssh-key\" is forbidden: <nil>"
        ]
    );
}

// Context("when secrets are recovered")

fn with_previous_condition(
    mut p: provisioningv1alpha1::DPFHCPProvisioner,
    reason: &str,
    message: &str,
) -> provisioningv1alpha1::DPFHCPProvisioner {
    let mut cond = meta::condition(
        provisioningv1alpha1::SECRETS_VALID,
        meta::CONDITION_FALSE,
        reason,
        message,
    );
    cond.last_transition_time = meta::now();
    p.status.conditions = vec![cond];
    p
}

#[tokio::test]
async fn validate_secrets_when_secrets_are_recovered_should_transition_from_ssh_key_secret_missing_to_secrets_valid()
 {
    let env = before_each();
    let ssh = secret("ssh-key", SSH_PUBLIC_KEY_SECRET_KEY, "ssh-rsa AAAAB3...");
    let pull = secret("pull-secret", PULL_SECRET_KEY, r#"{"auths":{}}"#);
    let mut p = with_previous_condition(
        provisioner("ssh-key", "pull-secret"),
        REASON_SSH_KEY_SECRET_MISSING,
        "SSH key secret not found",
    );
    let c = build(&env, vec![obj(&ssh), obj(&pull), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator.validate_secrets(&env.ctx, &mut p).await.unwrap();

    let updated = get_updated(&c, &p).await;
    let condition = secrets_valid(&updated);
    assert_eq!(condition.status, meta::CONDITION_TRUE);
    assert_eq!(condition.reason, REASON_SECRETS_VALID);
}

#[tokio::test]
async fn validate_secrets_when_secrets_are_recovered_should_transition_from_pull_secret_invalid_to_secrets_valid()
 {
    let env = before_each();
    let ssh = secret("ssh-key", SSH_PUBLIC_KEY_SECRET_KEY, "ssh-rsa AAAAB3...");
    let pull = secret("pull-secret", PULL_SECRET_KEY, r#"{"auths":{}}"#);
    let mut p = with_previous_condition(
        provisioner("ssh-key", "pull-secret"),
        REASON_PULL_SECRET_INVALID,
        "Pull secret is missing required key",
    );
    let c = build(&env, vec![obj(&ssh), obj(&pull), obj_mut(&mut p)]);
    let validator = env.validator(&c);

    validator.validate_secrets(&env.ctx, &mut p).await.unwrap();

    let updated = get_updated(&c, &p).await;
    let condition = secrets_valid(&updated);
    assert_eq!(condition.status, meta::CONDITION_TRUE);
    assert_eq!(condition.reason, REASON_SECRETS_VALID);
}
