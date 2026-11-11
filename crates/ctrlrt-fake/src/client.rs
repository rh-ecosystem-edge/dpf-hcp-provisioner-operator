//! Port of `sigs.k8s.io/controller-runtime/pkg/client/fake` (v0.22.4, `client.go`).
//!
//! The Go fake wraps client-go's field-managed `ObjectTracker` in a `versionedTracker` that
//! adds resourceVersion handling, status-subresource emulation and finalizer-aware deletion.
//! This module reproduces that layering on top of [`ObjectTracker`]:
//!
//! | Go | here |
//! |---|---|
//! | `versionedTracker.Add` | [`FakeClientBuilder::try_build`] (initial objects) |
//! | `versionedTracker.Create` | `FakeClient::tracker_create` |
//! | `versionedTracker.update` / `updateObject` | `FakeClient::versioned_update` / `update_object` |
//! | `fakeClient.Get/List/Create/Update/Patch/Delete` | [`ClientBackend`] impl |
//! | `fakeSubResourceClient` | `update_subresource` / `patch_subresource` |
//! | `fakeClient.deleteObjectLocked` | `FakeClient::delete_object_locked` |
//!
//! Notable Go behaviours kept on purpose:
//! * GVRs (and thus error messages) use `meta.UnsafeGuessKindToResource`, not the real plural;
//! * initial objects get resourceVersion `"999"`, created objects `"1"`, each write `+1`;
//! * no `uid`, `creationTimestamp` or `generation` is ever set;
//! * responses (get, list items, create/update/patch results) are typed objects, so
//!   `ensureTypeMeta` clears TypeMeta: no top-level `apiVersion`/`kind` (stored documents keep
//!   the GVK stamped by the field-managed tracker);
//! * a type not registered in the scheme fails every call with
//!   `no kind is registered for the type <pkg>.<Kind> in scheme "<name>"`.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use ctrlrt::error::{ApiError, NoKindMatchError, Result, StatusCause};
use ctrlrt::{
    Client, ClientBackend, DeleteOptions, ListOptions, Object, ObjectKey, Patch, ResourceInfo,
};
use serde_json::Value;

use crate::patch::apply_patch;
use crate::reactor::{
    Action, Reaction, ReactionFn, SimpleReactor, list_items, run_reactors, verbs,
};
use crate::scheme::Scheme;
use crate::tracker::{Gvr, ObjectTracker};
use crate::util::{
    clear_type_meta, deletion_timestamp, deletion_timestamp_equal, finalizer_count, labels,
    meta_str, now_rfc3339, parse_uint, rand_string, remove_meta, set_meta_str, stamp_type_meta,
};

const MAX_NAME_LENGTH: usize = 63;
const RANDOM_LENGTH: usize = 5;
const MAX_GENERATED_NAME_LENGTH: usize = MAX_NAME_LENGTH - RANDOM_LENGTH;
const SUBRESOURCE_SCALE: &str = "scale";

/// `trackerAddResourceVersion`: the "magic" resourceVersion given to initial objects.
pub const TRACKER_ADD_RESOURCE_VERSION: &str = "999";

/// GVK identity of a kind.
type Gvk = (String, String, String);

fn gvk_of(info: &ResourceInfo) -> Gvk {
    (info.group.clone(), info.version.clone(), info.kind.clone())
}

/// An object to seed the fake with (`client.Object` / `runtime.Object` in Go): its kind
/// plus its JSON document.
#[derive(Clone, Debug)]
pub struct InitObject {
    info: ResourceInfo,
    value: std::result::Result<Value, String>,
}

impl InitObject {
    /// From a typed object.
    pub fn new<K: Object>(obj: &K) -> Self {
        Self {
            info: K::resource_info(),
            value: ctrlrt::client::to_value(obj).map_err(|e| e.to_string()),
        }
    }

    /// From a typed object, mirroring Go's in-place mutation of the passed pointer by
    /// `versionedTracker.Add`: an empty resourceVersion is set to `"999"` on `obj` too.
    pub fn tracked<K: Object>(obj: &mut K) -> Self {
        if obj.resource_version().is_empty() {
            obj.object_meta_mut().resource_version = Some(TRACKER_ADD_RESOURCE_VERSION.into());
        }
        Self::new(&*obj)
    }

    /// From an untyped document (`unstructured.Unstructured`).
    pub fn from_value(info: ResourceInfo, value: Value) -> Self {
        Self {
            info,
            value: Ok(value),
        }
    }

    pub fn info(&self) -> &ResourceInfo {
        &self.info
    }

    pub(crate) fn value_result(&self) -> std::result::Result<Value, String> {
        self.value.clone()
    }

