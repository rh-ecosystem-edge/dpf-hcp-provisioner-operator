//! Kubernetes API types used by the DPF HCP provisioner operator.
//!
//! The structs are generated (`hack/gen-rust-apis.sh`: kopium over the CRDs, re-shaped against
//! the vendored Go types; see `GENERATION.md`) so that serde reproduces Go's `encoding/json`
//! output: Go field order, `omitempty`, nil slices/maps/pointers without `omitempty` as `null`,
//! zero `metav1.Time` as `null`, canonical `resource.Quantity`, Go-ordered `ObjectMeta` /
//! `Condition`. Every struct keeps unknown fields in `extra`.
//!
//! Module layout mirrors the Go import paths:
//!
//! | Go package | Rust module |
//! |---|---|
//! | `api/v1alpha1` (this repo) | [`provisioning::v1alpha1`] |
//! | `github.com/nvidia/doca-platform/api/provisioning/v1alpha1` | [`dpf::provisioning::v1alpha1`] |
//! | `github.com/nvidia/doca-platform/api/dpuservice/v1alpha1` | [`dpf::dpuservice::v1alpha1`] |
//! | `github.com/nvidia/doca-platform/api/operator/v1alpha1` | [`dpf::operator::v1alpha1`] |
//! | `github.com/openshift/hypershift/api/hypershift/v1beta1` | [`hypershift::v1beta1`] |
//! | `github.com/openshift/hypershift/api/util/ipnet` | [`hypershift::util::ipnet`] |
//! | `github.com/openshift/hypershift/support/infraid` | [`hypershift::support::infraid`] |
//! | `go.universe.tf/metallb/api/v1beta1` | [`metallb::v1beta1`] |
//! | `github.com/openshift/api/config/v1` | [`openshift::config::v1`] |
//! | `github.com/openshift/api/operator/v1` (embedded in hypershift) | [`openshift::operator::v1`] |
//! | `k8s.io/api/core/v1` (embedded types only) | [`k8s::core::v1`] |
//! | `k8s.io/apimachinery/pkg/apis/meta/v1` (embedded types only) | [`k8s::meta::v1`] |
//! | `github.com/Mellanox/nic-configuration-operator/api/v1alpha1` | [`nicconfig::v1alpha1`] |
//!
//! Go constants are `SCREAMING_SNAKE_CASE` (`hyperv1.HighlyAvailable` →
//! `hypershift::v1beta1::HIGHLY_AVAILABLE`); Go string types (enums) are `String` aliases.

pub mod generated;
pub mod go;
pub mod infraid;
pub mod ipnet;
pub mod provisioning;
pub mod resource;

pub mod dpf {
    pub mod provisioning {
        pub mod v1alpha1 {
            pub use crate::generated::dpf_provisioning_v1alpha1::*;
        }
    }
    pub mod dpuservice {
        pub mod v1alpha1 {
            pub use crate::generated::dpf_dpuservice_v1alpha1::*;
        }
    }
    pub mod operator {
        pub mod v1alpha1 {
            pub use crate::generated::dpf_operator_v1alpha1::*;
        }
    }
}

pub mod hypershift {
    pub mod v1beta1 {
        pub use crate::generated::hypershift_v1beta1::*;
    }
    pub mod util {
        pub mod ipnet {
            pub use crate::ipnet::*;
        }
    }
    pub mod support {
        pub mod infraid {
            pub use crate::infraid::*;
        }
    }
}

pub mod metallb {
    pub mod v1beta1 {
        pub use crate::generated::metallb_v1beta1::*;
    }
}

pub mod openshift {
    pub mod config {
        pub mod v1 {
            pub use crate::generated::openshift_config_v1::*;
        }
    }
    pub mod operator {
        pub mod v1 {
            pub use crate::generated::openshift_operator_v1::*;
        }
    }
}

pub mod k8s {
    pub mod core {
        pub mod v1 {
            //! Go-encoding mirrors of `k8s.io/api/core/v1` types embedded in the CRDs (field
            //! order and `omitempty` as in Go), with conversions to/from `k8s-openapi`.
            pub use crate::generated::k8s_core_v1::*;

            use k8s_openapi::api::core::v1 as corev1;

            impl From<LocalObjectReference> for corev1::LocalObjectReference {
                fn from(r: LocalObjectReference) -> Self {
                    corev1::LocalObjectReference { name: r.name }
                }
            }
            impl From<corev1::LocalObjectReference> for LocalObjectReference {
                fn from(r: corev1::LocalObjectReference) -> Self {
                    LocalObjectReference {
                        name: r.name,
                        ..Default::default()
                    }
                }
            }
            impl From<ObjectReference> for corev1::ObjectReference {
                fn from(r: ObjectReference) -> Self {
                    let opt = |s: String| if s.is_empty() { None } else { Some(s) };
                    corev1::ObjectReference {
                        api_version: opt(r.api_version),
                        field_path: opt(r.field_path),
                        kind: opt(r.kind),
                        name: opt(r.name),
                        namespace: opt(r.namespace),
                        resource_version: opt(r.resource_version),
                        uid: opt(r.uid),
                    }
                }
            }
            impl From<corev1::ObjectReference> for ObjectReference {
                fn from(r: corev1::ObjectReference) -> Self {
                    ObjectReference {
                        kind: r.kind.unwrap_or_default(),
                        namespace: r.namespace.unwrap_or_default(),
                        name: r.name.unwrap_or_default(),
                        uid: r.uid.unwrap_or_default(),
                        api_version: r.api_version.unwrap_or_default(),
                        resource_version: r.resource_version.unwrap_or_default(),
                        field_path: r.field_path.unwrap_or_default(),
                        ..Default::default()
                    }
                }
            }
        }
    }
    pub mod meta {
        pub mod v1 {
            //! Go-encoding mirrors of `k8s.io/apimachinery/pkg/apis/meta/v1` types embedded in
            //! the CRDs. `ObjectMeta`, `ListMeta`, `Condition` and `Time` are the `k8s-openapi`
            //! types (Go encoding via [`crate::go`]).
            pub use crate::generated::k8s_meta_v1::*;
        }
    }
}

pub mod nicconfig {
    pub mod v1alpha1 {
        pub use crate::generated::nicconfig_v1alpha1::*;
    }
}
