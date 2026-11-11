//! Reconcile types, watch events, predicates, handlers and the controller builder.
//!
//! The builder produces a type-erased [`ControllerSpec`] which a manager implementation
//! (`ctrlrt-runtime`) executes: one informer per watched resource feeding event handlers that
//! enqueue [`Request`]s into a rate-limited work queue drained by the [`Reconciler`].

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::client::{Client, from_value};
use crate::error::Result;
use crate::log::Logger;
use crate::object::{Object, ObjectKey, ResourceInfo};
use crate::recorder::Recorder;

/// `context.Context` analogue: carries the logger (`logf.FromContext`) and cancellation.
#[derive(Clone, Debug)]
pub struct Ctx {
    pub log: Logger,
    pub cancel: CancellationToken,
}

impl Default for Ctx {
    fn default() -> Self {
        Self::background()
    }
}

impl Ctx {
    /// `context.Background()` with the root logger.
    pub fn background() -> Self {
        Self {
            log: Logger::root(),
            cancel: CancellationToken::new(),
        }
    }
    /// Context with a specific logger.
    pub fn with_logger(&self, log: Logger) -> Self {
        Self {
            log,
            cancel: self.cancel.clone(),
        }
    }
    /// `logf.FromContext(ctx)`
    pub fn logger(&self) -> &Logger {
        &self.log
    }
    /// Child context cancelled with the parent.
    pub fn child(&self) -> Self {
        Self {
            log: self.log.clone(),
            cancel: self.cancel.child_token(),
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// `reconcile.Request` (a NamespacedName).
pub type Request = ObjectKey;

/// `ctrl.Result`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReconcileResult {
    /// Deprecated in controller-runtime but still honoured: requeue with rate limiting.
    pub requeue: bool,
    /// Requeue after the given duration (zero = not set).
    pub requeue_after: Duration,
}

impl ReconcileResult {
    pub fn done() -> Self {
        Self::default()
    }
    pub fn requeue() -> Self {
        Self {
            requeue: true,
            requeue_after: Duration::ZERO,
        }
    }
    pub fn requeue_after(d: Duration) -> Self {
        Self {
            requeue: false,
            requeue_after: d,
        }
    }
    /// `result.IsZero()`
    pub fn is_zero(&self) -> bool {
        !self.requeue && self.requeue_after.is_zero()
    }
}

/// `reconcile.Reconciler`
#[async_trait::async_trait]
pub trait Reconciler: Send + Sync + 'static {
    async fn reconcile(&self, ctx: &Ctx, req: Request) -> Result<ReconcileResult>;
}

/// `reconcile.TerminalError` marker: errors wrapping it are not retried.
#[derive(Debug)]
pub struct TerminalError(pub crate::Error);
impl std::fmt::Display for TerminalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "terminal error: {}", self.0)
    }
}
impl std::error::Error for TerminalError {}

// ---------------------------------------------------------------------------------------------
// Events & predicates
// ---------------------------------------------------------------------------------------------

/// `event.TypedCreateEvent` / Update / Delete / Generic.
#[derive(Clone, Debug)]
pub enum WatchEvent<K> {
    Create(Arc<K>),
    Update { old: Arc<K>, new: Arc<K> },
    Delete(Arc<K>),
    Generic(Arc<K>),
}

/// `predicate.Predicate`
pub trait Predicate<K>: Send + Sync + 'static {
    fn create(&self, _obj: &K) -> bool {
        true
    }
    fn update(&self, _old: &K, _new: &K) -> bool {
        true
    }
    fn delete(&self, _obj: &K) -> bool {
        true
    }
    fn generic(&self, _obj: &K) -> bool {
        true
    }
    /// Dispatch on the event kind.
    fn filter(&self, ev: &WatchEvent<K>) -> bool {
        match ev {
            WatchEvent::Create(o) => self.create(o),
            WatchEvent::Update { old, new } => self.update(old, new),
            WatchEvent::Delete(o) => self.delete(o),
            WatchEvent::Generic(o) => self.generic(o),
        }
    }
}

type PredFn<K> = Box<dyn Fn(&K) -> bool + Send + Sync>;
type PredFn2<K> = Box<dyn Fn(&K, &K) -> bool + Send + Sync>;

/// `predicate.Funcs`: unset functions default to `true`.
pub struct Funcs<K> {
    pub create: Option<PredFn<K>>,
    pub update: Option<PredFn2<K>>,
    pub delete: Option<PredFn<K>>,
    pub generic: Option<PredFn<K>>,
}

impl<K> Default for Funcs<K> {
    fn default() -> Self {
        Self {
            create: None,
            update: None,
            delete: None,
            generic: None,
        }
    }
}

