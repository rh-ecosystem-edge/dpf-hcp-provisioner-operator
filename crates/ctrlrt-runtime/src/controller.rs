//! Controller execution: port of controller-runtime's `internal/controller/controller.go`
//! (event sources, work queue, workers, `reconcileHandler` result handling, panic recovery,
//! metrics and logging) and `internal/source/kind.go`.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use ctrlrt::reconcile::{ControllerSpec, RawHandler, TerminalError, WatchSpec};
use ctrlrt::{Ctx, Logger, ReconcileResult, Reconciler, Request, kv};
use futures::FutureExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::Result;
use crate::cache::Cache;
use crate::manager::Runnable;
use crate::metrics;
use crate::ratelimit::{RateLimiter, default_controller_rate_limiter};
use crate::util::{go_duration, go_type_name, new_uuid};
use crate::workqueue::RateLimitingQueue;

const LABEL_ERROR: &str = "error";
const LABEL_REQUEUE_AFTER: &str = "requeue_after";
const LABEL_REQUEUE: &str = "requeue";
const LABEL_SUCCESS: &str = "success";

/// Default `CacheSyncTimeout` (2 minutes).
pub const DEFAULT_CACHE_SYNC_TIMEOUT: Duration = Duration::from_secs(120);

static USED_NAMES: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// `controller.checkName`: controller names must be unique within the process.
pub fn check_name(name: &str) -> Result<()> {
    let mut used = USED_NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if used.contains(name) {
        return Err(anyhow::anyhow!(
            "controller with name {name} already exists. Controller names must be unique to avoid multiple controllers reporting the same metric. This validation can be disabled via the SkipNameValidation option"
        ));
    }
    used.insert(name.to_string());
    Ok(())
}

/// Options of a controller (`controller.Options`, defaults from the manager's `config.Controller`).
#[derive(Clone, Debug)]
pub struct ControllerOptions {
    /// `CacheSyncTimeout` (default 2 minutes).
    pub cache_sync_timeout: Duration,
    /// `RecoverPanic` (default true).
    pub recover_panic: bool,
    /// `NeedLeaderElection` (default true).
    pub need_leader_election: bool,
    /// `ReconciliationTimeout` (default: none).
    pub reconciliation_timeout: Option<Duration>,
    /// `SkipNameValidation`.
    pub skip_name_validation: bool,
}

impl Default for ControllerOptions {
    fn default() -> Self {
        Self {
            cache_sync_timeout: DEFAULT_CACHE_SYNC_TIMEOUT,
            recover_panic: true,
            need_leader_election: true,
            reconciliation_timeout: None,
            skip_name_validation: false,
        }
    }
}

/// Builds the per-request logger exactly like the controller-runtime builder's
/// `LogConstructor`: `controller`, `controllerGroup`, `controllerKind`, then for a request
/// `<Kind>` ({name, namespace}), `namespace`, `name`.
#[derive(Clone)]
pub struct LogConstructor {
    base: Logger,
    /// Kind of the `For()` type; `None` when the controller has no GVK.
    kind: Option<String>,
}

impl LogConstructor {
    /// `gvk` is `Some((group, kind))` when the builder had a `For()` type.
    pub fn new(root: &Logger, spec_name: &str, gvk: Option<(&str, &str)>) -> Self {
        let mut base = root.with_values(kv!["controller" => spec_name]);
        if let Some((group, kind)) = gvk {
            base = base.with_values(kv!["controllerGroup" => group, "controllerKind" => kind]);
        }
        Self {
            base,
            kind: gvk.map(|(_, k)| k.to_string()),
        }
    }

    /// `LogConstructor(nil)`
    pub fn base(&self) -> Logger {
        self.base.clone()
    }

    /// `LogConstructor(&req)`
    pub fn for_request(&self, req: &Request) -> Logger {
        let mut kvs = Vec::with_capacity(3);
        if let Some(kind) = &self.kind {
            kvs.push((kind.clone(), kref(&req.namespace, &req.name)));
        }
        kvs.push(("namespace".into(), Value::String(req.namespace.clone())));
        kvs.push(("name".into(), Value::String(req.name.clone())));
        self.base.with_values(kvs)
    }
}

