//! Object typing: the Rust analogue of `client.Object` + scheme GVK lookups.

use std::fmt::Debug;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use serde::{Serialize, de::DeserializeOwned};

/// Compile-time scope information for a resource type.
pub trait ScopeInfo {
    const NAMESPACED: bool;
}
impl ScopeInfo for k8s_openapi::NamespaceResourceScope {
    const NAMESPACED: bool = true;
}
impl ScopeInfo for k8s_openapi::ClusterResourceScope {
    const NAMESPACED: bool = false;
}

/// Every statically typed Kubernetes object usable with [`crate::Client`].
///
/// Blanket-implemented for every `kube::Resource` with static type information,
/// i.e. all `k8s-openapi` types and all CRD types declared with
/// [`crate::impl_resource!`].
pub trait Object:
    kube::Resource<DynamicType = (), Scope: ScopeInfo>
    + Serialize
    + DeserializeOwned
    + Clone
    + Debug
    + Default
    + Send
    + Sync
    + 'static
{
    fn resource_info() -> ResourceInfo {
        ResourceInfo {
            group: Self::group(&()).into_owned(),
            version: Self::version(&()).into_owned(),
            kind: Self::kind(&()).into_owned(),
            plural: Self::plural(&()).into_owned(),
            namespaced: <Self::Scope as ScopeInfo>::NAMESPACED,
        }
    }

    fn object_meta(&self) -> &ObjectMeta {
        kube::Resource::meta(self)
    }
    fn object_meta_mut(&mut self) -> &mut ObjectMeta {
        kube::Resource::meta_mut(self)
    }

    /// `GetName()` (empty string if unset, like Go).
    fn name(&self) -> &str {
        self.object_meta().name.as_deref().unwrap_or("")
    }
    /// `GetNamespace()` (empty string if unset, like Go).
    fn namespace(&self) -> &str {
        self.object_meta().namespace.as_deref().unwrap_or("")
    }
    fn uid(&self) -> &str {
        self.object_meta().uid.as_deref().unwrap_or("")
    }
    fn generation(&self) -> i64 {
        self.object_meta().generation.unwrap_or(0)
    }
    fn resource_version(&self) -> &str {
        self.object_meta().resource_version.as_deref().unwrap_or("")
    }
    fn key(&self) -> ObjectKey {
        ObjectKey::new(self.namespace(), self.name())
    }
    /// `!obj.GetDeletionTimestamp().IsZero()`
    fn is_being_deleted(&self) -> bool {
        self.object_meta().deletion_timestamp.is_some()
    }
    fn labels(&self) -> &std::collections::BTreeMap<String, String> {
        static EMPTY: std::sync::OnceLock<std::collections::BTreeMap<String, String>> =
            std::sync::OnceLock::new();
        self.object_meta()
            .labels
            .as_ref()
            .unwrap_or_else(|| EMPTY.get_or_init(Default::default))
    }
    fn annotations(&self) -> &std::collections::BTreeMap<String, String> {
        static EMPTY: std::sync::OnceLock<std::collections::BTreeMap<String, String>> =
            std::sync::OnceLock::new();
        self.object_meta()
            .annotations
            .as_ref()
            .unwrap_or_else(|| EMPTY.get_or_init(Default::default))
    }
    fn owner_references(&self) -> &[OwnerReference] {
        self.object_meta()
            .owner_references
            .as_deref()
            .unwrap_or(&[])
    }
    fn finalizers(&self) -> &[String] {
        self.object_meta().finalizers.as_deref().unwrap_or(&[])
    }
}

impl<T> Object for T where
    T: kube::Resource<DynamicType = (), Scope: ScopeInfo>
        + Serialize
        + DeserializeOwned
        + Clone
        + Debug
        + Default
        + Send
        + Sync
        + 'static
{
}

/// GroupVersionKind + resource plural + scope; the runtime analogue of a RESTMapping.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceInfo {
    pub group: String,
    pub version: String,
    pub kind: String,
    pub plural: String,
    pub namespaced: bool,
}