impl<K> Funcs<K> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn on_create(mut self, f: impl Fn(&K) -> bool + Send + Sync + 'static) -> Self {
        self.create = Some(Box::new(f));
        self
    }
    pub fn on_update(mut self, f: impl Fn(&K, &K) -> bool + Send + Sync + 'static) -> Self {
        self.update = Some(Box::new(f));
        self
    }
    pub fn on_delete(mut self, f: impl Fn(&K) -> bool + Send + Sync + 'static) -> Self {
        self.delete = Some(Box::new(f));
        self
    }
    pub fn on_generic(mut self, f: impl Fn(&K) -> bool + Send + Sync + 'static) -> Self {
        self.generic = Some(Box::new(f));
        self
    }
}

impl<K: Send + Sync + 'static> Predicate<K> for Funcs<K> {
    fn create(&self, o: &K) -> bool {
        self.create.as_ref().is_none_or(|f| f(o))
    }
    fn update(&self, a: &K, b: &K) -> bool {
        self.update.as_ref().is_none_or(|f| f(a, b))
    }
    fn delete(&self, o: &K) -> bool {
        self.delete.as_ref().is_none_or(|f| f(o))
    }
    fn generic(&self, o: &K) -> bool {
        self.generic.as_ref().is_none_or(|f| f(o))
    }
}

/// `predicate.GenerationChangedPredicate{}`
#[derive(Clone, Copy, Debug, Default)]
pub struct GenerationChangedPredicate;
impl<K: Object> Predicate<K> for GenerationChangedPredicate {
    fn update(&self, old: &K, new: &K) -> bool {
        new.generation() != old.generation()
    }
}

/// `predicate.ResourceVersionChangedPredicate{}`
#[derive(Clone, Copy, Debug, Default)]
pub struct ResourceVersionChangedPredicate;
impl<K: Object> Predicate<K> for ResourceVersionChangedPredicate {
    fn update(&self, old: &K, new: &K) -> bool {
        new.resource_version() != old.resource_version()
    }
}

/// `predicate.LabelChangedPredicate{}`
#[derive(Clone, Copy, Debug, Default)]
pub struct LabelChangedPredicate;
impl<K: Object> Predicate<K> for LabelChangedPredicate {
    fn update(&self, old: &K, new: &K) -> bool {
        old.labels() != new.labels()
    }
}

/// `predicate.AnnotationChangedPredicate{}`
#[derive(Clone, Copy, Debug, Default)]
pub struct AnnotationChangedPredicate;
impl<K: Object> Predicate<K> for AnnotationChangedPredicate {
    fn update(&self, old: &K, new: &K) -> bool {
        old.annotations() != new.annotations()
    }
}

/// `predicate.Or(preds...)`
pub struct Or<K>(pub Vec<Box<dyn Predicate<K>>>);
impl<K: Send + Sync + 'static> Predicate<K> for Or<K> {
    fn create(&self, o: &K) -> bool {
        self.0.iter().any(|p| p.create(o))
    }
    fn update(&self, a: &K, b: &K) -> bool {
        self.0.iter().any(|p| p.update(a, b))
    }
    fn delete(&self, o: &K) -> bool {
        self.0.iter().any(|p| p.delete(o))
    }
    fn generic(&self, o: &K) -> bool {
        self.0.iter().any(|p| p.generic(o))
    }
}

/// `predicate.And(preds...)`
pub struct And<K>(pub Vec<Box<dyn Predicate<K>>>);
impl<K: Send + Sync + 'static> Predicate<K> for And<K> {
    fn create(&self, o: &K) -> bool {
        self.0.iter().all(|p| p.create(o))
    }
    fn update(&self, a: &K, b: &K) -> bool {
        self.0.iter().all(|p| p.update(a, b))
    }
    fn delete(&self, o: &K) -> bool {
        self.0.iter().all(|p| p.delete(o))
    }
    fn generic(&self, o: &K) -> bool {
        self.0.iter().all(|p| p.generic(o))
    }
}

// ---------------------------------------------------------------------------------------------
// Handlers (type erased)
// ---------------------------------------------------------------------------------------------

/// `handler.MapFunc`: `func(ctx, obj) []reconcile.Request`
pub type MapFn<K> = Arc<dyn Fn(Ctx, Arc<K>) -> BoxFuture<'static, Vec<Request>> + Send + Sync>;

