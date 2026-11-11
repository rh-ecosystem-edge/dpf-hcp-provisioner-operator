//! Patch application used by the fakes.
//!
//! * JSON merge patch: port of `gopkg.in/evanphx/json-patch.v4` `MergePatch` (the library
//!   client-go's fixture and controller-runtime's fake use), including its null-pruning
//!   quirks.
//! * Strategic merge patch: Go's `strategicpatch.StrategicMergePatch` is driven by the
//!   `patchStrategy`/`patchMergeKey` struct tags of the Go types. We approximate it with a table
//!   of the well-known merge keys of built-in types (by field name), supporting the
//!   `$patch: replace|delete` and `$deleteFromPrimitiveList/<field>` directives. Lists without
//!   a known merge key are replaced, like for types without patch tags.

use ctrlrt::Patch;
use ctrlrt::error::Result;
use serde_json::{Map, Value};

/// `jsonpatch.ErrBadJSONDoc`
pub const ERR_BAD_JSON_DOC: &str = "Invalid JSON Document";
/// `jsonpatch.ErrBadJSONPatch`
pub const ERR_BAD_JSON_PATCH: &str = "Invalid JSON Patch";

/// Apply a [`Patch`] to `doc`.
pub fn apply_patch(doc: &Value, patch: &Patch) -> Result<Value> {
    match patch {
        Patch::Merge(p) => merge_patch(doc, p),
        Patch::Strategic(p) => strategic_merge_patch(doc, p),
    }
}

/// `jsonpatch.MergePatch(doc, patch)` (evanphx/json-patch v4).
pub fn merge_patch(doc: &Value, patch: &Value) -> Result<Value> {
    if doc.is_null() {
        return Err(anyhow::anyhow!(ERR_BAD_JSON_DOC));
    }
    if patch.is_null() {
        return Err(anyhow::anyhow!(ERR_BAD_JSON_PATCH));
    }
    match (doc, patch) {
        (Value::Object(d), Value::Object(p)) => {
            let mut d = d.clone();
            merge_docs(&mut d, p);
            Ok(Value::Object(d))
        }
        // "Not an error, just not a doc, so we turn straight into the patch"
        (_, Value::Object(p)) => {
            let mut p = p.clone();
            prune_doc_nulls(&mut p);
            Ok(Value::Object(p))
        }
        (_, Value::Array(a)) => {
            let mut a = a.clone();
            prune_ary_nulls(&mut a);
            Ok(Value::Array(a))
        }
        _ => Err(anyhow::anyhow!(ERR_BAD_JSON_PATCH)),
    }
}

fn merge_docs(doc: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for (k, v) in patch {
        if v.is_null() {
            doc.remove(k);
            continue;
        }
        match doc.get_mut(k) {
            None | Some(Value::Null) => {
                let mut v = v.clone();
                prune_nulls(&mut v);
                doc.insert(k.clone(), v);
            }
            Some(cur) => merge(cur, v),
        }
    }
}

fn merge(cur: &mut Value, patch: &Value) {
    let Value::Object(cur_doc) = cur else {
        let mut p = patch.clone();
        prune_nulls(&mut p);
        *cur = p;
        return;
    };
    let Value::Object(patch_doc) = patch else {
        *cur = patch.clone();
        return;
    };
    merge_docs(cur_doc, patch_doc);
}

fn prune_nulls(v: &mut Value) {
    match v {
        Value::Object(m) => prune_doc_nulls(m),
        Value::Array(a) => prune_ary_nulls(a),
        _ => {}
    }
}

fn prune_doc_nulls(m: &mut Map<String, Value>) {
    m.retain(|_, v| !v.is_null());
    for v in m.values_mut() {
        prune_nulls(v);
    }
}