    fn describe(&self) -> String {
        match &self.value {
            Ok(v) => format!(
                "{} {}/{}",
                self.info.kind,
                meta_str(v, "namespace"),
                meta_str(v, "name")
            ),
            Err(_) => self.info.kind.clone(),
        }
    }
}

impl<K: Object> From<&K> for InitObject {
    fn from(obj: &K) -> Self {
        InitObject::new(obj)
    }
}

impl<K: Object> From<&mut K> for InitObject {
    fn from(obj: &mut K) -> Self {
        InitObject::tracked(obj)
    }
}

/// Shorthand for [`InitObject::new`] (handy for heterogeneous `with_objects` lists).
pub fn obj<K: Object>(o: &K) -> InitObject {
    InitObject::new(o)
}

/// Shorthand for [`InitObject::tracked`] (Go-like: sets resourceVersion `"999"` on `o`).
pub fn obj_mut<K: Object>(o: &mut K) -> InitObject {
    InitObject::tracked(o)
}

/// `fake.ClientBuilder`
#[derive(Default)]
pub struct FakeClientBuilder {
    scheme: Option<Scheme>,
    init_objects: Vec<InitObject>,
    init_lists: Vec<InitObject>,
    init_runtime_objects: Vec<InitObject>,
    with_status_subresource: Vec<ResourceInfo>,
    unserved: Vec<ResourceInfo>,
    reactors: Vec<SimpleReactor>,
    return_managed_fields: bool,
}

/// `fake.NewClientBuilder()`
pub fn new_client_builder() -> FakeClientBuilder {
    FakeClientBuilder::new()
}

/// `fake.NewFakeClient(initObjs...)` (client-go scheme).
pub fn new_fake_client<I>(objs: I) -> Client
where
    I: IntoIterator,
    I::Item: Into<InitObject>,
{
    FakeClientBuilder::new().with_runtime_objects(objs).build()
}

