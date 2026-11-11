//! Port of `internal/dpfdefaults`: the embedded per-DPF-version table holding the DOCA
//! version and the DPUServiceTemplate chart/image values for each supported DPF
//! `major.minor`, with a `default` row used for unknown versions.

use std::collections::{BTreeMap, HashSet};
use std::sync::{LazyLock, Mutex};

use ctrlrt::{Ctx, Result, kv};
use serde::{Deserialize, Serialize};

/// `//go:embed dpfversions.json`
static DPF_VERSIONS_JSON: &str = include_str!("../../../internal/dpfdefaults/dpfversions.json");

const DEFAULT_KEY: &str = "default";

/// `OVNTemplateValues`
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvnTemplateValues {
    #[serde(rename = "chartRepoURL", default)]
    pub chart_repo_url: String,
    #[serde(rename = "chartName", default)]
    pub chart_name: String,
    #[serde(rename = "chartVersion", default)]
    pub chart_version: String,
}

/// `DTSTemplateValues`
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DtsTemplateValues {
    #[serde(rename = "chartRepoURL", default)]
    pub chart_repo_url: String,
    #[serde(rename = "chartName", default)]
    pub chart_name: String,
    #[serde(rename = "chartVersion", default)]
    pub chart_version: String,
    #[serde(rename = "imageDTS", default)]
    pub image_dts: String,
    #[serde(rename = "prometheusPort", default)]
    pub prometheus_port: i32,
}

/// `HBNTemplateValues`
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HbnTemplateValues {
    #[serde(rename = "chartRepoURL", default)]
    pub chart_repo_url: String,
    #[serde(rename = "chartName", default)]
    pub chart_name: String,
    #[serde(rename = "chartVersion", default)]
    pub chart_version: String,
    #[serde(rename = "imageRepo", default)]
    pub image_repo: String,
    #[serde(rename = "imageTag", default)]
    pub image_tag: String,
    #[serde(rename = "bfSFCount", default)]
    pub bf_sf_count: i32,
}

/// `DPUServiceTemplateValues` — `ImagePullSecret` is `json:"-"`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DpuServiceTemplateValues {
    #[serde(default)]
    pub ovn: OvnTemplateValues,
    #[serde(default)]
    pub dts: DtsTemplateValues,
    #[serde(default)]
    pub hbn: HbnTemplateValues,
    #[serde(skip)]
    pub image_pull_secret: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct VersionEntry {
    #[serde(rename = "docaVersion", default)]
    doca_version: String,
    #[serde(default)]
    ovn: OvnTemplateValues,
    #[serde(default)]
    dts: DtsTemplateValues,
    #[serde(default)]
    hbn: HbnTemplateValues,
}

/// Parsed at first use; like Go's `init()` the process panics if the embedded table is
/// invalid or lacks the `default` row.
static BY_MAJOR_MINOR: LazyLock<BTreeMap<String, VersionEntry>> = LazyLock::new(|| {
    let m: BTreeMap<String, VersionEntry> = serde_json::from_str(DPF_VERSIONS_JSON)
        .unwrap_or_else(|e| panic!("failed to parse embedded dpfversions.json: {e}"));
    if !m.contains_key(DEFAULT_KEY) {
        panic!("embedded dpfversions.json is missing \"{DEFAULT_KEY}\"");
    }
    m
});

/// Versions for which the "using default" message has been logged (`loggedDefaults`).
static LOGGED_DEFAULTS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Force parsing of the embedded table (Go does it in `init()`).
pub fn init() {
    LazyLock::force(&BY_MAJOR_MINOR);
}

fn entry(ctx: &Ctx, major_minor: &str) -> Result<VersionEntry> {
    if major_minor.is_empty() {
        return Err(anyhow::anyhow!(
            "unsupported DPF version {}",
            ctrlrt::error::go_quote(major_minor)
        ));
    }
    if let Some(row) = BY_MAJOR_MINOR.get(major_minor) {
        return Ok(row.clone());
    }
    let row = BY_MAJOR_MINOR[DEFAULT_KEY].clone();
    let first = LOGGED_DEFAULTS
        .lock()
        .map(|mut s| s.insert(major_minor.to_string()))
        .unwrap_or(false);
    if first {
        ctx.log.info(
            "No embedded values for DPF version; using default",
            kv!["requestedVersion" => major_minor, "defaultDocaVersion" => row.doca_version],
        );
    }
    Ok(row)
}

/// `DocaVersion(ctx, majorMinor)`
pub fn doca_version(ctx: &Ctx, major_minor: &str) -> Result<String> {
    let row = entry(ctx, major_minor)?;
    if row.doca_version.is_empty() {
        return Err(anyhow::anyhow!(
            "docaVersion is empty for DPF version {}",
            ctrlrt::error::go_quote(major_minor)
        ));
    }
    Ok(row.doca_version)
}

/// `TemplateValues(ctx, majorMinor)` — returns a fresh copy.
pub fn template_values(ctx: &Ctx, major_minor: &str) -> Result<DpuServiceTemplateValues> {
    let row = entry(ctx, major_minor)?;
    Ok(DpuServiceTemplateValues {
        ovn: row.ovn,
        dts: row.dts,
        hbn: row.hbn,
        image_pull_secret: String::new(),
    })
}

/// `SupportedDPFVersions()` — sorted, without the `default` key.
pub fn supported_dpf_versions() -> Vec<String> {
    BY_MAJOR_MINOR
        .keys()
        .filter(|k| k.as_str() != DEFAULT_KEY)
        .cloned()
        .collect()
}
