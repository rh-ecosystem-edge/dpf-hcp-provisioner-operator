//! `k8s.io/client-go/testing` actions and reactors (`Action`, `ReactionFunc`,
//! `SimpleReactor`, `Fake.PrependReactor` / `AddReactor`), shared by both fakes.

use std::sync::Arc;

use ctrlrt::error::Result;
use ctrlrt::{DeleteOptions, ListOptions, Patch, ResourceInfo};
use serde_json::Value;

use crate::tracker::Gvr;

/// Verb strings used by client-go actions.
pub mod verbs {
    pub const GET: &str = "get";
    pub const LIST: &str = "list";
    pub const CREATE: &str = "create";
    pub const UPDATE: &str = "update";
    pub const PATCH: &str = "patch";
    pub const DELETE: &str = "delete";
}

/// A recorded / intercepted client call (`testing.Action`).
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    /// `get`, `list`, `create`, `update`, `patch`, `delete`
    pub verb: String,
    pub resource: Gvr,
    /// Kind information of the typed call.
    pub info: ResourceInfo,
    /// Subresource (`status`, `approval`, ...) or empty.
    pub subresource: String,
    pub namespace: String,
    /// Name for get/patch/delete and for create/update (taken from the object).
    pub name: String,
    /// Body for create/update.
    pub object: Option<Value>,
    pub patch: Option<Patch>,
    pub list_options: Option<ListOptions>,
    pub delete_options: Option<DeleteOptions>,
}

impl Action {
    /// `action.Matches(verb, resource)`
    pub fn matches(&self, verb: &str, resource: &str) -> bool {
        self.verb == verb && self.resource.resource == resource
    }
}

/// Result of a reaction (`handled, ret, err`).
pub enum Reaction {
    /// `handled == false`: continue with the next reactor in the chain.
    NotHandled,
    /// `handled == true`: return this result. `Ok(None)` mirrors a nil object, i.e. the
    /// caller receives an empty object (for `list`: no items). For `list` the value may be
    /// an array of items or a list document with `items`.
    Handled(Result<Option<Value>>),
}

impl Reaction {
    /// Handled with an error.
    pub fn error(err: impl Into<ctrlrt::Error>) -> Self {
        Reaction::Handled(Err(err.into()))
    }
    /// Handled with an object.
    pub fn object(v: Value) -> Self {
        Reaction::Handled(Ok(Some(v)))
    }
}

/// `testing.ReactionFunc`
pub type ReactionFn = Arc<dyn Fn(&Action) -> Reaction + Send + Sync>;

/// `testing.SimpleReactor`
#[derive(Clone)]
pub(crate) struct SimpleReactor {
    pub verb: String,
    pub resource: String,
    pub reaction: ReactionFn,
}

impl SimpleReactor {
    /// `SimpleReactor.Handles` + `resourceCovers`
    pub fn handles(&self, action: &Action) -> bool {
        if !(self.verb == "*" || self.verb == action.verb) {
            return false;
        }
        resource_covers(&self.resource, action)
    }
}

fn resource_covers(resource: &str, action: &Action) -> bool {
    if resource == "*" || resource == action.resource.resource {
        return true;
    }
    if let Some((res, sub)) = resource.split_once('/') {
        return res == action.resource.resource && sub == action.subresource;
    }
    false
}

/// Run the user reactors; `None` when no reactor handled the action.
pub(crate) fn run_reactors(
    reactors: &[SimpleReactor],
    action: &Action,
) -> Option<Result<Option<Value>>> {
    for r in reactors {
        if !r.handles(action) {
            continue;
        }
        match (r.reaction)(action) {
            Reaction::NotHandled => continue,
            Reaction::Handled(res) => return Some(res),
        }
    }
    None
}

/// Normalize a list reaction value into items.
pub(crate) fn list_items(v: Option<Value>) -> Vec<Value> {
    match v {
        Some(Value::Array(items)) => items,
        Some(Value::Object(mut m)) => match m.remove("items") {
            Some(Value::Array(items)) => items,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}