impl FakeClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// `WithScheme`. If not set, defaults to client-go's `scheme.Scheme` ([`Scheme::client_go`]).
    pub fn with_scheme(mut self, scheme: Scheme) -> Self {
        self.scheme = Some(scheme);
        self
    }

    /// `WithObjects`
    pub fn with_objects<I>(mut self, objs: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<InitObject>,
    {
        self.init_objects.extend(objs.into_iter().map(Into::into));
        self
    }

    /// `WithObjects` with a single typed object.
    pub fn with_object<K: Object>(mut self, obj: &K) -> Self {
        self.init_objects.push(InitObject::new(obj));
        self
    }

    /// `WithObjects` with a single typed object, setting resourceVersion `"999"` on the
    /// caller's object when empty (Go's `Add` mutates the object it is given).
    pub fn with_object_mut<K: Object>(mut self, obj: &mut K) -> Self {
        self.init_objects.push(InitObject::tracked(obj));
        self
    }

    /// `WithLists` (items are added after `WithObjects` and before `WithRuntimeObjects`).
    pub fn with_list<K: Object>(mut self, items: &[K]) -> Self {
        self.init_lists.extend(items.iter().map(InitObject::new));
        self
    }

    /// `WithRuntimeObjects`
    pub fn with_runtime_objects<I>(mut self, objs: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<InitObject>,
    {
        self.init_runtime_objects
            .extend(objs.into_iter().map(Into::into));
        self
    }

    /// `WithStatusSubresource(&T{})`
    pub fn with_status_subresource<K: Object>(mut self) -> Self {
        self.with_status_subresource.push(K::resource_info());
        self
    }

    /// `WithStatusSubresource(obj)` with type inference from an instance.
    pub fn with_status_subresource_for<K: Object>(self, _obj: &K) -> Self {
        self.with_status_subresource::<K>()
    }

    /// `WithStatusSubresource` for a kind given as runtime info.
    pub fn with_status_subresource_info(mut self, info: ResourceInfo) -> Self {
        self.with_status_subresource.push(info);
        self
    }

    /// Simulate a kind that is registered in the scheme but not served by the API server
    /// (CRD not installed): every call for it fails with [`NoKindMatchError`], like the
    /// real client's RESTMapper would (Go: `WithRESTMapper` with a mapper lacking the kind).
    pub fn with_unserved<K: Object>(self) -> Self {
        self.with_unserved_info(K::resource_info())
    }

    /// [`Self::with_unserved`] for a kind given as runtime info.
    pub fn with_unserved_info(mut self, info: ResourceInfo) -> Self {
        self.unserved.push(info);
        self
    }

    /// `WithReturnManagedFields`
    pub fn with_return_managed_fields(mut self) -> Self {
        self.return_managed_fields = true;
        self
    }

    /// Intercept calls (the analogue of `WithInterceptorFuncs`, expressed as a client-go
    /// style reactor). `verb` / `resource` may be `"*"`; `resource` is the guessed plural
    /// (optionally `plural/subresource`). Reactors run before any other processing, in
    /// registration order.
    pub fn with_reactor<F>(mut self, verb: &str, resource: &str, f: F) -> Self
    where
        F: Fn(&Action) -> Reaction + Send + Sync + 'static,
    {
        self.reactors.push(SimpleReactor {
            verb: verb.into(),
            resource: resource.into(),
            reaction: Arc::new(f),
        });
        self
    }

    /// `Build()`, returning errors instead of panicking.
    pub fn try_build(self) -> Result<FakeClient> {
        let scheme = self.scheme.unwrap_or_else(Scheme::client_go);
        let mut status: BTreeSet<Gvk> = in_tree_resources_with_status();
        for info in &self.with_status_subresource {
            if !scheme.recognizes(info) {
                let err = scheme.not_registered(info, false);
                return Err(anyhow::anyhow!(
                    "failed to get gvk for object {}: {}",
                    info.kind,
                    err
                ));
            }
            status.insert(gvk_of(info));
        }
        let fc = FakeClient {
            inner: Arc::new(Inner {
                tracker: ObjectTracker::new(),
                scheme,
                status,
                unserved: self.unserved.iter().map(gvk_of).collect(),
                write_lock: Mutex::new(()),
                reactors: Mutex::new(self.reactors),
                return_managed_fields: self.return_managed_fields,
            }),
        };
        let groups = [
            ("object", self.init_objects),
            ("list", self.init_lists),
            ("runtime object", self.init_runtime_objects),
        ];
        for (what, objs) in groups {
            for o in objs {
                if let Err(e) = fc.versioned_add(&o) {
                    return Err(anyhow::anyhow!(
                        "failed to add {} {} to fake client: {}",
                        what,
                        o.describe(),
                        e
                    ));
                }
            }
        }
        Ok(fc)
    }

    /// `Build()` returning the fake handle. Panics on invalid seed objects, like Go.
    pub fn build_fake(self) -> FakeClient {
        match self.try_build() {
            Ok(c) => c,
            Err(e) => panic!("{e}"),
        }
    }

    /// `Build()`. Panics on invalid seed objects, like Go.
    pub fn build(self) -> Client {
        self.build_fake().client()
    }
}

struct Inner {
    tracker: ObjectTracker,
    scheme: Scheme,
    status: BTreeSet<Gvk>,
    unserved: BTreeSet<Gvk>,
    /// `trackerWriteLock`
    write_lock: Mutex<()>,
    reactors: Mutex<Vec<SimpleReactor>>,
    return_managed_fields: bool,
}

/// The in-memory backend with controller-runtime fake client semantics. Cheap to clone
/// (shared state); safe to use from many tasks.
#[derive(Clone)]
pub struct FakeClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for FakeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeClient").finish_non_exhaustive()
    }
}

impl FakeClient {
    /// The typed client facade over this fake.
    pub fn client(&self) -> Client {
        Client::new(Arc::new(self.clone()))
    }

    /// The underlying object tracker (raw stored documents, keyed by guessed GVR).
    pub fn tracker(&self) -> &ObjectTracker {
        &self.inner.tracker
    }

    /// `fakeClient.Scheme()`
    pub fn scheme(&self) -> &Scheme {
        &self.inner.scheme
    }

    /// Read back the stored document of an object, bypassing scheme checks and reactors.
    pub fn stored_value(&self, info: &ResourceInfo, key: &ObjectKey) -> Option<Value> {
        let ns = if info.namespaced {
            key.namespace.as_str()
        } else {
            ""
        };
        self.inner
            .tracker
            .get(&Gvr::guess(info), ns, &key.name)
            .ok()
            .map(|mut v| {
                stamp_type_meta(&mut v, info);
                v
            })
    }

    /// Read back a stored object as `K` (None when missing or not decodable).
    /// TypeMeta is cleared like on a `Get`.
    pub fn stored<K: Object>(&self, key: &ObjectKey) -> Option<K> {
        self.stored_value(&K::resource_info(), key)
            .and_then(|mut v| {
                clear_type_meta(&mut v);
                ctrlrt::client::from_value(v).ok()
            })
    }

    /// All stored objects of kind `K` (sorted by namespace, name).
    pub fn stored_all<K: Object>(&self) -> Vec<K> {
        let info = K::resource_info();
        self.inner
            .tracker
            .list(&Gvr::guess(&info), "")
            .into_iter()
            .filter_map(|mut v| {
                clear_type_meta(&mut v);
                ctrlrt::client::from_value(v).ok()
            })
            .collect()
    }

