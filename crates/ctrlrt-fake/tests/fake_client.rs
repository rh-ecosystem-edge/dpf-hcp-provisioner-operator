//! Behaviour of `ctrlrt_fake::FakeClient`, each test encoding the semantics of
//! `vendor/sigs.k8s.io/controller-runtime/pkg/client/fake/client.go` (v0.22.4) and
//! `vendor/k8s.io/client-go/testing/fixture.go` it ports.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use ctrlrt::error::{
    ApiError, as_api_error, is_already_exists, is_bad_request, is_conflict, is_invalid,
    is_no_match, is_not_found,
};
use ctrlrt::k8s_openapi::api::certificates::v1::{
    CertificateSigningRequest, CertificateSigningRequestCondition, CertificateSigningRequestStatus,
};
use ctrlrt::k8s_openapi::api::core::v1::{ConfigMap, Namespace, Pod, PodStatus, Secret, Service};
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::{ManagedFieldsEntry, Time};
use ctrlrt::{Client, DeleteOptions, ListOptions, ObjectKey, Patch};
use ctrlrt_fake::{
    FakeClientBuilder, InitObject, Reaction, Scheme, TRACKER_ADD_RESOURCE_VERSION,
    is_not_registered, obj, obj_mut,
};
use serde_json::json;

fn scheme() -> Scheme {
    Scheme::named("test-scheme")
        .register::<Widget>()
        .register::<Gateway>()
        .register::<Region>()
        .with_client_go()
}

fn cm(ns: &str, name: &str) -> ConfigMap {
    ConfigMap {
        metadata: meta(ns, name),
        ..Default::default()
    }
}

fn key(ns: &str, name: &str) -> ObjectKey {
    ObjectKey::new(ns, name)
}

fn ts(s: &str) -> Time {
    serde_json::from_value(json!(s)).unwrap()
}

// ---------------------------------------------------------------------------------------------
// Builder / versionedTracker.Add
// ---------------------------------------------------------------------------------------------

/// versionedTracker.Add: empty resourceVersion → trackerAddResourceVersion ("999").
#[tokio::test]
async fn initial_objects_get_resource_version_999() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([obj(&widget("ns", "a")), obj(&cm("ns", "b"))])
        .build();
    let w: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(w.metadata.resource_version.as_deref(), Some("999"));
    let m: ConfigMap = c.get(&key("ns", "b")).await.unwrap();
    assert_eq!(m.metadata.resource_version.as_deref(), Some("999"));
    // the fake never sets uid / creationTimestamp / generation
    assert_eq!(w.metadata.uid, None);
    assert_eq!(w.metadata.creation_timestamp, None);
    assert_eq!(w.metadata.generation, None);
}

/// versionedTracker.Add keeps an explicit resourceVersion.
#[tokio::test]
async fn initial_objects_keep_explicit_resource_version() {
    let mut w = widget("ns", "a");
    w.metadata.resource_version = Some("42".into());
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&w)
        .build();
    let got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(got.metadata.resource_version.as_deref(), Some("42"));
}

/// Go's Add mutates the passed pointer; `obj_mut`/`with_object_mut` reproduce that.
#[tokio::test]
async fn obj_mut_sets_resource_version_on_caller_object() {
    let mut w = widget("ns", "a");
    let mut m = cm("ns", "b");
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([obj_mut(&mut w)])
        .with_object_mut(&mut m)
        .build();
    assert_eq!(
        w.metadata.resource_version.as_deref(),
        Some(TRACKER_ADD_RESOURCE_VERSION)
    );
    assert_eq!(m.metadata.resource_version.as_deref(), Some("999"));
    // so updating the caller's copy directly works, as in Go tests
    w.spec.size = "big".into();
    c.update(&mut w).await.unwrap();
    assert_eq!(w.metadata.resource_version.as_deref(), Some("1000"));
}

/// versionedTracker.Add: deletionTimestamp without finalizers is refused (Build panics).
#[test]
fn initial_object_with_deletion_timestamp_but_no_finalizers_is_refused() {
    let mut w = widget("ns", "a");
    w.metadata.deletion_timestamp = Some(ts("2024-01-01T00:00:00Z"));
    let err = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&w)
        .try_build()
        .unwrap_err();
    assert!(
        err.to_string().ends_with(
            "refusing to create obj a with metadata.deletionTimestamp but no finalizers"
        ),
        "{err}"
    );
    assert!(
        err.to_string().starts_with("failed to add object "),
        "{err}"
    );

    let res = std::panic::catch_unwind(|| {
        FakeClientBuilder::new()
            .with_scheme(scheme())
            .with_object(&w)
            .build()
    });
    assert!(res.is_err());
}

/// An initial object with deletionTimestamp *and* finalizers is accepted as-is.
#[tokio::test]
async fn initial_object_being_deleted_with_finalizers_is_accepted() {
    let mut w = widget("ns", "a");
    w.metadata.deletion_timestamp = Some(ts("2024-01-01T00:00:00Z"));
    w.metadata.finalizers = Some(vec!["f".into()]);
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&w)
        .build();
    let got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(
        got.metadata.deletion_timestamp,
        Some(ts("2024-01-01T00:00:00Z"))
    );
}

