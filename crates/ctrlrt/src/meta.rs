//! `k8s.io/apimachinery/pkg/api/meta` condition helpers, `metav1` owner helpers and
//! `controllerutil` finalizer / owner-reference helpers.

use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, OwnerReference, Time};
use k8s_openapi::jiff::Timestamp;

use crate::object::Object;

pub const CONDITION_TRUE: &str = "True";
pub const CONDITION_FALSE: &str = "False";
pub const CONDITION_UNKNOWN: &str = "Unknown";

/// Current time (`metav1.Now()`).
pub fn now() -> Time {
    Time(Timestamp::now())
}

/// The Rust stand-in for Go's zero `metav1.Time{}` (k8s-openapi's `Time::default()` is the
/// Unix epoch). Go's real zero value (`0001-01-01T00:00:00Z`) is also treated as zero.
pub fn zero_time() -> Time {
    Time(Timestamp::UNIX_EPOCH)
}

/// `metav1.Time.IsZero()`
pub fn is_zero_time(t: &Time) -> bool {
    t.0 == Timestamp::UNIX_EPOCH || t.0.as_second() == -62135596800
}

/// Build a condition the way Go code does with a `metav1.Condition{...}` literal
/// (LastTransitionTime left zero so `set_status_condition` fills it in).
pub fn condition(
    type_: impl Into<String>,
    status: impl Into<String>,
    reason: impl Into<String>,
    message: impl Into<String>,
) -> Condition {
    Condition {
        type_: type_.into(),
        status: status.into(),
        reason: reason.into(),
        message: message.into(),
        observed_generation: None,
        last_transition_time: zero_time(),
    }
}

/// Same as [`condition`] with `ObservedGeneration` set.
pub fn condition_gen(
    type_: impl Into<String>,
    status: impl Into<String>,
    reason: impl Into<String>,
    message: impl Into<String>,
    observed_generation: i64,
) -> Condition {
    let mut c = condition(type_, status, reason, message);
    c.observed_generation = if observed_generation == 0 {
        None
    } else {
        Some(observed_generation)
    };
    c
}

fn gen_of(c: &Condition) -> i64 {
    c.observed_generation.unwrap_or(0)
}

/// `meta.SetStatusCondition` — returns `changed`.
pub fn set_status_condition(conditions: &mut Vec<Condition>, mut new: Condition) -> bool {
    let Some(existing) = conditions.iter_mut().find(|c| c.type_ == new.type_) else {
        if is_zero_time(&new.last_transition_time) {
            new.last_transition_time = now();
        }
        conditions.push(new);
        return true;
    };
    let new_gen = gen_of(&new);
    let mut changed = false;
    if existing.status != new.status {
        existing.status = new.status;
        existing.last_transition_time = if !is_zero_time(&new.last_transition_time) {
            new.last_transition_time
        } else {
            now()
        };
        changed = true;
    }
    if existing.reason != new.reason {
        existing.reason = new.reason;
        changed = true;
    }
    if existing.message != new.message {
        existing.message = new.message;
        changed = true;
    }
    if gen_of(existing) != new_gen {
        existing.observed_generation = Some(new_gen).filter(|g| *g != 0);
        changed = true;
    }
    changed
}

/// `meta.SetStatusCondition` on an `Option<Vec<Condition>>` field (k8s-openapi style statuses).
pub fn set_status_condition_opt(conditions: &mut Option<Vec<Condition>>, new: Condition) -> bool {
    set_status_condition(conditions.get_or_insert_with(Vec::new), new)
}

/// `meta.RemoveStatusCondition` — returns `removed`.
pub fn remove_status_condition(conditions: &mut Vec<Condition>, type_: &str) -> bool {
    let before = conditions.len();
    conditions.retain(|c| c.type_ != type_);
    before != conditions.len()
}

