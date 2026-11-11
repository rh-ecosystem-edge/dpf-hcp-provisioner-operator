//! Go `encoding/json` compatibility helpers used by the generated API types.
//!
//! * [`is_empty`] — Go's `omitempty` test (`skip_serializing_if`).
//! * [`null_default`] — Go decodes JSON `null` into any field as a no-op (zero value).
//! * [`ser`] / [`de`] — Go encoding of apimachinery types that the generated structs take from
//!   `k8s-openapi` ([`ObjectMeta`], [`ListMeta`], [`Condition`], [`Time`], [`Quantity`]):
//!   Go field order, Go `omitempty` semantics, zero `metav1.Time` as `null`, canonical
//!   `resource.Quantity` strings.
//! * [`Duration`] — `metav1.Duration` (Go `time.Duration` string form).

use std::collections::BTreeMap;
use std::fmt;

use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{
    Condition, ListMeta, ManagedFieldsEntry, ObjectMeta, OwnerReference, Time,
};
use serde::de::Error as _;
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------------------------
// omitempty
// ---------------------------------------------------------------------------------------------

/// Go's notion of an "empty value" for `omitempty`: false, 0, nil pointer/interface, empty
/// string/slice/map. (Structs are never empty in Go and never use this.)
pub trait GoEmpty {
    fn go_empty(&self) -> bool;
}

impl GoEmpty for String {
    fn go_empty(&self) -> bool {
        self.is_empty()
    }
}
impl GoEmpty for str {
    fn go_empty(&self) -> bool {
        self.is_empty()
    }
}
impl GoEmpty for bool {
    fn go_empty(&self) -> bool {
        !*self
    }
}
macro_rules! go_empty_num {
    ($($t:ty),*) => {$(
        impl GoEmpty for $t {
            fn go_empty(&self) -> bool {
                *self == 0
            }
        }
    )*};
}
go_empty_num!(i8, i16, i32, i64, u8, u16, u32, u64);
impl GoEmpty for f64 {
    fn go_empty(&self) -> bool {
        *self == 0.0
    }
}
impl<T> GoEmpty for Vec<T> {
    fn go_empty(&self) -> bool {
        self.is_empty()
    }
}
impl<K, V> GoEmpty for BTreeMap<K, V> {
    fn go_empty(&self) -> bool {
        self.is_empty()
    }
}
impl<T> GoEmpty for Option<T> {
    fn go_empty(&self) -> bool {
        self.is_none()
    }
}
/// `interface{}`: only nil is empty.
impl GoEmpty for serde_json::Value {
    fn go_empty(&self) -> bool {
        self.is_null()
    }
}

/// `skip_serializing_if` predicate implementing Go's `omitempty`.
pub fn is_empty<T: GoEmpty + ?Sized>(v: &T) -> bool {
    v.go_empty()
}

/// `skip_serializing_if` for Go 1.24 `omitzero` on a `metav1.Time` (`IsZero()`).
pub fn is_zero_time(t: &Option<Time>) -> bool {
    t.as_ref().is_none_or(ctrlrt::meta::is_zero_time)
}

/// `skip_serializing_if` for Go 1.24 `omitzero` on a struct: every field has its zero value.
pub fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Deserialize a field the way Go does: JSON `null` leaves the zero value.
pub fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

// ---------------------------------------------------------------------------------------------
// Go encoding of k8s-openapi types
// ---------------------------------------------------------------------------------------------

/// Go-compatible (de)serialization for types whose `k8s-openapi` serde impls differ from Go.
pub trait GoSerde: Sized {
    fn go_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error>;
    fn go_deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error>;
}

/// `serialize_with` entry point.
pub fn ser<T: GoSerde, S: Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
    v.go_serialize(s)
}

/// `deserialize_with` entry point.
pub fn de<'de, T: GoSerde, D: Deserializer<'de>>(d: D) -> Result<T, D::Error> {
    T::go_deserialize(d)
}

/// Serialize-adapter: `Go(&value)` serializes `value` with its Go encoding.
pub struct Go<'a, T>(pub &'a T);

