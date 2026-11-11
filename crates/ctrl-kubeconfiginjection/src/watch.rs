//! Port of `internal/controller/kubeconfiginjection/watch.go`.
//!
//! Go's predicate and map function operate on `client.Object` and type-assert to
//! `*corev1.Secret`; here they are generic over the watched type `K` and downcast to
//! [`Secret`] (anything else behaves like a failed type assertion). The main controller uses
//! them with `K = Secret`:
//!
//! ```text
//! builder.watches::<Secret>(
//!     find_provisioner_for_kubeconfig_secret_map_fn(client),
//!     vec![Arc::new(is_hosted_cluster_kubeconfig_secret_predicate::<Secret>())],
//! )
//! ```

use std::any::Any;
use std::sync::Arc;

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::{Client, Ctx, Funcs, MapFn, Object, ObjectKey, Request, kv};
use k8s_openapi::api::core::v1::Secret;

use crate::injector::KUBECONFIG_SECRET_SUFFIX;

/// IsHostedClusterKubeconfigSecretPredicate returns a predicate that filters for HC kubeconfig secrets
/// Only watches secrets with name pattern "*-admin-kubeconfig"
pub fn is_hosted_cluster_kubeconfig_secret_predicate<K: Object>() -> Funcs<K> {
    Funcs::new()
        .on_create(|obj: &K| is_hosted_cluster_kubeconfig_secret(obj))
        .on_update(|_old: &K, new: &K| is_hosted_cluster_kubeconfig_secret(new))
        .on_delete(|obj: &K| is_hosted_cluster_kubeconfig_secret(obj))
        .on_generic(|_obj: &K| {
            // Don't watch generic events for secrets
            false
        })
}

/// `obj.(*corev1.Secret)`
fn as_secret<K: Object>(obj: &K) -> Option<&Secret> {
    (obj as &dyn Any).downcast_ref::<Secret>()
}

/// isHostedClusterKubeconfigSecret checks if a secret is an HC admin kubeconfig secret
/// Returns true if secret name ends with "-admin-kubeconfig"
pub(crate) fn is_hosted_cluster_kubeconfig_secret<K: Object>(obj: &K) -> bool {
    let Some(secret) = as_secret(obj) else {
        return false;
    };
    secret.name().ends_with(KUBECONFIG_SECRET_SUFFIX)
}

/// FindProvisionerForKubeconfigSecret maps HC kubeconfig secret to DPFHCPProvisioner CR
/// Extracts HC name from secret name and finds corresponding DPFHCPProvisioner
/// Returns reconcile requests for the matching DPFHCPProvisioner
pub async fn find_provisioner_for_kubeconfig_secret<K: Object>(
    ctx: &Ctx,
    c: &Client,
    obj: &K,
) -> Vec<Request> {
    let log = &ctx.log;

    let Some(secret) = as_secret(obj) else {
        let mut o = serde_json::Map::new();
        if !obj.namespace().is_empty() {
            o.insert("namespace".into(), obj.namespace().into());
        }
        o.insert("name".into(), obj.name().into());
        log.error_nil(
            "Failed to convert object to Secret",
            kv!["object" => serde_json::Value::Object(o)],
        );
        return Vec::new();
    };

    // Extract HC name from secret name (remove "-admin-kubeconfig" suffix)
    let hc_name = secret
        .name()
        .strip_suffix(KUBECONFIG_SECRET_SUFFIX)
        .unwrap_or(secret.name());

    // Find corresponding DPFHCPProvisioner
    let provisioner = match c
        .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ObjectKey::new(
            secret.namespace(),
            hc_name,
        ))
        .await
    {
        Ok(p) => p,
        Err(err) => {
            // Provisioner not found - this is normal if the secret belongs to a different HC
            log.v(1).info(
                "No DPFHCPProvisioner found for HC kubeconfig secret",
                kv![
                    "secretName" => secret.name(),
                    "secretNamespace" => secret.namespace(),
                    "extractedHCName" => hc_name,
                    "error" => err.to_string()
                ],
            );
            return Vec::new();
        }
    };

    log.info(
        "HC kubeconfig secret changed, triggering reconciliation",
        kv![
            "secretName" => secret.name(),
            "secretNamespace" => secret.namespace(),
            "provisioner" => provisioner.name(),
            "provisionerNamespace" => provisioner.namespace()
        ],
    );

    // Trigger reconciliation for this provisioner
    vec![ObjectKey::new(provisioner.namespace(), provisioner.name())]
}

/// `func(ctx, obj) []reconcile.Request { return FindProvisionerForKubeconfigSecret(ctx, c, obj) }`
/// as a [`MapFn`] for `ctrlrt::Builder::watches` (Go: `handler.EnqueueRequestsFromMapFunc`).
pub fn find_provisioner_for_kubeconfig_secret_map_fn<K: Object>(c: Client) -> MapFn<K> {
    ctrlrt::map_fn(move |ctx: Ctx, obj: Arc<K>| {
        let c = c.clone();
        async move { find_provisioner_for_kubeconfig_secret(&ctx, &c, obj.as_ref()).await }
    })
}
