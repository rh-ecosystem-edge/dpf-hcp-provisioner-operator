//! The controller manager: port of controller-runtime's `pkg/manager` (`internal.go`,
//! `runnable_group.go`, `server.go`) wiring the cache, clients, controllers, leader election,
//! health probes and graceful shutdown.

use std::net::{SocketAddr, TcpListener, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctrlrt::reconcile::ControllerSpec;
use ctrlrt::{Client, Ctx, Logger, ManagerHandle, kv};
use futures::future::BoxFuture;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::Result;
use crate::cache::{Cache, CacheOptions, CachedBackend};
use crate::client::{KubeBackend, RestClient, RestMapper};
use crate::config::RestConfig;
use crate::controller::{Controller, ControllerOptions, ControllerRunnable};
use crate::healthz::{Checker, Handler, ProbeMux};
use crate::leaderelection::{LeaderElectionConfig, LeaderElector, LeaseLock, ResourceLockOptions};
use crate::metrics;
use crate::signals::LoggingRecorder;
use crate::util::go_duration;

/// Creates event recorders (`mgr.GetEventRecorderFor(name)`).
pub type RecorderFactory = Arc<dyn Fn(&str) -> ctrlrt::Recorder + Send + Sync>;

/// `manager.Runnable` (+ `LeaderElectionRunnable`).
#[async_trait::async_trait]
pub trait Runnable: Send + Sync + 'static {
    /// Run until `ctx.cancel` is cancelled. Returning an error stops the manager.
    async fn start(&self, ctx: Ctx) -> Result<()>;
    /// `NeedLeaderElection()`; `None` when the runnable does not implement
    /// `LeaderElectionRunnable` (it is then treated as needing leader election).
    fn need_leader_election(&self) -> Option<bool> {
        None
    }
}

type RunFn = Arc<dyn Fn(Ctx) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// `manager.RunnableFunc`
pub struct RunnableFunc {
    f: RunFn,
    need_leader_election: Option<bool>,
}

/// Build a [`Runnable`] from an async closure (`manager.RunnableFunc`, needs leader election).
pub fn runnable_fn<F, Fut>(f: F) -> Arc<dyn Runnable>
where
    F: Fn(Ctx) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<()>> + Send + 'static,
{
    Arc::new(RunnableFunc {
        f: Arc::new(move |ctx| Box::pin(f(ctx))),
        need_leader_election: None,
    })
}

/// Like [`runnable_fn`] but with an explicit `NeedLeaderElection()` value.
pub fn runnable_fn_with_leader_election<F, Fut>(need: bool, f: F) -> Arc<dyn Runnable>
where
    F: Fn(Ctx) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<()>> + Send + 'static,
{
    Arc::new(RunnableFunc {
        f: Arc::new(move |ctx| Box::pin(f(ctx))),
        need_leader_election: Some(need),
    })
}

#[async_trait::async_trait]
impl Runnable for RunnableFunc {
    async fn start(&self, ctx: Ctx) -> Result<()> {
        (self.f)(ctx).await
    }
    fn need_leader_election(&self) -> Option<bool> {
        self.need_leader_election
    }
}

// ---------------------------------------------------------------------------------------------
// runnableGroup
// ---------------------------------------------------------------------------------------------

type ReadyCheck = Arc<dyn Fn(CancellationToken) -> BoxFuture<'static, bool> + Send + Sync>;

struct ReadyRunnable {
    runnable: Arc<dyn Runnable>,
    check: Option<ReadyCheck>,
}

struct GroupState {
    started: bool,
    start_once: bool,
    stopped: bool,
    start_queue: Vec<ReadyRunnable>,
}

/// `errRunnableGroupStopped`
const ERR_RUNNABLE_GROUP_STOPPED: &str = "can't accept new runnable as stop procedure is already engaged";

struct RunnableGroup {
    cancel: CancellationToken,
    state: Mutex<GroupState>,
    err_tx: mpsc::UnboundedSender<anyhow::Error>,
    tracker: TaskTracker,
    log: Logger,
}

