//! Shared informer (client-go `cache.SharedIndexInformer` + reflector + DeltaFIFO semantics) on
//! top of `kube::runtime::watcher`.
//!
//! Event semantics reproduced from client-go / controller-runtime:
//! * initial list: one `Create` per object;
//! * relist (after a watch expired): `Update{old,new}` for objects already known (even when
//!   unchanged), `Create` for new ones and `Delete` (final state unknown, last known object)
//!   for objects that disappeared;
//! * watch `ADDED`/`MODIFIED`: `Create` if unknown, else `Update{old,new}`; `DELETED`: `Delete`;
//! * periodic resync: `Update{old: obj, new: obj}` for every cached object;
//! * a handler registered on a running informer first receives a `Create` per cached object.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctrlrt::reconcile::RawEvent;
use futures::StreamExt;
use kube::api::{Api, DynamicObject};
use kube::runtime::watcher::{self, Event};
use kube::runtime::WatchStreamExt;
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::client::RestMapping;
use crate::klog;
use crate::util::go_type_name;

/// List/watch restrictions of an informer (`internal.InformersOpts` namespace + selectors).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InformerOptions {
    /// Namespace to list/watch (empty = all namespaces).
    pub namespace: String,
    /// Label selector string.
    pub label_selector: Option<String>,
    /// Field selector string.
    pub field_selector: Option<String>,
    /// Resync period (already jittered).
    pub resync: Duration,
}

struct Inner {
    store: BTreeMap<String, Arc<Value>>,
    handlers: Vec<mpsc::UnboundedSender<RawEvent>>,
    init_buffer: Option<Vec<(String, Arc<Value>)>>,
}

/// A shared informer for one resource type (and one namespace/selector restriction).
pub struct SharedInformer {
    mapping: RestMapping,
    opts: InformerOptions,
    client: kube::Client,
    inner: Mutex<Inner>,
    synced: watch::Sender<bool>,
    started: AtomicBool,
    stopped: CancellationToken,
}

/// `cache.MetaNamespaceKeyFunc`
pub fn object_key(v: &Value) -> String {
    let (ns, name) = ctrlrt::client::value_key(v);
    if ns.is_empty() {
        name
    } else {
        format!("{ns}/{name}")
    }
}