/// `meta.FindStatusCondition`
pub fn find_status_condition<'a>(
    conditions: &'a [Condition],
    type_: &str,
) -> Option<&'a Condition> {
    conditions.iter().find(|c| c.type_ == type_)
}

/// Mutable variant of [`find_status_condition`].
pub fn find_status_condition_mut<'a>(
    conditions: &'a mut [Condition],
    type_: &str,
) -> Option<&'a mut Condition> {
    conditions.iter_mut().find(|c| c.type_ == type_)
}

/// `meta.IsStatusConditionTrue`
pub fn is_status_condition_true(conditions: &[Condition], type_: &str) -> bool {
    is_status_condition_present_and_equal(conditions, type_, CONDITION_TRUE)
}

/// `meta.IsStatusConditionFalse`
pub fn is_status_condition_false(conditions: &[Condition], type_: &str) -> bool {
    is_status_condition_present_and_equal(conditions, type_, CONDITION_FALSE)
}

/// `meta.IsStatusConditionPresentAndEqual`
pub fn is_status_condition_present_and_equal(
    conditions: &[Condition],
    type_: &str,
    status: &str,
) -> bool {
    conditions
        .iter()
        .any(|c| c.type_ == type_ && c.status == status)
}

// ---------------------------------------------------------------------------------------------
// Finalizers (controllerutil)
// ---------------------------------------------------------------------------------------------

/// `controllerutil.ContainsFinalizer`
pub fn contains_finalizer<K: Object>(obj: &K, finalizer: &str) -> bool {
    obj.finalizers().iter().any(|f| f == finalizer)
}

/// `controllerutil.AddFinalizer` — returns whether finalizers were updated.
pub fn add_finalizer<K: Object>(obj: &mut K, finalizer: &str) -> bool {
    if contains_finalizer(obj, finalizer) {
        return false;
    }
    obj.object_meta_mut()
        .finalizers
        .get_or_insert_with(Vec::new)
        .push(finalizer.to_string());
    true
}

/// `controllerutil.RemoveFinalizer` — returns whether finalizers were updated.
pub fn remove_finalizer<K: Object>(obj: &mut K, finalizer: &str) -> bool {
    let Some(f) = obj.object_meta_mut().finalizers.as_mut() else {
        return false;
    };
    let before = f.len();
    f.retain(|x| x != finalizer);
    before != f.len()
}

// ---------------------------------------------------------------------------------------------
// Owner references
// ---------------------------------------------------------------------------------------------

/// `metav1.GetControllerOf`
pub fn get_controller_of<K: Object>(obj: &K) -> Option<&OwnerReference> {
    obj.owner_references()
        .iter()
        .find(|r| r.controller == Some(true))
}

/// `metav1.IsControlledBy(obj, owner)` — compares controller ref UID with owner UID.
pub fn is_controlled_by<K: Object, O: Object>(obj: &K, owner: &O) -> bool {
    match get_controller_of(obj) {
        Some(r) => r.uid == owner.uid(),
        None => false,
    }
}

/// `schema.ParseGroupVersion(apiVersion).Group` (returns None when unparsable).
fn group_of_api_version(api_version: &str) -> Option<String> {
    if api_version.is_empty() || api_version == "/" {
        return Some(String::new());
    }
    let parts: Vec<&str> = api_version.split('/').collect();
    match parts.len() {
        1 => Some(String::new()),
        2 => Some(parts[0].to_string()),
        _ => None,
    }
}

fn refer_same_object(a: &OwnerReference, b: &OwnerReference) -> bool {
    let (Some(ag), Some(bg)) = (
        group_of_api_version(&a.api_version),
        group_of_api_version(&b.api_version),
    ) else {
        return false;
    };
    ag == bg && a.kind == b.kind && a.name == b.name
}

fn upsert_owner_ref<K: Object>(r: OwnerReference, obj: &mut K) {
    let owners = obj
        .object_meta_mut()
        .owner_references
        .get_or_insert_with(Vec::new);
    if let Some(i) = owners.iter().position(|o| refer_same_object(o, &r)) {
        owners[i] = r;
    } else {
        owners.push(r);
    }
}