impl<T: GoSerde> Serialize for Go<'_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.go_serialize(s)
    }
}

struct GoDe<T>(T);

impl<'de, T: GoSerde> Deserialize<'de> for GoDe<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::go_deserialize(d).map(GoDe)
    }
}

impl<T: GoSerde> GoSerde for Option<T> {
    fn go_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            None => s.serialize_none(),
            Some(v) => v.go_serialize(s),
        }
    }
    fn go_deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Option::<GoDe<T>>::deserialize(d)?.map(|v| v.0))
    }
}

impl<T: GoSerde> GoSerde for Box<T> {
    fn go_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (**self).go_serialize(s)
    }
    fn go_deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::go_deserialize(d).map(Box::new)
    }
}

impl<T: GoSerde> GoSerde for Vec<T> {
    fn go_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.len()))?;
        for v in self {
            seq.serialize_element(&Go(v))?;
        }
        seq.end()
    }
    fn go_deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Option::<Vec<GoDe<T>>>::deserialize(d)?
            .unwrap_or_default()
            .into_iter()
            .map(|v| v.0)
            .collect())
    }
}

impl<T: GoSerde> GoSerde for BTreeMap<String, T> {
    fn go_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.len()))?;
        for (k, v) in self {
            m.serialize_entry(k, &Go(v))?;
        }
        m.end()
    }
    fn go_deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Option::<BTreeMap<String, GoDe<T>>>::deserialize(d)?
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k, v.0))
            .collect())
    }
}

/// `metav1.Time`: zero (see [`ctrlrt::meta::is_zero_time`]) encodes as `null`, otherwise
/// RFC 3339 in UTC with second precision.
impl GoSerde for Time {
    fn go_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if ctrlrt::meta::is_zero_time(self) {
            s.serialize_none()
        } else {
            self.serialize(s)
        }
    }
    fn go_deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Option::<Time>::deserialize(d)?.unwrap_or_else(ctrlrt::meta::zero_time))
    }
}

/// `resource.Quantity`: canonical form (see [`crate::resource`]); the zero value is `"0"`.
impl GoSerde for Quantity {
    fn go_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.0.is_empty() {
            return s.serialize_str("0");
        }
        match crate::resource::canonical(&self.0) {
            Ok(c) => s.serialize_str(&c),
            // Go cannot hold an unparsable Quantity; keep whatever the caller put in.
            Err(_) => s.serialize_str(&self.0),
        }
    }
    fn go_deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = match Option::<serde_json::Value>::deserialize(d)? {
            None => return Ok(Quantity::default()),
            Some(serde_json::Value::String(s)) => s,
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(other) => {
                return Err(D::Error::custom(format!(
                    "quantity must be a string or number, got {other}"
                )));
            }
        };
        crate::resource::canonical(raw.trim())
            .map(Quantity)
            .map_err(|e| D::Error::custom(e.to_string()))
    }
}

fn opt_str_empty(v: &Option<String>) -> bool {
    v.as_deref().is_none_or(str::is_empty)
}

fn opt_i64_empty(v: &Option<i64>) -> bool {
    v.is_none_or(|v| v == 0)
}

fn opt_vec_empty<T>(v: &Option<Vec<T>>) -> bool {
    v.as_ref().is_none_or(Vec::is_empty)
}

fn opt_map_empty<K, V>(v: &Option<BTreeMap<K, V>>) -> bool {
    v.as_ref().is_none_or(BTreeMap::is_empty)
}

struct GoOwnerRef<'a>(&'a OwnerReference);

impl Serialize for GoOwnerRef<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let r = self.0;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("apiVersion", &r.api_version)?;
        m.serialize_entry("kind", &r.kind)?;
        m.serialize_entry("name", &r.name)?;
        m.serialize_entry("uid", &r.uid)?;
        if let Some(c) = r.controller {
            m.serialize_entry("controller", &c)?;
        }
        if let Some(b) = r.block_owner_deletion {
            m.serialize_entry("blockOwnerDeletion", &b)?;
        }
        m.end()
    }
}

struct GoManagedFields<'a>(&'a ManagedFieldsEntry);