impl ResourceInfo {
    /// `apiVersion` string (`v1` for the core group, `group/version` otherwise).
    pub fn api_version(&self) -> String {
        if self.group.is_empty() {
            self.version.clone()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }
    /// `schema.GroupResource.String()`: `plural.group`, or `plural` for the core group.
    pub fn group_resource(&self) -> String {
        if self.group.is_empty() {
            self.plural.clone()
        } else {
            format!("{}.{}", self.plural, self.group)
        }
    }
    /// `schema.GroupKind.String()`: `Kind.group`, or `Kind` for the core group.
    pub fn group_kind(&self) -> String {
        if self.group.is_empty() {
            self.kind.clone()
        } else {
            format!("{}.{}", self.kind, self.group)
        }
    }
    pub fn api_resource(&self) -> kube::discovery::ApiResource {
        kube::discovery::ApiResource {
            group: self.group.clone(),
            version: self.version.clone(),
            api_version: self.api_version(),
            kind: self.kind.clone(),
            plural: self.plural.clone(),
        }
    }
}

/// `client.ObjectKey` / `types.NamespacedName`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectKey {
    pub namespace: String,
    pub name: String,
}

impl ObjectKey {
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }
    /// Cluster-scoped key.
    pub fn cluster(name: impl Into<String>) -> Self {
        Self {
            namespace: String::new(),
            name: name.into(),
        }
    }
    pub fn from_object<K: Object>(obj: &K) -> Self {
        obj.key()
    }
}

/// `types.NamespacedName.MarshalLog()`: `{"name": ..., "namespace": ...}` (namespace omitted
/// when empty) — how zap renders a NamespacedName log value.
impl serde::Serialize for ObjectKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("NamespacedName", 2)?;
        st.serialize_field("name", &self.name)?;
        if self.namespace.is_empty() {
            st.skip_field("namespace")?;
        } else {
            st.serialize_field("namespace", &self.namespace)?;
        }
        st.end()
    }
}

/// `types.NamespacedName.String()`: `namespace/name`, or `name` when namespace is empty.
impl std::fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.namespace.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{}/{}", self.namespace, self.name)
        }
    }
}

/// Declare `kube::Resource` for a CRD type whose struct has a `metadata: ObjectMeta` field.
///
/// ```ignore
/// ctrlrt::impl_resource!(DPUCluster, "provisioning.dpu.nvidia.com", "v1alpha1", "DPUCluster", "dpuclusters", Namespaced);
/// ```
#[macro_export]
macro_rules! impl_resource {
    ($ty:ty, $group:expr, $version:expr, $kind:expr, $plural:expr, Namespaced) => {
        $crate::impl_resource!(@impl $ty, $group, $version, $kind, $plural, $crate::__private::k8s_openapi::NamespaceResourceScope);
    };
    ($ty:ty, $group:expr, $version:expr, $kind:expr, $plural:expr, Cluster) => {
        $crate::impl_resource!(@impl $ty, $group, $version, $kind, $plural, $crate::__private::k8s_openapi::ClusterResourceScope);
    };
    (@impl $ty:ty, $group:expr, $version:expr, $kind:expr, $plural:expr, $scope:ty) => {
        impl $crate::__private::kube::Resource for $ty {
            type DynamicType = ();
            type Scope = $scope;
            fn kind(_: &()) -> ::std::borrow::Cow<'_, str> {
                ::std::borrow::Cow::Borrowed($kind)
            }
            fn group(_: &()) -> ::std::borrow::Cow<'_, str> {
                ::std::borrow::Cow::Borrowed($group)
            }
            fn version(_: &()) -> ::std::borrow::Cow<'_, str> {
                ::std::borrow::Cow::Borrowed($version)
            }
            fn plural(_: &()) -> ::std::borrow::Cow<'_, str> {
                ::std::borrow::Cow::Borrowed($plural)
            }
            fn meta(&self) -> &$crate::__private::ObjectMeta {
                &self.metadata
            }
            fn meta_mut(&mut self) -> &mut $crate::__private::ObjectMeta {
                &mut self.metadata
            }
        }
    };
}
