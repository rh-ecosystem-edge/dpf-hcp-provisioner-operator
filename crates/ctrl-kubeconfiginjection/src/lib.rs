//! Port of internal/controller/kubeconfiginjection
//!
//! * [`KubeconfigInjector`] (`injector.go`): copies the HostedCluster admin kubeconfig into
//!   the DPUCluster namespace, points the DPUCluster at it and maintains the
//!   `KubeConfigInjected` condition.
//! * [`CleanupHandler`] (`cleanup_handler.go`): finalizer cleanup of the copied secrets.
//! * [`watch`] (`watch.go`): predicate and map function for the HC kubeconfig secret watch.

mod cleanup_handler;
mod injector;
pub mod watch;

pub use cleanup_handler::CleanupHandler;
pub use injector::{
    DESTINATION_KUBECONFIG_SECRET_KEY, DpuClusterNotFoundError, KUBECONFIG_SECRET_SUFFIX,
    KubeconfigInjector, SOURCE_KUBECONFIG_SECRET_KEY,
};
pub use watch::{
    find_provisioner_for_kubeconfig_secret, find_provisioner_for_kubeconfig_secret_map_fn,
    is_hosted_cluster_kubeconfig_secret_predicate,
};

/// Log value of a `types.NamespacedName` as rendered by logr/zap (`MarshalLog`):
/// `{"name": ..., "namespace": ...}` with `namespace` omitted when empty.
pub(crate) fn namespaced_name(key: &ctrlrt::ObjectKey) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), serde_json::Value::String(key.name.clone()));
    if !key.namespace.is_empty() {
        m.insert(
            "namespace".into(),
            serde_json::Value::String(key.namespace.clone()),
        );
    }
    serde_json::Value::Object(m)
}

// Go tests live in package `kubeconfiginjection` (white-box), so they are unit tests here.
#[cfg(test)]
mod injector_test;
#[cfg(test)]
mod watch_test;
