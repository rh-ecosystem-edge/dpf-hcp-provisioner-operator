//! Port of internal/controller (DPFHCPProvisioner reconciler)
//!
//! * [`DPFHCPProvisionerReconciler`] (`dpfhcpprovisioner_controller.go`): the main
//!   reconciler, its watches/predicates/map functions ([`DPFHCPProvisionerReconciler::setup_with_manager`]).
//! * [`FINALIZER_NAME`], [`OPERATOR_NAMESPACE`] (used by `cmd/main.go`).

mod dpfhcpprovisioner_controller;
mod ignition;
mod nil;

pub use dpfhcpprovisioner_controller::{
    DPFHCPProvisionerReconciler, FINALIZER_NAME, HugepagesManager, OPERATOR_NAMESPACE,
    ReconcilerState,
};
pub use ignition::IgnitionGenerator;

// Go tests live in package `controller` (white-box), so they are unit-test modules here.
#[cfg(test)]
mod dpfhcpprovisioner_crd_test;
#[cfg(test)]
mod dpfhcpprovisioner_integration_test;
#[cfg(test)]
mod dpfhcpprovisioner_phase_test;
#[cfg(test)]
mod dpudeployment_watch_test;
#[cfg(test)]
mod dpuflavor_watch_test;
#[cfg(test)]
mod hostedcluster_lifecycle_foundation_test;
#[cfg(test)]
mod ignition_configmap_watch_test;
#[cfg(test)]
mod nil_pointer_panic_test;
#[cfg(test)]
mod suite_test;
