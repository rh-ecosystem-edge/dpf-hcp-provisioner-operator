//! Real Kubernetes event recording: client-go `tools/record` (`NewBroadcaster`,
//! `EventCorrelator` with aggregation, de-duplication and spam filtering, retrying sink) as
//! hosted by controller-runtime's recorder `Provider` (`mgr.GetEventRecorderFor(name)`).
//!
//! ```ignore
//! let provider = RecorderProvider::new(client.clone(), Logger::root().with_name("events"));
//! let recorder: ctrlrt::Recorder = provider.get_event_recorder_for("dpfhcpprovisioner-controller");
//! recorder.event(&obj, "Normal", "Created", "message");   // RecorderExt
//! // on manager shutdown:
//! provider.stop(Duration::from_secs(30)).await;
//! ```
//!
//! Every event is (asynchronously) written to the API server as a `core/v1` Event named
//! `<object name>.<unix nanos in hex>` in the object's namespace (`default` for cluster
//! scoped objects) and logged at `V(1)` on the provider logger:
//! `<message>  {"type": "...", "object": {...}, "reason": "..."}`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ctrlrt::client::Patch;
use ctrlrt::kv;
use ctrlrt::log::Logger;
use ctrlrt::recorder::EventRecorder;
use k8s_openapi::api::core::v1::{Event, EventSource, ObjectReference};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};
use k8s_openapi::jiff::Timestamp;
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::klog;

const MAX_TRIES_PER_EVENT: u32 = 12;
/// `defaultSleepDuration`
pub const DEFAULT_SLEEP_DURATION: Duration = Duration::from_secs(10);
const MAX_QUEUED_EVENTS: usize = 1000;
const MAX_LRU_CACHE_ENTRIES: usize = 4096;
const DEFAULT_AGGREGATE_MAX_EVENTS: usize = 10;
const DEFAULT_AGGREGATE_INTERVAL_SECONDS: i64 = 600;
const DEFAULT_SPAM_BURST: f64 = 25.0;
/// float32(1. / 300.) as Go stores it.
fn default_spam_qps() -> f64 {
    (1.0f32 / 300.0f32) as f64
}

/// Clock used for event timestamps.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

fn real_clock() -> Clock {
    Arc::new(Utc::now)
}

fn unix_nano(t: &DateTime<Utc>) -> i64 {
    t.timestamp_nanos_opt().unwrap_or(0)
}

/// `metav1.Time` as Go serializes it (second precision).
fn meta_time(t: &DateTime<Utc>) -> Time {
    Time(Timestamp::from_second(t.timestamp()).unwrap_or(Timestamp::UNIX_EPOCH))
}

fn meta_time_rfc3339(t: &Time) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Minimal LRU cache (`k8s.io/utils/lru`).
struct Lru<K, V> {
    cap: usize,
    tick: u64,
    map: HashMap<K, (V, u64)>,
    order: VecDeque<(u64, K)>,
}

impl<K: Eq + Hash + Clone, V: Clone> Lru<K, V> {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            tick: 0,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn touch(&mut self, k: &K) {
        self.tick += 1;
        if let Some(e) = self.map.get_mut(k) {
            e.1 = self.tick;
        }
        self.order.push_back((self.tick, k.clone()));
        // compact stale order entries occasionally
        if self.order.len() > self.cap * 4 + 16 {
            let map = &self.map;
            self.order
                .retain(|(t, k)| map.get(k).is_some_and(|e| e.1 == *t));
        }
    }

    fn get(&mut self, k: &K) -> Option<V> {
        let v = self.map.get(k).map(|e| e.0.clone())?;
        self.touch(k);
        Some(v)
    }

    fn add(&mut self, k: K, v: V) {
        self.map.insert(k.clone(), (v, 0));
        self.touch(&k);
        while self.map.len() > self.cap {
            let Some((t, old)) = self.order.pop_front() else {
                break;
            };
            if self.map.get(&old).is_some_and(|e| e.1 == t) {
                self.map.remove(&old);
            }
        }
    }
}

