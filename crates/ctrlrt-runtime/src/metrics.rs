//! controller-runtime / client-go Prometheus metrics (`sigs.k8s.io/controller-runtime/pkg/metrics`,
//! `pkg/internal/controller/metrics`).
//!
//! All metrics are registered in the `prometheus` crate's default registry (the analogue of
//! controller-runtime's `metrics.Registry`), with the same names, help texts, labels and
//! buckets as the Go implementation. The metrics HTTP server (crate `ctrlrt-cmd`) serves
//! [`registry()`].

use std::sync::LazyLock;

use prometheus::{
    CounterVec, GaugeVec, HistogramOpts, HistogramVec, Opts, Registry, exponential_buckets,
};

/// controller-runtime's `metrics.Registry`.
pub fn registry() -> &'static Registry {
    register_all();
    prometheus::default_registry()
}

fn reg<C: prometheus::core::Collector + Clone + 'static>(c: C) -> C {
    // Registration only fails for duplicates, which cannot happen for these statics.
    let _ = prometheus::default_registry().register(Box::new(c.clone()));
    c
}

fn counter_vec(name: &str, help: &str, labels: &[&str]) -> CounterVec {
    reg(CounterVec::new(Opts::new(name, help), labels).expect("valid metric"))
}

fn gauge_vec(name: &str, help: &str, labels: &[&str]) -> GaugeVec {
    reg(GaugeVec::new(Opts::new(name, help), labels).expect("valid metric"))
}

// ---------------------------------------------------------------------------------------------
// pkg/internal/controller/metrics
// ---------------------------------------------------------------------------------------------

pub static RECONCILE_TOTAL: LazyLock<CounterVec> = LazyLock::new(|| {
    counter_vec(
        "controller_runtime_reconcile_total",
        "Total number of reconciliations per controller",
        &["controller", "result"],
    )
});

pub static RECONCILE_ERRORS: LazyLock<CounterVec> = LazyLock::new(|| {
    counter_vec(
        "controller_runtime_reconcile_errors_total",
        "Total number of reconciliation errors per controller",
        &["controller"],
    )
});

pub static TERMINAL_RECONCILE_ERRORS: LazyLock<CounterVec> = LazyLock::new(|| {
    counter_vec(
        "controller_runtime_terminal_reconcile_errors_total",
        "Total number of terminal reconciliation errors per controller",
        &["controller"],
    )
});

pub static RECONCILE_PANICS: LazyLock<CounterVec> = LazyLock::new(|| {
    counter_vec(
        "controller_runtime_reconcile_panics_total",
        "Total number of reconciliation panics per controller",
        &["controller"],
    )
});

pub static RECONCILE_TIME: LazyLock<HistogramVec> = LazyLock::new(|| {
    reg(HistogramVec::new(
        HistogramOpts::new(
            "controller_runtime_reconcile_time_seconds",
            "Length of time per reconciliation per controller",
        )
        .buckets(vec![
            0.005, 0.01, 0.025, 0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.6, 0.7,
            0.8, 0.9, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 6.0, 7.0, 8.0, 9.0,
            10.0, 15.0, 20.0, 25.0, 30.0, 40.0, 50.0, 60.0,
        ]),
        &["controller"],
    )
    .expect("valid metric"))
});

pub static WORKER_COUNT: LazyLock<GaugeVec> = LazyLock::new(|| {
    gauge_vec(
        "controller_runtime_max_concurrent_reconciles",
        "Maximum number of concurrent reconciles per controller",
        &["controller"],
    )
});

pub static ACTIVE_WORKERS: LazyLock<GaugeVec> = LazyLock::new(|| {
    gauge_vec(
        "controller_runtime_active_workers",
        "Number of currently used workers per controller",
        &["controller"],
    )
});

// ---------------------------------------------------------------------------------------------
// pkg/metrics/workqueue (internal/metrics/workqueue.go)
// ---------------------------------------------------------------------------------------------

pub static WQ_DEPTH: LazyLock<GaugeVec> = LazyLock::new(|| {
    gauge_vec(
        "workqueue_depth",
        "Current depth of workqueue by workqueue and priority",
        &["name", "controller", "priority"],
    )
});

pub static WQ_ADDS: LazyLock<CounterVec> = LazyLock::new(|| {
    counter_vec(
        "workqueue_adds_total",
        "Total number of adds handled by workqueue",
        &["name", "controller"],
    )
});