/// `klog.KRef(ns, name)` as marshalled by zapr (`logr.Marshaler`).
pub fn kref(namespace: &str, name: &str) -> Value {
    if namespace.is_empty() {
        json!({ "name": name })
    } else {
        json!({ "name": name, "namespace": namespace })
    }
}

/// A controller ready to be run by the manager.
pub struct Controller {
    name: String,
    max_concurrent_reconciles: usize,
    reconciler: Arc<dyn Reconciler>,
    watches: Vec<WatchSpec>,
    cache: Cache,
    opts: ControllerOptions,
    log: LogConstructor,
    started: Mutex<bool>,
    queue: Mutex<Option<Arc<RateLimitingQueue<Request>>>>,
    rate_limiter: Mutex<Option<Box<dyn RateLimiter<Request>>>>,
}

impl Controller {
    /// `controller.NewTyped(name, mgr, options)` from a builder spec.
    pub fn new(
        spec: ControllerSpec,
        cache: Cache,
        root_log: &Logger,
        opts: ControllerOptions,
    ) -> Result<Arc<Self>> {
        if spec.name.is_empty() {
            return Err(anyhow::anyhow!("must specify Name for Controller"));
        }
        if !opts.skip_name_validation {
            check_name(&spec.name)?;
        }
        let log = LogConstructor::new(
            root_log,
            &spec.name,
            spec.for_resource
                .as_ref()
                .map(|f| (f.group.as_str(), f.kind.as_str())),
        );
        Ok(Arc::new(Self {
            name: spec.name,
            max_concurrent_reconciles: spec.max_concurrent_reconciles.max(1),
            reconciler: spec.reconciler,
            watches: spec.watches,
            cache,
            opts,
            log,
            started: Mutex::new(false),
            queue: Mutex::new(None),
            rate_limiter: Mutex::new(None),
        }))
    }

    /// Override the rate limiter (tests).
    pub fn set_rate_limiter(&self, rl: Box<dyn RateLimiter<Request>>) {
        *self.rate_limiter.lock().unwrap_or_else(|e| e.into_inner()) = Some(rl);
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The work queue (available once started).
    pub fn queue(&self) -> Option<Arc<RateLimitingQueue<Request>>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn init_metrics(&self) {
        for l in [
            LABEL_ERROR,
            LABEL_REQUEUE_AFTER,
            LABEL_REQUEUE,
            LABEL_SUCCESS,
        ] {
            metrics::RECONCILE_TOTAL
                .with_label_values(&[&self.name, l])
                .inc_by(0.0);
        }
        metrics::RECONCILE_ERRORS
            .with_label_values(&[&self.name])
            .inc_by(0.0);
        metrics::TERMINAL_RECONCILE_ERRORS
            .with_label_values(&[&self.name])
            .inc_by(0.0);
        metrics::RECONCILE_PANICS
            .with_label_values(&[&self.name])
            .inc_by(0.0);
        metrics::WORKER_COUNT
            .with_label_values(&[&self.name])
            .set(self.max_concurrent_reconciles as f64);
        metrics::ACTIVE_WORKERS
            .with_label_values(&[&self.name])
            .set(0.0);
    }

    /// `Start(ctx)`
    pub async fn run(self: Arc<Self>, ctx: CancellationToken) -> Result<()> {
        {
            let mut started = self.started.lock().unwrap_or_else(|e| e.into_inner());
            if *started {
                return Err(anyhow::anyhow!(
                    "controller was started more than once. This is likely to be caused by being added to a manager multiple times"
                ));
            }
            *started = true;
        }
        self.init_metrics();

        // startEventSourcesAndQueueLocked
        let rl = self
            .rate_limiter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .unwrap_or_else(|| Box::new(default_controller_rate_limiter()));
        let queue = RateLimitingQueue::new(&self.name, rl);
        *self.queue.lock().unwrap_or_else(|e| e.into_inner()) = Some(queue.clone());
        {
            let q = queue.clone();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                ctx.cancelled().await;
                q.shut_down();
            });
        }