/// tracker.add: duplicate initial objects → AlreadyExists (Build panics).
#[test]
fn duplicate_initial_objects_fail() {
    let err = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([&widget("ns", "a"), &widget("ns", "a")])
        .try_build()
        .unwrap_err();
    assert!(
        err.to_string()
            .ends_with(r#"widgets.example.com "a" already exists"#),
        "{err}"
    );
}

/// tracker.Add → scheme.ObjectKinds: unregistered kind.
#[test]
fn initial_object_of_unregistered_kind_fails() {
    let err = FakeClientBuilder::new()
        .with_scheme(Scheme::named("s"))
        .with_object(&widget("ns", "a"))
        .try_build()
        .unwrap_err();
    assert!(
        err.to_string()
            .ends_with(r#"no kind is registered for the type v1alpha1.Widget in scheme "s""#),
        "{err}"
    );
}

/// Build: WithStatusSubresource of an unregistered type panics.
#[test]
fn status_subresource_for_unregistered_kind_fails() {
    let err = FakeClientBuilder::new()
        .with_scheme(Scheme::named("s"))
        .with_status_subresource::<Widget>()
        .try_build()
        .unwrap_err();
    assert!(
        err.to_string()
            .contains(r#"no kind is registered for the type v1alpha1.Widget in scheme "s""#),
        "{err}"
    );
}

/// WithLists / WithRuntimeObjects are added like WithObjects.
#[tokio::test]
async fn with_list_and_runtime_objects() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_list(&[cm("ns", "l1"), cm("ns", "l2")])
        .with_runtime_objects([InitObject::from_value(
            <Widget as ctrlrt::Object>::resource_info(),
            json!({"apiVersion": "example.com/v1alpha1", "kind": "Widget",
                   "metadata": {"name": "u", "namespace": "ns"}, "spec": {"size": "s"}}),
        )])
        .build();
    assert_eq!(c.list_all::<ConfigMap>().await.unwrap().len(), 2);
    let w: Widget = c.get(&key("ns", "u")).await.unwrap();
    assert_eq!(w.spec.size, "s");
}

/// Default scheme is client-go's scheme.Scheme: built-ins work, CRDs are not registered.
#[tokio::test]
async fn default_scheme_is_client_go() {
    let c = FakeClientBuilder::new().with_object(&cm("ns", "a")).build();
    c.get::<ConfigMap>(&key("ns", "a")).await.unwrap();
    let err = c.get::<Widget>(&key("ns", "a")).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"no kind is registered for the type v1alpha1.Widget in scheme "k8s.io/client-go/kubernetes/scheme/register.go:84""#
    );
    assert!(is_not_registered(&err));
}

/// `Scheme::new()` is named after its call site, like runtime.NewScheme().
#[test]
fn scheme_named_after_call_site() {
    let (s, line) = (Scheme::new(), line!());
    let suffix = format!("fake_client.rs:{line}");
    assert!(s.name().ends_with(&suffix), "{}", s.name());
}

// ---------------------------------------------------------------------------------------------
// Unregistered / unserved kinds
// ---------------------------------------------------------------------------------------------

/// apiutil.GVKForObject on every call: "no kind is registered for the type ...".
#[tokio::test]
async fn unregistered_kind_errors_on_every_call() {
    let c = FakeClientBuilder::new()
        .with_scheme(Scheme::named("my-scheme").register::<ConfigMap>())
        .build();
    let msg = r#"no kind is registered for the type v1alpha1.Widget in scheme "my-scheme""#;
    let mut w = widget("ns", "a");
    assert_eq!(
        c.get::<Widget>(&key("ns", "a"))
            .await
            .unwrap_err()
            .to_string(),
        msg
    );
    assert_eq!(c.create(&mut w).await.unwrap_err().to_string(), msg);
    assert_eq!(c.update(&mut w).await.unwrap_err().to_string(), msg);
    assert_eq!(c.update_status(&mut w).await.unwrap_err().to_string(), msg);
    assert_eq!(c.delete(&w).await.unwrap_err().to_string(), msg);
    assert_eq!(
        c.patch(&mut w, &Patch::Merge(json!({})))
            .await
            .unwrap_err()
            .to_string(),
        msg
    );
    // List uses the list type
    assert_eq!(
        c.list_all::<Widget>().await.unwrap_err().to_string(),
        r#"no kind is registered for the type v1alpha1.WidgetList in scheme "my-scheme""#
    );
    let err = c.get::<Widget>(&key("ns", "a")).await.unwrap_err();
    assert!(!is_not_found(&err));
    assert!(!is_no_match(&err));
}

/// with_unserved: the RESTMapper knob → meta.NoKindMatchError.
#[tokio::test]
async fn unserved_kind_returns_no_kind_match() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_unserved::<Widget>()
        .build();
    let err = c.list_all::<Widget>().await.unwrap_err();
    assert!(is_no_match(&err));
    assert_eq!(
        err.to_string(),
        r#"no matches for kind "Widget" in version "example.com/v1alpha1""#
    );
    let mut w = widget("ns", "a");
    assert!(is_no_match(&c.create(&mut w).await.unwrap_err()));
    assert!(is_no_match(&c.delete(&w).await.unwrap_err()));
    // other kinds unaffected
    c.create(&mut cm("ns", "x")).await.unwrap();
}

// ---------------------------------------------------------------------------------------------
// Get / List
// ---------------------------------------------------------------------------------------------