fn s(o: &Option<String>) -> &str {
    o.as_deref().unwrap_or("")
}

/// `getEventKey`
pub fn get_event_key(e: &Event) -> String {
    let src = e.source.clone().unwrap_or_default();
    let io = &e.involved_object;
    [
        s(&src.component),
        s(&src.host),
        s(&io.kind),
        s(&io.namespace),
        s(&io.name),
        s(&io.field_path),
        s(&io.uid),
        s(&io.api_version),
        s(&e.type_),
        s(&e.reason),
        s(&e.message),
    ]
    .concat()
}

/// `getSpamKey`
pub fn get_spam_key(e: &Event) -> String {
    let src = e.source.clone().unwrap_or_default();
    let io = &e.involved_object;
    [
        s(&src.component),
        s(&src.host),
        s(&io.kind),
        s(&io.namespace),
        s(&io.name),
        s(&io.uid),
        s(&io.api_version),
        s(&e.type_),
    ]
    .concat()
}

/// `EventAggregatorByReasonFunc`
pub fn aggregator_by_reason(e: &Event) -> (String, String) {
    let src = e.source.clone().unwrap_or_default();
    let io = &e.involved_object;
    (
        [
            s(&src.component),
            s(&src.host),
            s(&io.kind),
            s(&io.namespace),
            s(&io.name),
            s(&io.uid),
            s(&io.api_version),
            s(&e.type_),
            s(&e.reason),
            s(&e.reporting_component),
            s(&e.reporting_instance),
        ]
        .concat(),
        s(&e.message).to_string(),
    )
}

/// `x/time/rate.Limiter` used passively (`TryAccept`).
#[derive(Clone)]
struct TokenBucket {
    qps: f64,
    burst: f64,
    tokens: f64,
    last: Option<DateTime<Utc>>,
}

