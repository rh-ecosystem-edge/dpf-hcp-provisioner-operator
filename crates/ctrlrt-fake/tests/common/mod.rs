#![allow(dead_code)]

use std::collections::BTreeMap;

use ctrlrt::__private::ObjectMeta;
use serde::{Deserialize, Serialize};

/// A namespaced CRD-like type (`example.com/v1alpha1, Kind=Widget`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Widget {
    #[serde(default)]
    pub metadata: ObjectMeta,
    #[serde(default, skip_serializing_if = "WidgetSpec::is_empty")]
    pub spec: WidgetSpec,
    #[serde(default, skip_serializing_if = "WidgetStatus::is_empty")]
    pub status: WidgetStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WidgetSpec {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub size: String,
}
impl WidgetSpec {
    fn is_empty(&self) -> bool {
        self.size.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WidgetStatus {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub phase: String,
}
impl WidgetStatus {
    fn is_empty(&self) -> bool {
        self.phase.is_empty()
    }
}

ctrlrt::impl_resource!(
    Widget,
    "example.com",
    "v1alpha1",
    "Widget",
    "widgets",
    Namespaced
);

/// Kind whose real plural differs from the guessed one (`gateways` vs `gatewaies`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Gateway {
    #[serde(default)]
    pub metadata: ObjectMeta,
}
ctrlrt::impl_resource!(
    Gateway,
    "example.com",
    "v1",
    "Gateway",
    "gateways",
    Namespaced
);

/// Cluster-scoped CRD-like type.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Region {
    #[serde(default)]
    pub metadata: ObjectMeta,
}
ctrlrt::impl_resource!(
    Region,
    "example.com",
    "v1alpha1",
    "Region",
    "regions",
    Cluster
);

pub fn meta(ns: &str, name: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.into()),
        namespace: if ns.is_empty() { None } else { Some(ns.into()) },
        ..Default::default()
    }
}

pub fn widget(ns: &str, name: &str) -> Widget {
    Widget {
        metadata: meta(ns, name),
        ..Default::default()
    }
}

pub fn labels<const N: usize>(kv: [(&str, &str); N]) -> Option<BTreeMap<String, String>> {
    Some(
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}