    /// Insert a reactor in front of the chain (see [`FakeClientBuilder::with_reactor`]).
    pub fn prepend_reactor<F>(&self, verb: &str, resource: &str, f: F)
    where
        F: Fn(&Action) -> Reaction + Send + Sync + 'static,
    {
        let r = SimpleReactor {
            verb: verb.into(),
            resource: resource.into(),
            reaction: Arc::new(f) as ReactionFn,
        };
        self.reactors().insert(0, r);
    }

    /// Append a reactor to the chain.
    pub fn add_reactor<F>(&self, verb: &str, resource: &str, f: F)
    where
        F: Fn(&Action) -> Reaction + Send + Sync + 'static,
    {
        self.reactors().push(SimpleReactor {
            verb: verb.into(),
            resource: resource.into(),
            reaction: Arc::new(f),
        });
    }

    fn reactors(&self) -> MutexGuard<'_, Vec<SimpleReactor>> {
        self.inner
            .reactors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn write_lock(&self) -> MutexGuard<'_, ()> {
        self.inner
            .write_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn react(&self, action: Action) -> Option<Result<Option<Value>>> {
        let reactors = self.reactors().clone();
        if reactors.is_empty() {
            return None;
        }
        run_reactors(&reactors, &action)
    }

    /// `apiutil.GVKForObject` (scheme lookup) + RESTMapper availability.
    fn check_kind(&self, info: &ResourceInfo, list: bool) -> Result<()> {
        if !self.inner.scheme.recognizes(info) {
            return Err(self.inner.scheme.not_registered(info, list).into());
        }
        if self.inner.unserved.contains(&gvk_of(info)) {
            return Err(NoKindMatchError {
                group: info.group.clone(),
                kind: info.kind.clone(),
                searched_versions: vec![info.version.clone()],
            }
            .into());
        }
        Ok(())
    }

    /// Prepare an object for returning to the caller (typed objects): `ensureTypeMeta`
    /// clears apiVersion/kind, managedFields are dropped unless `WithReturnManagedFields`.
    fn output(&self, _info: &ResourceInfo, mut v: Value) -> Value {
        clear_type_meta(&mut v);
        if !self.inner.return_managed_fields {
            remove_meta(&mut v, "managedFields");
        }
        v
    }

    fn has_status_subresource(&self, info: &ResourceInfo) -> bool {
        self.inner.status.contains(&gvk_of(info))
    }

    /// `versionedTracker.Add` for one object.
    fn versioned_add(&self, o: &InitObject) -> Result<()> {
        let mut v = o.value.clone().map_err(|e| anyhow::anyhow!(e))?;
        if deletion_timestamp(&v).is_some() && finalizer_count(&v) == 0 {
            return Err(anyhow::anyhow!(
                "refusing to create obj {} with metadata.deletionTimestamp but no finalizers",
                meta_str(&v, "name")
            ));
        }
        if meta_str(&v, "resourceVersion").is_empty() {
            set_meta_str(&mut v, "resourceVersion", TRACKER_ADD_RESOURCE_VERSION);
        }
        // tracker.Add → scheme.ObjectKinds(obj)
        if !self.inner.scheme.recognizes(&o.info) {
            return Err(self.inner.scheme.not_registered(&o.info, false).into());
        }
        normalize_scope(&o.info, &mut v);
        stamp_type_meta(&mut v, &o.info);
        self.inner.tracker.add(&o.info, v)
    }

    /// `versionedTracker.Create`
    fn tracker_create(&self, info: &ResourceInfo, gvr: &Gvr, mut obj: Value) -> Result<Value> {
        let name = meta_str(&obj, "name").to_string();
        if name.is_empty() {
            return Err(name_required(info, &name).into());
        }
        if !meta_str(&obj, "resourceVersion").is_empty() {
            return Err(ApiError::bad_request(
                "resourceVersion can not be set for Create requests",
            )
            .into());
        }
        set_meta_str(&mut obj, "resourceVersion", "1");
        // managedFieldObjectTracker.Create stamps the GVK
        stamp_type_meta(&mut obj, info);
        let ns = meta_str(&obj, "namespace").to_string();
        self.inner.tracker.create(gvr, obj.clone(), &ns)?;
        Ok(obj)
    }

    /// `versionedTracker.update`: returns the object as the caller sees it afterwards.
    fn versioned_update(
        &self,
        info: &ResourceInfo,
        gvr: &Gvr,
        obj: Value,
        is_status: bool,
        deleting: bool,
        patch_rv_default: bool,
    ) -> Result<Value> {
        let ns = meta_str(&obj, "namespace").to_string();
        let (obj, store) =
            self.update_object(info, gvr, obj, &ns, is_status, deleting, patch_rv_default)?;
        if store {
            // managedFieldObjectTracker.Update → tracker.Update
            self.inner.tracker.update(gvr, obj.clone(), &ns)?;
        }
        Ok(obj)
    }