/// tracker.Get: NotFound with the guessed GroupResource.
#[tokio::test]
async fn get_not_found_message() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let err = c.get::<Widget>(&key("ns", "missing")).await.unwrap_err();
    assert!(is_not_found(&err));
    assert_eq!(
        err.to_string(),
        r#"widgets.example.com "missing" not found"#
    );
    let err = c.get::<Secret>(&key("ns", "missing")).await.unwrap_err();
    assert_eq!(err.to_string(), r#"secrets "missing" not found"#);
    let api = as_api_error(&err).unwrap();
    assert_eq!(api.code, 404);
}

/// getGVRFromObject uses meta.UnsafeGuessKindToResource, so messages use the guessed plural.
#[tokio::test]
async fn resource_names_are_guessed_from_kind() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let err = c.get::<Gateway>(&key("ns", "g")).await.unwrap_err();
    assert_eq!(err.to_string(), r#"gatewaies.example.com "g" not found"#);
}

/// Get clears TypeMeta (ensureTypeMeta) and drops managedFields (unless
/// WithReturnManagedFields).
#[tokio::test]
async fn get_clears_type_meta_and_strips_managed_fields() {
    let mut w = widget("ns", "a");
    w.metadata.managed_fields = Some(vec![ManagedFieldsEntry {
        manager: Some("m".into()),
        ..Default::default()
    }]);
    let fc = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&w)
        .build_fake();
    let raw = fc
        .client()
        .backend()
        .get(&<Widget as ctrlrt::Object>::resource_info(), "ns", "a")
        .await
        .unwrap();
    assert!(raw.get("apiVersion").is_none());
    assert!(raw.get("kind").is_none());
    assert!(raw["metadata"].get("managedFields").is_none());

    let fc = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&w)
        .with_return_managed_fields()
        .build_fake();
    let got: Widget = fc.client().get(&key("ns", "a")).await.unwrap();
    assert!(got.metadata.managed_fields.is_some());
}

/// List: namespace filter, label selector, deterministic order (filterByNamespace).
#[tokio::test]
async fn list_namespace_and_labels() {
    let mut a = widget("ns1", "b");
    a.metadata.labels = labels([("app", "x")]);
    let mut b = widget("ns1", "a");
    b.metadata.labels = labels([("app", "x"), ("tier", "1")]);
    let mut d = widget("ns2", "a");
    d.metadata.labels = labels([("app", "x")]);
    let e = widget("ns1", "c");
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([&a, &b, &d, &e])
        .build();

    let names = |v: Vec<Widget>| -> Vec<String> {
        v.iter()
            .map(|w| {
                format!(
                    "{}/{}",
                    w.metadata.namespace.clone().unwrap_or_default(),
                    w.metadata.name.clone().unwrap_or_default()
                )
            })
            .collect()
    };
    assert_eq!(
        names(c.list_all().await.unwrap()),
        ["ns1/a", "ns1/b", "ns1/c", "ns2/a"]
    );
    assert_eq!(
        names(
            c.list(&ListOptions::new().in_namespace("ns1"))
                .await
                .unwrap()
        ),
        ["ns1/a", "ns1/b", "ns1/c"]
    );
    assert_eq!(
        names(
            c.list(&ListOptions::new().matching_labels([("app", "x")]))
                .await
                .unwrap()
        ),
        ["ns1/a", "ns1/b", "ns2/a"]
    );
    assert_eq!(
        names(
            c.list(
                &ListOptions::new()
                    .in_namespace("ns1")
                    .matching_labels([("app", "x"), ("tier", "1")])
            )
            .await
            .unwrap()
        ),
        ["ns1/a"]
    );
    assert!(c.list_all::<ConfigMap>().await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------------------------

/// versionedTracker.Create: resourceVersion "1", object written back to the caller.
#[tokio::test]
async fn create_sets_resource_version_1() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let mut w = widget("ns", "a");
    c.create(&mut w).await.unwrap();
    assert_eq!(w.metadata.resource_version.as_deref(), Some("1"));
    assert_eq!(w.metadata.uid, None);
    let got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(got, w);
}

/// versionedTracker.Create: resourceVersion must be empty.
#[tokio::test]
async fn create_with_resource_version_is_bad_request() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let mut w = widget("ns", "a");
    w.metadata.resource_version = Some("5".into());
    let err = c.create(&mut w).await.unwrap_err();
    assert!(is_bad_request(&err));
    assert_eq!(
        err.to_string(),
        "resourceVersion can not be set for Create requests"
    );
}

/// tracker.add: AlreadyExists.
#[tokio::test]
async fn create_existing_is_already_exists() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&cm("ns", "a"))
        .build();
    let err = c.create(&mut cm("ns", "a")).await.unwrap_err();
    assert!(is_already_exists(&err));
    assert_eq!(err.to_string(), r#"configmaps "a" already exists"#);
    // same name in another namespace is fine
    c.create(&mut cm("other", "a")).await.unwrap();
}

/// versionedTracker.Create: empty name → Invalid.
#[tokio::test]
async fn create_without_name_is_invalid() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let mut w = Widget::default();
    let err = c.create(&mut w).await.unwrap_err();
    assert!(is_invalid(&err));
    assert_eq!(
        err.to_string(),
        r#"Widget.example.com "" is invalid: metadata.name: Required value: name is required"#
    );
    let err = c.create(&mut ConfigMap::default()).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"ConfigMap "" is invalid: metadata.name: Required value: name is required"#
    );
    let api: &ApiError = as_api_error(&err).unwrap();
    assert_eq!(
        api.details.as_ref().unwrap().causes[0].type_,
        "FieldValueRequired"
    );
}

