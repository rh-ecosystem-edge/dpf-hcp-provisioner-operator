//! Port of internal/controller/ignitiongenerator
//!
//! | Go | Rust |
//! |---|---|
//! | `IgnitionGenerator`, `NewIgnitionGenerator(c, scheme, recorder)` | [`IgnitionGenerator`], [`IgnitionGenerator::new`] (no scheme) |
//! | `(*IgnitionGenerator).GenerateIgnition(ctx, cr)` | [`IgnitionGenerator::generate_ignition`] |
//! | `ConfigMapName` | [`config_map_name`] |
//! | `BfcfgTemplateLabel`, `BfcfgTemplate*Annotation` | [`BFCFG_TEMPLATE_LABEL`], [`BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION`], ... |
//! | `errIgnitionSyncPending` | [`IgnitionSyncPending`] |
//! | mode.go (`isZeroTrustMode`, ...) | `mode` (crate-private) |
//!
//! Go nil-pointer dereferences (runtime panics recovered by controller-runtime) are returned
//! as [`NilPointerDereference`] errors instead of panicking; see its docs.

mod http;
mod ignition_generator;
mod mode;

pub use ignition_generator::{
    BFCFG_TEMPLATE_BFB_NAME_ANNOTATION, BFCFG_TEMPLATE_BFB_NAMESPACE_ANNOTATION,
    BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION, BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION,
    BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION, BFCFG_TEMPLATE_DPU_FLAVOR_NAMESPACE_ANNOTATION,
    BFCFG_TEMPLATE_LABEL, BFCFG_TEMPLATE_OCP_RELEASE_IMAGE_ANNOTATION, IgnitionGenerator,
    IgnitionSyncPending, NilPointerDereference, config_map_name, is_nil_pointer_dereference,
};

#[cfg(test)]
mod testutil;

#[cfg(test)]
mod ignition_generator_test;

#[cfg(test)]
mod ignition_generator_integration_test;

#[cfg(test)]
mod mode_test;

#[cfg(test)]
mod golden_test;