    /// `versionedTracker.updateObject`. Returns the processed object and whether it must
    /// still be stored (false when it was created or deleted instead).
    #[allow(clippy::too_many_arguments)]
    fn update_object(
        &self,
        info: &ResourceInfo,
        gvr: &Gvr,
        mut obj: Value,
        ns: &str,
        is_status: bool,
        deleting: bool,
        patch_rv_default: bool,
    ) -> Result<(Value, bool)> {
        let name = meta_str(&obj, "name").to_string();
        if name.is_empty() {
            return Err(name_required(info, &name).into());
        }

        let old = match self.inner.tracker.get(gvr, ns, &name) {
            Ok(o) => o,
            Err(e) => {
                // If the resource is not found and the resource allows create on update,
                // issue a create instead.
                if ctrlrt::error::is_not_found(&e) && allows_create_on_update(info) {
                    let created = self.tracker_create(info, gvr, obj)?;
                    return Ok((created, false));
                }
                return Err(e);
            }
        };

        if self.has_status_subresource(info) {
            if is_status {
                // copy everything but status and metadata.ResourceVersion from original object
                let passed_rv = meta_str(&obj, "resourceVersion").to_string();
                let mut merged = old.clone();
                copy_status(&obj, &mut merged);
                obj = merged;
                set_meta_str(&mut obj, "resourceVersion", &passed_rv);
            } else {
                // copy status from original object
                copy_status(&old, &mut obj);
            }
        } else if is_status {
            return Err(gvr.not_found(&name).into());
        }

        // If the new object does not have the resource version set and it allows
        // unconditional update, default it to the resource version of the existing resource
        let old_rv = meta_str(&old, "resourceVersion").to_string();
        if meta_str(&obj, "resourceVersion").is_empty()
            && (allows_unconditional_update(info) || patch_rv_default)
        {
            set_meta_str(&mut obj, "resourceVersion", &old_rv);
        }
        if meta_str(&obj, "resourceVersion") != old_rv {
            return Err(ApiError::conflict(
                &gvr.group,
                &gvr.resource,
                &name,
                "object was modified",
            )
            .into());
        }
        let old_rv = if old_rv.is_empty() {
            "0".to_string()
        } else {
            old_rv
        };
        let rv = parse_uint(&old_rv).map_err(|e| {
            anyhow::anyhow!(
                "can not convert resourceVersion {} to int: {}",
                ctrlrt::error::go_quote(&old_rv),
                e
            )
        })?;
        set_meta_str(&mut obj, "resourceVersion", &(rv + 1).to_string());

        if !deleting && !deletion_timestamp_equal(&obj, &old) {
            return Err(anyhow::anyhow!(
                "error: Unable to edit {}: metadata.deletionTimestamp field is immutable",
                name
            ));
        }

        if deletion_timestamp(&obj).is_some() && finalizer_count(&obj) == 0 {
            let obj_ns = meta_str(&obj, "namespace").to_string();
            self.inner.tracker.delete(gvr, &obj_ns, &name)?;
            return Ok((obj, false));
        }
        Ok((obj, true))
    }

    /// `fakeClient.update(obj, isStatus)`
    fn do_update(&self, info: &ResourceInfo, mut obj: Value, is_status: bool) -> Result<Value> {
        self.check_kind(info, false)?;
        let gvr = Gvr::guess(info);
        normalize_scope(info, &mut obj);
        let _guard = self.write_lock();
        // Retain managed fields
        remove_meta(&mut obj, "managedFields");
        let ns = meta_str(&obj, "namespace").to_string();
        let name = meta_str(&obj, "name").to_string();
        if let Ok(current) = self.inner.tracker.get(&gvr, &ns, &name)
            && let Some(mf) = current.get("metadata").and_then(|m| m.get("managedFields"))
            && let Some(md) = crate::util::meta_mut(&mut obj)
        {
            md.insert("managedFields".into(), mf.clone());
        }
        let out = self.versioned_update(info, &gvr, obj, is_status, false, false)?;
        Ok(self.output(info, out))
    }