impl Serialize for GoManagedFields<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let e = self.0;
        let mut m = s.serialize_map(None)?;
        if !opt_str_empty(&e.manager) {
            m.serialize_entry("manager", &e.manager)?;
        }
        if !opt_str_empty(&e.operation) {
            m.serialize_entry("operation", &e.operation)?;
        }
        if !opt_str_empty(&e.api_version) {
            m.serialize_entry("apiVersion", &e.api_version)?;
        }
        if let Some(t) = &e.time {
            m.serialize_entry("time", &Go(t))?;
        }
        if !opt_str_empty(&e.fields_type) {
            m.serialize_entry("fieldsType", &e.fields_type)?;
        }
        if let Some(f) = &e.fields_v1 {
            m.serialize_entry("fieldsV1", &f.0)?;
        }
        if !opt_str_empty(&e.subresource) {
            m.serialize_entry("subresource", &e.subresource)?;
        }
        m.end()
    }
}

/// `metav1.ObjectMeta` with Go's field order and `omitempty`/`omitzero` (a zero
/// `creationTimestamp` is omitted: `json:"creationTimestamp,omitempty,omitzero"`).
impl GoSerde for ObjectMeta {
    fn go_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        if !opt_str_empty(&self.name) {
            m.serialize_entry("name", &self.name)?;
        }
        if !opt_str_empty(&self.generate_name) {
            m.serialize_entry("generateName", &self.generate_name)?;
        }
        if !opt_str_empty(&self.namespace) {
            m.serialize_entry("namespace", &self.namespace)?;
        }
        if !opt_str_empty(&self.self_link) {
            m.serialize_entry("selfLink", &self.self_link)?;
        }
        if !opt_str_empty(&self.uid) {
            m.serialize_entry("uid", &self.uid)?;
        }
        if !opt_str_empty(&self.resource_version) {
            m.serialize_entry("resourceVersion", &self.resource_version)?;
        }
        if !opt_i64_empty(&self.generation) {
            m.serialize_entry("generation", &self.generation)?;
        }
        if !is_zero_time(&self.creation_timestamp) {
            m.serialize_entry("creationTimestamp", &Go(&self.creation_timestamp))?;
        }
        if let Some(t) = &self.deletion_timestamp {
            m.serialize_entry("deletionTimestamp", &Go(t))?;
        }
        if let Some(g) = &self.deletion_grace_period_seconds {
            m.serialize_entry("deletionGracePeriodSeconds", g)?;
        }
        if !opt_map_empty(&self.labels) {
            m.serialize_entry("labels", &self.labels)?;
        }
        if !opt_map_empty(&self.annotations) {
            m.serialize_entry("annotations", &self.annotations)?;
        }
        if let Some(refs) = self.owner_references.as_ref().filter(|v| !v.is_empty()) {
            let refs: Vec<_> = refs.iter().map(GoOwnerRef).collect();
            m.serialize_entry("ownerReferences", &refs)?;
        }
        if !opt_vec_empty(&self.finalizers) {
            m.serialize_entry("finalizers", &self.finalizers)?;
        }
        if let Some(mf) = self.managed_fields.as_ref().filter(|v| !v.is_empty()) {
            let mf: Vec<_> = mf.iter().map(GoManagedFields).collect();
            m.serialize_entry("managedFields", &mf)?;
        }
        m.end()
    }
    fn go_deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Option::<ObjectMeta>::deserialize(d)?.unwrap_or_default())
    }
}

/// `metav1.ListMeta` with Go's field order.
impl GoSerde for ListMeta {
    fn go_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        if !opt_str_empty(&self.self_link) {
            m.serialize_entry("selfLink", &self.self_link)?;
        }
        if !opt_str_empty(&self.resource_version) {
            m.serialize_entry("resourceVersion", &self.resource_version)?;
        }
        if !opt_str_empty(&self.continue_) {
            m.serialize_entry("continue", &self.continue_)?;
        }
        if let Some(r) = &self.remaining_item_count {
            m.serialize_entry("remainingItemCount", r)?;
        }
        m.end()
    }
    fn go_deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Option::<ListMeta>::deserialize(d)?.unwrap_or_default())
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GoConditionDe {
    #[serde(rename = "type", deserialize_with = "null_default")]
    type_: String,
    #[serde(deserialize_with = "null_default")]
    status: String,
    #[serde(rename = "observedGeneration", deserialize_with = "null_default")]
    observed_generation: i64,
    #[serde(rename = "lastTransitionTime", deserialize_with = "de")]
    last_transition_time: Option<Time>,
    #[serde(deserialize_with = "null_default")]
    reason: String,
    #[serde(deserialize_with = "null_default")]
    message: String,
}

