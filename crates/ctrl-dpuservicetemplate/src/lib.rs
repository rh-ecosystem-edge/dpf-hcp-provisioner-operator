//! Port of internal/controller/dpuservicetemplate

mod controller;
mod dpuservicetemplate;
mod dpuservicetemplatevalues;
mod gojson;
mod release_image;

pub use controller::*;
pub use dpuservicetemplate::{
    ANNOTATION_SOURCE_OVN_IMAGE, DPUServiceTemplateManager, ovnk_daemonset_version_for_ocp,
};
pub use release_image::{ReleaseImageReader, RemoteReleaseImageReader};

#[cfg(test)]
mod release_image_test;
