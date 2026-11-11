//! Small helpers operating on JSON object documents (the `meta.Accessor` analogue) plus a
//! few ports of apimachinery utilities (`utilrand.String`, `metav1.Now`, `strconv.ParseUint`).

use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use ctrlrt::ResourceInfo;
use serde_json::{Map, Value};

/// `accessor.GetXxx()` for a string field in `metadata` ("" when unset / null).
pub(crate) fn meta_str<'a>(obj: &'a Value, key: &str) -> &'a str {
    obj.get("metadata")
        .and_then(|m| m.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Mutable `metadata` map (created when missing or not an object).
pub(crate) fn meta_mut(obj: &mut Value) -> Option<&mut Map<String, Value>> {
    let map = obj.as_object_mut()?;
    let md = map
        .entry("metadata")
        .or_insert_with(|| Value::Object(Map::new()));
    if !md.is_object() {
        *md = Value::Object(Map::new());
    }
    md.as_object_mut()
}

/// `accessor.SetXxx(v)` for a string field; an empty value removes the key (Go `omitempty`).
pub(crate) fn set_meta_str(obj: &mut Value, key: &str, val: &str) {
    if let Some(md) = meta_mut(obj) {
        if val.is_empty() {
            md.remove(key);
        } else {
            md.insert(key.to_string(), Value::String(val.to_string()));
        }
    }
}

/// Remove a `metadata` field.
pub(crate) fn remove_meta(obj: &mut Value, key: &str) {
    if let Some(md) = obj.get_mut("metadata").and_then(Value::as_object_mut) {
        md.remove(key);
    }
}

/// `metadata.deletionTimestamp` (None when unset / null).
pub(crate) fn deletion_timestamp(obj: &Value) -> Option<&Value> {
    obj.get("metadata")
        .and_then(|m| m.get("deletionTimestamp"))
        .filter(|v| !v.is_null())
}

/// `len(accessor.GetFinalizers())`
pub(crate) fn finalizer_count(obj: &Value) -> usize {
    obj.get("metadata")
        .and_then(|m| m.get("finalizers"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

/// `accessor.GetLabels()`
pub(crate) fn labels(obj: &Value) -> BTreeMap<String, String> {
    obj.get("metadata")
        .and_then(|m| m.get("labels"))
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Stamp `apiVersion`/`kind` (`ta.SetAPIVersion` / `ta.SetKind`).
pub(crate) fn stamp_type_meta(obj: &mut Value, info: &ResourceInfo) {
    if let Some(map) = obj.as_object_mut() {
        map.insert("apiVersion".into(), Value::String(info.api_version()));
        map.insert("kind".into(), Value::String(info.kind.clone()));
    }
}

/// `ensureTypeMeta` for typed objects: `ta.SetKind(""); ta.SetAPIVersion("")`, i.e. the
/// top-level `apiVersion`/`kind` are dropped (both are `omitempty` in Go's TypeMeta).
pub(crate) fn clear_type_meta(obj: &mut Value) {
    if let Some(map) = obj.as_object_mut() {
        map.remove("apiVersion");
        map.remove("kind");
    }
}

/// `deletionTimestampEqual` from controller-runtime's fake client: both nil, or both set and
/// less than one second apart (patch round-trips truncate to seconds).
pub(crate) fn deletion_timestamp_equal(new: &Value, old: &Value) -> bool {
    match (deletion_timestamp(new), deletion_timestamp(old)) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            if a == b {
                return true;
            }
            match (parse_time(a), parse_time(b)) {
                (Some(x), Some(y)) => (x - y).abs() < chrono::Duration::seconds(1),
                _ => false,
            }
        }
        _ => false,
    }
}

fn parse_time(v: &Value) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok()
}

/// `metav1.Now()` serialized the way `metav1.Time` marshals (RFC 3339, second precision, UTC).
pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// `strconv.ParseUint(s, 10, 64)` including Go's error messages.
pub(crate) fn parse_uint(s: &str) -> Result<u64, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "strconv.ParseUint: parsing {}: invalid syntax",
            ctrlrt::error::go_quote(s)
        ));
    }
    s.parse::<u64>().map_err(|_| {
        format!(
            "strconv.ParseUint: parsing {}: value out of range",
            ctrlrt::error::go_quote(s)
        )
    })
}

/// Alphabet of `k8s.io/apimachinery/pkg/util/rand` (vowels omitted).
const ALPHANUMS: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";

/// `utilrand.String(n)`
pub(crate) fn rand_string(n: usize) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut out = String::with_capacity(n);
    while out.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        let mut r = h.finish();
        for _ in 0..12 {
            if out.len() == n {
                break;
            }
            let idx = (r & 0x1f) as usize;
            r >>= 5;
            if idx < ALPHANUMS.len() {
                out.push(ALPHANUMS[idx] as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rand_string_uses_go_alphabet() {
        for _ in 0..50 {
            let s = rand_string(5);
            assert_eq!(s.len(), 5);
            assert!(s.bytes().all(|b| ALPHANUMS.contains(&b)), "{s}");
        }
    }

    #[test]
    fn parse_uint_messages() {
        assert_eq!(parse_uint("999"), Ok(999));
        assert_eq!(
            parse_uint("abc").unwrap_err(),
            r#"strconv.ParseUint: parsing "abc": invalid syntax"#
        );
        assert_eq!(
            parse_uint("99999999999999999999999").unwrap_err(),
            r#"strconv.ParseUint: parsing "99999999999999999999999": value out of range"#
        );
    }

    #[test]
    fn deletion_timestamp_equality_tolerates_sub_second() {
        let a = serde_json::json!({"metadata": {"deletionTimestamp": "2024-01-01T00:00:00Z"}});
        let b = serde_json::json!({"metadata": {"deletionTimestamp": "2024-01-01T00:00:00.5Z"}});
        let c = serde_json::json!({"metadata": {"deletionTimestamp": "2024-01-01T00:00:01Z"}});
        let none = serde_json::json!({"metadata": {}});
        assert!(deletion_timestamp_equal(&a, &b));
        assert!(!deletion_timestamp_equal(&a, &c));
        assert!(!deletion_timestamp_equal(&a, &none));
        assert!(deletion_timestamp_equal(&none, &none));
    }
}
