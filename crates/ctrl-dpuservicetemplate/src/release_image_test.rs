//! Port of internal/controller/dpuservicetemplate/release_image_test.go
//! ("Release Image Utilities", internal package tests).

use registry::mutate::{self, Extract, StaticImage, StaticLayer};
use serde_json::json;

use crate::release_image::*;

/// A ustar archive with one regular file (mode 0600) per entry (Go: `buildTestTar`).
fn build_test_tar(files: &[(&str, &[u8])]) -> Vec<u8> {
    fn octal(field: &mut [u8], v: u64) {
        let s = format!("{:0width$o}", v, width = field.len() - 1);
        field[..s.len()].copy_from_slice(s.as_bytes());
        field[s.len()] = 0;
    }
    let mut out = Vec::new();
    for (name, content) in files {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut h[100..108], 0o600);
        octal(&mut h[108..116], 0);
        octal(&mut h[116..124], 0);
        octal(&mut h[124..136], content.len() as u64);
        octal(&mut h[136..148], 0);
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|b| *b as u32).sum();
        let s = format!("{sum:06o}\0 ");
        h[148..156].copy_from_slice(s.as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(content);
        let pad = (512 - content.len() % 512) % 512;
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    out.extend(std::iter::repeat_n(0u8, 1024));
    out
}

/// The tar stream as read by the production code (`mutate.Extract` of a one-layer image).
fn reader(files: &[(&str, &[u8])]) -> Extract {
    let img = StaticImage(vec![StaticLayer(build_test_tar(files).into())]);
    mutate::extract(&img)
}

/// `json.Marshal(imageReferences{...})` for the given (name, from.name) tags.
fn refs(tags: &[(&str, &str)]) -> Vec<u8> {
    let tags: Vec<_> = tags
        .iter()
        .map(|(n, f)| json!({"name": n, "from": {"name": f}}))
        .collect();
    serde_json::to_vec(&json!({"spec": {"tags": tags}})).unwrap()
}

// Describe("findComponentInImageReferences")

#[tokio::test]
async fn release_image_utilities_find_component_in_image_references_should_find_a_component_by_name()
 {
    let data = refs(&[
        (
            "ovn-kubernetes",
            "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:abc123",
        ),
        ("kube-proxy", "quay.io/other:v1"),
    ]);
    let mut r = reader(&[(IMAGE_REFERENCES_PATH, &data)]);
    let result = find_component_in_image_references(&mut r, "ovn-kubernetes")
        .await
        .expect("no error");
    assert_eq!(
        result,
        "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:abc123"
    );
}

#[tokio::test]
async fn release_image_utilities_find_component_in_image_references_should_return_error_when_component_is_not_found()
 {
    let data = refs(&[("kube-proxy", "quay.io/other:v1")]);
    let mut r = reader(&[(IMAGE_REFERENCES_PATH, &data)]);
    let err = find_component_in_image_references(&mut r, "ovn-kubernetes")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}

#[tokio::test]
async fn release_image_utilities_find_component_in_image_references_should_return_error_when_image_references_file_is_missing()
 {
    let mut r = reader(&[("some-other-file", b"data")]);
    let err = find_component_in_image_references(&mut r, "ovn-kubernetes")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not found in release image"),
        "{err}"
    );
}

#[tokio::test]
async fn release_image_utilities_find_component_in_image_references_should_return_error_when_component_has_empty_image_reference()
 {
    let data = refs(&[("ovn-kubernetes", "")]);
    let mut r = reader(&[(IMAGE_REFERENCES_PATH, &data)]);
    let err = find_component_in_image_references(&mut r, "ovn-kubernetes")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("empty image reference"), "{err}");
}

// Describe("splitImage")

#[test]
fn release_image_utilities_split_image_should_split_image_with_tag() {
    let (repo, tag) = split_image("quay.io/openshift/ovn:v4.19.0").expect("no error");
    assert_eq!(repo, "quay.io/openshift/ovn");
    assert_eq!(tag, "v4.19.0");
}

#[test]
fn release_image_utilities_split_image_should_split_image_with_sha256_digest() {
    let (repo, tag) =
        split_image("quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:abcdef1234567890")
            .expect("no error");
    assert_eq!(
        repo,
        "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256"
    );
    assert_eq!(tag, "abcdef1234567890");
}

#[test]
fn release_image_utilities_split_image_should_return_error_when_no_tag_or_digest_present() {
    let err = split_image("quay.io/openshift/ovn").unwrap_err();
    assert!(err.to_string().contains("has no tag or digest"), "{err}");
}

#[test]
fn release_image_utilities_split_image_should_return_error_for_empty_image() {
    let err = split_image("").unwrap_err();
    assert!(err.to_string().contains("empty image reference"), "{err}");
}

// Additional (not in Go): exact Go error strings of the image-references parsing paths,
// checked against `encoding/json` output.

#[test]
fn image_references_unmarshal_go_error_messages() {
    let cases: &[(&str, &str)] = &[
        (
            r#"{"spec":{"tags":[{"name":5}]}}"#,
            "json: cannot unmarshal number into Go struct field .spec.tags.name of type string",
        ),
        (
            r#"{"spec":{"tags":[{"from":{"name":1}}]}}"#,
            "json: cannot unmarshal number into Go struct field .spec.tags.from.name of type string",
        ),
        (
            r#"{"spec":5}"#,
            r#"json: cannot unmarshal number into Go struct field imageReferences.spec of type struct { Tags []struct { Name string "json:\"name\""; From struct { Name string "json:\"name\"" } "json:\"from\"" } "json:\"tags\"" }"#,
        ),
        (
            r#"{"spec":{"tags":{}}}"#,
            r#"json: cannot unmarshal object into Go struct field .spec.tags of type []struct { Name string "json:\"name\""; From struct { Name string "json:\"name\"" } "json:\"from\"" }"#,
        ),
        (
            "[]",
            "json: cannot unmarshal array into Go value of type dpuservicetemplate.imageReferences",
        ),
        (
            r#"{"spec":{"tags":[5]}}"#,
            r#"json: cannot unmarshal number into Go struct field .spec.tags of type struct { Name string "json:\"name\""; From struct { Name string "json:\"name\"" } "json:\"from\"" }"#,
        ),
    ];
    for (input, want) in cases {
        let err = unmarshal_image_references(input.as_bytes()).unwrap_err();
        assert_eq!(&err, want, "input {input}");
    }
    let ok = unmarshal_image_references(
        br#"{"spec":{"TAGS":[{"Name":"a","from":{"name":"b"}}]},"x":1}"#,
    )
    .unwrap();
    assert_eq!(ok.spec.tags.len(), 1);
    assert_eq!(ok.spec.tags[0].name, "a");
    assert_eq!(ok.spec.tags[0].from.name, "b");
}
