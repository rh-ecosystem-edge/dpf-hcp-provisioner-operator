//! `runtime.Scheme` analogue: the set of kinds the fake client knows about.
//!
//! Go identifies types by their Go type; here a kind is identified by its
//! group/version/kind ([`ResourceInfo`]). Registering a kind also registers its `<Kind>List`
//! (Go's generated `AddToScheme` always registers both).

use std::collections::BTreeMap;
use std::fmt;

use ctrlrt::{Object, ResourceInfo};

/// Name of client-go's global `scheme.Scheme`: `runtime.NewScheme()` names a scheme after its
/// call site (`naming.GetNameFromCallsite`), which for `scheme.Scheme` is
/// `k8s.io/client-go/kubernetes/scheme/register.go:84`.
pub const CLIENT_GO_SCHEME_NAME: &str = "k8s.io/client-go/kubernetes/scheme/register.go:84";

/// Name of the private scheme used by `k8s.io/client-go/kubernetes/fake` (`NewSimpleClientset`).
pub const CLIENTSET_FAKE_SCHEME_NAME: &str = "k8s.io/client-go/kubernetes/fake/register.go:84";

/// Set of registered kinds plus the scheme name used in "not registered" errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scheme {
    name: String,
    kinds: BTreeMap<(String, String, String), ResourceInfo>,
}

impl Default for Scheme {
    #[track_caller]
    fn default() -> Self {
        Self::new()
    }
}

impl Scheme {
    /// `runtime.NewScheme()`. Like Go, the scheme is named after the call site (`file:line`).
    #[track_caller]
    pub fn new() -> Self {
        let loc = std::panic::Location::caller();
        Self::named(format!("{}:{}", loc.file(), loc.line()))
    }

