//! Port of the `github.com/google/go-containerregistry/pkg/v1` (and `v1/types`) data types
//! used by the operator.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use crate::goutil;

// ------------------------------------------------------------------ MediaType

/// `types.MediaType`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct MediaType(pub String);

impl MediaType {
    pub const OCI_CONTENT_DESCRIPTOR: &'static str = "application/vnd.oci.descriptor.v1+json";
    pub const OCI_IMAGE_INDEX: &'static str = "application/vnd.oci.image.index.v1+json";
    pub const OCI_MANIFEST_SCHEMA1: &'static str = "application/vnd.oci.image.manifest.v1+json";
    pub const OCI_CONFIG_JSON: &'static str = "application/vnd.oci.image.config.v1+json";
    pub const OCI_LAYER: &'static str = "application/vnd.oci.image.layer.v1.tar+gzip";
    pub const OCI_LAYER_ZSTD: &'static str = "application/vnd.oci.image.layer.v1.tar+zstd";
    pub const OCI_RESTRICTED_LAYER: &'static str =
        "application/vnd.oci.image.layer.nondistributable.v1.tar+gzip";
    pub const OCI_UNCOMPRESSED_LAYER: &'static str = "application/vnd.oci.image.layer.v1.tar";
    pub const OCI_UNCOMPRESSED_RESTRICTED_LAYER: &'static str =
        "application/vnd.oci.image.layer.nondistributable.v1.tar";
    pub const DOCKER_MANIFEST_SCHEMA1: &'static str =
        "application/vnd.docker.distribution.manifest.v1+json";
    pub const DOCKER_MANIFEST_SCHEMA1_SIGNED: &'static str =
        "application/vnd.docker.distribution.manifest.v1+prettyjws";
    pub const DOCKER_MANIFEST_SCHEMA2: &'static str =
        "application/vnd.docker.distribution.manifest.v2+json";
    pub const DOCKER_MANIFEST_LIST: &'static str =
        "application/vnd.docker.distribution.manifest.list.v2+json";
    pub const DOCKER_LAYER: &'static str = "application/vnd.docker.image.rootfs.diff.tar.gzip";
    pub const DOCKER_CONFIG_JSON: &'static str = "application/vnd.docker.container.image.v1+json";
    pub const DOCKER_PLUGIN_CONFIG: &'static str = "application/vnd.docker.plugin.v1+json";
    pub const DOCKER_FOREIGN_LAYER: &'static str =
        "application/vnd.docker.image.rootfs.foreign.diff.tar.gzip";
    pub const DOCKER_UNCOMPRESSED_LAYER: &'static str =
        "application/vnd.docker.image.rootfs.diff.tar";

