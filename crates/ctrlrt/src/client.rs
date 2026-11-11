//! `client.Client` analogue.
//!
//! [`Client`] is a cheap-to-clone typed facade over a dynamically-typed [`ClientBackend`]
//! that speaks `serde_json::Value`. Backends:
//! * `ctrlrt-runtime`: real API server (direct) and informer-cache backed reader.
//! * `ctrlrt-fake`: in-memory object tracker with controller-runtime fake client semantics.
//!
//! Semantics follow controller-runtime: `create`/`update`/`update_status` overwrite the passed
//! object with the server's response (resourceVersion, uid, defaults...), `get` replaces the
//! whole object.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result, wrap};
use crate::object::{Object, ObjectKey, ResourceInfo};

/// `client.ListOptions` subset used by the operator (`InNamespace`, `MatchingLabels`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListOptions {
    /// `client.InNamespace` (empty = all namespaces)
    pub namespace: String,
    /// `client.MatchingLabels` (equality-based selector, all must match)
    pub labels: BTreeMap<String, String>,
}

impl ListOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn in_namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }
    pub fn matching_labels<I, A, B>(mut self, labels: I) -> Self
    where
        I: IntoIterator<Item = (A, B)>,
        A: Into<String>,
        B: Into<String>,
    {
        for (k, v) in labels {
            self.labels.insert(k.into(), v.into());
        }
        self
    }
    /// Label selector string (`k1=v1,k2=v2`) or `None` when no labels requested.
    pub fn label_selector(&self) -> Option<String> {
        if self.labels.is_empty() {
            None
        } else {
            Some(
                self.labels
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(","),
            )
        }
    }
    /// Whether the given labels satisfy this selector.
    pub fn labels_match(&self, labels: Option<&BTreeMap<String, String>>) -> bool {
        self.labels
            .iter()
            .all(|(k, v)| labels.and_then(|l| l.get(k)) == Some(v))
    }
}

/// `metav1.DeletionPropagation`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropagationPolicy {
    Orphan,
    Background,
    Foreground,
}

impl PropagationPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            PropagationPolicy::Orphan => "Orphan",
            PropagationPolicy::Background => "Background",
            PropagationPolicy::Foreground => "Foreground",
        }
    }
}

/// `client.DeleteOptions` subset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeleteOptions {
    pub propagation_policy: Option<PropagationPolicy>,
    pub grace_period_seconds: Option<i64>,
    /// Precondition on resourceVersion.
    pub precondition_resource_version: Option<String>,
    /// Precondition on UID.
    pub precondition_uid: Option<String>,
}

/// Patch flavours (`client.MergeFrom` produces a JSON merge patch).
#[derive(Clone, Debug, PartialEq)]
pub enum Patch {
    /// RFC 7386 JSON merge patch.
    Merge(Value),
    /// Strategic merge patch (built-in types only).
    Strategic(Value),
}

/// Dynamically typed backend. All objects are full JSON documents including
/// `apiVersion`/`kind`/`metadata`.
#[async_trait::async_trait]
pub trait ClientBackend: Send + Sync + 'static {
    async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value>;
    async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>>;
    async fn create(&self, res: &ResourceInfo, obj: Value) -> Result<Value>;
    async fn update(&self, res: &ResourceInfo, obj: Value) -> Result<Value>;
    /// Update a subresource (`status`, `approval`, ...) with the full object as body.
    async fn update_subresource(
        &self,
        res: &ResourceInfo,
        subresource: &str,
        obj: Value,
    ) -> Result<Value>;
    async fn patch(
        &self,
        res: &ResourceInfo,
        namespace: &str,
        name: &str,
        patch: &Patch,
    ) -> Result<Value>;
    async fn patch_subresource(
        &self,
        res: &ResourceInfo,
        subresource: &str,
        namespace: &str,
        name: &str,
        patch: &Patch,
    ) -> Result<Value>;
    async fn delete(
        &self,
        res: &ResourceInfo,
        namespace: &str,
        name: &str,
        opts: &DeleteOptions,
    ) -> Result<()>;
}

/// The typed client handed to every component (Go: `client.Client` / `client.Reader`).
#[derive(Clone)]
pub struct Client {
    backend: Arc<dyn ClientBackend>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Client")
    }
}

/// Serialize a typed object to a full JSON document, filling `apiVersion`/`kind`
/// (Go's client always sends the scheme's GVK).
pub fn to_value<K: Object>(obj: &K) -> Result<Value> {
    let mut v = serde_json::to_value(obj)
        .map_err(|e| wrap(anyhow::anyhow!(e.to_string()), format!("failed to encode object: {e}")))?;
    let info = K::resource_info();
    if let Value::Object(map) = &mut v {
        map.insert("apiVersion".into(), Value::String(info.api_version()));
        map.insert("kind".into(), Value::String(info.kind.clone()));
    }
    Ok(v)
}

/// Deserialize a JSON document into a typed object.
pub fn from_value<K: Object>(v: Value) -> Result<K> {
    serde_json::from_value(v).map_err(|e| anyhow::anyhow!("failed to decode object: {e}"))
}

