//! Behaviour of `ctrlrt_fake::FakeClientset`, encoding the semantics of
//! `vendor/k8s.io/client-go/kubernetes/fake/clientset_generated.go` (`NewSimpleClientset`),
//! `vendor/k8s.io/client-go/gentype/fake.go` and `vendor/k8s.io/client-go/testing/{fake,fixture}.go`.

mod common;

use common::*;
use ctrlrt::error::{ApiError, is_already_exists, is_bad_request, is_not_found};
use ctrlrt::k8s_openapi::api::apps::v1::DaemonSet;
use ctrlrt::k8s_openapi::api::certificates::v1::{
    CertificateSigningRequest, CertificateSigningRequestCondition, CertificateSigningRequestStatus,
};
use ctrlrt::k8s_openapi::api::core::v1::{Namespace, Node, NodeStatus};
use ctrlrt::{ListOptions, ObjectKey, Patch};
use ctrlrt_fake::{FakeClientsetBuilder, Reaction, new_simple_clientset, obj, verbs};
use serde_json::json;

fn node(name: &str) -> Node {
    Node {
        metadata: meta("", name),
        ..Default::default()
    }
}

fn ns_obj(name: &str) -> Namespace {
    Namespace {
        metadata: meta("", name),
        ..Default::default()
    }
}

/// The plain tracker never touches resourceVersion / uid (tracker.add stores a copy as is).
#[tokio::test]
async fn create_get_without_resource_version() {
    let c = new_simple_clientset(Vec::<&Node>::new());
    let mut n = node("n1");
    c.create(&mut n).await.unwrap();
    assert_eq!(n.metadata.resource_version, None);
    assert_eq!(n.metadata.uid, None);
    let got: Node = c.get(&ObjectKey::cluster("n1")).await.unwrap();
    assert_eq!(got, n);
}

/// Seed objects (NewSimpleClientset(objects...)) are stored unchanged.
#[tokio::test]
async fn seeded_objects() {
    let c = FakeClientsetBuilder::new()
        .with_objects([
            obj(&ns_obj("openshift-doca-hugepages-holder")),
            obj(&node("n")),
        ])
        .build();
    let got: Namespace = c
        .get(&ObjectKey::cluster("openshift-doca-hugepages-holder"))
        .await
        .unwrap();
    assert_eq!(got.metadata.resource_version, None);
    c.get::<Node>(&ObjectKey::cluster("n")).await.unwrap();
}

/// tracker.Add with a kind unknown to the clientset scheme fails (NewSimpleClientset panics).
#[test]
fn seeding_unregistered_kind_fails() {
    let err = FakeClientsetBuilder::new()
        .with_object(&widget("ns", "a"))
        .try_build()
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"no kind is registered for the type v1alpha1.Widget in scheme "k8s.io/client-go/kubernetes/fake/register.go:84""#
    );
}