/// `metav1.Condition` with Go's field order (`type, status, observedGeneration,
/// lastTransitionTime, reason, message`). Decoding is lenient like Go: a missing/null
/// `lastTransitionTime` becomes the zero time, `observedGeneration: 0` becomes `None`.
impl GoSerde for Condition {
    fn go_serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("type", &self.type_)?;
        m.serialize_entry("status", &self.status)?;
        if !opt_i64_empty(&self.observed_generation) {
            m.serialize_entry("observedGeneration", &self.observed_generation)?;
        }
        m.serialize_entry("lastTransitionTime", &Go(&self.last_transition_time))?;
        m.serialize_entry("reason", &self.reason)?;
        m.serialize_entry("message", &self.message)?;
        m.end()
    }
    fn go_deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let c = Option::<GoConditionDe>::deserialize(d)?.unwrap_or_default();
        Ok(Condition {
            type_: c.type_,
            status: c.status,
            observed_generation: Some(c.observed_generation).filter(|g| *g != 0),
            last_transition_time: c
                .last_transition_time
                .unwrap_or_else(ctrlrt::meta::zero_time),
            reason: c.reason,
            message: c.message,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// metav1.Duration
// ---------------------------------------------------------------------------------------------

/// `metav1.Duration` — a Go `time.Duration` (nanoseconds) encoded as its `String()` form
/// (e.g. `"1h0m0s"`), decoded with `time.ParseDuration`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration(pub i64);

const NANOSECOND: u64 = 1;
const MICROSECOND: u64 = 1000 * NANOSECOND;
const MILLISECOND: u64 = 1000 * MICROSECOND;
const SECOND: u64 = 1000 * MILLISECOND;
const MINUTE: u64 = 60 * SECOND;
const HOUR: u64 = 60 * MINUTE;

impl Duration {
    pub const fn from_nanos(n: i64) -> Self {
        Duration(n)
    }
    pub const fn from_secs(s: i64) -> Self {
        Duration(s * SECOND as i64)
    }
    pub fn as_nanos(&self) -> i64 {
        self.0
    }

    /// `time.ParseDuration`
    pub fn parse(s: &str) -> Result<Duration, String> {
        parse_duration(s)
    }
}

/// `time.Duration.String()`
impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; 32];
        let mut w = buf.len();
        let neg = self.0 < 0;
        let mut u = self.0.unsigned_abs();
        if u < SECOND {
            let prec;
            w -= 1;
            buf[w] = b's';
            w -= 1;
            if u == 0 {
                buf[w] = b'0';
                return f.write_str(std::str::from_utf8(&buf[w..]).unwrap_or("0s"));
            } else if u < MICROSECOND {
                prec = 0;
                buf[w] = b'n';
            } else if u < MILLISECOND {
                prec = 3;
                // U+00B5 'µ' micro sign == 0xC2 0xB5
                w -= 1;
                buf[w] = 0xC2;
                buf[w + 1] = 0xB5;
            } else {
                prec = 6;
                buf[w] = b'm';
            }
            (w, u) = fmt_frac(&mut buf, w, u, prec);
            w = fmt_int(&mut buf, w, u);
        } else {
            w -= 1;
            buf[w] = b's';
            (w, u) = fmt_frac(&mut buf, w, u, 9);
            w = fmt_int(&mut buf, w, u % 60);
            u /= 60;
            if u > 0 {
                w -= 1;
                buf[w] = b'm';
                w = fmt_int(&mut buf, w, u % 60);
                u /= 60;
                if u > 0 {
                    w -= 1;
                    buf[w] = b'h';
                    w = fmt_int(&mut buf, w, u);
                }
            }
        }
        if neg {
            w -= 1;
            buf[w] = b'-';
        }
        f.write_str(std::str::from_utf8(&buf[w..]).map_err(|_| fmt::Error)?)
    }
}

