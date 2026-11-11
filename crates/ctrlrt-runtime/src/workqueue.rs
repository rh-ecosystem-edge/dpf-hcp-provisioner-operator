//! Port of `k8s.io/client-go/util/workqueue`: the de-duplicating FIFO queue (`Typed`), the
//! delaying queue (`AddAfter`) and the rate-limiting queue (`AddRateLimited`/`Forget`), including
//! the Prometheus metrics controller-runtime registers for them.
//!
//! Semantics (identical to client-go):
//! * an item is processed by at most one worker at a time (`processing` set);
//! * adding an item that is already queued is a no-op (`dirty` set);
//! * adding an item that is being processed marks it dirty, it is re-queued on `done`;
//! * `add_after` keeps only the earliest pending time per item.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prometheus::{Counter, Gauge, Histogram};
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::metrics;
use crate::ratelimit::RateLimiter;

/// Item bound for queues.
pub trait QueueItem: Eq + Hash + Clone + Send + Sync + 'static {}
impl<T: Eq + Hash + Clone + Send + Sync + 'static> QueueItem for T {}

struct QueueMetrics<T> {
    depth: Gauge,
    adds: Counter,
    latency: Histogram,
    work_duration: Histogram,
    unfinished: Gauge,
    longest_running: Gauge,
    add_times: HashMap<T, Instant>,
    processing_start_times: HashMap<T, Instant>,
}

impl<T: QueueItem> QueueMetrics<T> {
    fn new(name: &str) -> Self {
        Self {
            depth: metrics::WQ_DEPTH.with_label_values(&[name, name, ""]),
            adds: metrics::WQ_ADDS.with_label_values(&[name, name]),
            latency: metrics::WQ_LATENCY.with_label_values(&[name, name]),
            work_duration: metrics::WQ_WORK_DURATION.with_label_values(&[name, name]),
            unfinished: metrics::WQ_UNFINISHED.with_label_values(&[name, name]),
            longest_running: metrics::WQ_LONGEST_RUNNING.with_label_values(&[name, name]),
            add_times: HashMap::new(),
            processing_start_times: HashMap::new(),
        }
    }
    fn add(&mut self, item: &T) {
        self.adds.inc();
        self.depth.inc();
        self.add_times
            .entry(item.clone())
            .or_insert_with(Instant::now);
    }
    fn get(&mut self, item: &T) {
        self.depth.dec();
        self.processing_start_times
            .insert(item.clone(), Instant::now());
        if let Some(start) = self.add_times.remove(item) {
            self.latency.observe(start.elapsed().as_secs_f64());
        }
    }
    fn done(&mut self, item: &T) {
        if let Some(start) = self.processing_start_times.remove(item) {
            self.work_duration.observe(start.elapsed().as_secs_f64());
        }
    }
    fn update_unfinished_work(&self) {
        let mut total = 0.0;
        let mut oldest = 0.0f64;
        for t in self.processing_start_times.values() {
            let age = t.elapsed().as_secs_f64();
            total += age;
            if age > oldest {
                oldest = age;
            }
        }
        self.unfinished.set(total);
        self.longest_running.set(oldest);
    }
}

struct QueueState<T> {
    queue: VecDeque<T>,
    dirty: HashSet<T>,
    processing: HashSet<T>,
    shutting_down: bool,
    drain: bool,
    metrics: Option<QueueMetrics<T>>,
}

/// `workqueue.Typed[T]`: a de-duplicating work queue.
pub struct Queue<T: QueueItem> {
    state: Mutex<QueueState<T>>,
    cond: Notify,
    stop: CancellationToken,
}

