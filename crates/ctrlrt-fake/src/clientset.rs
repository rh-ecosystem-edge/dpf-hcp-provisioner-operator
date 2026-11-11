//! Port of `k8s.io/client-go/kubernetes/fake` (`NewSimpleClientset`) as a [`ClientBackend`].
//!
//! Every call becomes a client-go `testing.Action` that is recorded and run through the
//! reaction chain (`testing.Fake.Invokes`). The chain initially holds the `"*", "*"`
//! `testing.ObjectReaction(tracker)` reactor over a *plain* `ObjectTracker`, so — unlike
//! controller-runtime's fake — there is:
//! * no resourceVersion handling at all (no `"999"`/`"1"`, no conflicts, RV stays as given);
//! * no name validation, no `generateName` handling;
//! * no finalizer handling: `delete` removes the object immediately, preconditions ignored;
//! * no status-subresource emulation: `update_subresource` (`UpdateStatus`,
//!   `UpdateApproval`, ...) replaces the whole object like `update`;
//! * no scheme check on calls (typed clientsets only exist for built-in kinds);
//! * objects are stored and returned as given, i.e. with the empty TypeMeta of Go typed
//!   objects: responses carry no top-level `apiVersion`/`kind`.
//!
//! Resources are addressed with the kind's real plural (as the generated typed clients do),
//! whereas seed objects are added with `tracker.Add`, i.e. under the *guessed* plural —
//! identical for all built-in kinds.
//!
//! `NewClientset` (field-managed tracker) differs only by populating `managedFields` and
//! supporting server-side apply; neither is used by the operator's tests.

use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use ctrlrt::error::Result;
use ctrlrt::{Client, ClientBackend, DeleteOptions, ListOptions, Object, Patch, ResourceInfo};
use serde_json::Value;

use crate::client::InitObject;
use crate::patch::apply_patch;
use crate::reactor::{Action, Reaction, ReactionFn, SimpleReactor, list_items, verbs};
use crate::scheme::Scheme;
use crate::tracker::{Gvr, ObjectTracker};
use crate::util::{clear_type_meta, labels, meta_str};

/// Builder for [`FakeClientset`] (`fake.NewSimpleClientset(objects...)`).
#[derive(Default)]
pub struct FakeClientsetBuilder {
    objects: Vec<InitObject>,
}

/// `fake.NewSimpleClientset(objects...)` as a typed [`Client`].
pub fn new_simple_clientset<I>(objects: I) -> Client
where
    I: IntoIterator,
    I::Item: Into<InitObject>,
{
    FakeClientsetBuilder::new().with_objects(objects).build()
}

impl FakeClientsetBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed objects (`NewSimpleClientset(objects...)`).
    pub fn with_objects<I>(mut self, objs: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<InitObject>,
    {
        self.objects.extend(objs.into_iter().map(Into::into));
        self
    }

    /// Seed a single typed object.
    pub fn with_object<K: Object>(mut self, obj: &K) -> Self {
        self.objects.push(InitObject::new(obj));
        self
    }

    /// Build, returning errors instead of panicking.
    pub fn try_build(self) -> Result<FakeClientset> {
        let scheme = Scheme::clientset_fake();
        let tracker = ObjectTracker::new();
        for o in self.objects {
            // tracker.Add → scheme.ObjectKinds(obj)
            if !scheme.recognizes(o.info()) {
                return Err(scheme.not_registered(o.info(), false).into());
            }
            let mut v = o.value_result().map_err(|e| anyhow::anyhow!(e))?;
            if !o.info().namespaced {
                crate::util::remove_meta(&mut v, "namespace");
            }
            // Go's typed seed objects normally carry an empty TypeMeta.
            clear_type_meta(&mut v);
            tracker.add(o.info(), v)?;
        }
        Ok(FakeClientset {
            inner: Arc::new(CsInner {
                tracker,
                chain: Mutex::new(vec![ChainEntry::ObjectReaction]),
                actions: Mutex::new(Vec::new()),
            }),
        })
    }

    /// Build the fake handle (panics on invalid seed objects, like Go).
    pub fn build_fake(self) -> FakeClientset {
        match self.try_build() {
            Ok(c) => c,
            Err(e) => panic!("{e}"),
        }
    }

    /// Build a typed [`Client`] (panics on invalid seed objects, like Go).
    pub fn build(self) -> Client {
        self.build_fake().client()
    }
}