/// fakeClient.Create: generateName + utilrand.String(5), base truncated to 58 chars.
#[tokio::test]
async fn create_with_generate_name() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let mut w = Widget::default();
    w.metadata.namespace = Some("ns".into());
    w.metadata.generate_name = Some("gen-".into());
    c.create(&mut w).await.unwrap();
    let name = w.metadata.name.clone().unwrap();
    assert!(name.starts_with("gen-"));
    assert_eq!(name.len(), 4 + 5);
    assert!(
        name[4..]
            .bytes()
            .all(|b| b"bcdfghjklmnpqrstvwxz2456789".contains(&b))
    );
    c.get::<Widget>(&key("ns", &name)).await.unwrap();

    let mut long = Widget::default();
    long.metadata.generate_name = Some("x".repeat(70));
    c.create(&mut long).await.unwrap();
    let name = long.metadata.name.unwrap();
    assert_eq!(name.len(), 63);
    assert!(name.starts_with(&"x".repeat(58)));
}

/// fakeClient.Create: "Ignore attempts to set deletion timestamp".
#[tokio::test]
async fn create_ignores_deletion_timestamp() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let mut w = widget("ns", "a");
    w.metadata.deletion_timestamp = Some(ts("2024-01-01T00:00:00Z"));
    c.create(&mut w).await.unwrap();
    assert_eq!(w.metadata.deletion_timestamp, None);
    let got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(got.metadata.deletion_timestamp, None);
}

/// Cluster-scoped kinds are keyed without namespace.
#[tokio::test]
async fn cluster_scoped_objects() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let mut ns = Namespace {
        metadata: meta("", "team-a"),
        ..Default::default()
    };
    c.create(&mut ns).await.unwrap();
    let got: Namespace = c.get(&ObjectKey::cluster("team-a")).await.unwrap();
    assert_eq!(got.metadata.resource_version.as_deref(), Some("1"));
    // the namespace part of a key is ignored for cluster-scoped kinds
    c.get::<Namespace>(&key("whatever", "team-a"))
        .await
        .unwrap();

    let mut r = Region {
        metadata: meta("", "r1"),
    };
    c.create(&mut r).await.unwrap();
    let err = c
        .create(&mut Region {
            metadata: meta("", "r1"),
        })
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"regions.example.com "r1" already exists"#
    );
    c.delete(&r).await.unwrap();
    assert!(c.list_all::<Region>().await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------------------------

/// updateObject: every write increments the resourceVersion.
#[tokio::test]
async fn update_increments_resource_version() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .build();
    let mut w: Widget = c.get(&key("ns", "a")).await.unwrap();
    w.spec.size = "1".into();
    c.update(&mut w).await.unwrap();
    assert_eq!(w.metadata.resource_version.as_deref(), Some("1000"));
    w.spec.size = "2".into();
    c.update(&mut w).await.unwrap();
    assert_eq!(w.metadata.resource_version.as_deref(), Some("1001"));
    let got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(got.spec.size, "2");
    assert_eq!(got.metadata.resource_version.as_deref(), Some("1001"));
}

/// updateObject: stale resourceVersion → Conflict "object was modified".
#[tokio::test]
async fn update_with_stale_resource_version_conflicts() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .build();
    let mut first: Widget = c.get(&key("ns", "a")).await.unwrap();
    let mut stale = first.clone();
    c.update(&mut first).await.unwrap();
    let err = c.update(&mut stale).await.unwrap_err();
    assert!(is_conflict(&err));
    assert_eq!(
        err.to_string(),
        r#"Operation cannot be fulfilled on widgets.example.com "a": object was modified"#
    );
    // the stored object is untouched by the failed update
    let got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(got.metadata.resource_version.as_deref(), Some("1000"));
}

/// allowsUnconditionalUpdate: ConfigMap/Secret/... accept an empty resourceVersion,
/// CRDs (and other kinds not in the list) get a Conflict.
#[tokio::test]
async fn update_with_empty_resource_version() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([obj(&cm("ns", "a")), obj(&widget("ns", "w"))])
        .build();
    let mut m = cm("ns", "a");
    m.data = Some([("k".to_string(), "v".to_string())].into());
    c.update(&mut m).await.unwrap();
    assert_eq!(m.metadata.resource_version.as_deref(), Some("1000"));

    let err = c.update(&mut widget("ns", "w")).await.unwrap_err();
    assert!(is_conflict(&err));
    assert_eq!(
        err.to_string(),
        r#"Operation cannot be fulfilled on widgets.example.com "w": object was modified"#
    );
}

/// tracker.Get in updateObject: NotFound for missing objects.
#[tokio::test]
async fn update_missing_is_not_found() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let err = c.update(&mut widget("ns", "a")).await.unwrap_err();
    assert!(is_not_found(&err));
    assert_eq!(err.to_string(), r#"widgets.example.com "a" not found"#);
}

/// allowsCreateOnUpdate: a missing Service is created by Update (resourceVersion "1").
#[tokio::test]
async fn update_missing_service_creates_it() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let mut s = Service {
        metadata: meta("ns", "svc"),
        ..Default::default()
    };
    c.update(&mut s).await.unwrap();
    assert_eq!(s.metadata.resource_version.as_deref(), Some("1"));
    let got: Service = c.get(&key("ns", "svc")).await.unwrap();
    assert_eq!(got.metadata.resource_version.as_deref(), Some("1"));
}

/// updateObject: empty name → Invalid.
#[tokio::test]
async fn update_without_name_is_invalid() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let err = c.update(&mut Widget::default()).await.unwrap_err();
    assert!(is_invalid(&err));
}