/// tracker.add: create of an existing object → AlreadyExists; get of missing → NotFound.
#[tokio::test]
async fn already_exists_and_not_found() {
    let c = new_simple_clientset([&ns_obj("x")]);
    let err = c.create(&mut ns_obj("x")).await.unwrap_err();
    assert!(is_already_exists(&err));
    assert_eq!(err.to_string(), r#"namespaces "x" already exists"#);
    let err = c
        .get::<Namespace>(&ObjectKey::cluster("y"))
        .await
        .unwrap_err();
    assert!(is_not_found(&err));
    assert_eq!(err.to_string(), r#"namespaces "y" not found"#);
}

/// tracker.Update: replace without any resourceVersion check; missing → NotFound.
#[tokio::test]
async fn update_has_no_conflict_detection() {
    let c = new_simple_clientset([&node("n")]);
    let mut stale = node("n");
    stale.metadata.resource_version = Some("12345".into());
    stale.metadata.labels = labels([("a", "b")]);
    c.update(&mut stale).await.unwrap();
    assert_eq!(stale.metadata.resource_version.as_deref(), Some("12345"));
    let got: Node = c.get(&ObjectKey::cluster("n")).await.unwrap();
    assert_eq!(got.metadata.labels, labels([("a", "b")]));

    let err = c.update(&mut node("missing")).await.unwrap_err();
    assert!(is_not_found(&err));
    assert_eq!(err.to_string(), r#"nodes "missing" not found"#);
}

/// objectTrackerReact.Update ignores the subresource: UpdateStatus / UpdateApproval replace
/// the whole object (no status-subresource emulation).
#[tokio::test]
async fn update_approval_replaces_whole_object() {
    let csr = CertificateSigningRequest {
        metadata: meta("", "csr-1"),
        ..Default::default()
    };
    let fc = FakeClientsetBuilder::new().with_object(&csr).build_fake();
    let c = fc.client();
    let mut csr: CertificateSigningRequest = c.get(&ObjectKey::cluster("csr-1")).await.unwrap();
    csr.metadata.labels = labels([("changed", "too")]);
    csr.status = Some(CertificateSigningRequestStatus {
        conditions: Some(vec![CertificateSigningRequestCondition {
            type_: "Approved".into(),
            status: "True".into(),
            reason: Some("AutoApproved".into()),
            ..Default::default()
        }]),
        ..Default::default()
    });
    c.update_subresource(&mut csr, "approval").await.unwrap();
    let got: CertificateSigningRequest = c.get(&ObjectKey::cluster("csr-1")).await.unwrap();
    assert_eq!(got.metadata.labels, labels([("changed", "too")]));
    assert_eq!(got.status.unwrap().conditions.unwrap()[0].type_, "Approved");

    // the recorded action is an update of certificatesigningrequests/approval
    let a = fc
        .actions()
        .into_iter()
        .find(|a| a.verb == verbs::UPDATE)
        .unwrap();
    assert_eq!(a.resource.resource, "certificatesigningrequests");
    assert_eq!(a.subresource, "approval");
    assert_eq!(a.namespace, "");

    // UpdateApproval of a missing CSR → NotFound
    let mut missing = CertificateSigningRequest {
        metadata: meta("", "nope"),
        ..Default::default()
    };
    let err = c
        .update_subresource(&mut missing, "approval")
        .await
        .unwrap_err();
    assert!(is_not_found(&err));
}

/// tracker.Delete removes immediately (finalizers ignored); missing → NotFound.
#[tokio::test]
async fn delete_ignores_finalizers() {
    let mut n = node("n");
    n.metadata.finalizers = Some(vec!["f".into()]);
    let c = new_simple_clientset([&n]);
    c.delete(&n).await.unwrap();
    assert!(is_not_found(
        &c.get::<Node>(&ObjectKey::cluster("n")).await.unwrap_err()
    ));
    assert!(is_not_found(&c.delete(&n).await.unwrap_err()));
}

/// List: namespace filter (sorted) + alsoFakeLister label filtering.
#[tokio::test]
async fn list_with_namespace_and_labels() {
    let mut a = DaemonSet {
        metadata: meta("ns1", "b"),
        ..Default::default()
    };
    a.metadata.labels = labels([("app", "x")]);
    let b = DaemonSet {
        metadata: meta("ns1", "a"),
        ..Default::default()
    };
    let mut d = DaemonSet {
        metadata: meta("ns2", "a"),
        ..Default::default()
    };
    d.metadata.labels = labels([("app", "x")]);
    let c = new_simple_clientset([&a, &b, &d]);
    let names = |v: Vec<DaemonSet>| -> Vec<String> {
        v.into_iter()
            .map(|d| {
                format!(
                    "{}/{}",
                    d.metadata.namespace.unwrap(),
                    d.metadata.name.unwrap()
                )
            })
            .collect()
    };
    assert_eq!(
        names(c.list_all().await.unwrap()),
        ["ns1/a", "ns1/b", "ns2/a"]
    );
    assert_eq!(
        names(
            c.list(&ListOptions::new().in_namespace("ns1"))
                .await
                .unwrap()
        ),
        ["ns1/a", "ns1/b"]
    );
    assert_eq!(
        names(
            c.list(&ListOptions::new().matching_labels([("app", "x")]))
                .await
                .unwrap()
        ),
        ["ns1/b", "ns2/a"]
    );
}

/// ensureNamespace / ensureDaemonSet pattern (internal/controller/ovs-hugepages tests):
/// an unchanged CreateOrUpdate does not write, so the (empty) resourceVersion is unchanged.
#[tokio::test]
async fn create_or_update_is_idempotent() {
    let c = new_simple_clientset([&ns_obj("holder")]);
    let mk = || DaemonSet {
        metadata: meta("holder", "ds"),
        ..Default::default()
    };
    let mut ds = mk();
    let r = ctrlrt::create_or_update(&c, &mut ds, |d| {
        d.metadata.labels = labels([("app", "hp")]);
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(r, ctrlrt::OperationResult::Created);
    let before: DaemonSet = c.get(&ObjectKey::new("holder", "ds")).await.unwrap();
    let mut ds = mk();
    let r = ctrlrt::create_or_update(&c, &mut ds, |d| {
        d.metadata.labels = labels([("app", "hp")]);
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(r, ctrlrt::OperationResult::None);
    let after: DaemonSet = c.get(&ObjectKey::new("holder", "ds")).await.unwrap();
    assert_eq!(
        after.metadata.resource_version,
        before.metadata.resource_version
    );
    assert_eq!(c.list_all::<DaemonSet>().await.unwrap().len(), 1);
}

/// PrependReactor: inject an error for a verb/resource; actions are recorded in order.
#[tokio::test]
async fn prepend_reactor_injects_error() {
    let fc = FakeClientsetBuilder::new().build_fake();
    fc.prepend_reactor("create", "namespaces", |_| {
        Reaction::error(ApiError::forbidden("", "namespaces", "x", "nope"))
    });
    let c = fc.client();
    let err = c.create(&mut ns_obj("x")).await.unwrap_err();
    assert_eq!(err.to_string(), r#"namespaces "x" is forbidden: nope"#);
    // other resources still hit the tracker
    c.create(&mut node("n")).await.unwrap();
    let verbs: Vec<(String, String)> = fc
        .actions()
        .iter()
        .map(|a| (a.verb.clone(), a.resource.resource.clone()))
        .collect();
    assert_eq!(
        verbs,
        [
            ("create".to_string(), "namespaces".to_string()),
            ("create".to_string(), "nodes".to_string())
        ]
    );
    fc.clear_actions();
    assert!(fc.actions().is_empty());
}

/// Reactors matching `resource/subresource`, NotHandled fallthrough, nil-object results.
#[tokio::test]
async fn reactor_matching_and_fallthrough() {
    let csr = CertificateSigningRequest {
        metadata: meta("", "c"),
        ..Default::default()
    };
    let fc = FakeClientsetBuilder::new().with_object(&csr).build_fake();
    fc.prepend_reactor("update", "certificatesigningrequests/approval", |_| {
        Reaction::error(anyhow::anyhow!("approval blocked"))
    });
    fc.prepend_reactor("*", "*", |_| Reaction::NotHandled);
    let c = fc.client();
    let mut got: CertificateSigningRequest = c.get(&ObjectKey::cluster("c")).await.unwrap();
    // plain update is not covered by "certificatesigningrequests/approval"
    c.update(&mut got).await.unwrap();
    let err = c
        .update_subresource(&mut got, "approval")
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "approval blocked");

    // A reaction handled with a nil object yields an empty object / empty list.
    fc.prepend_reactor(verbs::GET, "nodes", |_| Reaction::Handled(Ok(None)));
    fc.prepend_reactor(verbs::LIST, "nodes", |_| Reaction::Handled(Ok(None)));
    let n: Node = c.get(&ObjectKey::cluster("anything")).await.unwrap();
    assert_eq!(n, Node::default());
    assert!(c.list_all::<Node>().await.unwrap().is_empty());
}

/// A reactor returning objects for list is still subject to label filtering.
#[tokio::test]
async fn list_reaction_items_are_label_filtered() {
    let fc = FakeClientsetBuilder::new().build_fake();
    fc.prepend_reactor(verbs::LIST, "nodes", |_| {
        Reaction::object(json!({"items": [
            {"metadata": {"name": "a", "labels": {"role": "dpu"}}},
            {"metadata": {"name": "b"}}
        ]}))
    });
    let c = fc.client();
    let got: Vec<Node> = c
        .list(&ListOptions::new().matching_labels([("role", "dpu")]))
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].metadata.name.as_deref(), Some("a"));
}

/// objectTrackerReact.Patch: merge patch applied and stored, no resourceVersion bump.
#[tokio::test]
async fn patch_merges_without_resource_version() {
    let c = new_simple_clientset([&node("n")]);
    let mut n = node("n");
    c.patch(
        &mut n,
        &Patch::Merge(json!({"metadata": {"labels": {"x": "y"}}, "status": {"phase": "Running"}})),
    )
    .await
    .unwrap();
    assert_eq!(n.metadata.resource_version, None);
    assert_eq!(n.metadata.labels, labels([("x", "y")]));
    assert_eq!(
        n.status,
        Some(NodeStatus {
            phase: Some("Running".into()),
            ..Default::default()
        })
    );
    let err = c
        .patch(&mut node("missing"), &Patch::Merge(json!({})))
        .await
        .unwrap_err();
    assert!(is_not_found(&err));
}

/// tracker.add: a namespace on an object addressed through a cluster-scoped client is a
/// BadRequest ("request namespace does not match object namespace").
#[tokio::test]
async fn namespace_mismatch_is_bad_request() {
    let c = new_simple_clientset(Vec::<&Node>::new());
    let mut n = node("n");
    n.metadata.namespace = Some("oops".into());
    let err = c.create(&mut n).await.unwrap_err();
    assert!(is_bad_request(&err));
    assert_eq!(
        err.to_string(),
        r#"request namespace does not match object namespace, request: "" object: "oops""#
    );
}

/// Objects can be inspected through the tracker.
#[tokio::test]
async fn tracker_access() {
    let fc = FakeClientsetBuilder::new()
        .with_object(&node("n"))
        .build_fake();
    let all = fc.tracker().all();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0.resource, "nodes");
}

/// NewSimpleClientset stores Go typed objects as given (empty TypeMeta) and returns them
/// unchanged, so no response carries apiVersion/kind.
#[tokio::test]
async fn responses_have_empty_type_meta() {
    let fc = FakeClientsetBuilder::new()
        .with_object(&node("seed"))
        .build_fake();
    let b = fc.client().backend().clone();
    let ni = <Node as ctrlrt::Object>::resource_info();
    let no_tm = |v: &serde_json::Value, what: &str| {
        assert!(v.get("apiVersion").is_none(), "{what}: {v}");
        assert!(v.get("kind").is_none(), "{what}: {v}");
    };
    no_tm(&b.get(&ni, "", "seed").await.unwrap(), "get seed");
    let created = b
        .create(
            &ni,
            json!({"apiVersion": "v1", "kind": "Node", "metadata": {"name": "n"}}),
        )
        .await
        .unwrap();
    no_tm(&created, "create");
    no_tm(&b.get(&ni, "", "n").await.unwrap(), "get");
    for v in b.list(&ni, &ListOptions::default()).await.unwrap() {
        no_tm(&v, "list");
    }
    let mut with_tm = created.clone();
    with_tm["apiVersion"] = json!("v1");
    with_tm["kind"] = json!("Node");
    no_tm(&b.update(&ni, with_tm.clone()).await.unwrap(), "update");
    no_tm(
        &b.update_subresource(&ni, "status", with_tm).await.unwrap(),
        "update status",
    );
    no_tm(
        &b.patch(
            &ni,
            "",
            "n",
            &Patch::Merge(json!({"metadata": {"labels": {"a": "b"}}})),
        )
        .await
        .unwrap(),
        "patch",
    );
    // recorded actions carry the typed object without TypeMeta, too
    let a = fc
        .actions()
        .into_iter()
        .find(|a| a.verb == verbs::CREATE)
        .unwrap();
    no_tm(a.object.as_ref().unwrap(), "action object");
    // typed round trip still works
    let n: Node = fc.client().get(&ObjectKey::cluster("n")).await.unwrap();
    assert_eq!(n.metadata.labels, labels([("a", "b")]));
}