impl<T: QueueItem> Queue<T> {
    /// `workqueue.NewTypedWithConfig(TypedQueueConfig{Name: name})`. Metrics are recorded when
    /// `name` is not empty (requires a Tokio runtime for the unfinished-work updater).
    pub fn new(name: &str) -> Arc<Self> {
        let q = Arc::new(Self {
            state: Mutex::new(QueueState {
                queue: VecDeque::new(),
                dirty: HashSet::new(),
                processing: HashSet::new(),
                shutting_down: false,
                drain: false,
                metrics: (!name.is_empty()).then(|| QueueMetrics::new(name)),
            }),
            cond: Notify::new(),
            stop: CancellationToken::new(),
        });
        if !name.is_empty() {
            let weak = Arc::downgrade(&q);
            let stop = q.stop.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_millis(500));
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => return,
                        _ = tick.tick() => {
                            let Some(q) = weak.upgrade() else { return };
                            let st = q.lock();
                            if !st.shutting_down
                                && let Some(m) = &st.metrics
                            {
                                m.update_unfinished_work();
                            }
                        }
                    }
                }
            });
        }
        q
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState<T>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `Add(item)`
    pub fn add(&self, item: T) {
        let mut st = self.lock();
        if st.shutting_down {
            return;
        }
        if st.dirty.contains(&item) {
            // the same item is already queued (or being processed and dirty): Touch is a no-op
            // for the default FIFO queue.
            return;
        }
        if let Some(m) = st.metrics.as_mut() {
            m.add(&item);
        }
        st.dirty.insert(item.clone());
        if st.processing.contains(&item) {
            return;
        }
        st.queue.push_back(item);
        drop(st);
        self.cond.notify_one();
    }

    /// `Len()`
    pub fn len(&self) -> usize {
        self.lock().queue.len()
    }

    /// `Len() == 0`
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `Get()`: blocks until an item is available. Returns `None` once the queue is shutting
    /// down and empty.
    pub async fn get(&self) -> Option<T> {
        loop {
            let notified = self.cond.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut st = self.lock();
                if let Some(item) = st.queue.pop_front() {
                    if let Some(m) = st.metrics.as_mut() {
                        m.get(&item);
                    }
                    st.processing.insert(item.clone());
                    st.dirty.remove(&item);
                    // Wake another waiter if more items are queued (Notify permits are not
                    // counted like a condition variable signal).
                    if !st.queue.is_empty() {
                        self.cond.notify_one();
                    }
                    return Some(item);
                }
                if st.shutting_down {
                    // propagate the broadcast to other waiters
                    self.cond.notify_one();
                    return None;
                }
            }
            notified.await;
        }
    }

    /// Non-blocking `Get` used by tests: `None` if nothing is queued.
    pub fn try_get(&self) -> Option<T> {
        let mut st = self.lock();
        let item = st.queue.pop_front()?;
        if let Some(m) = st.metrics.as_mut() {
            m.get(&item);
        }
        st.processing.insert(item.clone());
        st.dirty.remove(&item);
        Some(item)
    }

    /// `Done(item)`
    pub fn done(&self, item: &T) {
        let mut st = self.lock();
        if let Some(m) = st.metrics.as_mut() {
            m.done(item);
        }
        st.processing.remove(item);
        if st.dirty.contains(item) {
            st.queue.push_back(item.clone());
            drop(st);
            self.cond.notify_one();
        } else if st.processing.is_empty() {
            drop(st);
            self.cond.notify_waiters();
            self.cond.notify_one();
        }
    }

    /// `ShutDown()`: new items are ignored, `get` returns `None` once drained.
    pub fn shut_down(&self) {
        self.stop.cancel();
        let mut st = self.lock();
        st.drain = false;
        st.shutting_down = true;
        drop(st);
        self.cond.notify_waiters();
        self.cond.notify_one();
    }

    /// `ShutDownWithDrain()`: like `shut_down` but waits for all in-flight items to be done.
    pub async fn shut_down_with_drain(&self) {
        self.stop.cancel();
        {
            let mut st = self.lock();
            st.drain = true;
            st.shutting_down = true;
        }
        self.cond.notify_waiters();
        loop {
            let notified = self.cond.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let st = self.lock();
                if st.processing.is_empty() || !st.drain {
                    return;
                }
            }
            notified.await;
        }
    }

    /// `ShuttingDown()`
    pub fn shutting_down(&self) -> bool {
        self.lock().shutting_down
    }
}

