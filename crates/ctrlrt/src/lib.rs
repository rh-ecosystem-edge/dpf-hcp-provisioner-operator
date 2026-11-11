//! `ctrlrt` — a Rust analogue of the parts of controller-runtime / apimachinery used by the
//! DPF HCP provisioner operator. This crate only holds the backend-independent core: object
//! typing, Go-compatible errors, the typed client facade, condition/finalizer/owner helpers,
//! `CreateOrUpdate`, event recorder & logging facades, and the controller description types.
//!
//! Backends live in sibling crates:
//! * `ctrlrt-fake`    — controller-runtime fake client semantics for unit tests
//! * `ctrlrt-runtime` — real API client, informer cache, controllers, manager
//! * `ctrlrt-envtest` — envtest (etcd + kube-apiserver) harness for integration tests

pub mod client;
pub mod controllerutil;
pub mod error;
pub mod log;
pub mod meta;
pub mod object;
pub mod reconcile;
pub mod recorder;

pub use client::{Client, ClientBackend, DeleteOptions, ListOptions, Patch, PropagationPolicy};
pub use controllerutil::{OperationResult, create_or_update};
pub use error::{ApiError, Error, NoKindMatchError, Result, wrap};
pub use log::Logger;
pub use object::{Object, ObjectKey, ResourceInfo};
pub use reconcile::{
    Builder, ControllerSpec, Ctx, Funcs, GenerationChangedPredicate, ManagerHandle, MapFn,
    Predicate, ReconcileResult, Reconciler, Request, WatchEvent, map_fn, result_of_error, with_result,
};
pub use recorder::{EventRecorder, FakeRecorder, Recorder, RecorderExt};

/// Re-exported for re-use (`k8s_openapi`, `kube`).
pub use k8s_openapi;
pub use kube;

#[doc(hidden)]
pub mod __private {
    pub use anyhow;
    pub use k8s_openapi;
    pub use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    pub use kube;
    pub use serde_json;
}

/// `ptr.To` / `ptr.Deref` helpers.
pub mod ptr {
    pub fn to<T>(v: T) -> Option<T> {
        Some(v)
    }
    pub fn deref<T: Clone>(v: &Option<T>, default: T) -> T {
        v.clone().unwrap_or(default)
    }
}