fn fmt_frac(buf: &mut [u8], mut w: usize, mut v: u64, prec: usize) -> (usize, u64) {
    let mut print = false;
    for _ in 0..prec {
        let digit = v % 10;
        print = print || digit != 0;
        if print {
            w -= 1;
            buf[w] = digit as u8 + b'0';
        }
        v /= 10;
    }
    if print {
        w -= 1;
        buf[w] = b'.';
    }
    (w, v)
}

fn fmt_int(buf: &mut [u8], mut w: usize, mut v: u64) -> usize {
    if v == 0 {
        w -= 1;
        buf[w] = b'0';
    } else {
        while v > 0 {
            w -= 1;
            buf[w] = (v % 10) as u8 + b'0';
            v /= 10;
        }
    }
    w
}

/// The `time` package's `quote` (used in `ParseDuration` errors): bytes of non-ASCII and
/// control characters are written as `\xNN`, `"` and `\\` are backslash-escaped.
fn time_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if (c as u32) >= 0x80 || c < ' ' {
            let mut b = [0u8; 4];
            for byte in c.encode_utf8(&mut b).bytes() {
                out.push_str(&format!("\\x{byte:02x}"));
            }
        } else {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out.push('"');
    out
}

fn parse_duration(orig: &str) -> Result<Duration, String> {
    let invalid = || format!("time: invalid duration {}", time_quote(orig));
    let mut s = orig.as_bytes();
    let mut d: u64 = 0;
    let mut neg = false;
    if let Some(&c) = s.first()
        && (c == b'-' || c == b'+')
    {
        neg = c == b'-';
        s = &s[1..];
    }
    if s == b"0" {
        return Ok(Duration(0));
    }
    if s.is_empty() {
        return Err(invalid());
    }
    while !s.is_empty() {
        let mut f: u64 = 0;
        let mut scale: f64 = 1.0;
        if !(s[0] == b'.' || s[0].is_ascii_digit()) {
            return Err(invalid());
        }
        // leadingInt
        let pl = s.len();
        let mut v: u64 = 0;
        let mut i = 0;
        while i < s.len() && s[i].is_ascii_digit() {
            if v > (1u64 << 63) / 10 {
                return Err(invalid());
            }
            v = v * 10 + u64::from(s[i] - b'0');
            if v > 1u64 << 63 {
                return Err(invalid());
            }
            i += 1;
        }
        s = &s[i..];
        let pre = pl != s.len();
        let mut post = false;
        if !s.is_empty() && s[0] == b'.' {
            s = &s[1..];
            let pl = s.len();
            // leadingFraction
            let mut i = 0;
            let mut overflow = false;
            while i < s.len() && s[i].is_ascii_digit() {
                if !overflow {
                    if f > ((1u64 << 63) - 1) / 10 {
                        overflow = true;
                    } else {
                        let y = f * 10 + u64::from(s[i] - b'0');
                        if y > 1u64 << 63 {
                            overflow = true;
                        } else {
                            f = y;
                            scale *= 10.0;
                        }
                    }
                }
                i += 1;
            }
            s = &s[i..];
            post = pl != s.len();
        }
        if !pre && !post {
            return Err(invalid());
        }
        let mut i = 0;
        while i < s.len() && !(s[i] == b'.' || s[i].is_ascii_digit()) {
            i += 1;
        }
        if i == 0 {
            return Err(format!(
                "time: missing unit in duration {}",
                time_quote(orig)
            ));
        }
        let u = String::from_utf8_lossy(&s[..i]).into_owned();
        s = &s[i..];
        let unit = match u.as_str() {
            "ns" => NANOSECOND,
            "us" | "\u{00b5}s" | "\u{03bc}s" => MICROSECOND,
            "ms" => MILLISECOND,
            "s" => SECOND,
            "m" => MINUTE,
            "h" => HOUR,
            _ => {
                return Err(format!(
                    "time: unknown unit {} in duration {}",
                    time_quote(&u),
                    time_quote(orig)
                ));
            }
        };
        if v > (1u64 << 63) / unit {
            return Err(invalid());
        }
        v *= unit;
        if f > 0 {
            v += (f as f64 * (unit as f64 / scale)) as u64;
            if v > 1u64 << 63 {
                return Err(invalid());
            }
        }
        d += v;
        if d > 1u64 << 63 {
            return Err(invalid());
        }
    }
    if neg {
        return Ok(Duration((d as i64).wrapping_neg()));
    }
    if d > (1u64 << 63) - 1 {
        return Err(invalid());
    }
    Ok(Duration(d as i64))
}