impl Client {
    pub fn new(backend: Arc<dyn ClientBackend>) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &Arc<dyn ClientBackend> {
        &self.backend
    }

    /// `c.Get(ctx, key, obj)`
    pub async fn get<K: Object>(&self, key: &ObjectKey) -> Result<K> {
        let info = K::resource_info();
        let ns = if info.namespaced { key.namespace.as_str() } else { "" };
        let v = self.backend.get(&info, ns, &key.name).await?;
        from_value(v)
    }

    /// Convenience: `get` with namespace and name.
    pub async fn get_named<K: Object>(&self, namespace: &str, name: &str) -> Result<K> {
        self.get(&ObjectKey::new(namespace, name)).await
    }

    /// `c.Get(ctx, key, obj)` in place: on success `obj` is replaced with the server copy.
    pub async fn get_into<K: Object>(&self, key: &ObjectKey, obj: &mut K) -> Result<()> {
        *obj = self.get(key).await?;
        Ok(())
    }

    /// `c.List(ctx, &list, opts...)`
    pub async fn list<K: Object>(&self, opts: &ListOptions) -> Result<Vec<K>> {
        let info = K::resource_info();
        let items = self.backend.list(&info, opts).await?;
        items.into_iter().map(from_value).collect()
    }

    /// `c.List` across all namespaces with no selector.
    pub async fn list_all<K: Object>(&self) -> Result<Vec<K>> {
        self.list(&ListOptions::default()).await
    }

    /// `c.Create(ctx, obj)`; `obj` is updated with the server response.
    pub async fn create<K: Object>(&self, obj: &mut K) -> Result<()> {
        let info = K::resource_info();
        let v = self.backend.create(&info, to_value(obj)?).await?;
        *obj = from_value(v)?;
        Ok(())
    }

    /// `c.Update(ctx, obj)`; `obj` is updated with the server response.
    pub async fn update<K: Object>(&self, obj: &mut K) -> Result<()> {
        let info = K::resource_info();
        let v = self.backend.update(&info, to_value(obj)?).await?;
        *obj = from_value(v)?;
        Ok(())
    }

    /// `c.Status().Update(ctx, obj)`; `obj` is updated with the server response.
    pub async fn update_status<K: Object>(&self, obj: &mut K) -> Result<()> {
        self.update_subresource(obj, "status").await
    }

    /// `c.SubResource(name).Update(ctx, obj)` (e.g. CSR `approval`).
    pub async fn update_subresource<K: Object>(&self, obj: &mut K, subresource: &str) -> Result<()> {
        let info = K::resource_info();
        let v = self
            .backend
            .update_subresource(&info, subresource, to_value(obj)?)
            .await?;
        *obj = from_value(v)?;
        Ok(())
    }

    /// `c.Patch(ctx, obj, patch)`; `obj` is updated with the server response.
    pub async fn patch<K: Object>(&self, obj: &mut K, patch: &Patch) -> Result<()> {
        let info = K::resource_info();
        let ns = obj.namespace().to_string();
        let name = obj.name().to_string();
        let v = self.backend.patch(&info, &ns, &name, patch).await?;
        *obj = from_value(v)?;
        Ok(())
    }

    /// `c.Status().Patch(ctx, obj, patch)`
    pub async fn patch_status<K: Object>(&self, obj: &mut K, patch: &Patch) -> Result<()> {
        let info = K::resource_info();
        let ns = obj.namespace().to_string();
        let name = obj.name().to_string();
        let v = self
            .backend
            .patch_subresource(&info, "status", &ns, &name, patch)
            .await?;
        *obj = from_value(v)?;
        Ok(())
    }

    /// `c.Delete(ctx, obj)`
    pub async fn delete<K: Object>(&self, obj: &K) -> Result<()> {
        self.delete_with(obj, &DeleteOptions::default()).await
    }

    /// `c.Delete(ctx, obj, opts...)`
    pub async fn delete_with<K: Object>(&self, obj: &K, opts: &DeleteOptions) -> Result<()> {
        let info = K::resource_info();
        let ns = if info.namespaced { obj.namespace() } else { "" };
        self.backend.delete(&info, ns, obj.name(), opts).await
    }

    /// Delete by key without having the object.
    pub async fn delete_key<K: Object>(&self, key: &ObjectKey, opts: &DeleteOptions) -> Result<()> {
        let info = K::resource_info();
        let ns = if info.namespaced { key.namespace.as_str() } else { "" };
        self.backend.delete(&info, ns, &key.name, opts).await
    }
}

/// Helper used by backends: extract `metadata.namespace` / `metadata.name` from a document.
pub fn value_key(v: &Value) -> (String, String) {
    let md = v.get("metadata");
    let ns = md
        .and_then(|m| m.get("namespace"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let name = md
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    (ns, name)
}

/// Errors produced while (de)serializing are plain errors.
pub fn encode_error(e: impl std::fmt::Display) -> Error {
    anyhow::anyhow!("{e}")
}