impl SharedInformer {
    pub fn new(client: kube::Client, mapping: RestMapping, opts: InformerOptions) -> Arc<Self> {
        let (synced, _) = watch::channel(false);
        Arc::new(Self {
            mapping,
            opts,
            client,
            inner: Mutex::new(Inner {
                store: BTreeMap::new(),
                handlers: Vec::new(),
                init_buffer: None,
            }),
            synced,
            started: AtomicBool::new(false),
            stopped: CancellationToken::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn mapping(&self) -> &RestMapping {
        &self.mapping
    }

    pub fn options(&self) -> &InformerOptions {
        &self.opts
    }

    /// `HasSynced()`
    pub fn has_synced(&self) -> bool {
        *self.synced.borrow()
    }

    /// `IsStopped()`
    pub fn is_stopped(&self) -> bool {
        self.stopped.is_cancelled()
    }

    /// Wait until the initial list has been processed. Returns `false` if `cancel` fired or the
    /// informer stopped first.
    pub async fn wait_for_sync(&self, cancel: &CancellationToken) -> bool {
        let mut rx = self.synced.subscribe();
        loop {
            if *rx.borrow_and_update() {
                return true;
            }
            tokio::select! {
                _ = cancel.cancelled() => return false,
                _ = self.stopped.cancelled() => return *rx.borrow(),
                r = rx.changed() => if r.is_err() { return false },
            }
        }
    }

    /// `GetIndexer().GetByKey(key)` (shared reference, callers must clone before mutating).
    pub fn get_by_key(&self, key: &str) -> Option<Arc<Value>> {
        self.lock().store.get(key).cloned()
    }

    /// `GetIndexer().List()` / `ByIndex(NamespaceIndex, ns)` (sorted by key).
    pub fn list(&self, namespace: &str) -> Vec<Arc<Value>> {
        let st = self.lock();
        st.store
            .values()
            .filter(|v| {
                namespace.is_empty() || ctrlrt::client::value_key(v).0 == namespace
            })
            .cloned()
            .collect()
    }

    /// `AddEventHandler(handler)`: returns the stream of events for this handler. If the
    /// informer already holds objects, a `Create` is delivered for each of them first.
    pub fn add_event_handler(&self) -> mpsc::UnboundedReceiver<RawEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut st = self.lock();
        for obj in st.store.values() {
            let _ = tx.send(RawEvent::Create(obj.clone()));
        }
        st.handlers.push(tx);
        rx
    }

    fn dispatch(st: &mut Inner, ev: RawEvent) {
        st.handlers.retain(|h| h.send(ev.clone()).is_ok());
    }

    fn to_value(&self, obj: DynamicObject) -> Option<Arc<Value>> {
        let mut v = serde_json::to_value(obj).ok()?;
        if let Value::Object(m) = &mut v {
            m.insert(
                "apiVersion".into(),
                Value::String(if self.mapping.group.is_empty() {
                    self.mapping.version.clone()
                } else {
                    format!("{}/{}", self.mapping.group, self.mapping.version)
                }),
            );
            m.insert("kind".into(), Value::String(self.mapping.kind.clone()));
        }
        Some(Arc::new(v))
    }

    /// Apply one watcher event to the store and fan it out to the handlers.
    fn process(&self, ev: Event<DynamicObject>) {
        let mut st = self.lock();
        match ev {
            Event::Init => {
                st.init_buffer = Some(Vec::new());
            }
            Event::InitApply(obj) => {
                if let Some(v) = self.to_value(obj) {
                    let key = object_key(&v);
                    st.init_buffer.get_or_insert_with(Vec::new).push((key, v));
                }
            }
            Event::InitDone => {
                // DeltaFIFO.Replace + HandleDeltas
                let buffer = st.init_buffer.take().unwrap_or_default();
                let mut seen = HashSet::with_capacity(buffer.len());
                for (key, new) in buffer {
                    seen.insert(key.clone());
                    let ev = match st.store.insert(key, new.clone()) {
                        Some(old) => RawEvent::Update { old, new },
                        None => RawEvent::Create(new),
                    };
                    Self::dispatch(&mut st, ev);
                }
                let gone: Vec<String> = st
                    .store
                    .keys()
                    .filter(|k| !seen.contains(*k))
                    .cloned()
                    .collect();
                for key in gone {
                    if let Some(old) = st.store.remove(&key) {
                        Self::dispatch(&mut st, RawEvent::Delete(old));
                    }
                }
                drop(st);
                self.synced.send_replace(true);
            }
            Event::Apply(obj) => {
                if let Some(new) = self.to_value(obj) {
                    let key = object_key(&new);
                    let ev = match st.store.insert(key, new.clone()) {
                        Some(old) => RawEvent::Update { old, new },
                        None => RawEvent::Create(new),
                    };
                    Self::dispatch(&mut st, ev);
                }
            }
            Event::Delete(obj) => {
                if let Some(v) = self.to_value(obj) {
                    let key = object_key(&v);
                    st.store.remove(&key);
                    Self::dispatch(&mut st, RawEvent::Delete(v));
                }
            }
        }
    }

    /// Emit a resync (`Update{old: obj, new: obj}` for every cached object).
    pub fn resync(&self) {
        let mut st = self.lock();
        let objs: Vec<Arc<Value>> = st.store.values().cloned().collect();
        for o in objs {
            Self::dispatch(
                &mut st,
                RawEvent::Update {
                    old: o.clone(),
                    new: o,
                },
            );
        }
    }

    /// Test helper: feed synthetic watch events (same processing as the reflector).
    #[doc(hidden)]
    pub fn inject(&self, ev: Event<DynamicObject>) {
        self.process(ev)
    }

    /// `Run(ctx)`: start list/watch until `stop` is cancelled. Idempotent.
    pub fn run(self: &Arc<Self>, stop: CancellationToken) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            this.run_loop(stop).await;
            this.stopped.cancel();
        });
    }

    async fn run_loop(&self, stop: CancellationToken) {
        let ar = self.mapping.resource_info().api_resource();
        let api: Api<DynamicObject> = if self.mapping.namespaced && !self.opts.namespace.is_empty() {
            Api::namespaced_with(self.client.clone(), &self.opts.namespace, &ar)
        } else {
            Api::all_with(self.client.clone(), &ar)
        };
        let mut cfg = watcher::Config::default().any_semantic();
        if let Some(l) = self.opts.label_selector.as_deref().filter(|s| !s.is_empty()) {
            cfg = cfg.labels(l);
        }
        if let Some(f) = self.opts.field_selector.as_deref().filter(|s| !s.is_empty()) {
            cfg = cfg.fields(f);
        }
        let stream = watcher::watcher(api, cfg).default_backoff();
        tokio::pin!(stream);
        let resync = self.opts.resync;
        let mut resync_timer = tokio::time::interval_at(
            tokio::time::Instant::now() + resync.max(Duration::from_secs(1)),
            resync.max(Duration::from_secs(1)),
        );
        let type_name = go_type_name(&self.mapping.resource_info());
        loop {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = resync_timer.tick(), if !resync.is_zero() => {
                    if self.has_synced() {
                        self.resync();
                    }
                }
                item = stream.next() => {
                    match item {
                        None => return,
                        Some(Ok(ev)) => self.process(ev),
                        Some(Err(e)) => {
                            let msg = match &e {
                                watcher::Error::InitialListFailed(k) => {
                                    format!("failed to list {}: {}", type_name, kube_error_text(k))
                                }
                                watcher::Error::WatchStartFailed(k) | watcher::Error::WatchFailed(k) => {
                                    format!("failed to watch {}: {}", type_name, kube_error_text(k))
                                }
                                other => other.to_string(),
                            };
                            klog::error(
                                "reflector.go:205",
                                &msg,
                                "Failed to watch",
                                &[
                                    ("logger", Value::String("UnhandledError".into())),
                                    ("type", Value::String(type_name.clone())),
                                ],
                            );
                        }
                    }
                }
            }
        }
    }
}

