//! Informer cache (`sigs.k8s.io/controller-runtime/pkg/cache`): informer cache per namespace
//! restriction, multi-namespace cache, delegating-by-GVK cache (`ByObject`), the cache reader
//! (`internal.CacheReader`) and the cache-backed client.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctrlrt::error::ApiError;
use ctrlrt::{ClientBackend, DeleteOptions, ListOptions, Patch, ResourceInfo};
use serde_json::Value;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::Result;
use crate::client::{KubeBackend, RestMapper};
use crate::informer::{InformerOptions, SharedInformer, calculate_resync_period, object_key};
use crate::util::go_type_name;

/// `cache.AllNamespaces`
pub const ALL_NAMESPACES: &str = "";

/// `defaultSyncPeriod` (10 hours).
pub const DEFAULT_SYNC_PERIOD: Duration = Duration::from_secs(10 * 3600);

/// `cache.ErrCacheNotStarted`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrCacheNotStarted;
impl std::fmt::Display for ErrCacheNotStarted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the cache is not started, can not read objects")
    }
}
impl std::error::Error for ErrCacheNotStarted {}

/// `cache.Config`: per-namespace (or per-object) informer restrictions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheConfig {
    /// `LabelSelector` rendered as a selector string (see [`crate::util::selector_from_set`]).
    pub label_selector: Option<String>,
    /// `FieldSelector` rendered as a selector string.
    pub field_selector: Option<String>,
    /// `SyncPeriod`
    pub sync_period: Option<Duration>,
}

impl CacheConfig {
    /// `cache.Config{LabelSelector: labels.SelectorFromSet(set)}`
    pub fn with_labels<I, K, V>(set: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let m: BTreeMap<String, String> =
            set.into_iter().map(|(k, v)| (k.into(), v.into())).collect();
        Self {
            label_selector: Some(crate::util::selector_from_set(&m)),
            ..Default::default()
        }
    }
}

/// `cache.ByObject`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByObject {
    pub resource: ResourceInfo,
    /// `Namespaces` (`None` = not set).
    pub namespaces: Option<BTreeMap<String, CacheConfig>>,
    /// `Label`
    pub label_selector: Option<String>,
    /// `Field`
    pub field_selector: Option<String>,
    /// `SyncPeriod`
    pub sync_period: Option<Duration>,
}

impl ByObject {
    pub fn new<K: ctrlrt::Object>() -> Self {
        Self::for_resource(K::resource_info())
    }
    pub fn for_resource(resource: ResourceInfo) -> Self {
        Self {
            resource,
            namespaces: None,
            label_selector: None,
            field_selector: None,
            sync_period: None,
        }
    }
    /// Add a namespace entry (`Namespaces[ns] = cfg`).
    pub fn namespace(mut self, ns: impl Into<String>, cfg: CacheConfig) -> Self {
        self.namespaces
            .get_or_insert_with(BTreeMap::new)
            .insert(ns.into(), cfg);
        self
    }
    pub fn label(mut self, selector: impl Into<String>) -> Self {
        self.label_selector = Some(selector.into());
        self
    }
    pub fn field(mut self, selector: impl Into<String>) -> Self {
        self.field_selector = Some(selector.into());
        self
    }
    fn as_config(&self) -> CacheConfig {
        CacheConfig {
            label_selector: self.label_selector.clone(),
            field_selector: self.field_selector.clone(),
            sync_period: self.sync_period,
        }
    }
}

/// `cache.Options` (subset used by the operator).
#[derive(Clone, Debug, Default)]
pub struct CacheOptions {
    pub by_object: Vec<ByObject>,
    pub default_namespaces: BTreeMap<String, CacheConfig>,
    pub default_label_selector: Option<String>,
    pub default_field_selector: Option<String>,
    /// `SyncPeriod` (default 10h).
    pub sync_period: Option<Duration>,
}

impl CacheOptions {
    fn defaults_config(&self) -> CacheConfig {
        CacheConfig {
            label_selector: self.default_label_selector.clone(),
            field_selector: self.default_field_selector.clone(),
            sync_period: self.sync_period,
        }
    }
}

fn default_config(mut to: CacheConfig, from: &CacheConfig) -> CacheConfig {
    if to.label_selector.is_none() {
        to.label_selector = from.label_selector.clone();
    }
    if to.field_selector.is_none() {
        to.field_selector = from.field_selector.clone();
    }
    if to.sync_period.is_none() {
        to.sync_period = from.sync_period;
    }
    to
}

