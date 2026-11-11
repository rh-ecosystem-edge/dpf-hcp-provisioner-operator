//! Port of internal/controller/hostedcluster
//!
//! | Go | Rust |
//! |---|---|
//! | `HostedClusterManager` (hostedcluster.go) | [`HostedClusterManager`] |
//! | `NodePoolManager` (nodepool.go) | [`NodePoolManager`] |
//! | `SecretManager`, `PullSecretCopyName`, `SSHKeyCopyName` (secrets.go) | [`SecretManager`], [`pull_secret_copy_name`], [`ssh_key_copy_name`] |
//! | `BuildServicePublishingStrategy` (servicepublishing.go) | [`build_service_publishing_strategy`] |
//! | `StatusSyncer` (status.go) | [`StatusSyncer`] |
//! | `CleanupHandler`, `DeletionTimeout`, `DeletionRequeueInterval` (cleanup_handler.go) | [`CleanupHandler`] (implements [`ctrl_finalizer::CleanupHandler`]), [`DELETION_TIMEOUT`], [`DELETION_REQUEUE_INTERVAL`] |
//!
//! The Go constructors take a `*runtime.Scheme`; it is not needed in Rust (owner references
//! take the GVK from the owner's type) and has been dropped.

mod cleanup_handler;
mod hostedcluster;
mod nodepool;
mod secrets;
mod servicepublishing;
mod status;

pub use cleanup_handler::{CleanupHandler, DELETION_REQUEUE_INTERVAL, DELETION_TIMEOUT};
pub use hostedcluster::HostedClusterManager;
pub use nodepool::NodePoolManager;
pub use secrets::{SecretManager, pull_secret_copy_name, ssh_key_copy_name};
pub use servicepublishing::build_service_publishing_strategy;
pub use status::{CONDITION_MAPPINGS, StatusSyncer};

#[cfg(test)]
mod hostedcluster_test;
#[cfg(test)]
mod nodepool_test;
#[cfg(test)]
mod servicepublishing_test;
#[cfg(test)]
mod status_test;