    /// `fakeClient.patch` (status = called through the status subresource client,
    /// `main = called through fakeClient.Patch` which enables RV defaulting).
    fn do_patch(
        &self,
        info: &ResourceInfo,
        ns: &str,
        name: &str,
        patch: &Patch,
        is_status: bool,
        main: bool,
    ) -> Result<Value> {
        self.check_kind(info, false)?;
        let gvr = Gvr::guess(info);
        let ns = if info.namespaced { ns } else { "" };
        let _guard = self.write_lock();
        let old = self.inner.tracker.get(&gvr, ns, name)?;

        // dryPatch: validate that deletionTimestamp has not been changed
        let dry = apply_patch(&old, patch)?;
        check_patched_is_object(info, &dry)?;
        if !deletion_timestamp_equal(&dry, &old) {
            return Err(anyhow::anyhow!(
                "rejected patch, metadata.deletionTimestamp immutable"
            ));
        }

        // testing.ObjectReaction → objectTrackerReact.Patch → versionedTracker.Patch
        let current = self.inner.tracker.get(&gvr, ns, name)?;
        let mut patched = apply_patch(&current, patch)?;
        check_patched_is_object(info, &patched)?;
        normalize_scope(info, &mut patched);
        let pns = meta_str(&patched, "namespace").to_string();
        let (obj, store) = self.update_object(info, &gvr, patched, &pns, is_status, false, main)?;
        if store {
            self.inner.tracker.patch(&gvr, obj.clone(), ns)?;
        }
        Ok(self.output(info, obj))
    }

    /// `fakeClient.deleteObjectLocked`
    fn delete_object_locked(
        &self,
        info: &ResourceInfo,
        gvr: &Gvr,
        ns: &str,
        name: &str,
    ) -> Result<()> {
        if let Ok(mut old) = self.inner.tracker.get(gvr, ns, name)
            && finalizer_count(&old) > 0
        {
            set_meta_str(&mut old, "deletionTimestamp", &now_rfc3339());
            // Call update directly with mutability parameter set to true to allow
            // changes to deletionTimestamp
            self.versioned_update(info, gvr, old, false, true, false)?;
            return Ok(());
        }
        self.inner.tracker.delete(gvr, ns, name)
    }
}

/// Cluster-scoped objects never carry a namespace (the API server clears it).
fn normalize_scope(info: &ResourceInfo, obj: &mut Value) {
    if !info.namespaced {
        remove_meta(obj, "namespace");
    }
}

/// `apierrors.NewInvalid(gvk.GroupKind(), name, field.Required("metadata.name", "name is required"))`
fn name_required(info: &ResourceInfo, name: &str) -> ApiError {
    ApiError::invalid(
        &info.group,
        &info.kind,
        name,
        vec![StatusCause {
            type_: "FieldValueRequired".into(),
            message: "Required value: name is required".into(),
            field: "metadata.name".into(),
        }],
    )
}

/// After patching, Go unmarshals the result into the typed object, which fails for
/// non-object documents.
fn check_patched_is_object(info: &ResourceInfo, v: &Value) -> Result<()> {
    if v.is_object() {
        return Ok(());
    }
    let what = match v {
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "bool",
        _ => "null",
    };
    Err(anyhow::anyhow!(
        "json: cannot unmarshal {} into Go value of type {}.{}",
        what,
        info.version,
        info.kind
    ))
}

/// `copyStatusFrom(old, new)`: `new["status"] = old["status"]`.
fn copy_status(from: &Value, to: &mut Value) {
    let Some(map) = to.as_object_mut() else {
        return;
    };
    match from.get("status") {
        Some(s) if !s.is_null() => {
            map.insert("status".into(), s.clone());
        }
        _ => {
            map.remove("status");
        }
    }
}

/// `allowsUnconditionalUpdate(gvk)` (group names exactly as in Go, quirks included).
fn allows_unconditional_update(info: &ResourceInfo) -> bool {
    let k = info.kind.as_str();
    match info.group.as_str() {
        "apps" => matches!(
            k,
            "ControllerRevision" | "DaemonSet" | "Deployment" | "ReplicaSet" | "StatefulSet"
        ),
        "autoscaling" => k == "HorizontalPodAutoscaler",
        "batch" => matches!(k, "CronJob" | "Job"),
        "certificates" => k == "Certificates",
        "flowcontrol" => matches!(k, "FlowSchema" | "PriorityLevelConfiguration"),
        "networking" => matches!(k, "Ingress" | "IngressClass" | "NetworkPolicy"),
        "policy" => k == "PodSecurityPolicy",
        "rbac.authorization.k8s.io" => {
            matches!(
                k,
                "ClusterRole" | "ClusterRoleBinding" | "Role" | "RoleBinding"
            )
        }
        "scheduling" => k == "PriorityClass",
        "settings" => k == "PodPreset",
        "storage" => k == "StorageClass",
        "" => matches!(
            k,
            "ConfigMap"
                | "Endpoint"
                | "Event"
                | "LimitRange"
                | "Namespace"
                | "Node"
                | "PersistentVolume"
                | "PersistentVolumeClaim"
                | "Pod"
                | "PodTemplate"
                | "ReplicationController"
                | "ResourceQuota"
                | "Secret"
                | "Service"
                | "ServiceAccount"
                | "EndpointSlice"
        ),
        _ => false,
    }
}