fn prune_ary_nulls(a: &mut [Value]) {
    for v in a.iter_mut() {
        if !v.is_null() {
            prune_nulls(v);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Strategic merge patch (approximation, see module docs)
// ---------------------------------------------------------------------------------------------

enum ListStrategy {
    Replace,
    MergePrimitive,
    MergeByKey(&'static str),
}

fn list_strategy(field: &str, orig: &[Value], patch: &[Value]) -> ListStrategy {
    match field {
        "containers"
        | "initContainers"
        | "ephemeralContainers"
        | "volumes"
        | "env"
        | "imagePullSecrets" => ListStrategy::MergeByKey("name"),
        "volumeMounts" => ListStrategy::MergeByKey("mountPath"),
        "volumeDevices" => ListStrategy::MergeByKey("devicePath"),
        "conditions" | "addresses" => ListStrategy::MergeByKey("type"),
        "ownerReferences" => ListStrategy::MergeByKey("uid"),
        "hostAliases" => ListStrategy::MergeByKey("ip"),
        "topologySpreadConstraints" => ListStrategy::MergeByKey("topologyKey"),
        "ports" => {
            let has_container_port = orig
                .iter()
                .chain(patch.iter())
                .any(|v| v.get("containerPort").is_some());
            if has_container_port {
                ListStrategy::MergeByKey("containerPort")
            } else {
                ListStrategy::MergeByKey("port")
            }
        }
        "finalizers" => ListStrategy::MergePrimitive,
        _ => ListStrategy::Replace,
    }
}

/// Strategic merge patch of `patch` onto `doc` (see module docs for the supported subset).
pub fn strategic_merge_patch(doc: &Value, patch: &Value) -> Result<Value> {
    let Value::Object(p) = patch else {
        return Err(anyhow::anyhow!(ERR_BAD_JSON_PATCH));
    };
    let mut d = match doc {
        Value::Object(d) => d.clone(),
        _ => return Err(anyhow::anyhow!(ERR_BAD_JSON_DOC)),
    };
    smp_map(&mut d, p);
    Ok(Value::Object(d))
}

fn directive(m: &Map<String, Value>) -> Option<&str> {
    m.get("$patch").and_then(Value::as_str)
}

fn strip_directives(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| !k.starts_with('$'))
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), strip_directives(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(
            a.iter()
                .filter(|v| !matches!(v, Value::Object(m) if directive(m) == Some("delete")))
                .map(strip_directives)
                .collect(),
        ),
        other => other.clone(),
    }
}

fn smp_map(orig: &mut Map<String, Value>, patch: &Map<String, Value>) {
    if directive(patch) == Some("replace") {
        if let Value::Object(m) = strip_directives(&Value::Object(patch.clone())) {
            *orig = m;
        }
        return;
    }
    for (k, v) in patch {
        if let Some(field) = k.strip_prefix("$deleteFromPrimitiveList/") {
            if let (Some(Value::Array(list)), Value::Array(del)) = (orig.get_mut(field), v) {
                list.retain(|x| !del.contains(x));
            }
            continue;
        }
        if k.starts_with('$') {
            // $patch, $setElementOrder/..., $retainKeys: ordering/retention hints only.
            continue;
        }
        if v.is_null() {
            orig.remove(k);
            continue;
        }
        match (orig.get_mut(k), v) {
            (Some(Value::Object(o)), Value::Object(p)) => {
                if directive(p) == Some("delete") {
                    orig.remove(k);
                } else {
                    smp_map(o, p);
                }
            }
            (Some(Value::Array(o)), Value::Array(p)) => smp_list(k, o, p),
            (_, Value::Object(p)) if directive(p) == Some("delete") => {
                orig.remove(k);
            }
            _ => {
                orig.insert(k.clone(), strip_directives(v));
            }
        }
    }
}

fn smp_list(field: &str, orig: &mut Vec<Value>, patch: &[Value]) {
    if patch
        .iter()
        .any(|v| matches!(v, Value::Object(m) if directive(m) == Some("replace")))
    {
        *orig = patch
            .iter()
            .filter(|v| !matches!(v, Value::Object(m) if directive(m).is_some()))
            .map(strip_directives)
            .collect();
        return;
    }
    match list_strategy(field, orig, patch) {
        ListStrategy::Replace => {
            *orig = patch.iter().map(strip_directives).collect();
        }
        ListStrategy::MergePrimitive => {
            for v in patch {
                if !orig.contains(v) {
                    orig.push(v.clone());
                }
            }
        }
        ListStrategy::MergeByKey(key) => {
            for item in patch {
                let Value::Object(pm) = item else {
                    orig.push(item.clone());
                    continue;
                };
                let Some(kv) = pm.get(key) else {
                    orig.push(strip_directives(item));
                    continue;
                };
                let pos = orig.iter().position(|o| o.get(key) == Some(kv));
                match (pos, directive(pm)) {
                    (Some(i), Some("delete")) => {
                        orig.remove(i);
                    }
                    (None, Some("delete")) => {}
                    (Some(i), _) => {
                        if let Value::Object(om) = &mut orig[i] {
                            smp_map(om, pm);
                        } else {
                            orig[i] = strip_directives(item);
                        }
                    }
                    (None, _) => orig.push(strip_directives(item)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_patch_rfc7386_and_evanphx_quirks() {
        let doc = json!({"a": "b", "c": {"d": "e", "f": "g"}, "l": [1, 2]});
        let out = merge_patch(&doc, &json!({"a": "z", "c": {"f": null}, "l": [3]})).unwrap();
        assert_eq!(out, json!({"a": "z", "c": {"d": "e"}, "l": [3]}));
        // nulls nested inside a newly added object are pruned
        let out = merge_patch(&json!({}), &json!({"x": {"y": null, "z": 1}})).unwrap();
        assert_eq!(out, json!({"x": {"z": 1}}));
        // nulls inside arrays are kept
        let out = merge_patch(&json!({}), &json!({"x": [null, {"a": null}]})).unwrap();
        assert_eq!(out, json!({"x": [null, {}]}));
        // replacing a scalar with an object prunes nulls of the object
        let out = merge_patch(&json!({"x": 1}), &json!({"x": {"a": null, "b": 2}})).unwrap();
        assert_eq!(out, json!({"x": {"b": 2}}));
        assert_eq!(
            merge_patch(&doc, &Value::Null).unwrap_err().to_string(),
            "Invalid JSON Patch"
        );
        assert_eq!(
            merge_patch(&doc, &json!("str")).unwrap_err().to_string(),
            "Invalid JSON Patch"
        );
    }

    #[test]
    fn strategic_merge_by_name_and_finalizers() {
        let doc = json!({
            "metadata": {"finalizers": ["a"]},
            "spec": {"containers": [{"name": "c1", "image": "i1"}, {"name": "c2", "image": "i2"}],
                     "tolerations": [{"key": "x"}]}
        });
        let patch = json!({
            "metadata": {"finalizers": ["b"]},
            "spec": {"containers": [{"name": "c2", "image": "new"}, {"name": "c3", "image": "i3"},
                                    {"name": "c1", "$patch": "delete"}],
                     "tolerations": [{"key": "y"}]}
        });
        let out = strategic_merge_patch(&doc, &patch).unwrap();
        assert_eq!(
            out,
            json!({
                "metadata": {"finalizers": ["a", "b"]},
                "spec": {"containers": [{"name": "c2", "image": "new"}, {"name": "c3", "image": "i3"}],
                         "tolerations": [{"key": "y"}]}
            })
        );
        let out = strategic_merge_patch(
            &out,
            &json!({"metadata": {"$deleteFromPrimitiveList/finalizers": ["a"]}, "spec": {"$patch": "replace", "x": 1}}),
        )
        .unwrap();
        assert_eq!(
            out,
            json!({"metadata": {"finalizers": ["b"]}, "spec": {"x": 1}})
        );
    }
}
