//! `sigs.k8s.io/controller-runtime/pkg/controller/controllerutil` — CreateOrUpdate.

use crate::client::Client;
use crate::error::Result;
use crate::object::Object;

/// `controllerutil.OperationResult`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationResult {
    None,
    Created,
    Updated,
    UpdatedStatus,
    UpdatedStatusOnly,
}

impl OperationResult {
    /// Go string values (`"unchanged"`, `"created"`, ...).
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationResult::None => "unchanged",
            OperationResult::Created => "created",
            OperationResult::Updated => "updated",
            OperationResult::UpdatedStatus => "updatedStatus",
            OperationResult::UpdatedStatusOnly => "updatedStatusOnly",
        }
    }
}

impl std::fmt::Display for OperationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `controllerutil.MutateFn` error when the key changes.
fn validate_key<K: Object>(key: &crate::ObjectKey, obj: &K) -> Result<()> {
    if obj.key() != *key {
        return Err(anyhow::anyhow!(
            "MutateFn cannot mutate object name and/or object namespace"
        ));
    }
    Ok(())
}

/// Semantic deep equality used by CreateOrUpdate (`equality.Semantic.DeepEqual`).
/// Implemented as JSON-document equality of the serialized objects.
pub fn semantic_equal<K: Object>(a: &K, b: &K) -> bool {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// `controllerutil.CreateOrUpdate(ctx, c, obj, f)`.
///
/// `obj` must have name (and namespace if namespaced) set. On return `obj` holds the
/// server's view of the object (as in Go, where the passed pointer is filled in).
pub async fn create_or_update<K, F>(c: &Client, obj: &mut K, mut f: F) -> Result<OperationResult>
where
    K: Object,
    F: FnMut(&mut K) -> Result<()>,
{
    let key = obj.key();
    match c.get::<K>(&key).await {
        Err(e) => {
            if !crate::error::is_not_found(&e) {
                return Err(e);
            }
            f(obj)?;
            validate_key(&key, obj)?;
            c.create(obj).await?;
            Ok(OperationResult::Created)
        }
        Ok(current) => {
            *obj = current;
            let existing = obj.clone();
            f(obj)?;
            validate_key(&key, obj)?;
            if semantic_equal(&existing, obj) {
                return Ok(OperationResult::None);
            }
            c.update(obj).await?;
            Ok(OperationResult::Updated)
        }
    }
}