        let mut set = tokio::task::JoinSet::new();
        for w in &self.watches {
            let source = KindSource {
                cache: self.cache.clone(),
                watch: w.clone(),
            };
            let log = self
                .log
                .base()
                .with_values(kv!["source" => source.to_string()]);
            let queue = queue.clone();
            let ctx = ctx.clone();
            let timeout = self.opts.cache_sync_timeout;
            let name = self.name.clone();
            set.spawn(async move { start_source(source, log, queue, ctx, timeout, &name).await });
        }
        while let Some(r) = set.join_next().await {
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(anyhow::anyhow!("{e}")),
            }
        }

        let log = self.log.base();
        log.info("Starting Controller", kv![]);
        log.info(
            "Starting workers",
            kv!["worker count" => self.max_concurrent_reconciles],
        );
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..self.max_concurrent_reconciles {
            let this = self.clone();
            let queue = queue.clone();
            let ctx = ctx.clone();
            workers.spawn(async move { while this.process_next_work_item(&queue, &ctx).await {} });
        }

        ctx.cancelled().await;
        log.info(
            "Shutdown signal received, waiting for all workers to finish",
            kv![],
        );
        while workers.join_next().await.is_some() {}
        log.info("All workers finished", kv![]);
        Ok(())
    }

    async fn process_next_work_item(
        &self,
        queue: &Arc<RateLimitingQueue<Request>>,
        ctx: &CancellationToken,
    ) -> bool {
        let Some(req) = queue.get().await else {
            return false;
        };
        metrics::ACTIVE_WORKERS
            .with_label_values(&[&self.name])
            .add(1.0);
        self.reconcile_handler(queue, ctx, req.clone()).await;
        metrics::ACTIVE_WORKERS
            .with_label_values(&[&self.name])
            .sub(1.0);
        queue.done(&req);
        true
    }

    /// `Reconcile(ctx, req)` with panic recovery.
    async fn reconcile(&self, ctx: &Ctx, req: Request) -> Result<ReconcileResult> {
        let fut = AssertUnwindSafe(async {
            match self.opts.reconciliation_timeout {
                Some(t) if !t.is_zero() => {
                    let child = ctx.child();
                    let c2 = child.cancel.clone();
                    let timer = tokio::spawn(async move {
                        tokio::time::sleep(t).await;
                        c2.cancel();
                    });
                    let r = self.reconciler.reconcile(&child, req).await;
                    timer.abort();
                    r
                }
                _ => self.reconciler.reconcile(ctx, req).await,
            }
        });
        match fut.catch_unwind().await {
            Ok(r) => r,
            Err(payload) => {
                metrics::RECONCILE_PANICS
                    .with_label_values(&[&self.name])
                    .inc();
                let msg = panic_message(payload.as_ref());
                if self.opts.recover_panic {
                    // utilruntime.PanicHandlers: logPanic
                    let stack = std::backtrace::Backtrace::force_capture().to_string();
                    ctx.log.error_nil(
                        "Observed a panic",
                        kv!["panic" => msg, "stacktrace" => stack],
                    );
                    Err(anyhow::anyhow!("panic: {msg} [recovered]"))
                } else {
                    ctx.log
                        .info(&format!("Observed a panic in reconciler: {msg}"), kv![]);
                    std::panic::resume_unwind(payload)
                }
            }
        }
    }

    async fn reconcile_handler(
        &self,
        queue: &Arc<RateLimitingQueue<Request>>,
        ctx: &CancellationToken,
        req: Request,
    ) {
        let start = Instant::now();
        let reconcile_id = new_uuid();
        let log = self
            .log
            .for_request(&req)
            .with_values(kv!["reconcileID" => reconcile_id]);
        let rctx = Ctx {
            log: log.clone(),
            cancel: ctx.clone(),
        };
        log.v(5).info("Reconciling", kv![]);
        let res = self.reconcile(&rctx, req.clone()).await;
        match res {
            Err(err) => {
                if ctrlrt::error::find_in_chain::<TerminalError>(&err).is_some() {
                    metrics::TERMINAL_RECONCILE_ERRORS
                        .with_label_values(&[&self.name])
                        .inc();
                } else {
                    queue.add_rate_limited(req);
                }
                metrics::RECONCILE_ERRORS
                    .with_label_values(&[&self.name])
                    .inc();
                metrics::RECONCILE_TOTAL
                    .with_label_values(&[&self.name, LABEL_ERROR])
                    .inc();
                if !ctrlrt::reconcile::result_of_error(&err).is_zero() {
                    log.info("Warning: Reconciler returned both a non-zero result and a non-nil error. The result will always be ignored if the error is non-nil and the non-nil error causes requeuing with exponential backoff. For more details, see: https://pkg.go.dev/sigs.k8s.io/controller-runtime/pkg/reconcile#Reconciler", kv![]);
                }
                log.error(&err, "Reconciler error", kv![]);
            }
            Ok(result) if !result.requeue_after.is_zero() => {
                log.v(5).info(
                    &format!(
                        "Reconcile done, requeueing after {}",
                        go_duration(result.requeue_after)
                    ),
                    kv![],
                );
                queue.forget(&req);
                queue.add_after(req, result.requeue_after);
                metrics::RECONCILE_TOTAL
                    .with_label_values(&[&self.name, LABEL_REQUEUE_AFTER])
                    .inc();
            }
            Ok(result) if result.requeue => {
                log.v(5).info("Reconcile done, requeueing", kv![]);
                queue.add_rate_limited(req);
                metrics::RECONCILE_TOTAL
                    .with_label_values(&[&self.name, LABEL_REQUEUE])
                    .inc();
            }
            Ok(_) => {
                log.v(5).info("Reconcile successful", kv![]);
                queue.forget(&req);
                metrics::RECONCILE_TOTAL
                    .with_label_values(&[&self.name, LABEL_SUCCESS])
                    .inc();
            }
        }
        metrics::RECONCILE_TIME
            .with_label_values(&[&self.name])
            .observe(start.elapsed().as_secs_f64());
    }

    /// Test hook: run `reconcileHandler` for one request.
    #[doc(hidden)]
    pub async fn reconcile_handler_for_test(
        &self,
        queue: &Arc<RateLimitingQueue<Request>>,
        ctx: &CancellationToken,
        req: Request,
    ) {
        self.reconcile_handler(queue, ctx, req).await
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "Box<dyn Any>".to_string()
    }
}