fn kube_error_text(e: &kube::Error) -> String {
    match e {
        kube::Error::Api(s) => s.message.clone(),
        other => other.to_string(),
    }
}

/// `calculateResyncPeriod`: jitter the resync period by ±10%.
pub fn calculate_resync_period(resync: Duration) -> Duration {
    let factor = fastrand::f64() / 5.0 + 0.9;
    Duration::from_nanos((resync.as_nanos() as f64 * factor) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(ns: &str, name: &str, rv: &str) -> DynamicObject {
        serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {"namespace": ns, "name": name, "resourceVersion": rv},
            "data": {"k": rv}
        }))
        .unwrap()
    }

    fn informer() -> Arc<SharedInformer> {
        let client = kube::Client::new(
            tower::service_fn(|_req: http::Request<kube::client::Body>| async {
                Err::<http::Response<kube::client::Body>, std::io::Error>(std::io::Error::other(
                    "offline",
                ))
            }),
            "default",
        );
        SharedInformer::new(
            client,
            RestMapping {
                group: String::new(),
                version: "v1".into(),
                kind: "ConfigMap".into(),
                resource: "configmaps".into(),
                namespaced: true,
            },
            InformerOptions::default(),
        )
    }

    fn describe(ev: &RawEvent) -> String {
        let n = |v: &Value| {
            format!(
                "{}@{}",
                object_key(v),
                v.pointer("/metadata/resourceVersion").and_then(Value::as_str).unwrap_or("")
            )
        };
        match ev {
            RawEvent::Create(o) => format!("create {}", n(o)),
            RawEvent::Update { old, new } => format!("update {} -> {}", n(old), n(new)),
            RawEvent::Delete(o) => format!("delete {}", n(o)),
            RawEvent::Generic(o) => format!("generic {}", n(o)),
        }
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<RawEvent>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(describe(&ev));
        }
        out
    }

    #[tokio::test]
    async fn initial_list_watch_relist_and_late_subscriber() {
        let inf = informer();
        let mut early = inf.add_event_handler();
        assert!(!inf.has_synced());

        inf.inject(Event::Init);
        inf.inject(Event::InitApply(obj("a", "x", "1")));
        inf.inject(Event::InitApply(obj("a", "y", "1")));
        assert!(drain(&mut early).is_empty(), "nothing is delivered before the list completes");
        inf.inject(Event::InitDone);
        assert!(inf.has_synced());
        assert_eq!(drain(&mut early), vec!["create a/x@1", "create a/y@1"]);

        // stored objects carry apiVersion/kind (cache reader GVK behaviour)
        let x = inf.get_by_key("a/x").unwrap();
        assert_eq!(x["kind"], "ConfigMap");
        assert_eq!(x["apiVersion"], "v1");

        // watch events
        inf.inject(Event::Apply(obj("a", "x", "2")));
        inf.inject(Event::Apply(obj("b", "z", "3")));
        inf.inject(Event::Delete(obj("a", "y", "4")));
        assert_eq!(
            drain(&mut early),
            vec!["update a/x@1 -> a/x@2", "create b/z@3", "delete a/y@4"]
        );

        // late subscriber gets the current state replayed as creates
        let mut late = inf.add_event_handler();
        assert_eq!(drain(&mut late), vec!["create a/x@2", "create b/z@3"]);

        // relist: known objects -> update (even unchanged), new -> create, missing -> delete
        inf.inject(Event::Init);
        inf.inject(Event::InitApply(obj("a", "x", "2")));
        inf.inject(Event::InitApply(obj("c", "w", "5")));
        inf.inject(Event::InitDone);
        let want = vec!["update a/x@2 -> a/x@2", "create c/w@5", "delete b/z@3"];
        assert_eq!(drain(&mut early), want);
        assert_eq!(drain(&mut late), want);

        // resync
        inf.resync();
        assert_eq!(
            drain(&mut early),
            vec!["update a/x@2 -> a/x@2", "update c/w@5 -> c/w@5"]
        );

        assert_eq!(inf.list("").len(), 2);
        assert_eq!(inf.list("c").len(), 1);
        assert_eq!(inf.list("zzz").len(), 0);
    }

    #[test]
    fn resync_period_is_jittered_within_ten_percent() {
        for _ in 0..100 {
            let d = calculate_resync_period(Duration::from_secs(36000));
            assert!(d >= Duration::from_secs(32400) && d < Duration::from_secs(39600));
        }
    }
}