impl<T: QueueItem> Drop for Queue<T> {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

struct WaitingSet<T> {
    by_time: BTreeMap<(Instant, u64), T>,
    by_item: HashMap<T, (Instant, u64)>,
    seq: u64,
}

impl<T: QueueItem> WaitingSet<T> {
    fn insert(&mut self, item: T, ready_at: Instant) {
        // if the entry already exists, update the time only if it would cause the item to be
        // queued sooner
        if let Some(existing) = self.by_item.get(&item).copied() {
            if existing.0 > ready_at {
                self.by_time.remove(&existing);
                self.seq += 1;
                let key = (ready_at, self.seq);
                self.by_time.insert(key, item.clone());
                self.by_item.insert(item, key);
            }
            return;
        }
        self.seq += 1;
        let key = (ready_at, self.seq);
        self.by_time.insert(key, item.clone());
        self.by_item.insert(item, key);
    }
}

/// `workqueue.TypedDelayingInterface` + `TypedRateLimitingInterface`.
pub struct RateLimitingQueue<T: QueueItem> {
    queue: Arc<Queue<T>>,
    rate_limiter: Box<dyn RateLimiter<T>>,
    waiting_tx: mpsc::UnboundedSender<(T, Instant)>,
    stop: CancellationToken,
    retries: Option<Counter>,
}

impl<T: QueueItem> RateLimitingQueue<T> {
    /// `workqueue.NewTypedRateLimitingQueueWithConfig(rateLimiter, {Name: name})`.
    /// Must be called within a Tokio runtime.
    pub fn new(name: &str, rate_limiter: Box<dyn RateLimiter<T>>) -> Arc<Self> {
        let queue = Queue::new(name);
        let (tx, rx) = mpsc::unbounded_channel();
        let stop = CancellationToken::new();
        tokio::spawn(waiting_loop(queue.clone(), rx, stop.clone()));
        Arc::new(Self {
            queue,
            rate_limiter,
            waiting_tx: tx,
            stop,
            retries: (!name.is_empty())
                .then(|| metrics::WQ_RETRIES.with_label_values(&[name, name])),
        })
    }

    pub fn add(&self, item: T) {
        self.queue.add(item)
    }
    pub async fn get(&self) -> Option<T> {
        self.queue.get().await
    }
    pub fn try_get(&self) -> Option<T> {
        self.queue.try_get()
    }
    pub fn done(&self, item: &T) {
        self.queue.done(item)
    }
    pub fn len(&self) -> usize {
        self.queue.len()
    }
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    pub fn shutting_down(&self) -> bool {
        self.queue.shutting_down()
    }

    /// `ShutDown()`
    pub fn shut_down(&self) {
        self.queue.shut_down();
        self.stop.cancel();
    }

    /// `ShutDownWithDrain()`
    pub async fn shut_down_with_drain(&self) {
        self.stop.cancel();
        self.queue.shut_down_with_drain().await;
    }

    /// `AddAfter(item, duration)`
    pub fn add_after(&self, item: T, duration: Duration) {
        // don't add if we're already shutting down
        if self.queue.shutting_down() {
            return;
        }
        if let Some(r) = &self.retries {
            r.inc();
        }
        // immediately add things with no delay
        if duration.is_zero() {
            self.queue.add(item);
            return;
        }
        if self.stop.is_cancelled() {
            return;
        }
        let ready_at = Instant::now()
            .checked_add(duration)
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400 * 365 * 30));
        let _ = self.waiting_tx.send((item, ready_at));
    }

    /// `AddRateLimited(item)`
    pub fn add_rate_limited(&self, item: T) {
        let d = self.rate_limiter.when(&item);
        self.add_after(item, d);
    }

    /// `Forget(item)`
    pub fn forget(&self, item: &T) {
        self.rate_limiter.forget(item)
    }

    /// `NumRequeues(item)`
    pub fn num_requeues(&self, item: &T) -> i32 {
        self.rate_limiter.num_requeues(item)
    }
}