/// Wrapper making an `Arc<Controller>` a [`Runnable`].
pub struct ControllerRunnable(pub Arc<Controller>);

#[async_trait::async_trait]
impl Runnable for ControllerRunnable {
    async fn start(&self, ctx: Ctx) -> Result<()> {
        self.0.clone().run(ctx.cancel.clone()).await
    }
    fn need_leader_election(&self) -> Option<bool> {
        Some(self.0.opts.need_leader_election)
    }
}

// ---------------------------------------------------------------------------------------------
// source.Kind
// ---------------------------------------------------------------------------------------------

struct KindSource {
    cache: Cache,
    watch: WatchSpec,
}

impl std::fmt::Display for KindSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "kind source: {}", go_type_name(&self.watch.resource))
    }
}

fn log_kind() -> Logger {
    Logger::root()
        .with_name("controller-runtime")
        .with_name("source")
        .with_name("Kind")
}

/// `Kind.Start` + `WaitForSync` as orchestrated by `startEventSourcesAndQueueLocked`.
async fn start_source(
    source: KindSource,
    log: Logger,
    queue: Arc<RateLimitingQueue<Request>>,
    ctx: CancellationToken,
    cache_sync_timeout: Duration,
    controller_name: &str,
) -> Result<()> {
    log.info("Starting EventSource", kv![]);
    let start_cancel = ctx.child_token();
    let desc = source.to_string();
    let type_name = go_type_name(&source.watch.resource);
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<()>>();
    {
        let start_cancel = start_cancel.clone();
        let cache = source.cache.clone();
        let watch = source.watch.clone();
        tokio::spawn(async move {
            let r = kind_start(cache, watch, queue, start_cancel).await;
            let _ = tx.send(r);
        });
    }
    let res = tokio::select! {
        r = rx => r.unwrap_or(Ok(())),
        _ = tokio::time::sleep(cache_sync_timeout) => {
            start_cancel.cancel();
            if ctx.is_cancelled() {
                Ok(())
            } else {
                Err(anyhow::anyhow!("timed out waiting for cache to be synced for Kind {type_name}"))
            }
        }
        _ = ctx.cancelled() => Ok(()),
    };
    match res {
        Ok(()) => Ok(()),
        Err(e) => {
            let msg = format!("failed to wait for {controller_name} caches to sync {desc}: {e}");
            let err = ctrlrt::error::wrap(e, msg);
            log.error(&err, "Could not wait for Cache to sync", kv![]);
            Err(err)
        }
    }
}