impl RunnableGroup {
    fn new(err_tx: mpsc::UnboundedSender<anyhow::Error>, log: Logger) -> Arc<Self> {
        Arc::new(Self {
            cancel: CancellationToken::new(),
            state: Mutex::new(GroupState {
                started: false,
                start_once: false,
                stopped: false,
                start_queue: Vec::new(),
            }),
            err_tx,
            tracker: TaskTracker::new(),
            log,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GroupState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn add(self: &Arc<Self>, runnable: Arc<dyn Runnable>, check: Option<ReadyCheck>) -> Result<()> {
        let rr = ReadyRunnable { runnable, check };
        let mut st = self.lock();
        if st.stopped {
            return Err(anyhow::anyhow!(ERR_RUNNABLE_GROUP_STOPPED));
        }
        if !st.started {
            st.start_queue.push(rr);
            return Ok(());
        }
        drop(st);
        self.spawn(rr, None);
        Ok(())
    }

    fn spawn(self: &Arc<Self>, rr: ReadyRunnable, ready: Option<oneshot::Sender<()>>) {
        {
            let st = self.lock();
            if st.stopped {
                let _ = self.err_tx.send(anyhow::anyhow!(ERR_RUNNABLE_GROUP_STOPPED));
                return;
            }
        }
        let this = self.clone();
        let cancel = self.cancel.clone();
        self.tracker.spawn(async move {
            {
                let check = rr.check.clone();
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    let ok = match check {
                        Some(c) => c(cancel).await,
                        None => true,
                    };
                    if ok && let Some(tx) = ready {
                        let _ = tx.send(());
                    }
                });
            }
            let ctx = Ctx {
                log: this.log.clone(),
                cancel,
            };
            if let Err(e) = rr.runnable.start(ctx).await {
                let stopped = this.lock().stopped;
                if this.err_tx.send(e).is_err() && stopped {
                    // the manager stopped draining errors
                }
            }
        });
    }

    /// `Start(ctx)`: start all queued runnables and wait until they signal readiness.
    async fn start(self: &Arc<Self>, ctx: &CancellationToken) -> Result<()> {
        let queue = {
            let mut st = self.lock();
            if st.start_once {
                return Ok(());
            }
            st.start_once = true;
            st.started = true;
            std::mem::take(&mut st.start_queue)
        };
        let mut waits = Vec::with_capacity(queue.len());
        for rr in queue {
            let (tx, rx) = oneshot::channel();
            self.spawn(rr, Some(tx));
            waits.push(rx);
        }
        for w in waits {
            tokio::select! {
                _ = ctx.cancelled() => return Ok(()),
                _ = w => {}
            }
        }
        Ok(())
    }

    /// Prevent a later `start` (`startOnce.Do(func() {})`).
    fn prevent_start(&self) {
        self.lock().start_once = true;
    }

    /// `StopAndWait(ctx)`
    async fn stop_and_wait(&self, deadline: Option<tokio::time::Instant>) {
        {
            let mut st = self.lock();
            if st.stopped {
                return;
            }
            st.stopped = true;
            st.start_once = true;
        }
        self.cancel.cancel();
        self.tracker.close();
        match deadline {
            Some(d) => {
                let _ = tokio::time::timeout_at(d, self.tracker.wait()).await;
            }
            None => self.tracker.wait().await,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------------------------

/// `ctrl.Options` (the subset used by `cmd/main.go`, plus useful knobs).
#[derive(Clone)]
pub struct ManagerOptions {
    /// `Cache`
    pub cache: CacheOptions,
    /// `HealthProbeBindAddress` (`""` or `"0"` disables the probe server).
    pub health_probe_bind_address: String,
    /// `ReadinessEndpointName` (default `/readyz`).
    pub readiness_endpoint_name: String,
    /// `LivenessEndpointName` (default `/healthz`).
    pub liveness_endpoint_name: String,
    /// `LeaderElection`
    pub leader_election: bool,
    /// `LeaderElectionResourceLock` (default `leases`).
    pub leader_election_resource_lock: String,
    /// `LeaderElectionID`
    pub leader_election_id: String,
    /// `LeaderElectionNamespace` (default: the in-cluster namespace).
    pub leader_election_namespace: String,
    /// `LeaderElectionReleaseOnCancel`
    pub leader_election_release_on_cancel: bool,
    /// `LeaseDuration` (default 15s).
    pub lease_duration: Duration,
    /// `RenewDeadline` (default 10s).
    pub renew_deadline: Duration,
    /// `RetryPeriod` (default 2s).
    pub retry_period: Duration,
    /// `GracefulShutdownTimeout` (default 30s; `None` = wait forever, Go's negative value).
    pub graceful_shutdown_timeout: Option<Duration>,
    /// `Controller` (global controller options).
    pub controller: ControllerOptions,
    /// `Logger` (default: the root logger, `ctrl.Log`).
    pub logger: Option<Logger>,
    /// Event recorder factory (default: [`LoggingRecorder`]).
    pub recorder_factory: Option<RecorderFactory>,
}

impl Default for ManagerOptions {
    fn default() -> Self {
        Self {
            cache: CacheOptions::default(),
            health_probe_bind_address: String::new(),
            readiness_endpoint_name: "/readyz".into(),
            liveness_endpoint_name: "/healthz".into(),
            leader_election: false,
            leader_election_resource_lock: String::new(),
            leader_election_id: String::new(),
            leader_election_namespace: String::new(),
            leader_election_release_on_cancel: false,
            lease_duration: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_secs(2),
            graceful_shutdown_timeout: Some(Duration::from_secs(30)),
            controller: ControllerOptions::default(),
            logger: None,
            recorder_factory: None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------------------------

struct Groups {
    http_servers: Arc<RunnableGroup>,
    caches: Arc<RunnableGroup>,
    others: Arc<RunnableGroup>,
    leader_election: Arc<RunnableGroup>,
}

struct Inner {
    config: RestConfig,
    kube_client: kube::Client,
    mapper: Arc<RestMapper>,
    cache: Cache,
    client: Client,
    api_reader: Client,
    logger: Logger,
    recorder_factory: RecorderFactory,
    resource_lock: Option<Arc<LeaseLock>>,
    health_listener: Mutex<Option<TcpListener>>,
    health_addr: Option<SocketAddr>,
    readiness_endpoint_name: String,
    liveness_endpoint_name: String,
    healthz: Mutex<Option<Handler>>,
    readyz: Mutex<Option<Handler>>,
    started: Mutex<bool>,
    groups: Groups,
    err_tx: mpsc::UnboundedSender<anyhow::Error>,
    err_rx: Mutex<Option<mpsc::UnboundedReceiver<anyhow::Error>>>,
    elected: watch::Sender<bool>,
    graceful_shutdown_timeout: Mutex<Option<Duration>>,
    controller_opts: ControllerOptions,
    lease_duration: Duration,
    renew_deadline: Duration,
    retry_period: Duration,
    leader_election_id: String,
    leader_election_release_on_cancel: bool,
}

/// `manager.Manager`
#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

/// `defaultHealthProbeListener`
fn listen(addr: &str) -> Result<Option<TcpListener>> {
    if addr.is_empty() || addr == "0" {
        return Ok(None);
    }
    let err = |e: std::io::Error| {
        anyhow::anyhow!(
            "error listening on {addr}: listen tcp {addr}: bind: {}",
            crate::config::io_error_text(&e)
        )
    };
    let (host, port) = match addr.rsplit_once(':') {
        Some((h, p)) => (h.trim_start_matches('[').trim_end_matches(']'), p),
        None => {
            return Err(anyhow::anyhow!(
                "error listening on {addr}: listen tcp: address {addr}: missing port in address"
            ));
        }
    };
    let port: u16 = if port.is_empty() {
        0
    } else {
        port.parse().map_err(|_| {
            anyhow::anyhow!("error listening on {addr}: listen tcp: lookup tcp/{port}: unknown port")
        })?
    };
    let ln = if host.is_empty() {
        TcpListener::bind((std::net::Ipv6Addr::UNSPECIFIED, port))
            .or_else(|_| TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)))
            .map_err(err)?
    } else {
        let addrs: Vec<SocketAddr> = (host, port)
            .to_socket_addrs()
            .map_err(|e| anyhow::anyhow!("error listening on {addr}: listen tcp: {e}"))?
            .collect();
        TcpListener::bind(&addrs[..]).map_err(err)?
    };
    ln.set_nonblocking(true)?;
    Ok(Some(ln))
}

/// Runnable for the cache (`cluster` in Go: started in the Caches group, ready once synced).
struct CacheRunnable(Cache);

#[async_trait::async_trait]
impl Runnable for CacheRunnable {
    async fn start(&self, ctx: Ctx) -> Result<()> {
        self.0.start(ctx.cancel.clone()).await
    }
}

/// `manager.Server` for the health probes.
struct ProbeServer {
    name: String,
    listener: Mutex<Option<TcpListener>>,
    mux: ProbeMux,
}

#[async_trait::async_trait]
impl Runnable for ProbeServer {
    async fn start(&self, ctx: Ctx) -> Result<()> {
        let Some(std_ln) = self.listener.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return Ok(());
        };
        let addr = std_ln
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        let log = ctx
            .log
            .with_values(kv!["name" => self.name, "addr" => addr]);
        let ln = tokio::net::TcpListener::from_std(std_ln)?;
        log.info("starting server", kv![]);
        let graceful = hyper_util::server::graceful::GracefulShutdown::new();
        loop {
            tokio::select! {
                _ = ctx.cancel.cancelled() => break,
                acc = ln.accept() => {
                    let Ok((stream, _)) = acc else { continue };
                    let mux = self.mux.clone();
                    let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                        let mux = mux.clone();
                        async move {
                            let (parts, _body) = req.into_parts();
                            let req = http::Request::from_parts(parts, ());
                            Ok::<_, std::convert::Infallible>(mux.serve(&req))
                        }
                    });
                    let conn = hyper::server::conn::http1::Builder::new()
                        .timer(hyper_util::rt::TokioTimer::new())
                        .header_read_timeout(Duration::from_secs(32))
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), svc);
                    let fut = graceful.watch(conn);
                    tokio::spawn(async move {
                        let _ = fut.await;
                    });
                }
            }
        }
        log.info("shutting down server", kv![]);
        drop(ln);
        graceful.shutdown().await;
        Ok(())
    }
    fn need_leader_election(&self) -> Option<bool> {
        Some(false)
    }
}

