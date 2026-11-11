//! Port of client-go's `testing.ObjectTracker` (`k8s.io/client-go/testing/fixture.go`,
//! type `tracker`), storing JSON documents keyed by GroupVersionResource and
//! namespace/name.
//!
//! Semantics (from `tracker.add` / `Get` / `Delete` / `List` / `filterByNamespace`):
//! * the stored object is always a copy of the caller's object;
//! * an object without namespace inherits the request namespace; a different namespace is a
//!   `BadRequest` (`request namespace does not match object namespace, ...`);
//! * create of an existing key → `AlreadyExists`; update/patch of a missing key → `NotFound`;
//! * the tracker itself never touches `resourceVersion`, `uid`, timestamps or finalizers;
//! * `List` returns objects of the namespace (all namespaces for `""`) sorted by
//!   namespace, then name.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use ctrlrt::ResourceInfo;
use ctrlrt::error::{ApiError, Result};
use serde_json::Value;

use crate::util::{meta_str, set_meta_str};

/// `schema.GroupVersionResource`
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Gvr {
    pub group: String,
    pub version: String,
    pub resource: String,
}

impl Gvr {
    pub fn new(
        group: impl Into<String>,
        version: impl Into<String>,
        resource: impl Into<String>,
    ) -> Self {
        Self {
            group: group.into(),
            version: version.into(),
            resource: resource.into(),
        }
    }

    /// GVR using the resource's real plural (what a typed clientset uses).
    pub fn from_info(info: &ResourceInfo) -> Self {
        Self::new(&info.group, &info.version, &info.plural)
    }

    /// GVR guessed from the kind (`meta.UnsafeGuessKindToResource`), which is what
    /// controller-runtime's fake client and `tracker.Add` use.
    pub fn guess(info: &ResourceInfo) -> Self {
        Self::new(
            &info.group,
            &info.version,
            unsafe_guess_kind_to_resource(&info.kind),
        )
    }

    /// `schema.GroupResource.String()`
    pub fn group_resource(&self) -> String {
        if self.group.is_empty() {
            self.resource.clone()
        } else {
            format!("{}.{}", self.resource, self.group)
        }
    }

    pub(crate) fn not_found(&self, name: &str) -> ApiError {
        ApiError::not_found(&self.group, &self.resource, name)
    }
}

impl fmt::Display for Gvr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // schema.GroupVersionResource.String()
        write!(
            f,
            "{}/{}, Resource={}",
            self.group, self.version, self.resource
        )
    }
}

/// `meta.UnsafeGuessKindToResource(kind)` (plural only).
pub fn unsafe_guess_kind_to_resource(kind: &str) -> String {
    if kind.is_empty() {
        return String::new();
    }
    let singular = kind.to_lowercase();
    // unpluralizedSuffixes
    if singular.ends_with("endpoints") {
        return singular;
    }
    if let Some(stem) = singular.strip_suffix('y') {
        return format!("{stem}ies");
    }
    if singular.ends_with('s') {
        return format!("{singular}es");
    }
    format!("{singular}s")
}

type Store = HashMap<Gvr, BTreeMap<(String, String), Value>>;

/// In-memory object tracker (cheap to clone; clones share state).
#[derive(Clone, Default)]
pub struct ObjectTracker {
    objects: Arc<Mutex<Store>>,
}

impl fmt::Debug for ObjectTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObjectTracker").finish_non_exhaustive()
    }
}

impl ObjectTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Store> {
        self.objects.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `tracker.Get(gvr, ns, name)`
    pub fn get(&self, gvr: &Gvr, ns: &str, name: &str) -> Result<Value> {
        let objs = self.lock();
        objs.get(gvr)
            .and_then(|m| m.get(&(ns.to_string(), name.to_string())))
            .cloned()
            .ok_or_else(|| gvr.not_found(name).into())
    }

    /// `tracker.Create(gvr, obj, ns)`
    pub fn create(&self, gvr: &Gvr, obj: Value, ns: &str) -> Result<()> {
        self.add_at(gvr, obj, ns, false)
    }

    /// `tracker.Update(gvr, obj, ns)`
    pub fn update(&self, gvr: &Gvr, obj: Value, ns: &str) -> Result<()> {
        self.add_at(gvr, obj, ns, true)
    }

    /// `tracker.Patch(gvr, patchedObject, ns)` (stores the already patched object).
    pub fn patch(&self, gvr: &Gvr, obj: Value, ns: &str) -> Result<()> {
        self.add_at(gvr, obj, ns, true)
    }

    /// `tracker.Add(obj)`: the GVR is guessed from the kind, the namespace is the object's.
    pub fn add(&self, info: &ResourceInfo, obj: Value) -> Result<()> {
        let ns = meta_str(&obj, "namespace").to_string();
        self.add_at(&Gvr::guess(info), obj, &ns, false)
    }

    /// `tracker.Delete(gvr, ns, name)`
    pub fn delete(&self, gvr: &Gvr, ns: &str, name: &str) -> Result<()> {
        let mut objs = self.lock();
        let removed = objs
            .get_mut(gvr)
            .and_then(|m| m.remove(&(ns.to_string(), name.to_string())));
        match removed {
            Some(_) => Ok(()),
            None => Err(gvr.not_found(name).into()),
        }
    }