async fn kind_start(
    cache: Cache,
    watch: WatchSpec,
    queue: Arc<RateLimitingQueue<Request>>,
    ctx: CancellationToken,
) -> Result<()> {
    // wait.PollUntilContextCancel(ctx, 10s, immediate=true, GetInformer)
    let informers = loop {
        match cache.get_informers(&watch.resource).await {
            Ok(i) => break i,
            Err(e) => {
                if let Some(nk) = ctrlrt::error::find_in_chain::<ctrlrt::NoKindMatchError>(&e) {
                    log_kind().error(
                        &e,
                        "if kind is a CRD, it should be installed before calling Start",
                        kv!["kind" => json!({"Group": nk.group, "Kind": nk.kind})],
                    );
                } else {
                    log_kind().error(&e, "failed to get informer from cache", kv![]);
                }
                tokio::select! {
                    _ = ctx.cancelled() => {
                        let msg = format!("failed to get informer from cache: {e}");
                        return Err(ctrlrt::error::wrap(e, msg));
                    }
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                }
            }
        }
    };
    for inf in informers {
        let rx = inf.add_event_handler();
        spawn_event_handler(rx, watch.handler.clone(), queue.clone(), ctx.clone());
    }
    if !cache.wait_for_cache_sync(&ctx).await {
        return Err(anyhow::anyhow!("cache did not sync"));
    }
    Ok(())
}

/// The informer listener: invokes the type-erased handler (predicates + mapping) for every
/// event, sequentially, and adds the resulting requests to the queue.
fn spawn_event_handler(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<ctrlrt::reconcile::RawEvent>,
    handler: RawHandler,
    queue: Arc<RateLimitingQueue<Request>>,
    ctx: CancellationToken,
) {
    tokio::spawn(async move {
        let hctx = Ctx {
            log: Logger::root(),
            cancel: ctx.clone(),
        };
        loop {
            let ev = tokio::select! {
                _ = queue_closed(&queue) => return,
                ev = rx.recv() => match ev { Some(ev) => ev, None => return },
            };
            for req in handler(hctx.clone(), ev).await {
                queue.add(req);
            }
        }
    });
}