    /// Empty scheme with an explicit name.
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kinds: BTreeMap::new(),
        }
    }

    /// The scheme name (as rendered in `no kind is registered ...` errors).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `AddToScheme` for one kind (builder style).
    pub fn register<K: Object>(mut self) -> Self {
        self.add::<K>();
        self
    }

    /// `AddToScheme` for one kind given as runtime info (builder style).
    pub fn register_info(mut self, info: ResourceInfo) -> Self {
        self.add_info(info);
        self
    }

    /// `AddToScheme` for one kind.
    pub fn add<K: Object>(&mut self) -> &mut Self {
        self.add_info(K::resource_info())
    }

    /// `AddToScheme` for one kind given as runtime info.
    pub fn add_info(&mut self, info: ResourceInfo) -> &mut Self {
        self.kinds.insert(
            (info.group.clone(), info.version.clone(), info.kind.clone()),
            info,
        );
        self
    }

    /// Merge all kinds of `other` into this scheme (keeps this scheme's name).
    pub fn extend(mut self, other: &Scheme) -> Self {
        for info in other.kinds.values() {
            self.add_info(info.clone());
        }
        self
    }

    /// `clientgoscheme.AddToScheme(scheme)`: add every built-in kind (builder style).
    pub fn with_client_go(self) -> Self {
        let cg = Self::client_go();
        self.extend(&cg)
    }

    /// `scheme.Recognizes(gvk)`
    pub fn recognizes(&self, info: &ResourceInfo) -> bool {
        self.recognizes_gvk(&info.group, &info.version, &info.kind)
    }

    /// `scheme.Recognizes(gvk)` by group/version/kind.
    pub fn recognizes_gvk(&self, group: &str, version: &str, kind: &str) -> bool {
        self.kinds
            .contains_key(&(group.to_string(), version.to_string(), kind.to_string()))
    }

    /// Look up the registered info for a group/version/kind.
    pub fn lookup(&self, group: &str, version: &str, kind: &str) -> Option<&ResourceInfo> {
        self.kinds
            .get(&(group.to_string(), version.to_string(), kind.to_string()))
    }

    /// All registered kinds.
    pub fn kinds(&self) -> impl Iterator<Item = &ResourceInfo> {
        self.kinds.values()
    }

    /// The error `apiutil.GVKForObject` returns for an unregistered typed object
    /// (`list = true` for the `<Kind>List` type used by `List`).
    pub fn not_registered(&self, info: &ResourceInfo, list: bool) -> NotRegisteredError {
        NotRegisteredError {
            type_name: format!(
                "{}.{}{}",
                go_package_name(info),
                info.kind,
                if list { "List" } else { "" }
            ),
            scheme_name: self.name.clone(),
        }
    }

    /// client-go's `scheme.Scheme` (`k8s.io/client-go/kubernetes/scheme`): every built-in
    /// kind the operator may touch. Note: like Go, `apiextensions.k8s.io` is *not* included.
    pub fn client_go() -> Self {
        Self::client_go_named(CLIENT_GO_SCHEME_NAME)
    }

    /// The scheme of `k8s.io/client-go/kubernetes/fake` (same kinds as [`Scheme::client_go`]).
    pub fn clientset_fake() -> Self {
        Self::client_go_named(CLIENTSET_FAKE_SCHEME_NAME)
    }

    fn client_go_named(name: &str) -> Self {
        use k8s_openapi::api::{
            admissionregistration::v1 as admv1, apps::v1 as appsv1, autoscaling::v1 as asv1,
            autoscaling::v2 as asv2, batch::v1 as batchv1, certificates::v1 as certv1,
            coordination::v1 as coordv1, core::v1 as corev1, discovery::v1 as discv1,
            events::v1 as eventsv1, networking::v1 as netv1, node::v1 as nodev1,
            policy::v1 as policyv1, rbac::v1 as rbacv1, scheduling::v1 as schedv1,
            storage::v1 as storagev1,
        };
        let mut s = Self::named(name);
        s.add::<corev1::Binding>()
            .add::<corev1::ComponentStatus>()
            .add::<corev1::ConfigMap>()
            .add::<corev1::Endpoints>()
            .add::<corev1::Event>()
            .add::<corev1::LimitRange>()
            .add::<corev1::Namespace>()
            .add::<corev1::Node>()
            .add::<corev1::PersistentVolume>()
            .add::<corev1::PersistentVolumeClaim>()
            .add::<corev1::Pod>()
            .add::<corev1::PodTemplate>()
            .add::<corev1::ReplicationController>()
            .add::<corev1::ResourceQuota>()
            .add::<corev1::Secret>()
            .add::<corev1::Service>()
            .add::<corev1::ServiceAccount>()
            .add::<appsv1::ControllerRevision>()
            .add::<appsv1::DaemonSet>()
            .add::<appsv1::Deployment>()
            .add::<appsv1::ReplicaSet>()
            .add::<appsv1::StatefulSet>()
            .add::<batchv1::CronJob>()
            .add::<batchv1::Job>()
            .add::<certv1::CertificateSigningRequest>()
            .add::<coordv1::Lease>()
            .add::<discv1::EndpointSlice>()
            .add::<eventsv1::Event>()
            .add::<netv1::Ingress>()
            .add::<netv1::IngressClass>()
            .add::<netv1::NetworkPolicy>()
            .add::<nodev1::RuntimeClass>()
            .add::<policyv1::PodDisruptionBudget>()
            .add::<rbacv1::ClusterRole>()
            .add::<rbacv1::ClusterRoleBinding>()
            .add::<rbacv1::Role>()
            .add::<rbacv1::RoleBinding>()
            .add::<schedv1::PriorityClass>()
            .add::<storagev1::CSIDriver>()
            .add::<storagev1::CSINode>()
            .add::<storagev1::StorageClass>()
            .add::<storagev1::VolumeAttachment>()
            .add::<admv1::MutatingWebhookConfiguration>()
            .add::<admv1::ValidatingWebhookConfiguration>()
            .add::<asv1::HorizontalPodAutoscaler>()
            .add::<asv2::HorizontalPodAutoscaler>();
        s
    }
}

/// Go package name of a kind's type: by Kubernetes convention the API version
/// (`corev1` → `v1`, `provisioningv1alpha1` → `v1alpha1`, ...).
fn go_package_name(info: &ResourceInfo) -> &str {
    &info.version
}

/// `runtime.notRegisteredErr` for a type: `no kind is registered for the type <pkg>.<Type> in
/// scheme "<name>"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotRegisteredError {
    /// `reflect.Type` rendering, e.g. `v1alpha1.DPUCluster` / `v1.ConfigMapList`.
    pub type_name: String,
    pub scheme_name: String,
}

impl fmt::Display for NotRegisteredError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no kind is registered for the type {} in scheme {}",
            self.type_name,
            ctrlrt::error::go_quote(&self.scheme_name)
        )
    }
}

impl std::error::Error for NotRegisteredError {}

/// `runtime.IsNotRegisteredError` (searches the wrap chain).
pub fn is_not_registered(err: &ctrlrt::Error) -> bool {
    ctrlrt::error::find_in_chain::<NotRegisteredError>(err).is_some()
}
