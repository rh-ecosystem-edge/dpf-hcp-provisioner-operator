//! The parts of `internal/controller/ignitiongenerator` used by the main reconciler.

pub use ctrl_ignitiongenerator::{
    BFCFG_TEMPLATE_BFB_NAME_ANNOTATION, BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION,
    BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION, BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION,
    BFCFG_TEMPLATE_LABEL, IgnitionGenerator, config_map_name, is_nil_pointer_dereference,
};