async fn queue_closed(queue: &Arc<RateLimitingQueue<Request>>) {
    loop {
        if queue.shutting_down() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctrlrt::ObjectKey;
    use ctrlrt::log::CaptureSink;

    struct FnReconciler<F>(F);

    #[async_trait::async_trait]
    impl<F> Reconciler for FnReconciler<F>
    where
        F: Fn(&Ctx, Request) -> Result<ReconcileResult> + Send + Sync + 'static,
    {
        async fn reconcile(&self, ctx: &Ctx, req: Request) -> Result<ReconcileResult> {
            (self.0)(ctx, req)
        }
    }

    fn dummy_cache_client() -> kube::Client {
        kube::Client::new(
            tower::service_fn(|_req: http::Request<kube::client::Body>| async {
                Err::<http::Response<kube::client::Body>, std::io::Error>(std::io::Error::other(
                    "offline",
                ))
            }),
            "default",
        )
    }

    async fn test_controller<F>(name: &str, f: F, sink: Arc<CaptureSink>) -> Arc<Controller>
    where
        F: Fn(&Ctx, Request) -> Result<ReconcileResult> + Send + Sync + 'static,
    {
        let client = dummy_cache_client();
        let mapper = Arc::new(crate::client::RestMapper::new(
            crate::client::RestClient::new(client.clone()),
        ));
        let cache = Cache::new(client, mapper, Default::default())
            .await
            .unwrap();
        let spec = ctrlrt::Builder::new()
            .for_type::<k8s_openapi::api::core::v1::ConfigMap>(vec![])
            .named(name)
            .build(Arc::new(FnReconciler(f)))
            .unwrap();
        let root = Logger::with_sink(sink);
        Controller::new(
            spec,
            cache,
            &root,
            ControllerOptions {
                skip_name_validation: true,
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn queue_with(rl: Box<dyn RateLimiter<Request>>) -> Arc<RateLimitingQueue<Request>> {
        RateLimitingQueue::new("", rl)
    }

    fn req() -> Request {
        ObjectKey::new("foo", "bar")
    }

    fn counter(name: &str, label: &str) -> f64 {
        metrics::RECONCILE_TOTAL
            .with_label_values(&[name, label])
            .get()
    }

    // controller_test.go: "should requeue a Request if there is an error and continue processing items"
    #[tokio::test(start_paused = true)]
    async fn reconcile_error_requeues_rate_limited() {
        let sink = Arc::new(CaptureSink::default());
        let c = test_controller(
            "ctrl-err",
            |_ctx, _req| Err(anyhow::anyhow!("expected error: reconcile")),
            sink.clone(),
        )
        .await;
        let q = queue_with(Box::new(
            crate::ratelimit::ItemExponentialFailureRateLimiter::new(
                Duration::from_millis(5),
                Duration::from_secs(1000),
            ),
        ));
        let ctx = CancellationToken::new();
        c.reconcile_handler_for_test(&q, &ctx, req()).await;
        assert_eq!(q.num_requeues(&req()), 1);
        assert_eq!(q.len(), 0, "rate limited add is delayed");
        tokio::time::sleep(Duration::from_millis(6)).await;
        assert_eq!(q.len(), 1);
        assert_eq!(counter("ctrl-err", "error"), 1.0);
        assert_eq!(
            metrics::RECONCILE_ERRORS
                .with_label_values(&["ctrl-err"])
                .get(),
            1.0
        );
        let recs = sink.records.lock().unwrap();
        let (lvl, msg, kvs) = recs.last().unwrap();
        assert_eq!(lvl, "error");
        assert_eq!(msg, ": Reconciler error");
        let keys: Vec<&str> = kvs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "controller",
                "controllerGroup",
                "controllerKind",
                "ConfigMap",
                "namespace",
                "name",
                "reconcileID",
                "error"
            ]
        );
        assert_eq!(kvs[3].1, json!({"name": "bar", "namespace": "foo"}));
        assert_eq!(kvs[7].1, json!("expected error: reconcile"));
    }

    // "should not requeue if the error is a terminal error"
    #[tokio::test(start_paused = true)]
    async fn terminal_error_is_not_requeued() {
        let sink = Arc::new(CaptureSink::default());
        let c = test_controller(
            "ctrl-terminal",
            |_ctx, _req| Err(anyhow::Error::new(TerminalError(anyhow::anyhow!("boom")))),
            sink,
        )
        .await;
        let q = queue_with(Box::new(default_controller_rate_limiter()));
        let ctx = CancellationToken::new();
        c.reconcile_handler_for_test(&q, &ctx, req()).await;
        assert_eq!(q.num_requeues(&req()), 0);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(q.len(), 0);
        assert_eq!(
            metrics::TERMINAL_RECONCILE_ERRORS
                .with_label_values(&["ctrl-terminal"])
                .get(),
            1.0
        );
        assert_eq!(counter("ctrl-terminal", "error"), 1.0);
    }

    // "should requeue a Request after a duration (but not rate-limited) if the Result sets RequeueAfter"
    #[tokio::test(start_paused = true)]
    async fn requeue_after_forgets_and_delays() {
        let sink = Arc::new(CaptureSink::default());
        let c = test_controller(
            "ctrl-after",
            |_ctx, _req| Ok(ReconcileResult::requeue_after(Duration::from_millis(100))),
            sink,
        )
        .await;
        let q = queue_with(Box::new(default_controller_rate_limiter()));
        // pre-existing failures are forgotten
        q.add_rate_limited(req());
        tokio::time::sleep(Duration::from_millis(10)).await;
        let item = q.get().await.unwrap();
        assert_eq!(q.num_requeues(&req()), 1);
        let ctx = CancellationToken::new();
        c.reconcile_handler_for_test(&q, &ctx, item.clone()).await;
        q.done(&item);
        assert_eq!(q.num_requeues(&req()), 0);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(q.len(), 0);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(q.len(), 1);
        assert_eq!(counter("ctrl-after", "requeue_after"), 1.0);
    }

    // "should requeue a Request if the Result sets Requeue:true and continue processing items"
    #[tokio::test(start_paused = true)]
    async fn requeue_true_is_rate_limited() {
        let sink = Arc::new(CaptureSink::default());
        let c = test_controller(
            "ctrl-requeue",
            |_ctx, _req| Ok(ReconcileResult::requeue()),
            sink,
        )
        .await;
        let q = queue_with(Box::new(default_controller_rate_limiter()));
        let ctx = CancellationToken::new();
        c.reconcile_handler_for_test(&q, &ctx, req()).await;
        assert_eq!(q.num_requeues(&req()), 1);
        c.reconcile_handler_for_test(&q, &ctx, req()).await;
        assert_eq!(q.num_requeues(&req()), 2);
        assert_eq!(counter("ctrl-requeue", "requeue"), 2.0);
    }

    // "should forget an item if it is successfully reconciled"
    #[tokio::test(start_paused = true)]
    async fn success_forgets() {
        let sink = Arc::new(CaptureSink::default());
        let c = test_controller("ctrl-ok", |_ctx, _req| Ok(ReconcileResult::done()), sink).await;
        let q = queue_with(Box::new(default_controller_rate_limiter()));
        q.add_rate_limited(req());
        assert_eq!(q.num_requeues(&req()), 1);
        let ctx = CancellationToken::new();
        c.reconcile_handler_for_test(&q, &ctx, req()).await;
        assert_eq!(q.num_requeues(&req()), 0);
        assert_eq!(counter("ctrl-ok", "success"), 1.0);
    }

    // "should recover when reconciler panics"
    #[tokio::test(start_paused = true)]
    async fn panic_is_recovered_as_error() {
        let sink = Arc::new(CaptureSink::default());
        let c = test_controller(
            "ctrl-panic",
            |_ctx, _req| -> Result<ReconcileResult> { panic!("boom") },
            sink.clone(),
        )
        .await;
        let q = queue_with(Box::new(default_controller_rate_limiter()));
        let ctx = CancellationToken::new();
        c.reconcile_handler_for_test(&q, &ctx, req()).await;
        assert_eq!(q.num_requeues(&req()), 1);
        assert_eq!(
            metrics::RECONCILE_PANICS
                .with_label_values(&["ctrl-panic"])
                .get(),
            1.0
        );
        let recs = sink.records.lock().unwrap();
        let err = recs
            .iter()
            .find(|r| r.1.ends_with("Reconciler error"))
            .unwrap();
        assert_eq!(err.2.last().unwrap().1, json!("panic: boom [recovered]"));
        assert!(recs.iter().any(|r| r.1.ends_with("Observed a panic")));
    }

    #[tokio::test]
    async fn duplicate_names_are_rejected() {
        check_name("unique-ctrl-name-test").unwrap();
        let e = check_name("unique-ctrl-name-test").unwrap_err();
        assert!(
            e.to_string()
                .starts_with("controller with name unique-ctrl-name-test already exists.")
        );
    }
}

#[cfg(test)]
mod log_constructor_tests {
    use super::*;
    use ctrlrt::log::CaptureSink;

    #[test]
    fn omits_gvk_keys_without_for() {
        let sink = Arc::new(CaptureSink::default());
        let root = Logger::with_sink(sink.clone());
        LogConstructor::new(&root, "dpuservicetemplate", None)
            .for_request(&Request::new("", "ns1"))
            .info("x", kv![]);
        LogConstructor::new(
            &root,
            "dpfhcpprovisioner",
            Some(("provisioning.dpu.hcp.io", "DPFHCPProvisioner")),
        )
        .for_request(&Request::new("ns", "n"))
        .info("y", kv![]);
        let recs = sink.records.lock().unwrap();
        let keys = |i: usize| recs[i].2.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        assert_eq!(keys(0), vec!["controller", "namespace", "name"]);
        assert_eq!(
            keys(1),
            vec![
                "controller",
                "controllerGroup",
                "controllerKind",
                "DPFHCPProvisioner",
                "namespace",
                "name"
            ]
        );
    }
}