impl Serialize for Duration {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// Like Go: the JSON must be a string accepted by `time.ParseDuration` (`null` is an error for
/// a non-pointer field; pointer fields are `Option<Duration>` and accept `null`).
impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = Option::<String>::deserialize(d)?.unwrap_or_default();
        parse_duration(&s).map_err(D::Error::custom)
    }
}

// ---------------------------------------------------------------------------------------------
// runtime.RawExtension / apiextensionsv1.JSON
// ---------------------------------------------------------------------------------------------

/// Arbitrary JSON as carried by `runtime.RawExtension` / `apiextensionsv1.JSON` (`Raw []byte`).
///
/// Go re-emits the raw bytes verbatim, so object members keep their original order (and
/// duplicates); unlike `serde_json::Value` (sorted keys) this type preserves that order.
/// The zero value ([`Json::Null`], Go's nil `Raw`) encodes as `null`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Json {
    #[default]
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Parse JSON text (`RawExtension{Raw: data}`), keeping member order.
    pub fn from_slice(data: &[u8]) -> serde_json::Result<Json> {
        serde_json::from_slice(data)
    }

    /// The compact JSON text (`Raw`).
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Convert to a `serde_json::Value` (object keys become sorted; duplicates: last wins).
    pub fn to_value(&self) -> serde_json::Value {
        match self {
            Json::Null => serde_json::Value::Null,
            Json::Bool(b) => serde_json::Value::Bool(*b),
            Json::Number(n) => serde_json::Value::Number(n.clone()),
            Json::String(s) => serde_json::Value::String(s.clone()),
            Json::Array(a) => serde_json::Value::Array(a.iter().map(Json::to_value).collect()),
            Json::Object(o) => serde_json::Value::Object(
                o.iter().map(|(k, v)| (k.clone(), v.to_value())).collect(),
            ),
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }
}