/// Build a [`MapFn`] from an async closure.
pub fn map_fn<K, F, Fut>(f: F) -> MapFn<K>
where
    K: Send + Sync + 'static,
    F: Fn(Ctx, Arc<K>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Vec<Request>> + Send + 'static,
{
    Arc::new(move |ctx, obj| Box::pin(f(ctx, obj)))
}

/// A dynamically typed watch event as produced by informers.
#[derive(Clone, Debug)]
pub enum RawEvent {
    Create(Arc<Value>),
    Update { old: Arc<Value>, new: Arc<Value> },
    Delete(Arc<Value>),
    Generic(Arc<Value>),
}

/// Type-erased event handler: filters with predicates and maps to requests
/// (handler semantics of controller-runtime, incl. Update mapping both old and new object).
pub type RawHandler = Arc<dyn Fn(Ctx, RawEvent) -> BoxFuture<'static, Vec<Request>> + Send + Sync>;

/// How a watched resource is restricted (cache `ByObject` options).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchSelector {
    /// Label selector (`k=v,...`) applied to the watch; empty = everything.
    pub label_selector: Option<String>,
}

/// One watch of a controller.
#[derive(Clone)]
pub struct WatchSpec {
    pub resource: ResourceInfo,
    pub handler: RawHandler,
}

/// The fully-built controller description handed to the manager.
#[derive(Clone)]
pub struct ControllerSpec {
    pub name: String,
    /// GVK of the `For()` type (used for logging keys like the Go controller does).
    pub for_resource: ResourceInfo,
    pub watches: Vec<WatchSpec>,
    pub max_concurrent_reconciles: usize,
    pub reconciler: Arc<dyn Reconciler>,
}

fn decode<K: Object>(v: &Value) -> Option<Arc<K>> {
    from_value::<K>(v.clone()).ok().map(Arc::new)
}

fn typed_event<K: Object>(ev: &RawEvent) -> Option<WatchEvent<K>> {
    Some(match ev {
        RawEvent::Create(o) => WatchEvent::Create(decode(o)?),
        RawEvent::Update { old, new } => WatchEvent::Update {
            old: decode(old)?,
            new: decode(new)?,
        },
        RawEvent::Delete(o) => WatchEvent::Delete(decode(o)?),
        RawEvent::Generic(o) => WatchEvent::Generic(decode(o)?),
    })
}

fn passes<K: 'static>(preds: &[Arc<dyn Predicate<K>>], ev: &WatchEvent<K>) -> bool {
    preds.iter().all(|p| p.filter(ev))
}