impl TokenBucket {
    fn try_accept(&mut self, now: DateTime<Utc>) -> bool {
        let tokens = match self.last {
            None => self.burst,
            Some(last) => {
                let last = if now < last { now } else { last };
                let elapsed = (unix_nano(&now) - unix_nano(&last)) as f64 / 1e9;
                (self.tokens + elapsed * self.qps).min(self.burst)
            }
        };
        let tokens = tokens - 1.0;
        if tokens >= 0.0 {
            self.tokens = tokens;
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

#[derive(Clone)]
struct AggregateRecord {
    local_keys: Arc<Mutex<HashSet<String>>>,
    last_timestamp: Option<DateTime<Utc>>,
}

#[derive(Clone, Default)]
struct EventLog {
    count: i32,
    first_timestamp: Option<Time>,
    name: String,
    resource_version: String,
}

/// Result of [`EventCorrelator::correlate`].
#[derive(Clone, Debug)]
pub struct CorrelateResult {
    pub event: Event,
    /// Strategic merge patch to apply when `event.count > 1`.
    pub patch: Option<Value>,
    pub skip: bool,
}

/// `record.EventCorrelator` (default options).
pub struct EventCorrelator {
    clock: Clock,
    spam: Mutex<Lru<String, TokenBucket>>,
    aggregate: Mutex<Lru<String, AggregateRecord>>,
    logs: Mutex<Lru<String, EventLog>>,
}

impl EventCorrelator {
    pub fn new(clock: Clock) -> Self {
        Self {
            clock,
            spam: Mutex::new(Lru::new(MAX_LRU_CACHE_ENTRIES)),
            aggregate: Mutex::new(Lru::new(MAX_LRU_CACHE_ENTRIES)),
            logs: Mutex::new(Lru::new(MAX_LRU_CACHE_ENTRIES)),
        }
    }

    /// `EventAggregator.EventAggregate`
    fn aggregate(&self, new_event: &Event) -> (Event, String) {
        let now = (self.clock)();
        let event_key = get_event_key(new_event);
        let (aggregate_key, local_key) = aggregator_by_reason(new_event);
        let Ok(mut cache) = self.aggregate.lock() else {
            return (new_event.clone(), event_key);
        };
        let mut record = cache.get(&aggregate_key).unwrap_or(AggregateRecord {
            local_keys: Arc::new(Mutex::new(HashSet::new())),
            last_timestamp: None,
        });
        // Go: zero lastTimestamp -> interval is huge -> fresh record.
        let reset = match record.last_timestamp {
            None => true,
            Some(last) => {
                (unix_nano(&now) - unix_nano(&last))
                    > DEFAULT_AGGREGATE_INTERVAL_SECONDS * 1_000_000_000
            }
        };
        if reset {
            record = AggregateRecord {
                local_keys: Arc::new(Mutex::new(HashSet::new())),
                last_timestamp: None,
            };
        }
        let len = {
            let Ok(mut keys) = record.local_keys.lock() else {
                return (new_event.clone(), event_key);
            };
            keys.insert(local_key);
            keys.len()
        };
        record.last_timestamp = Some(now);
        cache.add(aggregate_key.clone(), record.clone());

        if len < DEFAULT_AGGREGATE_MAX_EVENTS {
            return (new_event.clone(), event_key);
        }
        // do not grow our local key set any larger than max
        if let Ok(mut keys) = record.local_keys.lock()
            && let Some(k) = keys.iter().next().cloned()
        {
            keys.remove(&k);
        }

        let name = format!(
            "{}.{:x}",
            s(&new_event.involved_object.name),
            unix_nano(&now)
        );
        let t = meta_time(&now);
        let copy = Event {
            metadata: ObjectMeta {
                name: Some(name),
                namespace: new_event.metadata.namespace.clone(),
                ..Default::default()
            },
            count: Some(1),
            first_timestamp: Some(t.clone()),
            involved_object: new_event.involved_object.clone(),
            last_timestamp: Some(t),
            message: Some(format!(
                "(combined from similar events): {}",
                s(&new_event.message)
            )),
            type_: new_event.type_.clone(),
            reason: new_event.reason.clone(),
            source: new_event.source.clone(),
            ..Default::default()
        };
        (copy, aggregate_key)
    }

    /// `eventLogger.eventObserve`
    fn observe(&self, new_event: &Event, key: &str) -> (Event, Option<Value>) {
        let mut event = new_event.clone();
        let Ok(mut cache) = self.logs.lock() else {
            return (event, None);
        };
        let last = cache.get(&key.to_string()).unwrap_or_default();
        let mut patch = None;
        if last.count > 0 {
            event.metadata.name = Some(last.name.clone());
            event.metadata.resource_version =
                Some(last.resource_version.clone()).filter(|s| !s.is_empty());
            event.first_timestamp = last.first_timestamp.clone();
            event.count = Some(last.count + 1);
            // CreateTwoWayMergePatch(old{count:0, lastTimestamp:epoch, message:""}, new)
            let mut m = Map::new();
            m.insert("count".into(), Value::from(event.count.unwrap_or(0)));
            if let Some(lt) = &event.last_timestamp {
                let lts = meta_time_rfc3339(lt);
                if lts != "1970-01-01T00:00:00Z" {
                    m.insert("lastTimestamp".into(), Value::String(lts));
                }
            }
            let msg = s(&event.message);
            if !msg.is_empty() {
                m.insert("message".into(), Value::String(msg.to_string()));
            }
            patch = Some(Value::Object(m));
        }
        cache.add(
            key.to_string(),
            EventLog {
                count: event.count.unwrap_or(0),
                first_timestamp: event.first_timestamp.clone(),
                name: s(&event.metadata.name).to_string(),
                resource_version: s(&event.metadata.resource_version).to_string(),
            },
        );
        (event, patch)
    }

    /// `EventSourceObjectSpamFilter.Filter`: true = drop.
    fn filter(&self, event: &Event) -> bool {
        let key = get_spam_key(event);
        let Ok(mut cache) = self.spam.lock() else {
            return false;
        };
        let mut rec = cache.get(&key).unwrap_or(TokenBucket {
            qps: default_spam_qps(),
            burst: DEFAULT_SPAM_BURST,
            tokens: 0.0,
            last: None,
        });
        let filter = !rec.try_accept((self.clock)());
        cache.add(key, rec);
        filter
    }

    /// `EventCorrelate`
    pub fn correlate(&self, new_event: &Event) -> CorrelateResult {
        let (aggregate_event, ckey) = self.aggregate(new_event);
        let (observed, patch) = self.observe(&aggregate_event, &ckey);
        if self.filter(&observed) {
            return CorrelateResult {
                event: observed,
                patch: None,
                skip: true,
            };
        }
        CorrelateResult {
            event: observed,
            patch,
            skip: false,
        }
    }

    /// `UpdateState`
    pub fn update_state(&self, event: &Event) {
        let key = get_event_key(event);
        if let Ok(mut cache) = self.logs.lock() {
            cache.add(
                key,
                EventLog {
                    count: event.count.unwrap_or(0),
                    first_timestamp: event.first_timestamp.clone(),
                    name: s(&event.metadata.name).to_string(),
                    resource_version: s(&event.metadata.resource_version).to_string(),
                },
            );
        }
    }
}

/// `apimachineryvalidation.NameIsDNSSubdomain`
pub fn is_dns1123_subdomain(name: &str) -> bool {
    if name.is_empty() || name.len() > 253 {
        return false;
    }
    name.split('.').all(|label| {
        let b = label.as_bytes();
        !b.is_empty()
            && b[0].is_ascii_alphanumeric()
            && !b[0].is_ascii_uppercase()
            && b[b.len() - 1].is_ascii_alphanumeric()
            && !b[b.len() - 1].is_ascii_uppercase()
            && b.iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
    })
}

fn uuid_v4() -> String {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 16];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// `util.GenerateEventName(refName, unixNano)`
pub fn generate_event_name(ref_name: &str, unix_nano: i64) -> String {
    let name = format!("{ref_name}.{unix_nano:x}");
    if !is_dns1123_subdomain(&name) {
        return uuid_v4();
    }
    name
}

/// klog's rendering of an event in error lines (Go prints the `*v1.Event` with `%+v`; here
/// the identifying fields are rendered).
fn klog_event(e: &Event) -> String {
    format!(
        "&Event{{ObjectMeta:{{Name:{} Namespace:{}}} InvolvedObject:{{Kind:{} Namespace:{} Name:{}}} Reason:{} Message:{} Type:{} Count:{}}}",
        s(&e.metadata.name),
        s(&e.metadata.namespace),
        s(&e.involved_object.kind),
        s(&e.involved_object.namespace),
        s(&e.involved_object.name),
        s(&e.reason),
        s(&e.message),
        s(&e.type_),
        e.count.unwrap_or(0)
    )
}

/// Outcome of a single write attempt (`recordEvent`): true = done (success or give up).
async fn record_event(
    client: &ctrlrt::Client,
    event: &mut Event,
    patch: Option<&Value>,
    update_existing: bool,
    correlator: &EventCorrelator,
) -> bool {
    let mut result: Option<ctrlrt::Result<Event>> = None;
    if update_existing && let Some(p) = patch {
        let mut ev = event.clone();
        result = Some(
            client
                .patch(&mut ev, &Patch::Strategic(p.clone()))
                .await
                .map(|_| ev),
        );
    }
    let key_not_found = matches!(&result, Some(Err(e)) if ctrlrt::error::as_api_error(e).is_some_and(|a| a.code == 404));
    if !update_existing || key_not_found || result.is_none() {
        event.metadata.resource_version = None;
        let mut ev = event.clone();
        result = Some(client.create(&mut ev).await.map(|_| ev));
    }
    let err = match result {
        Some(Ok(new_event)) => {
            correlator.update_state(&new_event);
            return true;
        }
        Some(Err(e)) => e,
        None => return true,
    };
    if let Some(api) = ctrlrt::error::as_api_error(&err) {
        let ns_terminating = api
            .details
            .as_ref()
            .is_some_and(|d| d.causes.iter().any(|c| c.type_ == "NamespaceTerminating"));
        if api.reason == ctrlrt::error::REASON_ALREADY_EXISTS || ns_terminating {
            // klog V(5): not printed at the default klog verbosity.
        } else {
            klog::structured(
                'E',
                "event.go:359",
                "Server rejected event (will not retry!)",
                Some(&err.to_string()),
                &[("event", klog_event(event))],
            );
        }
        return true;
    }
    klog::structured(
        'E',
        "event.go:368",
        "Unable to write event (may retry after sleeping)",
        Some(&err.to_string()),
        &[("event", klog_event(event))],
    );
    false
}

fn rand_f64() -> f64 {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 8];
    if ring::rand::SystemRandom::new().fill(&mut b).is_err() {
        return 0.5;
    }
    (u64::from_le_bytes(b) >> 11) as f64 / (1u64 << 53) as f64
}

/// `recordToSink`
async fn record_to_sink(
    client: &ctrlrt::Client,
    event: &Event,
    correlator: &EventCorrelator,
    sleep: Duration,
    cancel: &CancellationToken,
) {
    let result = correlator.correlate(event);
    if result.skip {
        return;
    }
    let mut ev = result.event;
    let update_existing = ev.count.unwrap_or(0) > 1;
    let mut tries = 0;
    loop {
        if record_event(
            client,
            &mut ev,
            result.patch.as_ref(),
            update_existing,
            correlator,
        )
        .await
        {
            break;
        }
        tries += 1;
        if tries >= MAX_TRIES_PER_EVENT {
            klog::structured(
                'E',
                "event.go:307",
                "Unable to write event (retry limit exceeded!)",
                None,
                &[("event", klog_event(event))],
            );
            break;
        }
        // Randomize the first sleep so that various clients won't all be
        // synced up if the master goes down.
        let delay = if tries == 1 {
            sleep.mul_f64(rand_f64())
        } else {
            sleep
        };
        tokio::select! {
            _ = cancel.cancelled() => {
                klog::structured(
                    'E',
                    "event.go:319",
                    "Unable to write event (broadcaster is shut down)",
                    None,
                    &[("event", klog_event(event))],
                );
                return;
            }
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

/// `record.EventBroadcaster` (`watch.NewLongQueueBroadcaster(1000, DropIfChannelFull)`) with
/// the sink watcher and the controller-runtime logging watcher started.
pub struct Broadcaster {
    incoming: Mutex<Option<mpsc::Sender<Event>>>,
    cancel: CancellationToken,
    distributor: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Broadcaster {
    /// `NewBroadcaster()` + `StartRecordingToSink` + `StartEventWatcher(log)`.
    fn start(
        handle: &tokio::runtime::Handle,
        client: ctrlrt::Client,
        logger: Logger,
        clock: Clock,
        sleep: Duration,
    ) -> Self {
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel::<Event>(MAX_QUEUED_EVENTS);
        let (sink_tx, mut sink_rx) = mpsc::channel::<Event>(MAX_QUEUED_EVENTS);
        let (log_tx, mut log_rx) = mpsc::channel::<Event>(MAX_QUEUED_EVENTS);

        let distributor = handle.spawn(async move {
            while let Some(ev) = rx.recv().await {
                // DropIfChannelFull
                let _ = sink_tx.try_send(ev.clone());
                let _ = log_tx.try_send(ev);
            }
        });

        let correlator = EventCorrelator::new(clock);
        let c2 = cancel.clone();
        handle.spawn(async move {
            loop {
                tokio::select! {
                    _ = c2.cancelled() => return,
                    ev = sink_rx.recv() => match ev {
                        Some(ev) => record_to_sink(&client, &ev, &correlator, sleep, &c2).await,
                        None => return,
                    },
                }
            }
        });

        let c3 = cancel.clone();
        handle.spawn(async move {
            loop {
                tokio::select! {
                    _ = c3.cancelled() => return,
                    ev = log_rx.recv() => match ev {
                        Some(e) => logger.v(1).info(
                            s(&e.message),
                            kv!["type" => s(&e.type_), "object" => e.involved_object, "reason" => s(&e.reason)],
                        ),
                        None => return,
                    },
                }
            }
        });

        Self {
            incoming: Mutex::new(Some(tx)),
            cancel,
            distributor: Mutex::new(Some(distributor)),
        }
    }

    /// `ActionOrDrop(watch.Added, event)`: Ok(false) when the queue is full.
    fn action_or_drop(&self, ev: Event) -> Result<bool, String> {
        let Ok(guard) = self.incoming.lock() else {
            return Err("broadcaster already stopped".into());
        };
        let Some(tx) = guard.as_ref() else {
            return Err("broadcaster already stopped".into());
        };
        match tx.try_send(ev) {
            Ok(()) => Ok(true),
            Err(mpsc::error::TrySendError::Full(_)) => Ok(false),
            Err(mpsc::error::TrySendError::Closed(_)) => Err("broadcaster already stopped".into()),
        }
    }

    /// `Shutdown()`: drain queued events to the watchers, then stop them.
    async fn shutdown(&self) {
        let tx = self.incoming.lock().ok().and_then(|mut g| g.take());
        drop(tx);
        let d = self.distributor.lock().ok().and_then(|mut g| g.take());
        if let Some(d) = d {
            let _ = d.await;
        }
        self.cancel.cancel();
    }
}

struct ProviderInner {
    client: ctrlrt::Client,
    logger: Logger,
    clock: Clock,
    sleep: Duration,
    handle: Option<tokio::runtime::Handle>,
    broadcaster: OnceLock<Option<Arc<Broadcaster>>>,
    stopped: RwLock<bool>,
}

impl ProviderInner {
    fn broadcaster(&self) -> Option<Arc<Broadcaster>> {
        self.broadcaster
            .get_or_init(|| {
                let handle = self
                    .handle
                    .clone()
                    .or_else(|| tokio::runtime::Handle::try_current().ok())?;
                Some(Arc::new(Broadcaster::start(
                    &handle,
                    self.client.clone(),
                    self.logger.clone(),
                    self.clock.clone(),
                    self.sleep,
                )))
            })
            .clone()
    }
}

/// controller-runtime's recorder `Provider` (one broadcaster shared by all recorders,
/// started lazily on first use).
#[derive(Clone)]
pub struct RecorderProvider {
    inner: Arc<ProviderInner>,
}

impl std::fmt::Debug for RecorderProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecorderProvider")
    }
}

impl RecorderProvider {
    /// `intrec.NewProvider(config, httpClient, scheme, logger, makeBroadcaster)`; the manager
    /// passes `options.Logger.WithName("events")`. Captures the current tokio runtime if any.
    pub fn new(client: ctrlrt::Client, logger: Logger) -> Self {
        Self::with_options(client, logger, real_clock(), DEFAULT_SLEEP_DURATION)
    }

    /// Like [`RecorderProvider::new`] with an explicit clock and retry sleep duration
    /// (`NewBroadcasterForTests`).
    pub fn with_options(
        client: ctrlrt::Client,
        logger: Logger,
        clock: Clock,
        sleep: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(ProviderInner {
                client,
                logger,
                clock,
                sleep,
                handle: tokio::runtime::Handle::try_current().ok(),
                broadcaster: OnceLock::new(),
                stopped: RwLock::new(false),
            }),
        }
    }

    /// `GetEventRecorderFor(name)`: events get `source.component = name`.
    pub fn get_event_recorder_for(&self, name: &str) -> ctrlrt::Recorder {
        Arc::new(LazyRecorder {
            inner: self.inner.clone(),
            name: name.to_string(),
        })
    }

    /// `Stop(shutdownCtx)`: shut down the broadcaster (bounded by `timeout`); afterwards
    /// events are dropped.
    pub async fn stop(&self, timeout: Duration) {
        let inner = self.inner.clone();
        let fut = async move {
            if let Some(b) = inner.broadcaster() {
                b.shutdown().await;
            }
            if let Ok(mut s) = inner.stopped.write() {
                *s = true;
            }
        };
        let _ = tokio::time::timeout(timeout, fut).await;
    }
}

/// `lazyRecorder` + `recorderImpl`.
struct LazyRecorder {
    inner: Arc<ProviderInner>,
    name: String,
}

impl LazyRecorder {
    fn make_event(
        &self,
        r: &ObjectReference,
        event_type: &str,
        reason: &str,
        message: &str,
    ) -> Event {
        let now = (self.inner.clock)();
        let t = meta_time(&now);
        let namespace = match r.namespace.as_deref() {
            Some(ns) if !ns.is_empty() => ns.to_string(),
            _ => "default".to_string(),
        };
        Event {
            metadata: ObjectMeta {
                name: Some(generate_event_name(s(&r.name), unix_nano(&now))),
                namespace: Some(namespace),
                ..Default::default()
            },
            involved_object: r.clone(),
            reason: Some(reason.to_string()),
            message: Some(message.to_string()),
            first_timestamp: Some(t.clone()),
            last_timestamp: Some(t),
            count: Some(1),
            type_: Some(event_type.to_string()),
            source: Some(EventSource {
                component: Some(self.name.clone()),
                host: None,
            }),
            reporting_component: Some(self.name.clone()),
            reporting_instance: Some(String::new()),
            ..Default::default()
        }
    }
}

impl EventRecorder for LazyRecorder {
    fn event_ref(&self, object: &ObjectReference, event_type: &str, reason: &str, message: &str) {
        let Some(b) = self.inner.broadcaster() else {
            klog::structured(
                'E',
                "event.go:464",
                "Unable to record event (will not retry!)",
                Some("no async runtime available"),
                &[],
            );
            return;
        };
        let Ok(stopped) = self.inner.stopped.read() else {
            return;
        };
        if *stopped {
            return;
        }
        if event_type != "Normal" && event_type != "Warning" {
            klog::structured(
                'E',
                "event.go:447",
                "Unsupported event type",
                None,
                &[("eventType", event_type.to_string())],
            );
            return;
        }
        let ev = self.make_event(object, event_type, reason, message);
        match b.action_or_drop(ev.clone()) {
            Err(e) => klog::structured(
                'E',
                "event.go:464",
                "Unable to record event (will not retry!)",
                Some(&e),
                &[],
            ),
            Ok(false) => klog::structured(
                'E',
                "event.go:468",
                "Unable to record event: too many queued events, dropped event",
                None,
                &[("event", klog_event(&ev))],
            ),
            Ok(true) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names() {
        assert_eq!(
            generate_event_name("foo", 0x17c0a7d8f0f3a9b2),
            "foo.17c0a7d8f0f3a9b2"
        );
        let n = generate_event_name("Foo_Bar", 1);
        assert_eq!(n.len(), 36);
        assert!(is_dns1123_subdomain("a.b-c.d"));
        assert!(!is_dns1123_subdomain("a..b"));
        assert!(!is_dns1123_subdomain("-a"));
    }

    #[test]
    fn token_bucket() {
        let t0 = DateTime::<Utc>::from_timestamp(1000, 0).unwrap();
        let mut b = TokenBucket {
            qps: default_spam_qps(),
            burst: 25.0,
            tokens: 0.0,
            last: None,
        };
        for _ in 0..25 {
            assert!(b.try_accept(t0));
        }
        assert!(!b.try_accept(t0));
        // one token every 300s
        assert!(!b.try_accept(t0 + chrono::Duration::seconds(299)));
        assert!(b.try_accept(t0 + chrono::Duration::seconds(301)));
        assert!(!b.try_accept(t0 + chrono::Duration::seconds(301)));
    }

    #[test]
    fn lru_evicts_oldest() {
        let mut l: Lru<u32, u32> = Lru::new(2);
        l.add(1, 1);
        l.add(2, 2);
        assert_eq!(l.get(&1), Some(1));
        l.add(3, 3);
        assert_eq!(l.get(&2), None);
        assert_eq!(l.get(&1), Some(1));
        assert_eq!(l.get(&3), Some(3));
    }
}