/// `allowsCreateOnUpdate(gvk)` (group names exactly as in Go, quirks included).
fn allows_create_on_update(info: &ResourceInfo) -> bool {
    let k = info.kind.as_str();
    match info.group.as_str() {
        "coordination" => k == "Lease",
        "node" => k == "RuntimeClass",
        "rbac" => matches!(
            k,
            "ClusterRole" | "ClusterRoleBinding" | "Role" | "RoleBinding"
        ),
        "" => matches!(k, "Endpoint" | "Event" | "LimitRange" | "Service"),
        _ => false,
    }
}

/// `inTreeResourcesWithStatus()`
fn in_tree_resources_with_status() -> BTreeSet<Gvk> {
    [
        ("", "v1", "Namespace"),
        ("", "v1", "Node"),
        ("", "v1", "PersistentVolumeClaim"),
        ("", "v1", "PersistentVolume"),
        ("", "v1", "Pod"),
        ("", "v1", "ReplicationController"),
        ("", "v1", "Service"),
        ("apps", "v1", "Deployment"),
        ("apps", "v1", "DaemonSet"),
        ("apps", "v1", "ReplicaSet"),
        ("apps", "v1", "StatefulSet"),
        ("autoscaling", "v1", "HorizontalPodAutoscaler"),
        ("batch", "v1", "CronJob"),
        ("batch", "v1", "Job"),
        ("certificates.k8s.io", "v1", "CertificateSigningRequest"),
        ("networking.k8s.io", "v1", "Ingress"),
        ("networking.k8s.io", "v1", "NetworkPolicy"),
        ("policy", "v1", "PodDisruptionBudget"),
        ("storage.k8s.io", "v1", "VolumeAttachment"),
        ("apiextensions.k8s.io", "v1", "CustomResourceDefinition"),
        ("flowcontrol.apiserver.k8s.io", "v1beta2", "FlowSchema"),
        (
            "flowcontrol.apiserver.k8s.io",
            "v1beta2",
            "PriorityLevelConfiguration",
        ),
        ("flowcontrol.apiserver.k8s.io", "v1", "FlowSchema"),
        (
            "flowcontrol.apiserver.k8s.io",
            "v1",
            "PriorityLevelConfiguration",
        ),
    ]
    .into_iter()
    .map(|(g, v, k)| (g.to_string(), v.to_string(), k.to_string()))
    .collect()
}

fn action(verb: &str, info: &ResourceInfo, sub: &str, ns: &str, name: &str) -> Action {
    Action {
        verb: verb.into(),
        resource: Gvr::guess(info),
        info: info.clone(),
        subresource: sub.into(),
        namespace: ns.into(),
        name: name.into(),
        object: None,
        patch: None,
        list_options: None,
        delete_options: None,
    }
}

/// A reactor's result is handed back as-is (like an interceptor writing into `obj`); a nil
/// object yields an empty object.
fn reacted_object(_info: &ResourceInfo, v: Option<Value>) -> Value {
    v.unwrap_or_else(|| serde_json::json!({"metadata": {}}))
}

#[async_trait]
impl ClientBackend for FakeClient {
    /// `fakeClient.Get`
    async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value> {
        if let Some(r) = self.react(action(verbs::GET, res, "", namespace, name)) {
            return r.map(|v| reacted_object(res, v));
        }
        self.check_kind(res, false)?;
        let ns = if res.namespaced { namespace } else { "" };
        let o = self.inner.tracker.get(&Gvr::guess(res), ns, name)?;
        Ok(self.output(res, o))
    }

    /// `fakeClient.List`
    async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>> {
        let mut a = action(verbs::LIST, res, "", &opts.namespace, "");
        a.list_options = Some(opts.clone());
        if let Some(r) = self.react(a) {
            return r.map(|v| {
                list_items(v)
                    .into_iter()
                    .map(|i| reacted_object(res, Some(i)))
                    .collect()
            });
        }
        self.check_kind(res, true)?;
        let ns = if res.namespaced {
            opts.namespace.as_str()
        } else {
            ""
        };
        let items = self.inner.tracker.list(&Gvr::guess(res), ns);
        Ok(items
            .into_iter()
            .filter(|o| opts.labels.is_empty() || opts.labels_match(Some(&labels(o))))
            .map(|o| self.output(res, o))
            .collect())
    }