impl Manager {
    /// `ctrl.NewManager(config, options)`.
    pub async fn new(config: RestConfig, options: ManagerOptions) -> Result<Self> {
        metrics::register_all();
        let logger = options.logger.clone().unwrap_or_else(Logger::root);

        // cluster.New
        let kube_client = config.build_client()?;
        let rest = RestClient::new(kube_client.clone()).with_base(config.host());
        let mapper = Arc::new(RestMapper::new(rest.clone()));
        let direct = KubeBackend::with_mapper(rest, mapper.clone());
        let cache = Cache::new(kube_client.clone(), mapper.clone(), options.cache.clone()).await?;
        let client = Client::new(Arc::new(CachedBackend::new(cache.clone(), direct.clone())));
        let api_reader = Client::new(Arc::new(direct));

        // recorder provider
        let recorder_factory: RecorderFactory = options.recorder_factory.clone().unwrap_or_else(|| {
            let log = logger.with_name("events");
            Arc::new(move |_name: &str| -> ctrlrt::Recorder { Arc::new(LoggingRecorder::new(log.clone())) })
        });

        // resource lock
        let rf = recorder_factory.clone();
        let resource_lock = crate::leaderelection::new_resource_lock(
            &config,
            &move |n: &str| rf(n),
            ResourceLockOptions {
                leader_election: options.leader_election,
                leader_election_resource_lock: options.leader_election_resource_lock.clone(),
                leader_election_namespace: options.leader_election_namespace.clone(),
                leader_election_id: options.leader_election_id.clone(),
                renew_deadline: options.renew_deadline,
            },
        )?;

        // health probes listener
        let health_listener = listen(&options.health_probe_bind_address)?;
        let health_addr = health_listener.as_ref().and_then(|l| l.local_addr().ok());

        let (err_tx, err_rx) = mpsc::unbounded_channel();
        let groups = Groups {
            http_servers: RunnableGroup::new(err_tx.clone(), logger.clone()),
            caches: RunnableGroup::new(err_tx.clone(), logger.clone()),
            others: RunnableGroup::new(err_tx.clone(), logger.clone()),
            leader_election: RunnableGroup::new(err_tx.clone(), logger.clone()),
        };
        let (elected, _) = watch::channel(false);
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                kube_client,
                mapper,
                cache,
                client,
                api_reader,
                logger,
                recorder_factory,
                resource_lock,
                health_listener: Mutex::new(health_listener),
                health_addr,
                readiness_endpoint_name: options.readiness_endpoint_name.clone(),
                liveness_endpoint_name: options.liveness_endpoint_name.clone(),
                healthz: Mutex::new(None),
                readyz: Mutex::new(None),
                started: Mutex::new(false),
                groups,
                err_tx,
                err_rx: Mutex::new(Some(err_rx)),
                elected,
                graceful_shutdown_timeout: Mutex::new(options.graceful_shutdown_timeout),
                controller_opts: options.controller.clone(),
                lease_duration: options.lease_duration,
                renew_deadline: options.renew_deadline,
                retry_period: options.retry_period,
                leader_election_id: options.leader_election_id.clone(),
                leader_election_release_on_cancel: options.leader_election_release_on_cancel,
            }),
        })
    }

    /// `GetConfig()`
    pub fn config(&self) -> &RestConfig {
        &self.inner.config
    }
    /// The underlying `kube::Client` (`GetHTTPClient()` analogue).
    pub fn kube_client(&self) -> kube::Client {
        self.inner.kube_client.clone()
    }
    /// `GetRESTMapper()`
    pub fn rest_mapper(&self) -> Arc<RestMapper> {
        self.inner.mapper.clone()
    }
    /// `GetCache()`
    pub fn cache(&self) -> Cache {
        self.inner.cache.clone()
    }
    /// `GetLogger()`
    pub fn logger(&self) -> Logger {
        self.inner.logger.clone()
    }
    /// The bound health probe address (useful with port 0).
    pub fn health_probe_addr(&self) -> Option<SocketAddr> {
        self.inner.health_addr
    }

    /// `Add(runnable)`: routed to the right group like `runnables.Add`.
    pub fn add(&self, r: Arc<dyn Runnable>) -> Result<()> {
        let g = &self.inner.groups;
        match r.need_leader_election() {
            Some(false) => g.others.add(r, None),
            _ => g.leader_election.add(r, None),
        }
    }

    /// Add an HTTP server runnable (metrics server, extra servers): started before the caches,
    /// independently of leader election, and stopped last.
    pub fn add_http_server(&self, r: Arc<dyn Runnable>) -> Result<()> {
        self.inner.groups.http_servers.add(r, None)
    }

    /// Alias of [`Manager::add_http_server`] for the metrics server (Go adds it to the
    /// HTTPServers group directly).
    pub fn add_metrics_server(&self, r: Arc<dyn Runnable>) -> Result<()> {
        self.add_http_server(r)
    }

    fn check_not_started(&self) -> Result<()> {
        if *self.inner.started.lock().unwrap_or_else(|e| e.into_inner()) {
            return Err(anyhow::anyhow!(
                "unable to add new checker because healthz endpoint has already been created"
            ));
        }
        Ok(())
    }

    /// `AddHealthzCheck(name, check)`
    pub fn add_healthz_check(&self, name: &str, check: Checker) -> Result<()> {
        self.check_not_started()?;
        self.inner
            .healthz
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(Handler::new)
            .checks
            .insert(name.to_string(), check);
        Ok(())
    }

    /// `AddReadyzCheck(name, check)`
    pub fn add_readyz_check(&self, name: &str, check: Checker) -> Result<()> {
        self.check_not_started()?;
        self.inner
            .readyz
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(Handler::new)
            .checks
            .insert(name.to_string(), check);
        Ok(())
    }

    /// `Elected()`: resolves once this manager is the leader (or immediately after start when
    /// leader election is disabled).
    pub async fn elected(&self) {
        let mut rx = self.inner.elected.subscribe();
        let _ = rx.wait_for(|e| *e).await;
    }

    /// Whether `Elected()` is closed.
    pub fn is_elected(&self) -> bool {
        *self.inner.elected.borrow()
    }

    /// `Start(ctx)`: runs until `ctx` is cancelled or a runnable fails. Returns the error that
    /// stopped the manager (e.g. `leader election lost`).
    pub async fn start(&self, ctx: CancellationToken) -> Result<()> {
        let inner = &self.inner;
        {
            let mut started = inner.started.lock().unwrap_or_else(|e| e.into_inner());
            if *started {
                return Err(anyhow::anyhow!("manager already started"));
            }
            *started = true;
        }
        let Some(mut err_rx) = inner
            .err_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        else {
            return Err(anyhow::anyhow!("manager already started"));
        };
        let internal = ctx.child_token();

        // Leader elector must be created before the stop procedure.
        let elector = match &inner.resource_lock {
            Some(lock) => match LeaderElector::new(
                LeaderElectionConfig {
                    lease_duration: inner.lease_duration,
                    renew_deadline: inner.renew_deadline,
                    retry_period: inner.retry_period,
                    release_on_cancel: inner.leader_election_release_on_cancel,
                    name: inner.leader_election_id.clone(),
                },
                lock.clone(),
            ) {
                Ok(e) => Some(Arc::new(e)),
                Err(e) => {
                    let msg = format!("failed during initialization leader election process: {e}");
                    return Err(ctrlrt::error::wrap(e, msg));
                }
            },
            None => None,
        };

        let le_cancel = CancellationToken::new();
        let le_stopped = CancellationToken::new();
        let res = self
            .start_inner(&ctx, &internal, &mut err_rx, elector, &le_cancel, &le_stopped)
            .await;
        let stop_err = self
            .engage_stop_procedure(&internal, err_rx, &le_cancel, &le_stopped)
            .await;
        match (res, stop_err) {
            (Ok(()), None) => Ok(()),
            (Ok(()), Some(e)) => Err(e),
            (Err(e), None) => Err(e),
            (Err(e), Some(s)) => Err(anyhow::anyhow!("[{e}, {s}]")),
        }
    }

    async fn start_inner(
        &self,
        ctx: &CancellationToken,
        internal: &CancellationToken,
        err_rx: &mut mpsc::UnboundedReceiver<anyhow::Error>,
        elector: Option<Arc<LeaderElector>>,
        le_cancel: &CancellationToken,
        le_stopped: &CancellationToken,
    ) -> Result<()> {
        let inner = &self.inner;
        let g = &inner.groups;

        // Add the cluster runnable.
        let cache = inner.cache.clone();
        let check: ReadyCheck = Arc::new(move |c: CancellationToken| {
            let cache = cache.clone();
            Box::pin(async move { cache.wait_for_cache_sync(&c).await })
        });
        if let Err(e) = g.caches.add(Arc::new(CacheRunnable(inner.cache.clone())), Some(check)) {
            let msg = format!("failed to add cluster to runnables: {e}");
            return Err(ctrlrt::error::wrap(e, msg));
        }

        // Serve health probes.
        let listener = inner
            .health_listener
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(ln) = listener {
            let mux = ProbeMux {
                readiness_endpoint: inner.readiness_endpoint_name.clone(),
                liveness_endpoint: inner.liveness_endpoint_name.clone(),
                readyz: inner.readyz.lock().unwrap_or_else(|e| e.into_inner()).clone(),
                healthz: inner.healthz.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            };
            let srv = Arc::new(ProbeServer {
                name: "health probe".into(),
                listener: Mutex::new(Some(ln)),
                mux,
            });
            if let Err(e) = g.http_servers.add(srv, None) {
                let msg = format!("failed to add health probe server: {e}");
                return Err(ctrlrt::error::wrap(e, msg));
            }
        }

        // First start any HTTP servers, which includes health probes and metrics.
        if let Err(e) = g.http_servers.start(internal).await {
            let msg = format!("failed to start HTTP servers: {e}");
            return Err(ctrlrt::error::wrap(e, msg));
        }
        // Start and wait for caches.
        if let Err(e) = g.caches.start(internal).await {
            let msg = format!("failed to start caches: {e}");
            return Err(ctrlrt::error::wrap(e, msg));
        }
        // Start the non-leaderelection Runnables after the cache has synced.
        if let Err(e) = g.others.start(internal).await {
            let msg = format!("failed to start other runnables: {e}");
            return Err(ctrlrt::error::wrap(e, msg));
        }

        // Start the leader election and all required runnables.
        match elector {
            Some(el) => {
                let this = self.clone();
                let this2 = self.clone();
                let internal_c = internal.clone();
                let le_cancel_c = le_cancel.clone();
                let le_stopped_c = le_stopped.clone();
                tokio::spawn(async move {
                    el.run(
                        le_cancel_c.clone(),
                        move || {
                            tokio::spawn(async move {
                                let le = this.inner.groups.leader_election.clone();
                                if let Err(e) = le.start(&internal_c).await {
                                    let _ = this.inner.err_tx.send(e);
                                    return;
                                }
                                this.inner.elected.send_replace(true);
                            });
                        },
                        move || {
                            // Make sure graceful shutdown is skipped if we lost the leader lock
                            // without intending to.
                            *this2
                                .inner
                                .graceful_shutdown_timeout
                                .lock()
                                .unwrap_or_else(|e| e.into_inner()) = Some(Duration::ZERO);
                            let _ = this2
                                .inner
                                .err_tx
                                .send(anyhow::anyhow!("leader election lost"));
                        },
                    )
                    .await;
                    le_cancel_c.cancelled().await;
                    le_stopped_c.cancel();
                });
            }
            None => {
                let this = self.clone();
                let internal_c = internal.clone();
                tokio::spawn(async move {
                    // Treat not having leader election enabled the same as being elected.
                    let le = this.inner.groups.leader_election.clone();
                    if let Err(e) = le.start(&internal_c).await {
                        let _ = this.inner.err_tx.send(e);
                    }
                    this.inner.elected.send_replace(true);
                });
            }
        }

        tokio::select! {
            _ = ctx.cancelled() => Ok(()),
            e = err_rx.recv() => match e {
                Some(e) => Err(e),
                None => Ok(()),
            },
        }
    }

    /// `engageStopProcedure`
    async fn engage_stop_procedure(
        &self,
        internal: &CancellationToken,
        mut err_rx: mpsc::UnboundedReceiver<anyhow::Error>,
        le_cancel: &CancellationToken,
        le_stopped: &CancellationToken,
    ) -> Option<anyhow::Error> {
        let inner = &self.inner;
        let timeout = *inner
            .graceful_shutdown_timeout
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let deadline = timeout.map(|t| tokio::time::Instant::now() + t);

        // Drain errors while stopping.
        let stop_complete = CancellationToken::new();
        let logger = inner.logger.clone();
        let drain_stop = stop_complete.clone();
        let log_err = move |e: anyhow::Error| {
            if !is_context_canceled(&e) {
                logger.error(&e, "error received after stop sequence was engaged", kv![]);
            }
        };
        internal.cancel();
        let drain = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    e = err_rx.recv() => match e {
                        Some(e) => log_err(e),
                        None => return,
                    },
                    _ = drain_stop.cancelled() => {
                        while let Ok(e) = err_rx.try_recv() {
                            log_err(e);
                        }
                        return;
                    }
                }
            }
        });

        let this = self.clone();
        let stop_all = tokio::spawn(async move {
            let g = &this.inner.groups;
            let log = &this.inner.logger;
            log.info("Stopping and waiting for non leader election runnables", kv![]);
            g.others.stop_and_wait(deadline).await;
            log.info("Stopping and waiting for leader election runnables", kv![]);
            // Prevent leader election when shutting down a non-elected manager
            g.leader_election.prevent_start();
            g.leader_election.stop_and_wait(deadline).await;
            log.info("Stopping and waiting for caches", kv![]);
            g.caches.stop_and_wait(deadline).await;
            log.info("Stopping and waiting for webhooks", kv![]);
            log.info("Stopping and waiting for HTTP servers", kv![]);
            g.http_servers.stop_and_wait(deadline).await;
            log.info("Wait completed, proceeding to shutdown the manager", kv![]);
        });

        let mut result = None;
        match deadline {
            None => {
                let _ = stop_all.await;
            }
            Some(d) => {
                if tokio::time::timeout_at(d, stop_all).await.is_err() {
                    let t = timeout.unwrap_or_default();
                    if !t.is_zero() {
                        result = Some(anyhow::anyhow!(
                            "failed waiting for all runnables to end within grace period of {}: context deadline exceeded",
                            go_duration(t)
                        ));
                    }
                }
            }
        }

        // Cancel leader election only after we waited.
        if inner.resource_lock.is_some() {
            le_cancel.cancel();
            le_stopped.cancelled().await;
        }
        stop_complete.cancel();
        let _ = drain.await;
        result
    }

    /// Spec registration used by [`ManagerHandle::add_controller`].
    pub fn new_controller(&self, spec: ControllerSpec) -> Result<Arc<Controller>> {
        Controller::new(
            spec,
            self.inner.cache.clone(),
            &self.inner.logger,
            self.inner.controller_opts.clone(),
        )
    }
}