    pub fn new(s: impl Into<String>) -> Self {
        MediaType(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `IsDistributable()`
    pub fn is_distributable(&self) -> bool {
        !matches!(
            self.0.as_str(),
            Self::DOCKER_FOREIGN_LAYER
                | Self::OCI_RESTRICTED_LAYER
                | Self::OCI_UNCOMPRESSED_RESTRICTED_LAYER
        )
    }

    /// `IsImage()`
    pub fn is_image(&self) -> bool {
        matches!(
            self.0.as_str(),
            Self::OCI_MANIFEST_SCHEMA1 | Self::DOCKER_MANIFEST_SCHEMA2
        )
    }

    /// `IsIndex()`
    pub fn is_index(&self) -> bool {
        matches!(
            self.0.as_str(),
            Self::OCI_IMAGE_INDEX | Self::DOCKER_MANIFEST_LIST
        )
    }

    /// `IsConfig()`
    pub fn is_config(&self) -> bool {
        matches!(
            self.0.as_str(),
            Self::OCI_CONFIG_JSON | Self::DOCKER_CONFIG_JSON
        )
    }

    /// `IsSchema1()`
    pub fn is_schema1(&self) -> bool {
        matches!(
            self.0.as_str(),
            Self::DOCKER_MANIFEST_SCHEMA1 | Self::DOCKER_MANIFEST_SCHEMA1_SIGNED
        )
    }

    /// `IsLayer()`
    pub fn is_layer(&self) -> bool {
        matches!(
            self.0.as_str(),
            Self::DOCKER_LAYER
                | Self::DOCKER_UNCOMPRESSED_LAYER
                | Self::OCI_LAYER
                | Self::OCI_LAYER_ZSTD
                | Self::OCI_UNCOMPRESSED_LAYER
                | Self::DOCKER_FOREIGN_LAYER
                | Self::OCI_RESTRICTED_LAYER
                | Self::OCI_UNCOMPRESSED_RESTRICTED_LAYER
        )
    }
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for MediaType {
    fn from(s: &str) -> Self {
        MediaType(s.to_string())
    }
}

impl PartialEq<str> for MediaType {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for MediaType {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl Serialize for MediaType {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for MediaType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(MediaType(Option::<String>::deserialize(d)?.unwrap_or_default()))
    }
}

// ------------------------------------------------------------------ Hash

/// `v1.Hash`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Hash {
    pub algorithm: String,
    pub hex: String,
}

impl Hash {
    /// `v1.NewHash`
    pub fn new(s: &str) -> Result<Hash, HashError> {
        let Some((algo, body)) = s.split_once(':') else {
            return Err(HashError(format!("cannot parse hash: {}", goutil::quote(s))));
        };
        if algo.is_empty() || body.is_empty() {
            return Err(HashError(format!("cannot parse hash: {}", goutil::quote(s))));
        }
        if let Some(c) = body.bytes().find(|b| !matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(HashError(format!(
                "found non-hex character in hash: {}",
                char::from(c)
            )));
        }
        let want_bytes = match algo {
            "sha256" => 32,
            other => {
                return Err(HashError(format!(
                    "unsupported hash: {}",
                    goutil::quote(other)
                )));
            }
        };
        if body.len() != want_bytes * 2 {
            return Err(HashError(format!(
                "wrong number of hex digits for {algo}: {body}"
            )));
        }
        Ok(Hash {
            algorithm: algo.to_string(),
            hex: body.to_string(),
        })
    }

    /// `v1.SHA256` over a byte slice (digest only).
    pub fn sha256(data: &[u8]) -> Hash {
        Hash {
            algorithm: "sha256".to_string(),
            hex: hex::encode(Sha256::digest(data)),
        }
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.hex)
    }
}

impl std::str::FromStr for Hash {
    type Err = HashError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Hash::new(s)
    }
}

/// Error from parsing a [`Hash`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashError(pub String);

impl fmt::Display for HashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HashError {}

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Option::<String>::deserialize(d)? {
            // `null` leaves the zero value in Go.
            None => Ok(Hash::default()),
            Some(s) => Hash::new(&s).map_err(serde::de::Error::custom),
        }
    }
}

// ------------------------------------------------------------------ Platform

/// `v1.Platform`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Platform {
    #[serde(default, deserialize_with = "null_default")]
    pub architecture: String,
    #[serde(default, deserialize_with = "null_default")]
    pub os: String,
    #[serde(
        rename = "os.version",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub os_version: String,
    #[serde(
        rename = "os.features",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub os_features: Vec<String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub variant: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

impl Platform {
    /// `Platform.String()`
    pub fn string(&self) -> String {
        if self.os.is_empty() {
            return String::new();
        }
        let mut b = self.os.clone();
        if !self.architecture.is_empty() {
            b.push('/');
            b.push_str(&self.architecture);
        }
        if !self.variant.is_empty() {
            b.push('/');
            b.push_str(&self.variant);
        }
        if !self.os_version.is_empty() {
            b.push(':');
            b.push_str(&self.os_version);
        }
        b
    }

}

/// `remote.defaultPlatform` (linux/amd64).
pub fn default_platform() -> Platform {
    Platform {
        architecture: "amd64".to_string(),
        os: "linux".to_string(),
        ..Default::default()
    }
}

// ------------------------------------------------------------------ Descriptor / Manifest

/// `v1.Descriptor`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Descriptor {
    #[serde(rename = "mediaType", default)]
    pub media_type: MediaType,
    #[serde(default, deserialize_with = "null_default")]
    pub size: i64,
    #[serde(default)]
    pub digest: Hash,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "base64_opt"
    )]
    pub data: Option<Vec<u8>>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
    #[serde(
        rename = "artifactType",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub artifact_type: String,
}

/// `v1.Manifest`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(rename = "schemaVersion", default, deserialize_with = "null_default")]
    pub schema_version: i64,
    #[serde(rename = "mediaType", default, skip_serializing_if = "is_empty_mt")]
    pub media_type: MediaType,
    #[serde(default)]
    pub config: Descriptor,
    #[serde(default, deserialize_with = "null_default")]
    pub layers: Vec<Descriptor>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Descriptor>,
}

/// `v1.IndexManifest`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IndexManifest {
    #[serde(rename = "schemaVersion", default, deserialize_with = "null_default")]
    pub schema_version: i64,
    #[serde(rename = "mediaType", default, skip_serializing_if = "is_empty_mt")]
    pub media_type: MediaType,
    #[serde(default, deserialize_with = "null_default")]
    pub manifests: Vec<Descriptor>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Descriptor>,
}

fn is_empty_mt(m: &MediaType) -> bool {
    m.0.is_empty()
}

fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

mod base64_opt {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_str(&crate::gobase64::encode_std(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        match Option::<String>::deserialize(d)? {
            None => Ok(None),
            Some(s) => crate::gobase64::decode_std(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}

/// Decode the first JSON value of `data` the way `json.NewDecoder(r).Decode(&v)` does
/// (trailing data after the first value is ignored).
pub(crate) fn decode_json<T: serde::de::DeserializeOwned>(data: &[u8]) -> anyhow::Result<T> {
    let mut it = serde_json::Deserializer::from_slice(data).into_iter::<serde_json::Value>();
    let value = match it.next() {
        None => anyhow::bail!("EOF"),
        Some(Err(e)) => anyhow::bail!(goutil::json_syntax_error(data, &e)),
        Some(Ok(v)) => v,
    };
    serde_json::from_value(value).map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// `v1.ParseManifest`
pub fn parse_manifest(data: &[u8]) -> anyhow::Result<Manifest> {
    decode_json(data)
}

/// `v1.ParseIndexManifest`
pub fn parse_index_manifest(data: &[u8]) -> anyhow::Result<IndexManifest> {
    decode_json(data)
}

// ------------------------------------------------------------------ ConfigFile

/// `v1.ConfigFile` (the fields the operator and common tooling need; unknown fields are
/// ignored as in Go).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default, deserialize_with = "null_default")]
    pub architecture: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub container: String,
    /// RFC 3339 creation time as found in the config (Go: `v1.Time`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(
        rename = "docker_version",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub docker_version: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<History>,
    #[serde(default, deserialize_with = "null_default")]
    pub os: String,
    #[serde(default)]
    pub rootfs: RootFS,
    #[serde(default)]
    pub config: Config,
    #[serde(
        rename = "os.version",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub os_version: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub variant: String,
    #[serde(
        rename = "os.features",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub os_features: Vec<String>,
}

/// `v1.History`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(
        rename = "created_by",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub created_by: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub comment: String,
    #[serde(
        rename = "empty_layer",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub empty_layer: bool,
}

/// `v1.RootFS`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RootFS {
    #[serde(rename = "type", default, deserialize_with = "null_default")]
    pub type_: String,
    #[serde(rename = "diff_ids", default, deserialize_with = "null_default")]
    pub diff_ids: Vec<Hash>,
}

/// `v1.Config`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Config {
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "std::ops::Not::not")]
    pub attach_stderr: bool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "std::ops::Not::not")]
    pub attach_stdin: bool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "std::ops::Not::not")]
    pub attach_stdout: bool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub cmd: Vec<String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub domainname: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub entrypoint: Vec<String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub hostname: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub image: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub on_build: Vec<String>,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "std::ops::Not::not")]
    pub open_stdin: bool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "std::ops::Not::not")]
    pub stdin_once: bool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "std::ops::Not::not")]
    pub tty: bool,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub user: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub stop_signal: String,
    #[serde(default, deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub shell: Vec<String>,
}

/// `v1.ParseConfigFile`
pub fn parse_config_file(data: &[u8]) -> anyhow::Result<ConfigFile> {
    decode_json(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_parsing() {
        let h = Hash::new(&format!("sha256:{}", "a".repeat(64))).unwrap();
        assert_eq!(h.to_string(), format!("sha256:{}", "a".repeat(64)));
        assert_eq!(Hash::new("nope").unwrap_err().to_string(), "cannot parse hash: \"nope\"");
        assert_eq!(
            Hash::new("sha256:xyz").unwrap_err().to_string(),
            "found non-hex character in hash: x"
        );
        assert_eq!(
            Hash::new("sha256:abc").unwrap_err().to_string(),
            "wrong number of hex digits for sha256: abc"
        );
        assert_eq!(
            Hash::new("md5:abc").unwrap_err().to_string(),
            "unsupported hash: \"md5\""
        );
    }

    #[test]
    fn platform_string() {
        assert_eq!(default_platform().string(), "linux/amd64");
        let p = Platform {
            os: "linux".into(),
            architecture: "arm64".into(),
            variant: "v8".into(),
            os_version: "1".into(),
            ..Default::default()
        };
        assert_eq!(p.string(), "linux/arm64/v8:1");
    }

    #[test]
    fn config_labels() {
        let cfg = parse_config_file(
            br#"{"architecture":"amd64","os":"linux","config":{"Labels":{"io.openshift.release":"4.19.0"}},"rootfs":{"type":"layers","diff_ids":[]}}"#,
        )
        .unwrap();
        assert_eq!(cfg.config.labels["io.openshift.release"], "4.19.0");
    }
}
