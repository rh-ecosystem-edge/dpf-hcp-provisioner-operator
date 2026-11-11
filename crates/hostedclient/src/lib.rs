//! Port of `internal/hostedclient`.
//!
//! Package hostedclient provides a single path for reaching a HyperShift hosted (guest)
//! cluster from the management cluster. It loads the hosted cluster's admin kubeconfig,
//! rewrites its endpoint to the internal service DNS name, and caches one client per hosted
//! cluster so every reconciler shares the same connection instead of duplicating kubeconfig
//! handling.
//!
//! Go's `*kubernetes.Clientset` is represented by [`HostedClient`]: a typed
//! [`ctrlrt::Client`] (CRUD, subresources) plus a discovery handle used for
//! `Discovery().ServerVersion()` ([`test_connection`]). Unit tests build a [`HostedClient`]
//! from a `ctrlrt-fake` clientset with [`HostedClient::fake`]; typed per-resource clients
//! (`CoreV1().Namespaces()`, `AppsV1().DaemonSets(ns)`) are [`ResourceClient`]s obtained with
//! [`HostedClient::resource`].

mod client_manager;
mod create_or_update;
mod hosted_client;

pub use client_manager::{ClientManager, replace_server_with_internal_endpoint, test_connection};
pub use create_or_update::{
    HostedResourceClient, MutateFn, OperationResult, create_or_update, semantic_deep_equal,
};
pub use hosted_client::{HostedClient, ResourceClient, ServerVersionFn};
