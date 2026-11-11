//! Generic `CreateOrUpdate[T]` for typed hosted-cluster clients.

use async_trait::async_trait;
use ctrlrt::error::{Result, is_not_found};
use ctrlrt::{Ctx, Object};
use serde_json::Value;

/// `HostedResourceClient[T]`: the typed client methods used by [`create_or_update`].
/// Kubernetes typed clients (e.g. `CoreV1().Namespaces()`) satisfy this interface; here
/// [`crate::ResourceClient`] does.
#[async_trait]
pub trait HostedResourceClient<K: Object>: Send + Sync {
    /// `Get(ctx, name, metav1.GetOptions{})`
    async fn get(&self, ctx: &Ctx, name: &str) -> Result<K>;
    /// `Create(ctx, obj, metav1.CreateOptions{})`
    async fn create(&self, ctx: &Ctx, obj: &K) -> Result<K>;
    /// `Update(ctx, obj, metav1.UpdateOptions{})`
    async fn update(&self, ctx: &Ctx, obj: &K) -> Result<K>;
}

/// `hostedclient.OperationResult`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OperationResult {
    /// `OperationResultNone` (`"unchanged"`)
    None,
    /// `OperationResultCreated` (`"created"`)
    Created,
    /// `OperationResultUpdated` (`"updated"`)
    Updated,
}

impl OperationResult {
    /// The Go string value.
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationResult::None => "unchanged",
            OperationResult::Created => "created",
            OperationResult::Updated => "updated",
        }
    }
}

impl std::fmt::Display for OperationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The mutate function (`func(T) error`). `None` mirrors a nil `mutateFn`.
pub type MutateFn<'a, K> = &'a mut (dyn FnMut(&mut K) -> Result<()> + Send);

/// `CreateOrUpdate` gets an object by name and either creates it (if not found) or updates it
/// (if the mutate function changed it). It mirrors `controllerutil.CreateOrUpdate` but works
/// with typed hosted-cluster clients instead of controller-runtime's client.
///
/// As in Go, `obj` is mutated by `mutate_fn` (and sent) only when the object does not exist
/// yet; otherwise the mutate function is applied to the fetched object. `obj` is never
/// overwritten with the server's response.
pub async fn create_or_update<K, C>(
    ctx: &Ctx,
    c: &C,
    obj: &mut K,
    mutate_fn: Option<MutateFn<'_, K>>,
) -> Result<OperationResult>
where
    K: Object,
    C: HostedResourceClient<K> + ?Sized,
{
    let name = obj.name().to_string();
    let namespace = obj.namespace().to_string();

    let mut existing = match c.get(ctx, &name).await {
        Ok(existing) => existing,
        Err(err) => {
            if !is_not_found(&err) {
                return Err(err);
            }

            if let Some(f) = mutate_fn {
                f(obj)?;
                if obj.name() != name || obj.namespace() != namespace {
                    return Err(anyhow::anyhow!(
                        "mutate function cannot change object name or namespace"
                    ));
                }
            }

            c.create(ctx, obj).await?;
            return Ok(OperationResult::Created);
        }
    };

    let before = existing.clone();
    if let Some(f) = mutate_fn {
        f(&mut existing)?;
        if existing.name() != name || existing.namespace() != namespace {
            return Err(anyhow::anyhow!(
                "mutate function cannot change object name or namespace"
            ));
        }
    }

    if semantic_deep_equal(&before, &existing) {
        return Ok(OperationResult::None);
    }
    c.update(ctx, &existing).await?;
    Ok(OperationResult::Updated)
}

/// `equality.Semantic.DeepEqual(a, b)` for typed objects.
///
/// Implemented as equality of the JSON documents of both objects after normalisation that
/// mirrors apimachinery's forked `reflect.DeepEqual`, which treats nil and empty slices/maps
/// as equal: `null` values, empty arrays and empty objects are dropped recursively before
/// comparing. Quantities and times are compared by their serialized form (both objects of a
/// round-trip carry the canonical representation).
pub fn semantic_deep_equal<K: Object>(a: &K, b: &K) -> bool {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(x), Ok(y)) => normalize(x) == normalize(y),
        _ => false,
    }
}

fn normalize(v: Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter_map(|(k, v)| {
                    let v = normalize(v);
                    (!is_empty(&v)).then_some((k, v))
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        other => other,
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}