#[derive(Clone)]
enum ChainEntry {
    /// `testing.ObjectReaction(tracker)` registered for `"*", "*"`.
    ObjectReaction,
    User(SimpleReactor),
}

struct CsInner {
    tracker: ObjectTracker,
    chain: Mutex<Vec<ChainEntry>>,
    actions: Mutex<Vec<Action>>,
}

/// In-memory clientset fake. Cheap to clone (shared state); safe to use from many tasks.
#[derive(Clone)]
pub struct FakeClientset {
    inner: Arc<CsInner>,
}

impl std::fmt::Debug for FakeClientset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeClientset").finish_non_exhaustive()
    }
}

impl FakeClientset {
    /// The typed client facade over this fake.
    pub fn client(&self) -> Client {
        Client::new(Arc::new(self.clone()))
    }

    /// `clientset.Tracker()`
    pub fn tracker(&self) -> &ObjectTracker {
        &self.inner.tracker
    }

    /// `Fake.PrependReactor(verb, resource, reaction)`
    pub fn prepend_reactor<F>(&self, verb: &str, resource: &str, f: F)
    where
        F: Fn(&Action) -> Reaction + Send + Sync + 'static,
    {
        let r = SimpleReactor {
            verb: verb.into(),
            resource: resource.into(),
            reaction: Arc::new(f) as ReactionFn,
        };
        self.chain().insert(0, ChainEntry::User(r));
    }

    /// `Fake.AddReactor(verb, resource, reaction)` (runs after the default object
    /// reaction, which handles every verb — so only reached for unhandled actions).
    pub fn add_reactor<F>(&self, verb: &str, resource: &str, f: F)
    where
        F: Fn(&Action) -> Reaction + Send + Sync + 'static,
    {
        let r = SimpleReactor {
            verb: verb.into(),
            resource: resource.into(),
            reaction: Arc::new(f) as ReactionFn,
        };
        self.chain().push(ChainEntry::User(r));
    }

    /// `Fake.Actions()`
    pub fn actions(&self) -> Vec<Action> {
        self.actions_lock().clone()
    }

    /// `Fake.ClearActions()`
    pub fn clear_actions(&self) {
        self.actions_lock().clear();
    }

    fn chain(&self) -> MutexGuard<'_, Vec<ChainEntry>> {
        self.inner.chain.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn actions_lock(&self) -> MutexGuard<'_, Vec<Action>> {
        self.inner.actions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `Fake.Invokes(action, defaultReturnObj)`; `Ok(None)` = default (empty) object.
    fn invokes(&self, action: Action) -> Result<Option<Value>> {
        self.actions_lock().push(action.clone());
        let chain = self.chain().clone();
        for entry in chain {
            match entry {
                ChainEntry::ObjectReaction => return self.object_reaction(&action),
                ChainEntry::User(r) => {
                    if !r.handles(&action) {
                        continue;
                    }
                    match (r.reaction)(&action) {
                        Reaction::NotHandled => continue,
                        Reaction::Handled(res) => return res,
                    }
                }
            }
        }
        Ok(None)
    }

    /// `testing.ObjectReaction(tracker)`
    fn object_reaction(&self, action: &Action) -> Result<Option<Value>> {
        let t = &self.inner.tracker;
        let gvr = &action.resource;
        let ns = action.namespace.as_str();
        match action.verb.as_str() {
            verbs::GET => t.get(gvr, ns, &action.name).map(Some),
            verbs::LIST => Ok(Some(Value::Array(t.list(gvr, ns)))),
            verbs::CREATE => {
                let obj = action.object.clone().unwrap_or(Value::Null);
                let name = meta_str(&obj, "name").to_string();
                t.create(gvr, obj, ns)?;
                t.get(gvr, ns, &name).map(Some)
            }
            verbs::UPDATE => {
                let obj = action.object.clone().unwrap_or(Value::Null);
                let name = meta_str(&obj, "name").to_string();
                t.update(gvr, obj, ns)?;
                t.get(gvr, ns, &name).map(Some)
            }
            verbs::DELETE => t.delete(gvr, ns, &action.name).map(|_| None),
            verbs::PATCH => {
                let obj = t.get(gvr, ns, &action.name)?;
                let Some(patch) = &action.patch else {
                    return Err(anyhow::anyhow!("PatchType  is not supported"));
                };
                let patched = apply_patch(&obj, patch)?;
                if !patched.is_object() {
                    return Err(anyhow::anyhow!(
                        "json: cannot unmarshal into Go value of type {}.{}",
                        action.info.version,
                        action.info.kind
                    ));
                }
                t.patch(gvr, patched.clone(), ns)?;
                Ok(Some(patched))
            }
            other => Err(anyhow::anyhow!("no reaction implemented for {}", other)),
        }
    }
}

