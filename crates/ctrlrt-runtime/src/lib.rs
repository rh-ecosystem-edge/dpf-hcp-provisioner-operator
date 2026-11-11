//! `ctrlrt-runtime` — the runtime half of controller-runtime v0.22 on top of `kube`:
//!
//! * [`client::KubeBackend`]: direct API client (`ctrlrt::ClientBackend`) with client-go error
//!   semantics and a lazy discovery RESTMapper ([`client::RestMapper`]).
//! * [`config`]: `ctrl.GetConfigOrDie()`, user agent, rate limiting, hosted-cluster clients.
//! * [`cache`]: informer cache (`ByObject`, multi-namespace) and the cache-backed client.
//! * [`workqueue`] / [`ratelimit`]: client-go's work queue and rate limiters.
//! * [`controller`]: the controller loop (`reconcileHandler` semantics, metrics, logging).
//! * [`manager`]: the manager (runnable groups, leader election, health probes, shutdown).
//! * [`metrics`]: controller-runtime's Prometheus metrics in the default registry.

pub mod cache;
pub mod client;
pub mod config;
pub mod controller;
pub mod healthz;
pub mod informer;
pub mod klog;
pub mod leaderelection;
pub mod manager;
pub mod metrics;
pub mod ratelimit;
pub mod signals;
pub mod util;
pub mod workqueue;

pub use cache::{ALL_NAMESPACES, ByObject, Cache, CacheConfig, CacheOptions, CachedBackend};
pub use client::{KubeBackend, RestClient, RestMapper, RestMapping};
pub use config::{
    KubeconfigOverrides, RestConfig, client_from_kubeconfig_bytes, client_from_rest_config,
    get_config, get_config_or_die, parse_kubeconfig, server_version,
};
pub use controller::{Controller, ControllerOptions};
pub use healthz::{Checker, ping};
pub use manager::{Manager, ManagerOptions, RecorderFactory, Runnable, runnable_fn};
pub use signals::{LoggingRecorder, setup_signal_handler};

pub type Error = anyhow::Error;
pub type Result<T, E = Error> = std::result::Result<T, E>;