impl<T: QueueItem> Drop for RateLimitingQueue<T> {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn waiting_loop<T: QueueItem>(
    queue: Arc<Queue<T>>,
    mut rx: mpsc::UnboundedReceiver<(T, Instant)>,
    stop: CancellationToken,
) {
    let mut waiting = WaitingSet {
        by_time: BTreeMap::new(),
        by_item: HashMap::new(),
        seq: 0,
    };
    loop {
        if queue.shutting_down() {
            return;
        }
        let now = Instant::now();
        // Add ready entries
        while let Some((&key, _)) = waiting.by_time.iter().next() {
            if key.0 > now {
                break;
            }
            if let Some(item) = waiting.by_time.remove(&key) {
                waiting.by_item.remove(&item);
                queue.add(item);
            }
        }
        let next_ready_at = waiting.by_time.keys().next().map(|k| k.0);
        let sleep = async {
            match next_ready_at {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = stop.cancelled() => return,
            // heartbeat: continue the loop at least every 10s, like client-go
            _ = tokio::time::sleep(Duration::from_secs(10)) => {}
            _ = sleep => {}
            entry = rx.recv() => {
                let Some((item, ready_at)) = entry else { return };
                let mut handle = |item: T, ready_at: Instant| {
                    if ready_at > Instant::now() {
                        waiting.insert(item, ready_at);
                    } else {
                        queue.add(item);
                    }
                };
                handle(item, ready_at);
                while let Ok((item, ready_at)) = rx.try_recv() {
                    handle(item, ready_at);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratelimit::{ItemExponentialFailureRateLimiter, default_controller_rate_limiter};

    // client-go queue_test.go: TestBasic (adapted to tasks)
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_basic() {
        let q: Arc<Queue<i32>> = Queue::new("");
        let producers = 50;
        let mut handles = Vec::new();
        for i in 0..producers {
            let q = q.clone();
            handles.push(tokio::spawn(async move {
                for j in 0..50 {
                    q.add(i);
                    tokio::time::sleep(Duration::from_micros(j)).await;
                }
            }));
        }
        let consumers = 10;
        let mut consumer_handles = Vec::new();
        for _ in 0..consumers {
            let q = q.clone();
            consumer_handles.push(tokio::spawn(async move {
                let mut n = 0;
                while let Some(i) = q.get().await {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    q.done(&i);
                    n += 1;
                }
                n
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        q.shut_down();
        q.add(0);
        for h in consumer_handles {
            h.await.unwrap();
        }
        assert_eq!(q.len(), 0, "Expected the queue to be empty");
    }

    // client-go: TestAddWhileProcessing
    #[tokio::test]
    async fn test_add_while_processing() {
        let q: Arc<Queue<i32>> = Queue::new("");
        q.add(1);
        let i = q.get().await.unwrap();
        // re-add while processing: not handed out again until done
        q.add(i);
        assert_eq!(q.len(), 0);
        assert!(q.try_get().is_none());
        q.done(&i);
        assert_eq!(q.len(), 1);
        assert_eq!(q.get().await, Some(1));
        q.done(&1);
        assert_eq!(q.len(), 0);
    }

    // client-go: TestLen
    #[tokio::test]
    async fn test_len() {
        let q: Arc<Queue<&str>> = Queue::new("");
        q.add("foo");
        assert_eq!(q.len(), 1);
        q.add("bar");
        assert_eq!(q.len(), 2);
        q.add("foo"); // should not increase the queue length.
        assert_eq!(q.len(), 2);
    }

    // client-go: TestReinsert
    #[tokio::test]
    async fn test_reinsert() {
        let q: Arc<Queue<&str>> = Queue::new("");
        q.add("foo");
        // Start processing
        let i = q.get().await.unwrap();
        assert_eq!(i, "foo");
        // Add it back while processing
        q.add(i);
        // Finish it up
        q.done(&i);
        // It should be back on the queue
        let i = q.get().await.unwrap();
        assert_eq!(i, "foo");
        // Finish that one up
        q.done(&i);
        assert_eq!(q.len(), 0);
    }

    // client-go: TestQueueDrainageUsingShutDownWithDrain
    #[tokio::test]
    async fn test_queue_drainage_using_shut_down_with_drain() {
        let q: Arc<Queue<&str>> = Queue::new("");
        q.add("foo");
        q.add("bar");
        let first = q.get().await.unwrap();
        let second = q.get().await.unwrap();
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f2 = finished.clone();
        let q2 = q.clone();
        let h = tokio::spawn(async move {
            q2.shut_down_with_drain().await;
            f2.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!finished.load(std::sync::atomic::Ordering::SeqCst));
        q.done(&first);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!finished.load(std::sync::atomic::Ordering::SeqCst));
        q.done(&second);
        h.await.unwrap();
        assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
    }

    // client-go: TestForceQueueShutdownUsingShutDown
    #[tokio::test]
    async fn test_force_queue_shutdown_using_shut_down() {
        let q: Arc<Queue<&str>> = Queue::new("");
        q.add("foo");
        q.add("bar");
        let _ = q.get().await;
        let _ = q.get().await;
        q.shut_down();
        // get returns immediately with shutdown
        assert_eq!(q.get().await, None);
        q.add("baz");
        assert_eq!(q.len(), 0);
    }

    // client-go delaying_queue_test.go: TestSimpleQueue
    #[tokio::test(start_paused = true)]
    async fn test_simple_delaying_queue() {
        let q: Arc<RateLimitingQueue<&str>> = RateLimitingQueue::new(
            "",
            Box::new(ItemExponentialFailureRateLimiter::new(
                Duration::from_millis(1),
                Duration::from_secs(1),
            )),
        );
        q.add_after("foo", Duration::from_millis(50));
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(q.len(), 0);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(q.len(), 1);
        let item = q.get().await.unwrap();
        q.done(&item);
        assert_eq!(item, "foo");
        assert_eq!(q.len(), 0);
    }

    // client-go: TestDeduping
    #[tokio::test(start_paused = true)]
    async fn test_deduping() {
        let q: Arc<RateLimitingQueue<&str>> = RateLimitingQueue::new(
            "",
            Box::new(ItemExponentialFailureRateLimiter::new(
                Duration::from_millis(1),
                Duration::from_secs(1),
            )),
        );
        q.add_after("foo", Duration::from_millis(50));
        q.add_after("foo", Duration::from_millis(70));
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(q.len(), 0);
        // the first add wins (earliest)
        tokio::time::sleep(Duration::from_millis(55)).await;
        assert_eq!(q.len(), 1);
        let item = q.get().await.unwrap();
        q.done(&item);
        // the second add was folded into the first one
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(q.len(), 0);

        // test again, but this time the earlier should override
        q.add_after("foo", Duration::from_millis(70));
        q.add_after("foo", Duration::from_millis(50));
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(q.len(), 0);
        tokio::time::sleep(Duration::from_millis(55)).await;
        assert_eq!(q.len(), 1);
        let item = q.get().await.unwrap();
        q.done(&item);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(q.len(), 0);
    }

    // client-go: TestAddTwoFireEarly
    #[tokio::test(start_paused = true)]
    async fn test_add_two_fire_early() {
        let q: Arc<RateLimitingQueue<&str>> = RateLimitingQueue::new(
            "",
            Box::new(ItemExponentialFailureRateLimiter::new(
                Duration::from_millis(1),
                Duration::from_secs(1),
            )),
        );
        q.add_after("foo", Duration::from_secs(1));
        q.add_after("bar", Duration::from_millis(50));
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(q.len(), 0);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(q.len(), 1);
        assert_eq!(q.get().await, Some("bar"));
        q.add_after("foo", Duration::from_millis(50));
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(q.len(), 1);
        assert_eq!(q.get().await, Some("foo"));
    }

    // client-go: TestCopyShifting
    #[tokio::test(start_paused = true)]
    async fn test_copy_shifting() {
        let q: Arc<RateLimitingQueue<&str>> = RateLimitingQueue::new(
            "",
            Box::new(ItemExponentialFailureRateLimiter::new(
                Duration::from_millis(1),
                Duration::from_secs(1),
            )),
        );
        q.add_after("foo", Duration::from_secs(1));
        q.add_after("bar", Duration::from_millis(500));
        q.add_after("zed", Duration::from_millis(250));
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(q.len(), 0);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(q.len(), 3);
        assert_eq!(q.get().await, Some("zed"));
        assert_eq!(q.get().await, Some("bar"));
        assert_eq!(q.get().await, Some("foo"));
    }

    // client-go rate_limiting_queue_test.go: TestRateLimitingQueue
    #[tokio::test(start_paused = true)]
    async fn test_rate_limiting_queue() {
        let q: Arc<RateLimitingQueue<&str>> = RateLimitingQueue::new(
            "",
            Box::new(ItemExponentialFailureRateLimiter::new(
                Duration::from_millis(1),
                Duration::from_secs(1),
            )),
        );
        q.add_rate_limited("one");
        q.add_rate_limited("one");
        q.add_rate_limited("two");
        assert_eq!(q.num_requeues(&"one"), 2);
        assert_eq!(q.num_requeues(&"two"), 1);
        q.forget(&"one");
        assert_eq!(q.num_requeues(&"one"), 0);
        q.add_rate_limited("one");
        assert_eq!(q.num_requeues(&"one"), 1);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(q.len(), 2);
    }

    #[tokio::test]
    async fn named_queue_records_metrics() {
        let q: Arc<RateLimitingQueue<String>> = RateLimitingQueue::new(
            "wq-metrics-test",
            Box::new(default_controller_rate_limiter()),
        );
        q.add("a".into());
        q.add("a".into());
        let item = q.get().await.unwrap();
        q.done(&item);
        q.add_after("b".into(), Duration::ZERO);
        let text = metrics::gather_text();
        assert!(text.contains(
            r#"workqueue_adds_total{controller="wq-metrics-test",name="wq-metrics-test"} 2"#
        ));
        assert!(text.contains(
            r#"workqueue_depth{controller="wq-metrics-test",name="wq-metrics-test",priority=""} 1"#
        ));
        assert!(text.contains(
            r#"workqueue_retries_total{controller="wq-metrics-test",name="wq-metrics-test"} 1"#
        ));
        q.shut_down();
    }
}