/// Without a status subresource, Update writes the status too.
#[tokio::test]
async fn update_without_status_subresource_writes_status() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .build();
    let mut w: Widget = c.get(&key("ns", "a")).await.unwrap();
    w.status.phase = "Ready".into();
    c.update(&mut w).await.unwrap();
    let got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(got.status.phase, "Ready");
}

/// Without a status subresource, Status().Update is NotFound.
#[tokio::test]
async fn status_update_without_status_subresource_is_not_found() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .build();
    let mut w: Widget = c.get(&key("ns", "a")).await.unwrap();
    w.status.phase = "Ready".into();
    let err = c.update_status(&mut w).await.unwrap_err();
    assert!(is_not_found(&err));
    assert_eq!(err.to_string(), r#"widgets.example.com "a" not found"#);
}

/// WithStatusSubresource: Update ignores status changes, Status().Update only changes status
/// (and keeps the passed resourceVersion for the conflict check).
#[tokio::test]
async fn status_subresource_semantics() {
    let mut seed = widget("ns", "a");
    seed.status.phase = "Initial".into();
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&seed)
        .with_status_subresource::<Widget>()
        .build();

    let mut w: Widget = c.get(&key("ns", "a")).await.unwrap();
    w.spec.size = "big".into();
    w.status.phase = "Ignored".into();
    c.update(&mut w).await.unwrap();
    // the returned object carries the stored status
    assert_eq!(w.status.phase, "Initial");
    assert_eq!(w.spec.size, "big");
    assert_eq!(w.metadata.resource_version.as_deref(), Some("1000"));

    w.spec.size = "ignored".into();
    w.metadata.labels = labels([("ignored", "true")]);
    w.status.phase = "Ready".into();
    c.update_status(&mut w).await.unwrap();
    assert_eq!(w.status.phase, "Ready");
    assert_eq!(w.spec.size, "big");
    assert_eq!(w.metadata.labels, None);
    assert_eq!(w.metadata.resource_version.as_deref(), Some("1001"));

    let got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(got, w);

    // stale status update conflicts
    let mut stale = got.clone();
    stale.metadata.resource_version = Some("1000".into());
    let err = c.update_status(&mut stale).await.unwrap_err();
    assert!(is_conflict(&err));
}

/// The status of an object can be cleared through the status subresource.
#[tokio::test]
async fn status_update_can_clear_status() {
    let mut seed = widget("ns", "a");
    seed.status.phase = "Initial".into();
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&seed)
        .with_status_subresource_for(&seed)
        .build();
    let mut w: Widget = c.get(&key("ns", "a")).await.unwrap();
    w.status = Default::default();
    c.update_status(&mut w).await.unwrap();
    let got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(got.status.phase, "");
}

/// inTreeResourcesWithStatus: Pod has a status subresource without registration.
#[tokio::test]
async fn in_tree_kinds_have_status_subresource() {
    let c = FakeClientBuilder::new()
        .with_object(&Pod {
            metadata: meta("ns", "p"),
            ..Default::default()
        })
        .build();
    let mut p: Pod = c.get(&key("ns", "p")).await.unwrap();
    p.status = Some(PodStatus {
        phase: Some("Running".into()),
        ..Default::default()
    });
    c.update(&mut p).await.unwrap();
    assert_eq!(p.status, None);
    p.status = Some(PodStatus {
        phase: Some("Running".into()),
        ..Default::default()
    });
    c.update_status(&mut p).await.unwrap();
    let got: Pod = c.get(&key("ns", "p")).await.unwrap();
    assert_eq!(got.status.unwrap().phase.as_deref(), Some("Running"));
}

/// SubResource("approval").Update behaves like a status update (isStatus = true).
#[tokio::test]
async fn csr_approval_subresource_updates_status_only() {
    let csr = CertificateSigningRequest {
        metadata: meta("", "csr-1"),
        ..Default::default()
    };
    let c = FakeClientBuilder::new().with_object(&csr).build();
    let mut got: CertificateSigningRequest = c.get(&ObjectKey::cluster("csr-1")).await.unwrap();
    got.metadata.labels = labels([("ignored", "x")]);
    got.status = Some(CertificateSigningRequestStatus {
        conditions: Some(vec![CertificateSigningRequestCondition {
            type_: "Approved".into(),
            status: "True".into(),
            reason: Some("AutoApproved".into()),
            ..Default::default()
        }]),
        ..Default::default()
    });
    c.update_subresource(&mut got, "approval").await.unwrap();
    let stored: CertificateSigningRequest = c.get(&ObjectKey::cluster("csr-1")).await.unwrap();
    assert_eq!(stored.metadata.labels, None);
    assert_eq!(
        stored.status.unwrap().conditions.unwrap()[0].type_,
        "Approved"
    );
    assert_eq!(stored.metadata.resource_version.as_deref(), Some("1000"));
}

/// Scale subresource updates need a SubResourceBody → BadRequest.
#[tokio::test]
async fn scale_subresource_update_without_body() {
    let c = FakeClientBuilder::new().with_object(&cm("ns", "a")).build();
    let mut m = cm("ns", "a");
    let err = c.update_subresource(&mut m, "scale").await.unwrap_err();
    assert!(is_bad_request(&err));
    assert_eq!(err.to_string(), "missing SubResourceBody");
    let err = c
        .update_subresource(&mut cm("ns", "missing"), "scale")
        .await
        .unwrap_err();
    assert!(is_not_found(&err));
}

