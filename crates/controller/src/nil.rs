//! Go nil-pointer semantics for the optional reconciler dependencies.
//!
//! The Go reconciler is a struct of pointers/interfaces; tests construct it with only a
//! subset set (`&DPFHCPProvisionerReconciler{}`, the envtest suite's reconciler without
//! MetalLB/ignition/hugepages managers). Calling through a nil field panics with Go's runtime
//! error, which controller-runtime recovers (`RecoverPanic`, default true) into
//! `panic: runtime error: invalid memory address or nil pointer dereference [recovered]`.
//! The Rust controller (`ctrlrt-runtime`) recovers panics the same way, so a missing
//! dependency panics with the same message. The operator binary always sets every field.

use std::sync::Arc;

use ctrlrt::{
    ClientBackend, DeleteOptions, ListOptions, Patch, Recorder, ResourceInfo,
    k8s_openapi::api::core::v1::ObjectReference,
};
use serde_json::Value;

/// Go's runtime error message for a nil pointer dereference.
pub(crate) const NIL_DEREF_MESSAGE: &str =
    "runtime error: invalid memory address or nil pointer dereference";

/// Panic like a Go nil pointer dereference.
pub(crate) fn nil_deref() -> ! {
    panic!("{}", NIL_DEREF_MESSAGE)
}

/// A nil `client.Client` interface: any call panics.
struct NilClient;

#[async_trait::async_trait]
impl ClientBackend for NilClient {
    async fn get(&self, _: &ResourceInfo, _: &str, _: &str) -> ctrlrt::Result<Value> {
        nil_deref()
    }
    async fn list(&self, _: &ResourceInfo, _: &ListOptions) -> ctrlrt::Result<Vec<Value>> {
        nil_deref()
    }
    async fn create(&self, _: &ResourceInfo, _: Value) -> ctrlrt::Result<Value> {
        nil_deref()
    }
    async fn update(&self, _: &ResourceInfo, _: Value) -> ctrlrt::Result<Value> {
        nil_deref()
    }
    async fn update_subresource(
        &self,
        _: &ResourceInfo,
        _: &str,
        _: Value,
    ) -> ctrlrt::Result<Value> {
        nil_deref()
    }
    async fn patch(&self, _: &ResourceInfo, _: &str, _: &str, _: &Patch) -> ctrlrt::Result<Value> {
        nil_deref()
    }
    async fn patch_subresource(
        &self,
        _: &ResourceInfo,
        _: &str,
        _: &str,
        _: &str,
        _: &Patch,
    ) -> ctrlrt::Result<Value> {
        nil_deref()
    }
    async fn delete(
        &self,
        _: &ResourceInfo,
        _: &str,
        _: &str,
        _: &DeleteOptions,
    ) -> ctrlrt::Result<()> {
        nil_deref()
    }
}

pub(crate) fn nil_client() -> ctrlrt::Client {
    ctrlrt::Client::new(Arc::new(NilClient))
}

/// A nil `record.EventRecorder` interface: any call panics.
struct NilRecorder;

impl ctrlrt::EventRecorder for NilRecorder {
    fn event_ref(&self, _: &ObjectReference, _: &str, _: &str, _: &str) {
        nil_deref()
    }
}

pub(crate) fn nil_recorder() -> Recorder {
    Arc::new(NilRecorder)
}