fn dedup(reqs: Vec<Request>) -> Vec<Request> {
    let mut out: Vec<Request> = Vec::with_capacity(reqs.len());
    for r in reqs {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// `handler.EnqueueRequestForObject`
pub fn enqueue_for_object<K: Object>(preds: Vec<Arc<dyn Predicate<K>>>) -> RawHandler {
    Arc::new(move |_ctx, ev| {
        let out = match typed_event::<K>(&ev) {
            Some(tev) if passes(&preds, &tev) => {
                let o = match &tev {
                    WatchEvent::Create(o) | WatchEvent::Delete(o) | WatchEvent::Generic(o) => o,
                    WatchEvent::Update { new, .. } => new,
                };
                vec![o.key()]
            }
            _ => vec![],
        };
        Box::pin(async move { out })
    })
}

/// `handler.EnqueueRequestsFromMapFunc`
pub fn enqueue_from_map_fn<K: Object>(
    map: MapFn<K>,
    preds: Vec<Arc<dyn Predicate<K>>>,
) -> RawHandler {
    Arc::new(move |ctx, ev| {
        let map = map.clone();
        let tev = typed_event::<K>(&ev).filter(|t| passes(&preds, t));
        Box::pin(async move {
            let Some(tev) = tev else { return vec![] };
            match tev {
                WatchEvent::Create(o) | WatchEvent::Delete(o) | WatchEvent::Generic(o) => {
                    dedup(map(ctx, o).await)
                }
                WatchEvent::Update { old, new } => {
                    let mut all = map(ctx.clone(), old).await;
                    all.extend(map(ctx, new).await);
                    dedup(all)
                }
            }
        })
    })
}

/// `handler.EnqueueRequestForOwner(scheme, mapper, &Owner{}, [handler.OnlyControllerOwner()])`
pub fn enqueue_for_owner<K: Object, O: Object>(
    only_controller: bool,
    preds: Vec<Arc<dyn Predicate<K>>>,
) -> RawHandler {
    let owner = O::resource_info();
    Arc::new(move |_ctx, ev| {
        let owner = owner.clone();
        let reqs = typed_event::<K>(&ev)
            .filter(|t| passes(&preds, t))
            .map(|tev| {
                let objs: Vec<Arc<K>> = match tev {
                    WatchEvent::Create(o) | WatchEvent::Delete(o) | WatchEvent::Generic(o) => {
                        vec![o]
                    }
                    WatchEvent::Update { old, new } => vec![old, new],
                };
                let mut out = Vec::new();
                for o in objs {
                    let refs: Vec<_> = if only_controller {
                        crate::meta::get_controller_of(o.as_ref())
                            .cloned()
                            .into_iter()
                            .collect()
                    } else {
                        o.owner_references().to_vec()
                    };
                    for r in refs {
                        let group = match r.api_version.split('/').collect::<Vec<_>>()[..] {
                            [_v] => String::new(),
                            [g, _v] => g.to_string(),
                            _ => continue,
                        };
                        if r.kind == owner.kind && group == owner.group {
                            let ns = if owner.namespaced {
                                o.namespace().to_string()
                            } else {
                                String::new()
                            };
                            out.push(ObjectKey::new(ns, r.name.clone()));
                        }
                    }
                }
                dedup(out)
            })
            .unwrap_or_default();
        Box::pin(async move { reqs })
    })
}

/// `ctrl.NewControllerManagedBy(mgr)` builder.
pub struct Builder {
    name: Option<String>,
    for_resource: Option<ResourceInfo>,
    watches: Vec<WatchSpec>,
    max_concurrent_reconciles: usize,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Self {
            name: None,
            for_resource: None,
            watches: Vec::new(),
            max_concurrent_reconciles: 1,
        }
    }

    /// `.For(&K{}, builder.WithPredicates(...))`
    pub fn for_type<K: Object>(mut self, preds: Vec<Arc<dyn Predicate<K>>>) -> Self {
        let info = K::resource_info();
        self.for_resource = Some(info.clone());
        self.watches.push(WatchSpec {
            resource: info,
            handler: enqueue_for_object::<K>(preds),
        });
        self
    }

    /// `.Owns(&K{})` — enqueue the controller owner of type of the `For()` resource.
    /// `O` must be the `For()` type.
    pub fn owns<K: Object, O: Object>(mut self, preds: Vec<Arc<dyn Predicate<K>>>) -> Self {
        self.watches.push(WatchSpec {
            resource: K::resource_info(),
            handler: enqueue_for_owner::<K, O>(true, preds),
        });
        self
    }

    /// `.Watches(&K{}, handler.EnqueueRequestsFromMapFunc(f), builder.WithPredicates(...))`
    pub fn watches<K: Object>(mut self, map: MapFn<K>, preds: Vec<Arc<dyn Predicate<K>>>) -> Self {
        self.watches.push(WatchSpec {
            resource: K::resource_info(),
            handler: enqueue_from_map_fn::<K>(map, preds),
        });
        self
    }

    /// `.Watches(&K{}, handler.EnqueueRequestForOwner(..., &O{}, [OnlyControllerOwner]), preds)`
    pub fn watches_owner<K: Object, O: Object>(
        mut self,
        only_controller: bool,
        preds: Vec<Arc<dyn Predicate<K>>>,
    ) -> Self {
        self.watches.push(WatchSpec {
            resource: K::resource_info(),
            handler: enqueue_for_owner::<K, O>(only_controller, preds),
        });
        self
    }

    /// `.Watches` with a custom raw handler.
    pub fn watches_raw(mut self, resource: ResourceInfo, handler: RawHandler) -> Self {
        self.watches.push(WatchSpec { resource, handler });
        self
    }

    /// `.Named(name)`
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// `.WithOptions(controller.Options{MaxConcurrentReconciles: n})`
    pub fn max_concurrent_reconciles(mut self, n: usize) -> Self {
        self.max_concurrent_reconciles = n.max(1);
        self
    }

    /// `.Complete(r)` — produce the spec. The default name is the lower-cased kind of the
    /// `For()` type (controller-runtime behaviour).
    pub fn build(self, reconciler: Arc<dyn Reconciler>) -> Result<ControllerSpec> {
        let for_resource = self
            .for_resource
            .ok_or_else(|| anyhow::anyhow!("must provide an object for reconciliation"))?;
        let name = self
            .name
            .unwrap_or_else(|| for_resource.kind.to_lowercase());
        Ok(ControllerSpec {
            name,
            for_resource,
            watches: self.watches,
            max_concurrent_reconciles: self.max_concurrent_reconciles,
            reconciler,
        })
    }
}

/// What controllers need from a manager during `SetupWithManager`.
pub trait ManagerHandle: Send + Sync {
    /// `mgr.GetClient()` (cache-backed reads, direct writes).
    fn client(&self) -> Client;
    /// `mgr.GetAPIReader()` (uncached reads).
    fn api_reader(&self) -> Client;
    /// `mgr.GetEventRecorderFor(name)`
    fn event_recorder_for(&self, name: &str) -> Recorder;
    /// Register a controller (`Complete`).
    fn add_controller(&self, spec: ControllerSpec) -> Result<()>;
}