fn validate_owner<O: Object, K: Object>(owner: &O, obj: &K) -> crate::Result<()> {
    let owner_ns = owner.namespace();
    if !owner_ns.is_empty() {
        let obj_ns = obj.namespace();
        if obj_ns.is_empty() {
            return Err(anyhow::anyhow!(
                "cluster-scoped resource must not have a namespace-scoped owner, owner's namespace {}",
                owner_ns
            ));
        }
        if owner_ns != obj_ns {
            return Err(anyhow::anyhow!(
                "cross-namespace owner references are disallowed, owner's namespace {}, obj's namespace {}",
                owner_ns,
                obj_ns
            ));
        }
    }
    Ok(())
}

/// `controllerutil.AlreadyOwnedError`
#[derive(Debug, Clone)]
pub struct AlreadyOwnedError {
    pub object_namespace: String,
    pub object_name: String,
    pub owner: OwnerReference,
}

impl std::fmt::Display for AlreadyOwnedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Object {}/{} is already owned by another {} controller {}",
            self.object_namespace, self.object_name, self.owner.kind, self.owner.name
        )
    }
}
impl std::error::Error for AlreadyOwnedError {}

fn owner_ref_for<O: Object>(owner: &O, controller: bool) -> OwnerReference {
    let info = O::resource_info();
    OwnerReference {
        api_version: info.api_version(),
        kind: info.kind,
        name: owner.name().to_string(),
        uid: owner.uid().to_string(),
        block_owner_deletion: Some(true),
        controller: if controller { Some(true) } else { None },
    }
}

/// `controllerutil.SetControllerReference(owner, controlled, scheme)`
pub fn set_controller_reference<O: Object, K: Object>(
    owner: &O,
    controlled: &mut K,
) -> crate::Result<()> {
    validate_owner(owner, controlled)?;
    let r = owner_ref_for(owner, true);
    if let Some(existing) = get_controller_of(controlled)
        && !refer_same_object(existing, &r)
    {
        return Err(AlreadyOwnedError {
            object_namespace: controlled.namespace().to_string(),
            object_name: controlled.name().to_string(),
            owner: existing.clone(),
        }
        .into());
    }
    upsert_owner_ref(r, controlled);
    Ok(())
}

/// `controllerutil.SetOwnerReference(owner, object, scheme)` (non-controller reference).
pub fn set_owner_reference<O: Object, K: Object>(owner: &O, obj: &mut K) -> crate::Result<()> {
    validate_owner(owner, obj)?;
    let mut r = owner_ref_for(owner, false);
    r.block_owner_deletion = None;
    upsert_owner_ref(r, obj);
    Ok(())
}

/// `controllerutil.HasControllerReference`
pub fn has_controller_reference<K: Object>(obj: &K) -> bool {
    get_controller_of(obj).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_condition_semantics() {
        let mut conds = vec![];
        assert!(set_status_condition(
            &mut conds,
            condition("Ready", CONDITION_FALSE, "A", "m")
        ));
        assert!(!is_zero_time(&conds[0].last_transition_time));
        let ltt = conds[0].last_transition_time.clone();
        assert!(!set_status_condition(
            &mut conds,
            condition("Ready", CONDITION_FALSE, "A", "m")
        ));
        assert!(set_status_condition(
            &mut conds,
            condition("Ready", CONDITION_FALSE, "B", "m")
        ));
        assert_eq!(conds[0].last_transition_time, ltt);
        assert!(set_status_condition(
            &mut conds,
            condition("Ready", CONDITION_TRUE, "B", "m")
        ));
        assert!(is_status_condition_true(&conds, "Ready"));
        assert!(remove_status_condition(&mut conds, "Ready"));
        assert!(conds.is_empty());
    }
}