    /// `fakeClient.Create`
    async fn create(&self, res: &ResourceInfo, mut obj: Value) -> Result<Value> {
        let mut a = action(
            verbs::CREATE,
            res,
            "",
            meta_str(&obj, "namespace"),
            meta_str(&obj, "name"),
        );
        a.object = Some(obj.clone());
        if let Some(r) = self.react(a) {
            return r.map(|v| reacted_object(res, v));
        }
        self.check_kind(res, false)?;
        let gvr = Gvr::guess(res);
        normalize_scope(res, &mut obj);
        if meta_str(&obj, "name").is_empty() && !meta_str(&obj, "generateName").is_empty() {
            let mut base = meta_str(&obj, "generateName").to_string();
            if base.len() > MAX_GENERATED_NAME_LENGTH {
                let mut cut = MAX_GENERATED_NAME_LENGTH;
                while !base.is_char_boundary(cut) {
                    cut -= 1;
                }
                base.truncate(cut);
            }
            let name = format!("{}{}", base, rand_string(RANDOM_LENGTH));
            set_meta_str(&mut obj, "name", &name);
        }
        // Ignore attempts to set deletion timestamp
        remove_meta(&mut obj, "deletionTimestamp");

        let _guard = self.write_lock();
        let out = self.tracker_create(res, &gvr, obj)?;
        Ok(self.output(res, out))
    }

    /// `fakeClient.Update`
    async fn update(&self, res: &ResourceInfo, obj: Value) -> Result<Value> {
        let mut a = action(
            verbs::UPDATE,
            res,
            "",
            meta_str(&obj, "namespace"),
            meta_str(&obj, "name"),
        );
        a.object = Some(obj.clone());
        if let Some(r) = self.react(a) {
            return r.map(|v| reacted_object(res, v));
        }
        self.do_update(res, obj, false)
    }

    /// `fakeSubResourceClient.Update` (`Status().Update`, `SubResource(x).Update`).
    async fn update_subresource(
        &self,
        res: &ResourceInfo,
        subresource: &str,
        obj: Value,
    ) -> Result<Value> {
        let mut a = action(
            verbs::UPDATE,
            res,
            subresource,
            meta_str(&obj, "namespace"),
            meta_str(&obj, "name"),
        );
        a.object = Some(obj.clone());
        if let Some(r) = self.react(a) {
            return r.map(|v| reacted_object(res, v));
        }
        if subresource == SUBRESOURCE_SCALE {
            // The scale subresource needs a SubResourceBody (autoscalingv1.Scale), which this
            // API cannot pass: Go fetches the object first, then rejects the missing body.
            let ns = meta_str(&obj, "namespace").to_string();
            let name = meta_str(&obj, "name").to_string();
            self.get(res, &ns, &name).await?;
            return Err(ApiError::bad_request("missing SubResourceBody").into());
        }
        self.do_update(res, obj, true)
    }

    /// `fakeClient.Patch`
    async fn patch(
        &self,
        res: &ResourceInfo,
        namespace: &str,
        name: &str,
        patch: &Patch,
    ) -> Result<Value> {
        let mut a = action(verbs::PATCH, res, "", namespace, name);
        a.patch = Some(patch.clone());
        if let Some(r) = self.react(a) {
            return r.map(|v| reacted_object(res, v));
        }
        self.do_patch(res, namespace, name, patch, false, true)
    }

    /// `fakeSubResourceClient.Patch` (`Status().Patch` → statusPatch; other subresources
    /// behave like a plain patch of the main resource).
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
        if let Some(r) = self.react(a) {
            return r.map(|v| reacted_object(res, v));
        }
        self.do_patch(res, namespace, name, patch, subresource == "status", false)
    }

    /// `fakeClient.Delete`
    async fn delete(
        &self,
        res: &ResourceInfo,
        namespace: &str,
        name: &str,
        opts: &DeleteOptions,
    ) -> Result<()> {
        let mut a = action(verbs::DELETE, res, "", namespace, name);
        a.delete_options = Some(opts.clone());
        if let Some(r) = self.react(a) {
            return r.map(|_| ());
        }
        self.check_kind(res, false)?;
        let gvr = Gvr::guess(res);
        let ns = if res.namespaced { namespace } else { "" };
        let _guard = self.write_lock();
        // Check the ResourceVersion if that Precondition was specified.
        if let Some(expect_rv) = &opts.precondition_resource_version {
            let db_obj = self.inner.tracker.get(&gvr, ns, name)?;
            let actual_rv = meta_str(&db_obj, "resourceVersion");
            if actual_rv != expect_rv {
                let msg = format!(
                    "the ResourceVersion in the precondition ({expect_rv}) does not match the ResourceVersion in record ({actual_rv}). The object might have been modified"
                );
                return Err(ApiError::conflict(&gvr.group, &gvr.resource, name, msg).into());
            }
        }
        self.delete_object_locked(res, &gvr, ns, name)
    }
}
