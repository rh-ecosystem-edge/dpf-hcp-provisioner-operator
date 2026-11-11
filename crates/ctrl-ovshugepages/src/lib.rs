//! Port of `internal/controller/ovs-hugepages` (Go package `ovshugepages`).
//!
//! Package ovshugepages reconciles a DaemonSet inside the hosted (guest) cluster that
//! reserves hugepages on every node. Kubelet does not report hugepages utilized by the
//! system, so the DaemonSet runs one idle "dummy" pod per node to occupy the hugepages
//! needed by OVS and keep them from being consumed by other workloads.
//! See <https://github.com/kubernetes/enhancements/pull/6253>.

mod ovs_hugepages;
mod quantity;

pub use ovs_hugepages::{
    DAEMON_SET_NAME, DEFAULT_HUGEPAGES_AMOUNT, DEFAULT_HUGEPAGES_SIZE, Manager,
    OVS_HUGEPAGES_NAMESPACE,
};

#[cfg(test)]
mod ovs_hugepages_test;
#[cfg(test)]
mod quantity_golden_test;