fn is_context_canceled(e: &anyhow::Error) -> bool {
    e.to_string() == "context canceled"
}

impl ManagerHandle for Manager {
    fn client(&self) -> Client {
        self.inner.client.clone()
    }
    fn api_reader(&self) -> Client {
        self.inner.api_reader.clone()
    }
    fn event_recorder_for(&self, name: &str) -> ctrlrt::Recorder {
        (self.inner.recorder_factory)(name)
    }
    fn add_controller(&self, spec: ControllerSpec) -> Result<()> {
        let c = self.new_controller(spec)?;
        self.add(Arc::new(ControllerRunnable(c)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_disabled_and_errors() {
        assert!(listen("").unwrap().is_none());
        assert!(listen("0").unwrap().is_none());
        let ln = listen("127.0.0.1:0").unwrap().unwrap();
        let addr = ln.local_addr().unwrap().to_string();
        let err = listen(&addr).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("error listening on {addr}: listen tcp {addr}: bind: address already in use")
        );
        let any = listen(":0").unwrap().unwrap();
        assert!(any.local_addr().unwrap().ip().is_unspecified());
    }

    #[tokio::test]
    async fn runnable_group_starts_queued_and_late_runnables() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let g = RunnableGroup::new(tx, Logger::discard());
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h1 = hits.clone();
        g.add(
            runnable_fn(move |ctx| {
                let h1 = h1.clone();
                async move {
                    h1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    ctx.cancel.cancelled().await;
                    Ok(())
                }
            }),
            None,
        )
        .unwrap();
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        let ctx = CancellationToken::new();
        g.start(&ctx).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        g.add(runnable_fn(|_ctx| async { Err(anyhow::anyhow!("boom")) }), None)
            .unwrap();
        let e = rx.recv().await.unwrap();
        assert_eq!(e.to_string(), "boom");
        g.stop_and_wait(Some(tokio::time::Instant::now() + Duration::from_secs(5)))
            .await;
        assert_eq!(
            g.add(runnable_fn(|_ctx| async { Ok(()) }), None)
                .unwrap_err()
                .to_string(),
            ERR_RUNNABLE_GROUP_STOPPED
        );
    }
}