impl From<serde_json::Value> for Json {
    fn from(v: serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => Json::Null,
            serde_json::Value::Bool(b) => Json::Bool(b),
            serde_json::Value::Number(n) => Json::Number(n),
            serde_json::Value::String(s) => Json::String(s),
            serde_json::Value::Array(a) => Json::Array(a.into_iter().map(Json::from).collect()),
            serde_json::Value::Object(o) => {
                Json::Object(o.into_iter().map(|(k, v)| (k, Json::from(v))).collect())
            }
        }
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => s.serialize_unit(),
            Json::Bool(b) => s.serialize_bool(*b),
            Json::Number(n) => n.serialize(s),
            Json::String(v) => s.serialize_str(v),
            Json::Array(a) => {
                let mut seq = s.serialize_seq(Some(a.len()))?;
                for v in a {
                    seq.serialize_element(v)?;
                }
                seq.end()
            }
            Json::Object(o) => {
                let mut m = s.serialize_map(Some(o.len()))?;
                for (k, v) in o {
                    m.serialize_entry(k, v)?;
                }
                m.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Json;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_none<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Json, D::Error> {
                Json::deserialize(d)
            }
            fn visit_bool<E>(self, b: bool) -> Result<Json, E> {
                Ok(Json::Bool(b))
            }
            fn visit_i64<E>(self, n: i64) -> Result<Json, E> {
                Ok(Json::Number(n.into()))
            }
            fn visit_u64<E>(self, n: u64) -> Result<Json, E> {
                Ok(Json::Number(n.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, n: f64) -> Result<Json, E> {
                serde_json::Number::from_f64(n)
                    .map(Json::Number)
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E>(self, v: &str) -> Result<Json, E> {
                Ok(Json::String(v.to_string()))
            }
            fn visit_string<E>(self, v: String) -> Result<Json, E> {
                Ok(Json::String(v))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<Json, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element::<Json>()? {
                    out.push(v);
                }
                Ok(Json::Array(out))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a: A) -> Result<Json, A::Error> {
                let mut out = Vec::new();
                while let Some((k, v)) = a.next_entry::<String, Json>()? {
                    out.push((k, v));
                }
                Ok(Json::Object(out))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_string() {
        let cases: &[(i64, &str)] = &[
            (0, "0s"),
            (1, "1ns"),
            (1100, "1.1µs"),
            (2_200_000, "2.2ms"),
            (3_300_000_000, "3.3s"),
            (4 * 60 * 1_000_000_000 + 5 * 1_000_000_000, "4m5s"),
            (4 * 60 * 1_000_000_000 + 5_001_000_000, "4m5.001s"),
            (
                5 * 3600 * 1_000_000_000 + 6 * 60 * 1_000_000_000 + 7_001_000_000,
                "5h6m7.001s",
            ),
            (8 * 60 * 1_000_000_000 + 1, "8m0.000000001s"),
            (i64::MAX, "2562047h47m16.854775807s"),
            (i64::MIN, "-2562047h47m16.854775808s"),
            (-1, "-1ns"),
            (3600 * 1_000_000_000, "1h0m0s"),
        ];
        for (n, s) in cases {
            assert_eq!(Duration(*n).to_string(), *s);
        }
    }

    #[test]
    fn duration_parse() {
        let ok: &[(&str, i64)] = &[
            ("0", 0),
            ("5s", 5_000_000_000),
            ("30s", 30_000_000_000),
            ("1478s", 1_478_000_000_000),
            ("-5s", -5_000_000_000),
            ("+5s", 5_000_000_000),
            ("-0", 0),
            ("+0", 0),
            ("5.0s", 5_000_000_000),
            ("5.6s", 5_600_000_000),
            ("5.s", 5_000_000_000),
            (".5s", 500_000_000),
            ("1.004s", 1_004_000_000),
            ("1.0040s", 1_004_000_000),
            ("100.00100s", 100_001_000_000),
            ("10ns", 10),
            ("11us", 11_000),
            ("12µs", 12_000),
            ("12μs", 12_000),
            ("13ms", 13_000_000),
            ("14s", 14_000_000_000),
            ("15m", 900_000_000_000),
            ("16h", 57_600_000_000_000),
            ("3h30m", 12_600_000_000_000),
            ("10.5s4m", 250_500_000_000),
            ("-2m3.4s", -123_400_000_000),
            ("1h2m3s4ms5us6ns", 3_723_004_005_006),
            ("39h9m14.425s", 140_954_425_000_000),
            ("52763797000ns", 52_763_797_000),
            ("0.3333333333333333333h", 1_200_000_000_000),
            ("9007199254740993ns", 9_007_199_254_740_993),
            ("9223372036854775807ns", i64::MAX),
            ("9223372036854775.807us", i64::MAX),
            ("-9223372036854775808ns", i64::MIN),
            ("0.100000000000000000000h", 360_000_000_000),
        ];
        for (s, n) in ok {
            assert_eq!(parse_duration(s), Ok(Duration(*n)), "{s}");
        }
        assert_eq!(
            parse_duration(""),
            Err("time: invalid duration \"\"".to_string())
        );
        assert_eq!(
            parse_duration("3"),
            Err("time: missing unit in duration \"3\"".to_string())
        );
        assert_eq!(
            parse_duration("1d"),
            Err("time: unknown unit \"d\" in duration \"1d\"".to_string())
        );
        for bad in ["-", "s", ".", "-.", ".s", "+.s", "9223372036854775808ns"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }
}