fn request_ns(info: &ResourceInfo, ns: &str) -> String {
    if info.namespaced {
        ns.to_string()
    } else {
        String::new()
    }
}

fn action(verb: &str, info: &ResourceInfo, sub: &str, ns: &str, name: &str) -> Action {
    Action {
        verb: verb.into(),
        resource: Gvr::from_info(info),
        info: info.clone(),
        subresource: sub.into(),
        namespace: request_ns(info, ns),
        name: name.into(),
        object: None,
        patch: None,
        list_options: None,
        delete_options: None,
    }
}

/// Typed clients return `emptyResult` when the reaction yields a nil object; otherwise the
/// reaction's object is returned as-is.
fn output(_info: &ResourceInfo, v: Option<Value>) -> Value {
    v.unwrap_or_else(|| serde_json::json!({"metadata": {}}))
}

impl FakeClientset {
    fn write(&self, verb: &str, res: &ResourceInfo, sub: &str, mut obj: Value) -> Result<Value> {
        // Go typed objects passed to a clientset carry an empty TypeMeta (`ctrlrt::client::to_value`
        // always stamps it), and the plain tracker stores and returns them unchanged.
        clear_type_meta(&mut obj);
        let mut a = action(
            verb,
            res,
            sub,
            meta_str(&obj, "namespace"),
            meta_str(&obj, "name"),
        );
        a.object = Some(obj);
        self.invokes(a).map(|v| output(res, v))
    }
}

#[async_trait]
impl ClientBackend for FakeClientset {
    async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value> {
        self.invokes(action(verbs::GET, res, "", namespace, name))
            .map(|v| output(res, v))
    }

    async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>> {
        let mut a = action(verbs::LIST, res, "", &opts.namespace, "");
        a.list_options = Some(opts.clone());
        let items = list_items(self.invokes(a)?);
        // alsoFakeLister.List filters the reaction's items with the label selector.
        Ok(items
            .into_iter()
            .filter(|o| opts.labels_match(Some(&labels(o))))
            .map(|o| output(res, Some(o)))
            .collect())
    }

    async fn create(&self, res: &ResourceInfo, obj: Value) -> Result<Value> {
        self.write(verbs::CREATE, res, "", obj)
    }

    async fn update(&self, res: &ResourceInfo, obj: Value) -> Result<Value> {
        self.write(verbs::UPDATE, res, "", obj)
    }

    async fn update_subresource(
        &self,
        res: &ResourceInfo,
        subresource: &str,
        obj: Value,
    ) -> Result<Value> {
        self.write(verbs::UPDATE, res, subresource, obj)
    }

    async fn patch(
        &self,
        res: &ResourceInfo,
        namespace: &str,
        name: &str,
        patch: &Patch,
    ) -> Result<Value> {
        let mut a = action(verbs::PATCH, res, "", namespace, name);
        a.patch = Some(patch.clone());
        self.invokes(a).map(|v| output(res, v))
    }

    async fn patch_subresource(
        &self,
        res: &ResourceInfo,
        subresource: &str,
        namespace: &str,
        name: &str,
        patch: &Patch,
    ) -> Result<Value> {
        let mut a = action(verbs::PATCH, res, subresource, namespace, name);
        a.patch = Some(patch.clone());
        self.invokes(a).map(|v| output(res, v))
    }

    async fn delete(
        &self,
        res: &ResourceInfo,
        namespace: &str,
        name: &str,
        opts: &DeleteOptions,
    ) -> Result<()> {
        let mut a = action(verbs::DELETE, res, "", namespace, name);
        a.delete_options = Some(opts.clone());
        self.invokes(a).map(|_| ())
    }
}
