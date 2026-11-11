//! Port of internal/controller/dpuservicetemplate/release_image.go

use std::sync::Arc;

use async_trait::async_trait;
use ctrlrt::{Ctx, Result, wrapf};
use gocompat::strconv::quote;
use registry::authn::Keychain;
use registry::mutate::{self, Extract};
use registry::{name, remote};
use serde::Deserialize;

use crate::gojson::{self, GoType};

pub(crate) const IMAGE_REFERENCES_PATH: &str = "release-manifests/image-references";
pub(crate) const OVN_KUBERNETES_NAME: &str = "ovn-kubernetes";

/// ReleaseImageReader extracts component images from an OCP release payload.
#[async_trait]
pub trait ReleaseImageReader: Send + Sync {
    async fn get_component_image(
        &self,
        ctx: &Ctx,
        release_image_ref: &str,
        component_name: &str,
        keychain: Arc<dyn Keychain>,
    ) -> Result<String>;
}

/// RemoteReleaseImageReader pulls an OCP release image from a registry and
/// extracts component image references from its image-references manifest.
#[derive(Clone, Copy, Debug, Default)]
pub struct RemoteReleaseImageReader;

#[async_trait]
impl ReleaseImageReader for RemoteReleaseImageReader {
    async fn get_component_image(
        &self,
        _ctx: &Ctx,
        release_image_ref: &str,
        component_name: &str,
        keychain: Arc<dyn Keychain>,
    ) -> Result<String> {
        let r = match name::parse_reference(release_image_ref, &[]) {
            Ok(r) => r,
            Err(e) => {
                let e = anyhow::Error::from(e);
                return Err(wrapf!(
                    e,
                    "parsing release image ref {}: {}",
                    quote(release_image_ref),
                    e
                ));
            }
        };

        let img = match remote::image(&r, &[remote::with_auth_from_keychain(keychain)]).await {
            Ok(i) => i,
            Err(e) => {
                return Err(wrapf!(
                    e,
                    "pulling release image {}: {}",
                    quote(release_image_ref),
                    e
                ));
            }
        };

        let mut reader = mutate::extract(&img);

        match find_component_in_image_references(&mut reader, component_name).await {
            Ok(image_ref) => Ok(image_ref),
            Err(e) => Err(wrapf!(
                e,
                "extracting {} from release image {}: {}",
                component_name,
                quote(release_image_ref),
                e
            )),
        }
    }
}

/// imageReferences is a minimal representation of the OpenShift ImageStream
/// stored at release-manifests/image-references in a release payload image.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ImageReferences {
    pub spec: ImageReferencesSpec,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ImageReferencesSpec {
    #[serde(deserialize_with = "null_default")]
    pub tags: Vec<ImageReferencesTag>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ImageReferencesTag {
    pub name: String,
    pub from: ImageReferencesFrom,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ImageReferencesFrom {
    pub name: String,
}

fn null_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

static IMAGE_REFERENCES_TAG_TYPE: GoType = GoType::Struct {
    name: "",
    type_string: r#"struct { Name string "json:\"name\""; From struct { Name string "json:\"name\"" } "json:\"from\"" }"#,
    fields: &[
        ("name", GoType::String),
        (
            "from",
            GoType::Struct {
                name: "",
                type_string: r#"struct { Name string "json:\"name\"" }"#,
                fields: &[("name", GoType::String)],
            },
        ),
    ],
};

/// The Go type `imageReferences` as seen by `encoding/json`.
static IMAGE_REFERENCES_TYPE: GoType = GoType::Struct {
    name: "imageReferences",
    type_string: "dpuservicetemplate.imageReferences",
    fields: &[(
        "spec",
        GoType::Struct {
            name: "",
            type_string: r#"struct { Tags []struct { Name string "json:\"name\""; From struct { Name string "json:\"name\"" } "json:\"from\"" } "json:\"tags\"" }"#,
            fields: &[(
                "tags",
                GoType::Slice {
                    type_string: r#"[]struct { Name string "json:\"name\""; From struct { Name string "json:\"name\"" } "json:\"from\"" }"#,
                    elem: &IMAGE_REFERENCES_TAG_TYPE,
                },
            )],
        },
    )],
};

/// `json.Unmarshal(data, &refs)` with Go's semantics and error messages.
pub(crate) fn unmarshal_image_references(
    data: &[u8],
) -> std::result::Result<ImageReferences, String> {
    let v = gojson::unmarshal(data, &IMAGE_REFERENCES_TYPE)?;
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// Scan the (flattened) release image filesystem for the image-references manifest and
/// return the image of `component_name`.
pub(crate) async fn find_component_in_image_references(
    reader: &mut Extract,
    component_name: &str,
) -> Result<String> {
    loop {
        let header = match reader.next_entry().await {
            Ok(Some(h)) => h,
            Ok(None) => {
                return Err(anyhow::anyhow!(
                    "file {} not found in release image",
                    IMAGE_REFERENCES_PATH
                ));
            }
            Err(e) => return Err(wrapf!(e, "reading tar: {}", e)),
        };

        if header.name != IMAGE_REFERENCES_PATH {
            continue;
        }

        let data = match reader.read_to_end().await {
            Ok(d) => d,
            Err(e) => return Err(wrapf!(e, "reading {}: {}", IMAGE_REFERENCES_PATH, e)),
        };

        let refs = match unmarshal_image_references(&data) {
            Ok(r) => r,
            Err(msg) => {
                let e = anyhow::anyhow!(msg);
                return Err(wrapf!(e, "parsing {}: {}", IMAGE_REFERENCES_PATH, e));
            }
        };

        for tag in &refs.spec.tags {
            if tag.name == component_name {
                if tag.from.name.is_empty() {
                    return Err(anyhow::anyhow!(
                        "component {} has empty image reference",
                        quote(component_name)
                    ));
                }
                return Ok(tag.from.name.clone());
            }
        }

        return Err(anyhow::anyhow!(
            "component {} not found in {}",
            quote(component_name),
            IMAGE_REFERENCES_PATH
        ));
    }
}

/// splitImage splits a container image reference into repository and tag/digest.
pub(crate) fn split_image(image: &str) -> Result<(String, String)> {
    if image.is_empty() {
        return Err(anyhow::anyhow!("empty image reference"));
    }

    if let Some(idx) = image.rfind("@sha256:").filter(|i| *i > 0) {
        return Ok((
            format!("{}@sha256", &image[..idx]),
            image[idx + "@sha256:".len()..].to_string(),
        ));
    }

    if let Some(last_colon) = image.rfind(':').filter(|i| *i > 0)
        && !image[last_colon..].contains('/')
    {
        return Ok((
            image[..last_colon].to_string(),
            image[last_colon + 1..].to_string(),
        ));
    }

    Err(anyhow::anyhow!(
        "image reference {} has no tag or digest",
        quote(image)
    ))
}
