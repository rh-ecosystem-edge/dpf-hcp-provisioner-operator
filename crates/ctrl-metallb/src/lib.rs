//! Port of internal/controller/metallb
//!
//! * [`MetalLBManager`] (`metallb.go`): creates/maintains the IPAddressPool and
//!   L2Advertisement for a DPFHCPProvisioner and the `MetalLBConfigured` condition.
//! * [`CleanupHandler`] (`cleanup_handler.go`): finalizer cleanup of those resources.

mod cleanup_handler;
mod metallb;

pub use cleanup_handler::CleanupHandler;
pub use metallb::MetalLBManager;

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