/// `fields.AndSelectors(namespaceAllSelector(namespaces)..., existing)`.String()
fn namespace_all_field_selector(namespaces: &[String], existing: Option<&str>) -> Option<String> {
    let mut nss: Vec<&String> = namespaces.iter().filter(|n| !n.is_empty()).collect();
    nss.sort();
    let mut parts: Vec<String> = nss
        .into_iter()
        .map(|n| format!("metadata.namespace!={}", escape_field_value(n)))
        .collect();
    if let Some(e) = existing {
        parts.push(e.to_string());
    }
    Some(parts.join(","))
}

/// `fields.EscapeValue`
fn escape_field_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            '=' => out.push_str("\\="),
            c => out.push(c),
        }
    }
    out
}

/// `restrictNamespaceBySelector`: an exact `metadata.namespace=` field selector restricts the
/// list/watch to that namespace.
fn restrict_namespace_by_selector(namespace: &str, field_selector: Option<&str>) -> String {
    if !namespace.is_empty() {
        return namespace.to_string();
    }
    let Some(fs) = field_selector.filter(|s| !s.is_empty()) else {
        return String::new();
    };
    for term in fs.split(',') {
        if let Some(v) = term
            .strip_prefix("metadata.namespace==")
            .or_else(|| term.strip_prefix("metadata.namespace="))
        {
            return v.to_string();
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------------------------
// internal.Informers + informerCache
// ---------------------------------------------------------------------------------------------

type GvkKey = (String, String, String);

fn gvk_key(res: &ResourceInfo) -> GvkKey {
    (res.group.clone(), res.version.clone(), res.kind.clone())
}

struct InformersState {
    informers: HashMap<GvkKey, Arc<SharedInformer>>,
    started: bool,
    stopped: bool,
    ctx: Option<CancellationToken>,
}

/// One informer map restricted to a namespace / selectors (controller-runtime `informerCache`).
pub struct InformerCache {
    client: kube::Client,
    mapper: Arc<RestMapper>,
    namespace: String,
    config: CacheConfig,
    state: Mutex<InformersState>,
    start_wait: watch::Sender<bool>,
}

impl InformerCache {
    fn new(
        client: kube::Client,
        mapper: Arc<RestMapper>,
        config: CacheConfig,
        namespace: &str,
    ) -> Arc<Self> {
        let (start_wait, _) = watch::channel(false);
        Arc::new(Self {
            client,
            mapper,
            namespace: namespace.to_string(),
            config,
            state: Mutex::new(InformersState {
                informers: HashMap::new(),
                started: false,
                stopped: false,
                ctx: None,
            }),
            start_wait,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, InformersState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `Informers.Get` without blocking: returns (started, informer), creating the informer.
    async fn informer(&self, res: &ResourceInfo) -> Result<(bool, Arc<SharedInformer>)> {
        let key = gvk_key(res);
        {
            let st = self.lock();
            if let Some(i) = st.informers.get(&key) {
                return Ok((st.started, i.clone()));
            }
        }
        // makeListWatcher / RESTMapping: fails with NoKindMatchError for unknown kinds.
        let mapping = self
            .mapper
            .rest_mapping(&res.group, &res.kind, &res.version)
            .await?;
        let namespace = if mapping.namespaced {
            restrict_namespace_by_selector(&self.namespace, self.config.field_selector.as_deref())
        } else {
            String::new()
        };
        let opts = InformerOptions {
            namespace,
            label_selector: self.config.label_selector.clone(),
            field_selector: self.config.field_selector.clone(),
            resync: calculate_resync_period(self.config.sync_period.unwrap_or(DEFAULT_SYNC_PERIOD)),
        };
        let mut st = self.lock();
        if let Some(i) = st.informers.get(&key) {
            return Ok((st.started, i.clone()));
        }
        let inf = SharedInformer::new(self.client.clone(), mapping, opts);
        st.informers.insert(key, inf.clone());
        if st.started
            && !st.stopped
            && let Some(ctx) = &st.ctx
        {
            inf.run(ctx.clone());
        }
        Ok((st.started, inf))
    }

    /// `Informers.Get` with `BlockUntilSynced`.
    async fn synced_informer(&self, res: &ResourceInfo) -> Result<Arc<SharedInformer>> {
        let (started, inf) = self.informer(res).await?;
        if !started {
            return Err(ErrCacheNotStarted.into());
        }
        if !inf.has_synced() {
            let ctx = self.lock().ctx.clone().unwrap_or_default();
            if !inf.wait_for_sync(&ctx).await {
                return Err(ApiError::new(
                    504,
                    ctrlrt::error::REASON_TIMEOUT,
                    format!(
                        "Timeout: failed waiting for {} Informer to sync",
                        go_type_name(res)
                    ),
                )
                .into());
            }
        }
        Ok(inf)
    }

    async fn start(&self, ctx: CancellationToken) -> Result<()> {
        {
            let mut st = self.lock();
            if st.started {
                return Err(anyhow::anyhow!("Informer already started"));
            }
            st.ctx = Some(ctx.clone());
            for i in st.informers.values() {
                i.run(ctx.clone());
            }
            st.started = true;
        }
        self.start_wait.send_replace(true);
        ctx.cancelled().await;
        self.lock().stopped = true;
        Ok(())
    }

    async fn wait_for_cache_sync(&self, cancel: &CancellationToken) -> bool {
        let mut rx = self.start_wait.subscribe();
        loop {
            if *rx.borrow_and_update() {
                break;
            }
            tokio::select! {
                _ = cancel.cancelled() => return false,
                r = rx.changed() => if r.is_err() { return false },
            }
        }
        let infs: Vec<Arc<SharedInformer>> = self.lock().informers.values().cloned().collect();
        for i in infs {
            if !i.wait_for_sync(cancel).await {
                return false;
            }
        }
        true
    }

    /// `CacheReader.Get`
    async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value> {
        let inf = self.synced_informer(res).await?;
        let ns = if inf.mapping().namespaced {
            namespace
        } else {
            ""
        };
        let key = if ns.is_empty() {
            name.to_string()
        } else {
            format!("{ns}/{name}")
        };
        match inf.get_by_key(&key) {
            Some(v) => Ok((*v).clone()),
            None => Err(ApiError::not_found(&res.group, &res.kind, name).into()),
        }
    }

    /// `CacheReader.List`
    async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>> {
        let inf = self.synced_informer(res).await?;
        Ok(inf
            .list(&opts.namespace)
            .into_iter()
            .filter(|v| {
                let labels: Option<BTreeMap<String, String>> = v
                    .pointer("/metadata/labels")
                    .and_then(|l| serde_json::from_value(l.clone()).ok());
                opts.labels_match(labels.as_ref())
            })
            .map(|v| (*v).clone())
            .collect())
    }
}

// ---------------------------------------------------------------------------------------------
// Cache implementations
// ---------------------------------------------------------------------------------------------

#[async_trait::async_trait]
trait CacheImpl: Send + Sync {
    async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value>;
    async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>>;
    async fn get_informers(&self, res: &ResourceInfo) -> Result<Vec<Arc<SharedInformer>>>;
    async fn start(&self, ctx: CancellationToken) -> Result<()>;
    async fn wait_for_cache_sync(&self, cancel: &CancellationToken) -> bool;
}

#[async_trait::async_trait]
impl CacheImpl for InformerCache {
    async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value> {
        InformerCache::get(self, res, namespace, name).await
    }
    async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>> {
        InformerCache::list(self, res, opts).await
    }
    async fn get_informers(&self, res: &ResourceInfo) -> Result<Vec<Arc<SharedInformer>>> {
        Ok(vec![self.informer(res).await?.1])
    }
    async fn start(&self, ctx: CancellationToken) -> Result<()> {
        InformerCache::start(self, ctx).await
    }
    async fn wait_for_cache_sync(&self, cancel: &CancellationToken) -> bool {
        InformerCache::wait_for_cache_sync(self, cancel).await
    }
}

/// `multiNamespaceCache`
struct MultiNamespaceCache {
    mapper: Arc<RestMapper>,
    namespace_to_cache: BTreeMap<String, Arc<InformerCache>>,
    cluster_cache: Option<Arc<InformerCache>>,
}

impl MultiNamespaceCache {
    fn cluster(&self) -> Result<&Arc<InformerCache>> {
        self.cluster_cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no cache for cluster-scoped objects configured"))
    }
}

#[async_trait::async_trait]
impl CacheImpl for MultiNamespaceCache {
    async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value> {
        if !self.mapper.is_namespaced(res).await? {
            return self.cluster()?.get(res, namespace, name).await;
        }
        match self.namespace_to_cache.get(namespace) {
            Some(c) => c.get(res, namespace, name).await,
            None => match self.namespace_to_cache.get(ALL_NAMESPACES) {
                Some(global) => global.get(res, namespace, name).await,
                None => Err(anyhow::anyhow!(
                    "unable to get: {} because of unknown namespace for the cache",
                    ctrlrt::ObjectKey::new(namespace, name)
                )),
            },
        }
    }

    async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>> {
        if !self.mapper.is_namespaced(res).await? {
            return self.cluster()?.list(res, opts).await;
        }
        if !opts.namespace.is_empty() {
            return match self.namespace_to_cache.get(&opts.namespace) {
                Some(c) => c.list(res, opts).await,
                None => match self.namespace_to_cache.get(ALL_NAMESPACES) {
                    Some(global) => global.list(res, opts).await,
                    None => Err(anyhow::anyhow!(
                        "unable to list: {} because of unknown namespace for the cache",
                        opts.namespace
                    )),
                },
            };
        }
        let mut all = Vec::new();
        for c in self.namespace_to_cache.values() {
            all.extend(c.list(res, opts).await?);
        }
        Ok(all)
    }

    async fn get_informers(&self, res: &ResourceInfo) -> Result<Vec<Arc<SharedInformer>>> {
        if !self.mapper.is_namespaced(res).await? {
            return Ok(vec![self.cluster()?.informer(res).await?.1]);
        }
        let mut out = Vec::new();
        for c in self.namespace_to_cache.values() {
            out.push(c.informer(res).await?.1);
        }
        Ok(out)
    }

    async fn start(&self, ctx: CancellationToken) -> Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<anyhow::Error>();
        if let Some(c) = &self.cluster_cache {
            let c = c.clone();
            let ctx = ctx.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                if let Err(e) = c.start(ctx).await {
                    let msg = format!("failed to start cluster-scoped cache: {e}");
                    let _ = tx.send(ctrlrt::error::wrap(e, msg));
                }
            });
        }
        for (ns, c) in &self.namespace_to_cache {
            let c = c.clone();
            let ctx = ctx.clone();
            let tx = tx.clone();
            let ns = ns.clone();
            tokio::spawn(async move {
                if let Err(e) = c.start(ctx).await {
                    let msg = format!("failed to start cache for namespace {ns}: {e}");
                    let _ = tx.send(ctrlrt::error::wrap(e, msg));
                }
            });
        }
        tokio::select! {
            _ = ctx.cancelled() => Ok(()),
            Some(e) = rx.recv() => Err(e),
        }
    }

    async fn wait_for_cache_sync(&self, cancel: &CancellationToken) -> bool {
        let mut synced = true;
        for c in self.namespace_to_cache.values() {
            if !c.wait_for_cache_sync(cancel).await {
                synced = false;
            }
        }
        if let Some(c) = &self.cluster_cache
            && !c.wait_for_cache_sync(cancel).await
        {
            synced = false;
        }
        synced
    }
}

/// `delegatingByGVKCache`
struct DelegatingByGvkCache {
    caches: HashMap<GvkKey, Arc<dyn CacheImpl>>,
    default_cache: Arc<dyn CacheImpl>,
}

impl DelegatingByGvkCache {
    fn cache_for(&self, res: &ResourceInfo) -> &Arc<dyn CacheImpl> {
        self.caches
            .get(&gvk_key(res))
            .unwrap_or(&self.default_cache)
    }
}

#[async_trait::async_trait]
impl CacheImpl for DelegatingByGvkCache {
    async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value> {
        self.cache_for(res).get(res, namespace, name).await
    }
    async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>> {
        self.cache_for(res).list(res, opts).await
    }
    async fn get_informers(&self, res: &ResourceInfo) -> Result<Vec<Arc<SharedInformer>>> {
        self.cache_for(res).get_informers(res).await
    }
    async fn start(&self, ctx: CancellationToken) -> Result<()> {
        let all: Vec<Arc<dyn CacheImpl>> = self
            .caches
            .values()
            .cloned()
            .chain(std::iter::once(self.default_cache.clone()))
            .collect();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<anyhow::Error>();
        for c in all {
            let ctx = ctx.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                if let Err(e) = c.start(ctx).await {
                    let _ = tx.send(e);
                }
            });
        }
        tokio::select! {
            _ = ctx.cancelled() => Ok(()),
            Some(e) = rx.recv() => Err(e),
        }
    }
    async fn wait_for_cache_sync(&self, cancel: &CancellationToken) -> bool {
        let mut synced = true;
        for c in self
            .caches
            .values()
            .chain(std::iter::once(&self.default_cache))
        {
            if !c.wait_for_cache_sync(cancel).await {
                synced = false;
            }
        }
        synced
    }
}

// ---------------------------------------------------------------------------------------------
// Public Cache
// ---------------------------------------------------------------------------------------------

/// `cache.Cache`
#[derive(Clone)]
pub struct Cache {
    inner: Arc<dyn CacheImpl>,
}

impl Cache {
    /// `cache.New(config, opts)` (including `defaultOpts`, which needs the RESTMapper to decide
    /// whether `ByObject` types are namespaced).
    pub async fn new(
        client: kube::Client,
        mapper: Arc<RestMapper>,
        opts: CacheOptions,
    ) -> Result<Self> {
        let opts = default_opts(&mapper, opts).await?;
        let global = opts.defaults_config();
        let new_cache = |cfg: CacheConfig, ns: &str| {
            InformerCache::new(client.clone(), mapper.clone(), cfg, ns)
        };

        let default_cache: Arc<dyn CacheImpl> = if !opts.default_namespaces.is_empty() {
            Arc::new(MultiNamespaceCache {
                mapper: mapper.clone(),
                namespace_to_cache: opts
                    .default_namespaces
                    .iter()
                    .map(|(ns, cfg)| (ns.clone(), new_cache(cfg.clone(), ns)))
                    .collect(),
                cluster_cache: Some(new_cache(global.clone(), ALL_NAMESPACES)),
            })
        } else {
            new_cache(global.clone(), ALL_NAMESPACES)
        };
        if opts.by_object.is_empty() {
            return Ok(Self {
                inner: default_cache,
            });
        }
        let mut caches: HashMap<GvkKey, Arc<dyn CacheImpl>> = HashMap::new();
        for bo in &opts.by_object {
            let c: Arc<dyn CacheImpl> = match &bo.namespaces {
                Some(nss) if !nss.is_empty() => Arc::new(MultiNamespaceCache {
                    mapper: mapper.clone(),
                    namespace_to_cache: nss
                        .iter()
                        .map(|(ns, cfg)| (ns.clone(), new_cache(cfg.clone(), ns)))
                        .collect(),
                    cluster_cache: None,
                }),
                _ => new_cache(bo.as_config(), ALL_NAMESPACES),
            };
            caches.insert(gvk_key(&bo.resource), c);
        }
        Ok(Self {
            inner: Arc::new(DelegatingByGvkCache {
                caches,
                default_cache,
            }),
        })
    }

    /// `Get` from the cache (deep copy with apiVersion/kind set).
    pub async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value> {
        self.inner.get(res, namespace, name).await
    }

    /// `List` from the cache.
    pub async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>> {
        self.inner.list(res, opts).await
    }

    /// `GetInformer`: the informer(s) backing a type (one per namespace for multi-namespace
    /// caches). Creates (and, if the cache runs, starts) them.
    pub async fn get_informers(&self, res: &ResourceInfo) -> Result<Vec<Arc<SharedInformer>>> {
        self.inner.get_informers(res).await
    }

    /// `Start(ctx)`: blocks until `ctx` is cancelled.
    pub async fn start(&self, ctx: CancellationToken) -> Result<()> {
        self.inner.start(ctx).await
    }

    /// `WaitForCacheSync(ctx)`
    pub async fn wait_for_cache_sync(&self, cancel: &CancellationToken) -> bool {
        self.inner.wait_for_cache_sync(cancel).await
    }
}

async fn default_opts(mapper: &RestMapper, mut opts: CacheOptions) -> Result<CacheOptions> {
    let global = opts.defaults_config();
    let default_namespaces = opts.default_namespaces.clone();
    let mut by_object = std::mem::take(&mut opts.by_object);
    for bo in by_object.iter_mut() {
        let namespaced = match mapper.is_namespaced(&bo.resource).await {
            Ok(n) => n,
            Err(e) => {
                let msg = format!(
                    "failed to determine if {} is namespaced: {}",
                    go_type_name(&bo.resource),
                    e
                );
                return Err(ctrlrt::error::wrap(e, msg));
            }
        };
        if !namespaced && bo.namespaces.is_some() {
            return Err(anyhow::anyhow!(
                "type {} is not namespaced, but its ByObject.Namespaces setting is not nil",
                go_type_name(&bo.resource)
            ));
        }
        if namespaced && bo.namespaces.is_none() {
            bo.namespaces = Some(default_namespaces.clone());
        }
        let bo_cfg = bo.as_config();
        if let Some(nss) = bo.namespaces.as_mut() {
            let keys: Vec<String> = nss.keys().cloned().collect();
            for (ns, cfg) in nss.iter_mut() {
                let mut c = default_config(cfg.clone(), &bo_cfg);
                if let Some(d) = default_namespaces.get(ns) {
                    c = default_config(c, d);
                }
                c = default_config(c, &global);
                if ns.is_empty() {
                    c.field_selector =
                        namespace_all_field_selector(&keys, c.field_selector.as_deref());
                }
                *cfg = c;
            }
        }
        if !namespaced || bo.namespaces.as_ref().is_none_or(|n| n.is_empty()) {
            let d = default_config(bo_cfg, &global);
            bo.label_selector = d.label_selector;
            bo.field_selector = d.field_selector;
            bo.sync_period = d.sync_period;
        }
    }
    opts.by_object = by_object;
    let keys: Vec<String> = opts.default_namespaces.keys().cloned().collect();
    for (ns, cfg) in opts.default_namespaces.iter_mut() {
        let mut c = default_config(cfg.clone(), &global);
        if ns.is_empty() {
            c.field_selector = namespace_all_field_selector(&keys, c.field_selector.as_deref());
        }
        *cfg = c;
    }
    Ok(opts)
}

// ---------------------------------------------------------------------------------------------
// Cache-backed client
// ---------------------------------------------------------------------------------------------

/// `client.New(config, client.Options{Cache: &client.CacheOptions{Reader: cache}})`: reads come
/// from the informer cache, writes go directly to the API server.
#[derive(Clone)]
pub struct CachedBackend {
    cache: Cache,
    direct: KubeBackend,
}

impl CachedBackend {
    pub fn new(cache: Cache, direct: KubeBackend) -> Self {
        Self { cache, direct }
    }
}

#[async_trait::async_trait]
impl ClientBackend for CachedBackend {
    async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value> {
        self.cache.get(res, namespace, name).await
    }
    async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>> {
        self.cache.list(res, opts).await
    }
    async fn create(&self, res: &ResourceInfo, obj: Value) -> Result<Value> {
        self.direct.create(res, obj).await
    }
    async fn update(&self, res: &ResourceInfo, obj: Value) -> Result<Value> {
        self.direct.update(res, obj).await
    }
    async fn update_subresource(&self, res: &ResourceInfo, sub: &str, obj: Value) -> Result<Value> {
        self.direct.update_subresource(res, sub, obj).await
    }
    async fn patch(
        &self,
        res: &ResourceInfo,
        ns: &str,
        name: &str,
        patch: &Patch,
    ) -> Result<Value> {
        self.direct.patch(res, ns, name, patch).await
    }
    async fn patch_subresource(
        &self,
        res: &ResourceInfo,
        sub: &str,
        ns: &str,
        name: &str,
        patch: &Patch,
    ) -> Result<Value> {
        self.direct
            .patch_subresource(res, sub, ns, name, patch)
            .await
    }
    async fn delete(
        &self,
        res: &ResourceInfo,
        ns: &str,
        name: &str,
        opts: &DeleteOptions,
    ) -> Result<()> {
        self.direct.delete(res, ns, name, opts).await
    }
}

/// Key of a cached object (`namespace/name` or `name`).
pub fn cache_key(v: &Value) -> String {
    object_key(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_all_selector_excludes_other_namespaces() {
        assert_eq!(
            namespace_all_field_selector(
                &["".to_string(), "op-ns".to_string(), "a".to_string()],
                None
            ),
            Some("metadata.namespace!=a,metadata.namespace!=op-ns".to_string())
        );
        assert_eq!(
            namespace_all_field_selector(&["".to_string()], Some("x=y")),
            Some("x=y".to_string())
        );
        assert_eq!(
            restrict_namespace_by_selector("", Some("metadata.namespace!=a")),
            ""
        );
        assert_eq!(
            restrict_namespace_by_selector("", Some("metadata.namespace=a")),
            "a"
        );
        assert_eq!(restrict_namespace_by_selector("b", None), "b");
    }

    #[test]
    fn config_defaulting() {
        let a = CacheConfig {
            label_selector: None,
            field_selector: Some("f".into()),
            sync_period: None,
        };
        let b = CacheConfig {
            label_selector: Some("l".into()),
            field_selector: Some("g".into()),
            sync_period: Some(Duration::from_secs(1)),
        };
        let c = default_config(a, &b);
        assert_eq!(c.label_selector.as_deref(), Some("l"));
        assert_eq!(c.field_selector.as_deref(), Some("f"));
        assert_eq!(c.sync_period, Some(Duration::from_secs(1)));
    }
}