/// updateObject: resourceVersion that does not parse as uint.
#[tokio::test]
async fn update_with_unparsable_stored_resource_version() {
    let mut w = widget("ns", "a");
    w.metadata.resource_version = Some("abc".into());
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&w)
        .build();
    let mut got: Widget = c.get(&key("ns", "a")).await.unwrap();
    let err = c.update(&mut got).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"can not convert resourceVersion "abc" to int: strconv.ParseUint: parsing "abc": invalid syntax"#
    );
}

// ---------------------------------------------------------------------------------------------
// Delete / finalizers
// ---------------------------------------------------------------------------------------------

/// tracker.Delete: object without finalizers is removed; deleting again → NotFound.
#[tokio::test]
async fn delete_without_finalizers_removes() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .build();
    c.delete(&widget("ns", "a")).await.unwrap();
    assert!(is_not_found(
        &c.get::<Widget>(&key("ns", "a")).await.unwrap_err()
    ));
    let err = c.delete(&widget("ns", "a")).await.unwrap_err();
    assert!(is_not_found(&err));
    assert_eq!(err.to_string(), r#"widgets.example.com "a" not found"#);
}

/// deleteObjectLocked: with finalizers the object is kept with a deletionTimestamp and a
/// bumped resourceVersion; removing the last finalizer via Update deletes it.
#[tokio::test]
async fn delete_with_finalizers_sets_deletion_timestamp() {
    let mut w = widget("ns", "a");
    w.metadata.finalizers = Some(vec!["example.com/f".into()]);
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&w)
        .build();
    c.delete(&w).await.unwrap();
    let mut got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert!(got.metadata.deletion_timestamp.is_some());
    assert_eq!(got.metadata.resource_version.as_deref(), Some("1000"));
    assert_eq!(
        got.metadata.finalizers.as_deref(),
        Some(&["example.com/f".to_string()][..])
    );

    // further updates keep working while finalizers remain
    got.spec.size = "x".into();
    c.update(&mut got).await.unwrap();
    assert_eq!(got.metadata.resource_version.as_deref(), Some("1001"));

    // removing the last finalizer deletes the object; caller gets RV+1
    got.metadata.finalizers = None;
    c.update(&mut got).await.unwrap();
    assert_eq!(got.metadata.resource_version.as_deref(), Some("1002"));
    assert!(is_not_found(
        &c.get::<Widget>(&key("ns", "a")).await.unwrap_err()
    ));
}

/// updateObject: deletionTimestamp is immutable through Update.
#[tokio::test]
async fn update_cannot_change_deletion_timestamp() {
    let mut w = widget("ns", "a");
    w.metadata.finalizers = Some(vec!["f".into()]);
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&w)
        .build();
    let mut got: Widget = c.get(&key("ns", "a")).await.unwrap();
    got.metadata.deletion_timestamp = Some(ts("2024-01-01T00:00:00Z"));
    let err = c.update(&mut got).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "error: Unable to edit a: metadata.deletionTimestamp field is immutable"
    );
    assert!(as_api_error(&err).is_none());
}

/// Delete precondition on resourceVersion (Preconditions.ResourceVersion).
#[tokio::test]
async fn delete_with_resource_version_precondition() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .build();
    let opts = DeleteOptions {
        precondition_resource_version: Some("1".into()),
        ..Default::default()
    };
    let err = c.delete_with(&widget("ns", "a"), &opts).await.unwrap_err();
    assert!(is_conflict(&err));
    assert_eq!(
        err.to_string(),
        r#"Operation cannot be fulfilled on widgets.example.com "a": the ResourceVersion in the precondition (1) does not match the ResourceVersion in record (999). The object might have been modified"#
    );
    let opts = DeleteOptions {
        precondition_resource_version: Some("999".into()),
        ..Default::default()
    };
    c.delete_with(&widget("ns", "a"), &opts).await.unwrap();
    let err = c.delete_with(&widget("ns", "a"), &opts).await.unwrap_err();
    assert!(is_not_found(&err));
}

/// UID preconditions and propagation policies are ignored by the Go fake.
#[tokio::test]
async fn delete_ignores_uid_precondition_and_propagation() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .build();
    let opts = DeleteOptions {
        precondition_uid: Some("does-not-match".into()),
        propagation_policy: Some(ctrlrt::PropagationPolicy::Foreground),
        ..Default::default()
    };
    c.delete_with(&widget("ns", "a"), &opts).await.unwrap();
}

// ---------------------------------------------------------------------------------------------
// Patch
// ---------------------------------------------------------------------------------------------