    /// `tracker.List(gvr, gvk, ns)` → items (`filterByNamespace`: sorted by namespace, name).
    pub fn list(&self, gvr: &Gvr, ns: &str) -> Vec<Value> {
        let objs = self.lock();
        let Some(m) = objs.get(gvr) else {
            return Vec::new();
        };
        // BTreeMap keyed by (namespace, name) is already in Go's sort order.
        m.iter()
            .filter(|((ons, _), _)| ns.is_empty() || ons == ns)
            .map(|(_, v)| v.clone())
            .collect()
    }

    /// Every stored object (for assertions / debugging).
    pub fn all(&self) -> Vec<(Gvr, Value)> {
        let objs = self.lock();
        let mut out: Vec<(Gvr, Value)> = objs
            .iter()
            .flat_map(|(gvr, m)| m.values().map(move |v| (gvr.clone(), v.clone())))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// `tracker.add(gvr, obj, ns, replaceExisting)`
    fn add_at(&self, gvr: &Gvr, mut obj: Value, ns: &str, replace_existing: bool) -> Result<()> {
        if !obj.is_object() {
            return Err(anyhow::anyhow!(
                "object does not implement the Object interfaces"
            ));
        }
        // Propagate namespace to the new object if hasn't already been set.
        if meta_str(&obj, "namespace").is_empty() {
            set_meta_str(&mut obj, "namespace", ns);
        }
        let obj_ns = meta_str(&obj, "namespace").to_string();
        if ns != obj_ns {
            return Err(ApiError::bad_request(format!(
                "request namespace does not match object namespace, request: {} object: {}",
                ctrlrt::error::go_quote(ns),
                ctrlrt::error::go_quote(&obj_ns)
            ))
            .into());
        }
        let name = meta_str(&obj, "name").to_string();
        let mut objs = self.lock();
        let m = objs.entry(gvr.clone()).or_default();
        let key = (obj_ns, name.clone());
        if let Some(slot) = m.get_mut(&key) {
            if replace_existing {
                *slot = obj;
                return Ok(());
            }
            return Err(ApiError::already_exists(&gvr.group, &gvr.resource, &name).into());
        }
        if replace_existing {
            return Err(gvr.not_found(&name).into());
        }
        m.insert(key, obj);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn guess_kind_to_resource_matches_apimachinery() {
        assert_eq!(unsafe_guess_kind_to_resource("ConfigMap"), "configmaps");
        assert_eq!(unsafe_guess_kind_to_resource("Endpoints"), "endpoints");
        assert_eq!(unsafe_guess_kind_to_resource("Ingress"), "ingresses");
        assert_eq!(
            unsafe_guess_kind_to_resource("NetworkPolicy"),
            "networkpolicies"
        );
        assert_eq!(
            unsafe_guess_kind_to_resource("DPFHCPProvisioner"),
            "dpfhcpprovisioners"
        );
        assert_eq!(unsafe_guess_kind_to_resource("Gateway"), "gatewaies");
        assert_eq!(unsafe_guess_kind_to_resource(""), "");
    }

    #[test]
    fn add_semantics() {
        let t = ObjectTracker::new();
        let gvr = Gvr::new("", "v1", "configmaps");
        t.create(&gvr, json!({"metadata": {"name": "a"}}), "ns")
            .unwrap();
        // namespace propagated
        assert_eq!(
            t.get(&gvr, "ns", "a").unwrap()["metadata"]["namespace"],
            "ns"
        );
        let e = t
            .create(&gvr, json!({"metadata": {"name": "a"}}), "ns")
            .unwrap_err();
        assert_eq!(e.to_string(), r#"configmaps "a" already exists"#);
        let e = t
            .update(&gvr, json!({"metadata": {"name": "b"}}), "ns")
            .unwrap_err();
        assert_eq!(e.to_string(), r#"configmaps "b" not found"#);
        let e = t
            .create(
                &gvr,
                json!({"metadata": {"name": "c", "namespace": "x"}}),
                "ns",
            )
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            r#"request namespace does not match object namespace, request: "ns" object: "x""#
        );
        assert!(ctrlrt::error::is_bad_request(&e));
        let e = t.delete(&gvr, "ns", "zzz").unwrap_err();
        assert!(ctrlrt::error::is_not_found(&e));
        t.delete(&gvr, "ns", "a").unwrap();
        assert!(t.list(&gvr, "").is_empty());
    }

    #[test]
    fn list_sorted_by_namespace_then_name() {
        let t = ObjectTracker::new();
        let gvr = Gvr::new("", "v1", "secrets");
        for (ns, n) in [("b", "x"), ("a", "z"), ("a", "y")] {
            t.create(&gvr, json!({"metadata": {"name": n}}), ns)
                .unwrap();
        }
        let names: Vec<String> = t
            .list(&gvr, "")
            .iter()
            .map(|o| format!("{}/{}", meta_str(o, "namespace"), meta_str(o, "name")))
            .collect();
        assert_eq!(names, ["a/y", "a/z", "b/x"]);
        assert_eq!(t.list(&gvr, "a").len(), 2);
    }
}