pub static WQ_LATENCY: LazyLock<HistogramVec> = LazyLock::new(|| {
    reg(HistogramVec::new(
        HistogramOpts::new(
            "workqueue_queue_duration_seconds",
            "How long in seconds an item stays in workqueue before being requested",
        )
        .buckets(exponential_buckets(10e-9, 10.0, 12).expect("valid buckets")),
        &["name", "controller"],
    )
    .expect("valid metric"))
});

pub static WQ_WORK_DURATION: LazyLock<HistogramVec> = LazyLock::new(|| {
    reg(HistogramVec::new(
        HistogramOpts::new(
            "workqueue_work_duration_seconds",
            "How long in seconds processing an item from workqueue takes.",
        )
        .buckets(exponential_buckets(10e-9, 10.0, 12).expect("valid buckets")),
        &["name", "controller"],
    )
    .expect("valid metric"))
});

pub static WQ_UNFINISHED: LazyLock<GaugeVec> = LazyLock::new(|| {
    gauge_vec(
        "workqueue_unfinished_work_seconds",
        "How many seconds of work has been done that is in progress and hasn't been observed by work_duration. Large values indicate stuck threads. One can deduce the number of stuck threads by observing the rate at which this increases.",
        &["name", "controller"],
    )
});

pub static WQ_LONGEST_RUNNING: LazyLock<GaugeVec> = LazyLock::new(|| {
    gauge_vec(
        "workqueue_longest_running_processor_seconds",
        "How many seconds has the longest running processor for workqueue been running.",
        &["name", "controller"],
    )
});

pub static WQ_RETRIES: LazyLock<CounterVec> = LazyLock::new(|| {
    counter_vec(
        "workqueue_retries_total",
        "Total number of retries handled by workqueue",
        &["name", "controller"],
    )
});

// ---------------------------------------------------------------------------------------------
// pkg/metrics/leaderelection.go
// ---------------------------------------------------------------------------------------------

pub static LEADER_GAUGE: LazyLock<GaugeVec> = LazyLock::new(|| {
    gauge_vec(
        "leader_election_master_status",
        "Gauge of if the reporting system is master of the relevant lease, 0 indicates backup, 1 indicates master. 'name' is the string used to identify the lease. Please make sure to group by name.",
        &["name"],
    )
});

/// Not registered in Go either (only the gauge is registered), kept for the adapter.
pub static LEADER_SLOWPATH: LazyLock<CounterVec> = LazyLock::new(|| {
    CounterVec::new(
        Opts::new(
            "leader_election_slowpath_total",
            "Total number of slow path exercised in renewing leader leases. 'name' is the string used to identify the lease. Please make sure to group by name.",
        ),
        &["name"],
    )
    .expect("valid metric")
});

// ---------------------------------------------------------------------------------------------
// pkg/metrics/client_go_adapter.go
// ---------------------------------------------------------------------------------------------

pub static REST_CLIENT_REQUESTS: LazyLock<CounterVec> = LazyLock::new(|| {
    counter_vec(
        "rest_client_requests_total",
        "Number of HTTP requests, partitioned by status code, method, and host.",
        &["code", "method", "host"],
    )
});

static PROCESS: LazyLock<()> = LazyLock::new(|| {
    #[cfg(target_os = "linux")]
    {
        let _ = prometheus::default_registry()
            .register(Box::new(prometheus::process_collector::ProcessCollector::for_self()));
    }
});

/// Register every metric (Go registers them in `init()`).
pub fn register_all() {
    LazyLock::force(&RECONCILE_TOTAL);
    LazyLock::force(&RECONCILE_ERRORS);
    LazyLock::force(&TERMINAL_RECONCILE_ERRORS);
    LazyLock::force(&RECONCILE_PANICS);
    LazyLock::force(&RECONCILE_TIME);
    LazyLock::force(&WORKER_COUNT);
    LazyLock::force(&ACTIVE_WORKERS);
    LazyLock::force(&WQ_DEPTH);
    LazyLock::force(&WQ_ADDS);
    LazyLock::force(&WQ_LATENCY);
    LazyLock::force(&WQ_WORK_DURATION);
    LazyLock::force(&WQ_UNFINISHED);
    LazyLock::force(&WQ_LONGEST_RUNNING);
    LazyLock::force(&WQ_RETRIES);
    LazyLock::force(&LEADER_GAUGE);
    LazyLock::force(&REST_CLIENT_REQUESTS);
    LazyLock::force(&PROCESS);
}

/// Text exposition of the registry (convenience for tests and simple servers).
pub fn gather_text() -> String {
    use prometheus::Encoder;
    let mut buf = Vec::new();
    let _ = prometheus::TextEncoder::new().encode(&registry().gather(), &mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}