/// fakeClient.Patch with a JSON merge patch: applied to the stored object, RV bumped.
#[tokio::test]
async fn merge_patch() {
    let mut seed = widget("ns", "a");
    seed.metadata.labels = labels([("keep", "1"), ("drop", "1")]);
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&seed)
        .build();
    let mut w = widget("ns", "a");
    c.patch(
        &mut w,
        &Patch::Merge(
            json!({"metadata": {"labels": {"drop": null, "new": "2"}}, "spec": {"size": "L"}}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(w.metadata.resource_version.as_deref(), Some("1000"));
    assert_eq!(w.metadata.labels, labels([("keep", "1"), ("new", "2")]));
    assert_eq!(w.spec.size, "L");
    let got: Widget = c.get(&key("ns", "a")).await.unwrap();
    assert_eq!(got, w);
}

/// A patch carrying a stale resourceVersion conflicts; `resourceVersion: null` is defaulted
/// to the stored one (the `fakeClient.Patch` call-stack special case).
#[tokio::test]
async fn patch_resource_version_handling() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .build();
    let mut w = widget("ns", "a");
    let err = c
        .patch(
            &mut w,
            &Patch::Merge(json!({"metadata": {"resourceVersion": "5"}})),
        )
        .await
        .unwrap_err();
    assert!(is_conflict(&err));
    c.patch(
        &mut w,
        &Patch::Merge(json!({"metadata": {"resourceVersion": null}, "spec": {"size": "x"}})),
    )
    .await
    .unwrap();
    assert_eq!(w.metadata.resource_version.as_deref(), Some("1000"));
    c.patch(
        &mut w,
        &Patch::Merge(json!({"metadata": {"resourceVersion": "1000"}})),
    )
    .await
    .unwrap();
    assert_eq!(w.metadata.resource_version.as_deref(), Some("1001"));
}

/// fakeClient.patch: missing object → NotFound.
#[tokio::test]
async fn patch_missing_is_not_found() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let err = c
        .patch(&mut widget("ns", "a"), &Patch::Merge(json!({})))
        .await
        .unwrap_err();
    assert!(is_not_found(&err));
}

/// Patch with a status subresource ignores status; Status().Patch only changes status.
#[tokio::test]
async fn patch_and_status_patch_with_status_subresource() {
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .with_status_subresource::<Widget>()
        .build();
    let mut w = widget("ns", "a");
    c.patch(
        &mut w,
        &Patch::Merge(json!({"spec": {"size": "s"}, "status": {"phase": "No"}})),
    )
    .await
    .unwrap();
    assert_eq!(w.status.phase, "");
    assert_eq!(w.spec.size, "s");
    c.patch_status(
        &mut w,
        &Patch::Merge(json!({"spec": {"size": "ignored"}, "status": {"phase": "Yes"}})),
    )
    .await
    .unwrap();
    assert_eq!(w.status.phase, "Yes");
    assert_eq!(w.spec.size, "s");
    assert_eq!(w.metadata.resource_version.as_deref(), Some("1001"));

    // Status().Patch on a kind without status subresource → NotFound
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .build();
    let err = c
        .patch_status(
            &mut widget("ns", "a"),
            &Patch::Merge(json!({"status": {"phase": "x"}})),
        )
        .await
        .unwrap_err();
    assert!(is_not_found(&err));
}

/// "rejected patch, metadata.deletionTimestamp immutable"
#[tokio::test]
async fn patch_cannot_change_deletion_timestamp() {
    let mut seed = widget("ns", "a");
    seed.metadata.finalizers = Some(vec!["f".into()]);
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&seed)
        .build();
    let err = c
        .patch(
            &mut widget("ns", "a"),
            &Patch::Merge(json!({"metadata": {"deletionTimestamp": "2024-01-01T00:00:00Z"}})),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "rejected patch, metadata.deletionTimestamp immutable"
    );
}

/// A patch removing the last finalizer of a deleting object deletes it.
#[tokio::test]
async fn patch_removing_last_finalizer_deletes() {
    let mut seed = widget("ns", "a");
    seed.metadata.finalizers = Some(vec!["f".into()]);
    let c = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&seed)
        .build();
    c.delete(&seed).await.unwrap();
    let mut w = widget("ns", "a");
    c.patch(
        &mut w,
        &Patch::Merge(json!({"metadata": {"finalizers": null}})),
    )
    .await
    .unwrap();
    assert!(is_not_found(
        &c.get::<Widget>(&key("ns", "a")).await.unwrap_err()
    ));
}

/// Strategic merge patch on a built-in kind.
#[tokio::test]
async fn strategic_merge_patch() {
    let c = FakeClientBuilder::new().with_object(&cm("ns", "a")).build();
    let mut m = cm("ns", "a");
    c.patch(
        &mut m,
        &Patch::Strategic(json!({"data": {"a": "b"}, "metadata": {"finalizers": ["x"]}})),
    )
    .await
    .unwrap();
    assert_eq!(m.data, Some([("a".to_string(), "b".to_string())].into()));
    assert_eq!(m.metadata.finalizers, Some(vec!["x".to_string()]));
}

// ---------------------------------------------------------------------------------------------
// Readback, reactors, concurrency
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn stored_readback() {
    let fc = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([&widget("ns", "a"), &widget("ns", "b")])
        .build_fake();
    let w: Widget = fc.stored(&key("ns", "a")).unwrap();
    assert_eq!(w.metadata.resource_version.as_deref(), Some("999"));
    assert!(fc.stored::<Widget>(&key("ns", "zzz")).is_none());
    assert_eq!(fc.stored_all::<Widget>().len(), 2);
    assert_eq!(fc.tracker().all().len(), 2);
}

/// Reactors (interceptor analogue) can inject errors and observe calls.
#[tokio::test]
async fn reactor_injects_errors() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = calls.clone();
    let fc = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&widget("ns", "a"))
        .with_reactor("update", "widgets", move |a| {
            calls2.fetch_add(1, Ordering::SeqCst);
            assert_eq!(a.name, "a");
            assert_eq!(a.subresource, "status");
            Reaction::error(ApiError::conflict("example.com", "widgets", "a", "boom"))
        })
        .build_fake();
    let c = fc.client();
    let mut w: Widget = c.get(&key("ns", "a")).await.unwrap();
    // "update"/"widgets" covers subresource updates too; a prepended reactor runs first and
    // handles only the plain update.
    fc.prepend_reactor("update", "widgets", |a| {
        if a.subresource.is_empty() {
            Reaction::object(json!({"metadata": {"name": "intercepted"}}))
        } else {
            Reaction::NotHandled
        }
    });
    c.update(&mut w).await.unwrap();
    assert_eq!(w.metadata.name.as_deref(), Some("intercepted"));
    let mut w: Widget = c.get(&key("ns", "a")).await.unwrap();
    let err = c.update_status(&mut w).await.unwrap_err();
    assert!(is_conflict(&err));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// The fake is shareable across tasks and serializes writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_use() {
    let c: Client = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_object(&cm("ns", "counter"))
        .build();
    let mut handles = Vec::new();
    for i in 0..16 {
        let c = c.clone();
        handles.push(tokio::spawn(async move {
            c.create(&mut cm("ns", &format!("cm-{i}"))).await.unwrap();
            // retry-on-conflict increments
            loop {
                let mut m: ConfigMap = c.get(&key("ns", "counter")).await.unwrap();
                let n: u32 = m
                    .data
                    .as_ref()
                    .and_then(|d| d.get("n"))
                    .map(|s| s.parse().unwrap())
                    .unwrap_or(0);
                m.data = Some([("n".to_string(), (n + 1).to_string())].into());
                match c.update(&mut m).await {
                    Ok(()) => break,
                    Err(e) if is_conflict(&e) => continue,
                    Err(e) => panic!("{e}"),
                }
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(c.list_all::<ConfigMap>().await.unwrap().len(), 17);
    let m: ConfigMap = c.get(&key("ns", "counter")).await.unwrap();
    assert_eq!(m.data.unwrap()["n"], "16");
    assert_eq!(m.metadata.resource_version.as_deref(), Some("1015"));
}

/// controllerutil.CreateOrUpdate against the fake: unchanged objects are not written.
#[tokio::test]
async fn create_or_update_round_trip() {
    let c = FakeClientBuilder::new().with_scheme(scheme()).build();
    let mut m = cm("ns", "a");
    let r = ctrlrt::create_or_update(&c, &mut m, |m| {
        m.data = Some([("k".to_string(), "v".to_string())].into());
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(r, ctrlrt::OperationResult::Created);
    let mut m = cm("ns", "a");
    let r = ctrlrt::create_or_update(&c, &mut m, |m| {
        m.data = Some([("k".to_string(), "v".to_string())].into());
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(r, ctrlrt::OperationResult::None);
    assert_eq!(m.metadata.resource_version.as_deref(), Some("1"));
    let r = ctrlrt::create_or_update(&c, &mut m, |m| {
        m.data = Some([("k".to_string(), "v2".to_string())].into());
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(r, ctrlrt::OperationResult::Updated);
    assert_eq!(m.metadata.resource_version.as_deref(), Some("2"));
}

/// ensureTypeMeta clears TypeMeta on every typed response: get, list items, create, update,
/// status update, patch, status patch. Built-ins and CRDs alike.
#[tokio::test]
async fn every_response_has_empty_type_meta() {
    let fc = FakeClientBuilder::new()
        .with_scheme(scheme())
        .with_objects([obj(&widget("ns", "a")), obj(&cm("ns", "c"))])
        .with_status_subresource::<Widget>()
        .build_fake();
    let b = fc.client().backend().clone();
    let wi = <Widget as ctrlrt::Object>::resource_info();
    let ci = <ConfigMap as ctrlrt::Object>::resource_info();
    let no_tm = |v: &serde_json::Value, what: &str| {
        assert!(v.get("apiVersion").is_none(), "{what}: {v}");
        assert!(v.get("kind").is_none(), "{what}: {v}");
    };
    no_tm(&b.get(&wi, "ns", "a").await.unwrap(), "get");
    no_tm(&b.get(&ci, "ns", "c").await.unwrap(), "get cm");
    for v in b.list(&wi, &ListOptions::default()).await.unwrap() {
        no_tm(&v, "list");
    }
    let created = b
        .create(
            &wi,
            json!({"apiVersion": "example.com/v1alpha1", "kind": "Widget",
                   "metadata": {"name": "n", "namespace": "ns"}}),
        )
        .await
        .unwrap();
    no_tm(&created, "create");
    let updated = b.update(&wi, created.clone()).await.unwrap();
    no_tm(&updated, "update");
    let updated = b.update_subresource(&wi, "status", updated).await.unwrap();
    no_tm(&updated, "status update");
    let patched = b
        .patch(
            &wi,
            "ns",
            "n",
            &Patch::Merge(json!({"spec": {"size": "s"}})),
        )
        .await
        .unwrap();
    no_tm(&patched, "patch");
    let patched = b
        .patch_subresource(
            &wi,
            "status",
            "ns",
            "n",
            &Patch::Merge(json!({"status": {"phase": "p"}})),
        )
        .await
        .unwrap();
    no_tm(&patched, "status patch");
    // typed readback mirrors Get; the raw stored document keeps the tracker's GVK
    let w: Widget = fc.stored(&key("ns", "n")).unwrap();
    assert_eq!(w.status.phase, "p");
    let raw = fc.stored_value(&wi, &key("ns", "n")).unwrap();
    assert_eq!(raw["kind"], "Widget");
}
