//! Port of `internal/ignition/ignition_test.go` (Ginkgo suite "Ignition Suite").

use apis::dpf::provisioning::v1alpha1::{
    ConfigFile, DPUFlavor, DPUFlavorOVS, DPUFlavorSpec, FILE_APPEND, FILE_OVERRIDE,
};
use ignition::types::{Config, File, Resource};
use ignition::{
    IGNITION_VERSION, add_dpu_flavor_json, add_dpu_flavor_yaml, add_flavor_ovs_script,
    clean_dpu_flavor, decode_ignition, enable_mtu, encode_gzip_file, encode_ignition, files,
    gzip_ignition_files, marshal_json, merge_flavor_config_files, new_empty_ignition, ptr,
    replace_machine_os_url,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde_json::{Value, json};

const COMPRESSION_GZIP: &str = "gzip";
const TEST_IGNITION_VERSION: &str = "3.4.0";
const TEST_DATA_SOURCE: &str = "data:,hello";
const TEST_MLNX_BF_CONF_PATH: &str = "/etc/mellanox/mlnx-bf.conf";
const DPU_FLAVOR_YAML_PATH: &str = "/etc/dpf/dpuflavor.yaml";
const DPU_FLAVOR_JSON_PATH: &str = "/etc/dpf/dpuflavor.json";

fn file_with_source(path: &str, source: &str) -> File {
    let mut f = File::new(path.to_string());
    f.contents = Some(Resource {
        source: Some(source.to_string()),
        ..Default::default()
    });
    f
}

fn source(f: &File) -> &str {
    ignition::file_source(f).unwrap()
}

fn compression(f: &File) -> Option<&str> {
    f.contents.as_ref().and_then(|c| c.compression.as_deref())
}

fn gunzip(b: &[u8]) -> Vec<u8> {
    gocompat::gzip::decompress(b).unwrap()
}

fn b64dec(s: &str) -> Vec<u8> {
    gocompat::base64::decode_string(s.as_bytes()).unwrap()
}

fn unmarshal_map(data: &[u8]) -> serde_json::Map<String, Value> {
    gocompat::json::unmarshal_object(data).unwrap().unwrap()
}

// ---------------------------------------------------------------------- NewEmptyIgnition

#[test]
fn new_empty_ignition_should_create_ignition_with_the_specified_version() {
    let ign = new_empty_ignition(TEST_IGNITION_VERSION);
    assert_eq!(ign.ignition.version, TEST_IGNITION_VERSION);
}

#[test]
fn new_empty_ignition_should_default_to_ignition_version_when_version_is_empty() {
    let ign = new_empty_ignition("");
    assert_eq!(ign.ignition.version, IGNITION_VERSION);
}

// ---------------------------------------------------------------------------- MarshalJSON

#[test]
fn marshal_json_should_strip_empty_objects_from_json_output() {
    let ign = new_empty_ignition("3.4.0");
    let data = marshal_json(&ign).unwrap();
    let m = unmarshal_map(&data);
    // Should have ignition version but no empty storage/systemd/passwd
    assert!(m.contains_key("ignition"));
    assert!(!m.contains_key("storage"));
    assert!(!m.contains_key("systemd"));
    assert!(!m.contains_key("passwd"));
}

#[test]
fn marshal_json_should_preserve_non_empty_data() {
    let mut ign = new_empty_ignition("3.4.0");
    ignition::files_mut(&mut ign).push(file_with_source("/etc/test", TEST_DATA_SOURCE));
    let data = marshal_json(&ign).unwrap();
    let m = unmarshal_map(&data);
    assert!(m.contains_key("storage"));
}

// ------------------------------------------------------------ EncodeIgnition / DecodeIgnition

#[test]
fn encode_ignition_decode_ignition_should_round_trip_encode_and_decode_an_ignition_config() {
    let mut ign = new_empty_ignition("3.4.0");
    ignition::files_mut(&mut ign).push(file_with_source("/etc/test", "data:,test-content"));
    let encoded = encode_ignition(&ign).unwrap();
    assert!(!encoded.is_empty());
    let decoded = decode_ignition(&encoded).unwrap();
    assert_eq!(decoded.ignition.version, "3.4.0");
    assert_eq!(files(&decoded).len(), 1);
    assert_eq!(files(&decoded)[0].path, "/etc/test");
}

#[test]
fn encode_ignition_decode_ignition_should_fail_to_decode_invalid_base64() {
    let err = decode_ignition("not-valid-base64!!!").unwrap_err();
    assert!(err.to_string().contains("base64"));
}

#[test]
fn encode_ignition_decode_ignition_should_fail_to_decode_invalid_gzip() {
    let not_gzip = gocompat::base64::encode_to_string(b"this is not gzip");
    let err = decode_ignition(&not_gzip).unwrap_err();
    assert!(err.to_string().contains("gzip"));
}

// --------------------------------------------------------------------------- EncodeGzipFile

#[test]
fn encode_gzip_file_should_return_a_gzip_compressed_base64_encoded_resource() {
    let resource = encode_gzip_file(b"#!/bin/bash\necho hello").unwrap();
    let src = resource.source.as_deref().expect("source");
    assert!(src.starts_with("data:;base64,"));
    assert_eq!(
        resource.compression.as_deref().expect("compression"),
        COMPRESSION_GZIP
    );
}

#[test]
fn encode_gzip_file_should_produce_decompressible_content_that_matches_the_input() {
    let content = b"test file content for round-trip";
    let resource = encode_gzip_file(content).unwrap();
    // Extract base64 data
    let b64 = resource
        .source
        .as_deref()
        .unwrap()
        .trim_start_matches("data:;base64,");
    let compressed = b64dec(b64);
    // Decompress
    assert_eq!(gunzip(&compressed), content);
}

// ---------------------------------------------------------------------- ReplaceMachineOSURL

/// Helper to build an ignition config with the machine config encapsulated file.
fn build_ign_with_machine_config(os_image_url: &str) -> Config {
    let machine_config = json!({"spec": {"osImageURL": os_image_url}});
    let json_bytes = gocompat::json::marshal(&machine_config);
    let encoded = gocompat::url::query_escape(&json_bytes);
    let source = format!("data:,{encoded}");
    let mut ign = new_empty_ignition("3.4.0");
    ignition::files_mut(&mut ign).push(file_with_source(
        "/etc/ignition-machine-config-encapsulated.json",
        &source,
    ));
    ign
}

#[test]
fn replace_machine_os_url_should_replace_the_os_image_url_in_the_machine_config() {
    let mut ign = build_ign_with_machine_config("https://old-url.example.com/image");
    replace_machine_os_url(&mut ign, "https://new-url.example.com/image").unwrap();
    // Decode and verify
    let src = source(&files(&ign)[0]);
    let encoded_data = src.trim_start_matches("data:,");
    let decoded = gocompat::url::query_unescape(encoded_data).unwrap();
    let config = unmarshal_map(&decoded);
    assert_eq!(
        config["spec"]["osImageURL"],
        json!("https://new-url.example.com/image")
    );
}

#[test]
fn replace_machine_os_url_should_return_error_when_the_file_is_not_found() {
    let mut ign = new_empty_ignition("3.4.0"); // no files
    let err = replace_machine_os_url(&mut ign, "https://example.com").unwrap_err();
    assert!(err.to_string().contains("machine OS URL file not found"));
}

#[test]
fn replace_machine_os_url_should_return_error_when_the_data_uri_format_is_unexpected() {
    let mut ign = new_empty_ignition("3.4.0");
    ignition::files_mut(&mut ign).push(file_with_source(
        "/etc/ignition-machine-config-encapsulated.json",
        "not-a-data-uri",
    ));
    let err = replace_machine_os_url(&mut ign, "https://example.com").unwrap_err();
    assert!(err.to_string().contains("unexpected data URI format"));
}

#[test]
fn replace_machine_os_url_should_return_error_when_data_uri_contains_invalid_json() {
    let src = format!("data:,{}", gocompat::url::query_escape("not-json"));
    let mut ign = new_empty_ignition("3.4.0");
    ignition::files_mut(&mut ign).push(file_with_source(
        "/etc/ignition-machine-config-encapsulated.json",
        &src,
    ));
    let err = replace_machine_os_url(&mut ign, "https://example.com").unwrap_err();
    assert!(err.to_string().contains("failed to unmarshal"));
}

// ----------------------------------------------------------------------- AddFlavorOVSScript

fn ovs_spec(script: &str) -> DPUFlavorSpec {
    DPUFlavorSpec {
        ovs: DPUFlavorOVS {
            raw_config_script: script.to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn add_flavor_ovs_script_should_add_the_ovs_script_file_to_ignition() {
    let mut ign = new_empty_ignition("3.4.0");
    add_flavor_ovs_script(&mut ign, &ovs_spec("#!/bin/bash\novs-vsctl add-br br0"));
    assert_eq!(files(&ign).len(), 1);
    let file = &files(&ign)[0];
    assert_eq!(file.path, "/usr/local/bin/dpf-ovs-script.sh");
    assert_eq!(file.mode.unwrap(), 0o755);
    assert!(file.overwrite.unwrap());
}

#[test]
fn add_flavor_ovs_script_should_base64_encode_the_script_content() {
    let mut ign = new_empty_ignition("3.4.0");
    let script = "#!/bin/bash\necho test";
    add_flavor_ovs_script(&mut ign, &ovs_spec(script));
    let src = source(&files(&ign)[0]);
    assert!(src.starts_with("data:text/plain;charset=utf-8;base64,"));
    let b64 = src.trim_start_matches("data:text/plain;charset=utf-8;base64,");
    assert_eq!(String::from_utf8(b64dec(b64)).unwrap(), script);
}

#[test]
fn add_flavor_ovs_script_should_handle_an_empty_script() {
    let mut ign = new_empty_ignition("3.4.0");
    add_flavor_ovs_script(&mut ign, &ovs_spec(""));
    assert_eq!(files(&ign).len(), 1);
}

// -------------------------------------------------------------------------------- EnableMTU

#[test]
fn enable_mtu_should_add_nm_connection_files_for_p0_p1_pf0hpf_pf1hpf() {
    let mut ign = new_empty_ignition("3.4.0");
    enable_mtu(&mut ign, 9000);
    let expected = ["p0", "p1", "pf0hpf", "pf1hpf"];
    assert_eq!(files(&ign).len(), expected.len());
    for (i, iface) in expected.iter().enumerate() {
        assert_eq!(
            files(&ign)[i].path,
            format!("/etc/NetworkManager/system-connections/{iface}.nmconnection")
        );
    }
}

#[test]
fn enable_mtu_should_set_correct_mtu_value_in_the_generated_config() {
    let mut ign = new_empty_ignition("3.4.0");
    enable_mtu(&mut ign, 9000);
    let src = source(&files(&ign)[0]);
    let decoded =
        gocompat::url::query_unescape(src.trim_start_matches("data:text/plain,")).unwrap();
    assert!(String::from_utf8(decoded).unwrap().contains("mtu=9000"));
}

#[test]
fn enable_mtu_should_set_file_mode_0600_for_nm_connection_files() {
    let mut ign = new_empty_ignition("3.4.0");
    enable_mtu(&mut ign, 9000);
    for file in files(&ign) {
        assert_eq!(file.mode.unwrap(), 0o600);
    }
}

#[test]
fn enable_mtu_should_modify_existing_pf0vf0_nmconnection_to_add_mtu() {
    let mut ign = new_empty_ignition("3.4.0");
    let existing = format!(
        "data:text/plain,{}",
        gocompat::url::query_escape("[connection]\nid=pf0vf0\n\n[ethernet]\nwake-on-lan=0\n")
    );
    ignition::files_mut(&mut ign).push(file_with_source(
        "/etc/NetworkManager/system-connections/pf0vf0.nmconnection",
        &existing,
    ));
    enable_mtu(&mut ign, 9000);
    // Find the pf0vf0 file (should be the first one, before the 4 new ones)
    let pf0vf0 = files(&ign)
        .iter()
        .find(|f| f.path.ends_with("pf0vf0.nmconnection"))
        .map(source)
        .unwrap_or("");
    let decoded = String::from_utf8(
        gocompat::url::query_unescape(pf0vf0.trim_start_matches("data:text/plain,")).unwrap(),
    )
    .unwrap();
    assert!(decoded.contains("mtu=9000"));
    assert!(decoded.contains("[ethernet]\nmtu=9000"));
}

// ------------------------------------------------------------------------ GzipIgnitionFiles

#[test]
fn gzip_ignition_files_should_compress_data_uri_files() {
    let mut ign = new_empty_ignition("3.4.0");
    ignition::files_mut(&mut ign).push(file_with_source("/etc/test", "data:,hello+world"));
    gzip_ignition_files(&mut ign).unwrap();
    let file = &files(&ign)[0];
    assert_eq!(compression(file).expect("compression"), COMPRESSION_GZIP);
    assert!(source(file).starts_with("data:;base64,"));
}

#[test]
fn gzip_ignition_files_should_compress_base64_files() {
    let mut ign = new_empty_ignition("3.4.0");
    let encoded = gocompat::base64::encode_to_string(b"test content");
    ignition::files_mut(&mut ign).push(file_with_source(
        "/etc/test",
        &format!("data:;base64,{encoded}"),
    ));
    gzip_ignition_files(&mut ign).unwrap();
    assert_eq!(compression(&files(&ign)[0]).unwrap(), COMPRESSION_GZIP);
}

#[test]
fn gzip_ignition_files_should_skip_already_compressed_files() {
    let mut ign = new_empty_ignition("3.4.0");
    let src = "data:;base64,already-compressed";
    let mut f = file_with_source("/etc/test", src);
    f.contents.as_mut().unwrap().compression = Some(COMPRESSION_GZIP.to_string());
    ignition::files_mut(&mut ign).push(f);
    gzip_ignition_files(&mut ign).unwrap();
    // Source should be unchanged
    assert_eq!(source(&files(&ign)[0]), src);
}

#[test]
fn gzip_ignition_files_should_skip_files_with_nil_or_empty_source() {
    let mut ign = new_empty_ignition("3.4.0");
    let mut f = File::new("/etc/test".to_string());
    f.contents = Some(Resource::default());
    ignition::files_mut(&mut ign).push(f);
    gzip_ignition_files(&mut ign).unwrap();
    assert!(ignition::file_source(&files(&ign)[0]).is_none());
}

#[test]
fn gzip_ignition_files_should_produce_content_that_decompresses_to_the_original() {
    let mut ign = new_empty_ignition("3.4.0");
    let original = "original file content here";
    ignition::files_mut(&mut ign).push(file_with_source(
        "/etc/test",
        &format!("data:,{}", gocompat::url::query_escape(original)),
    ));
    gzip_ignition_files(&mut ign).unwrap();
    // Decompress and verify
    let b64 = source(&files(&ign)[0]).trim_start_matches("data:;base64,");
    assert_eq!(String::from_utf8(gunzip(&b64dec(b64))).unwrap(), original);
}

// --------------------------------------------------------------------------- cleanDPUFlavor

#[test]
fn clean_dpu_flavor_should_keep_only_type_meta_name_namespace_and_spec() {
    let flavor = DPUFlavor {
        api_version: "provisioning.dpu.nvidia.com/v1alpha1".into(),
        kind: "DPUFlavor".into(),
        metadata: ObjectMeta {
            name: Some("test-flavor".into()),
            namespace: Some("test-ns".into()),
            uid: Some("some-uid".into()),
            resource_version: Some("12345".into()),
            labels: Some([("key".to_string(), "value".to_string())].into()),
            annotations: Some([("ann".to_string(), "val".to_string())].into()),
            ..Default::default()
        },
        spec: ovs_spec("#!/bin/bash"),
        ..Default::default()
    };
    let cleaned = clean_dpu_flavor(&flavor);
    assert_eq!(
        (&cleaned.api_version, &cleaned.kind),
        (&flavor.api_version, &flavor.kind)
    );
    assert_eq!(cleaned.metadata.name.as_deref(), Some("test-flavor"));
    assert_eq!(cleaned.metadata.namespace.as_deref(), Some("test-ns"));
    assert_eq!(cleaned.spec, flavor.spec);
    assert!(cleaned.metadata.uid.unwrap_or_default().is_empty());
    assert!(
        cleaned
            .metadata
            .resource_version
            .unwrap_or_default()
            .is_empty()
    );
    assert!(cleaned.metadata.labels.is_none());
    assert!(cleaned.metadata.annotations.is_none());
}

// ------------------------------------------------------------------------- AddDPUFlavorYAML

fn flavor(resource_version: Option<&str>) -> DPUFlavor {
    DPUFlavor {
        metadata: ObjectMeta {
            name: Some("my-flavor".into()),
            namespace: Some("ns".into()),
            resource_version: resource_version.map(String::from),
            ..Default::default()
        },
        spec: ovs_spec("#!/bin/bash"),
        ..Default::default()
    }
}

#[test]
fn add_dpu_flavor_yaml_should_add_a_yaml_file_at_the_correct_path() {
    let mut ign = new_empty_ignition(TEST_IGNITION_VERSION);
    add_dpu_flavor_yaml(&mut ign, &flavor(None)).unwrap();
    assert_eq!(files(&ign).len(), 1);
    assert_eq!(files(&ign)[0].path, DPU_FLAVOR_YAML_PATH);
    assert_eq!(files(&ign)[0].mode.unwrap(), 0o644);
    assert!(files(&ign)[0].overwrite.unwrap());
}

#[test]
fn add_dpu_flavor_yaml_should_produce_valid_yaml_content_with_only_cleaned_fields() {
    let mut ign = new_empty_ignition(TEST_IGNITION_VERSION);
    add_dpu_flavor_yaml(&mut ign, &flavor(Some("999"))).unwrap();
    let src = source(&files(&ign)[0]);
    let decoded = b64dec(src.trim_start_matches("data:text/plain;charset=utf-8;base64,"));
    let result = gocompat::yaml::unmarshal_any(&decoded).unwrap();
    let metadata = result["metadata"].as_object().unwrap();
    assert_eq!(metadata["name"], json!("my-flavor"));
    assert_eq!(metadata["namespace"], json!("ns"));
    assert!(!metadata.contains_key("resourceVersion"));
}

// ------------------------------------------------------------------------- AddDPUFlavorJSON

#[test]
fn add_dpu_flavor_json_should_add_a_json_file_at_the_correct_path() {
    let mut ign = new_empty_ignition(TEST_IGNITION_VERSION);
    add_dpu_flavor_json(&mut ign, &flavor(None)).unwrap();
    assert_eq!(files(&ign).len(), 1);
    assert_eq!(files(&ign)[0].path, DPU_FLAVOR_JSON_PATH);
    assert_eq!(files(&ign)[0].mode.unwrap(), 0o644);
    assert!(files(&ign)[0].overwrite.unwrap());
}

#[test]
fn add_dpu_flavor_json_should_produce_valid_json_content_with_only_cleaned_fields() {
    let mut ign = new_empty_ignition(TEST_IGNITION_VERSION);
    add_dpu_flavor_json(&mut ign, &flavor(Some("999"))).unwrap();
    let src = source(&files(&ign)[0]);
    let decoded = b64dec(src.trim_start_matches("data:text/plain;charset=utf-8;base64,"));
    let result = unmarshal_map(&decoded);
    let metadata = result["metadata"].as_object().unwrap();
    assert_eq!(metadata["name"], json!("my-flavor"));
    assert_eq!(metadata["namespace"], json!("ns"));
    assert!(!metadata.contains_key("resourceVersion"));
}

// ------------------------------------------------------------------- MergeFlavorConfigFiles

/// decodeFlavorFileContent extracts raw content from a gzip+base64-encoded ignition file source.
fn decode_flavor_file_content(source: &str) -> String {
    let b64 = source.trim_start_matches("data:;base64,");
    String::from_utf8(gunzip(&b64dec(b64))).unwrap()
}

/// BeforeEach: base ignition with gzip-compressed default files.
fn base_ign() -> Config {
    let mut base = new_empty_ignition(TEST_IGNITION_VERSION);
    let default_resource = encode_gzip_file(b"DEFAULT_CONTENT").unwrap();
    let mk = |path: &str, mode: i64| {
        let mut f = File::new(path.to_string());
        f.overwrite = ptr(true);
        f.contents = Some(default_resource.clone());
        f.mode = ptr(mode);
        f
    };
    *ignition::files_mut(&mut base) = vec![
        mk(TEST_MLNX_BF_CONF_PATH, 0o644),
        mk("/etc/mellanox/mlnx-ovs.conf", 0o644),
        mk("/usr/local/bin/install-dpu-agent.sh", 0o755),
    ];
    base
}

fn cf(path: &str, operation: &str, raw: Option<&str>, permissions: &str) -> ConfigFile {
    ConfigFile {
        path: path.into(),
        operation: operation.into(),
        raw: raw.map(String::from),
        permissions: permissions.into(),
        ..Default::default()
    }
}

fn spec(files: Vec<ConfigFile>) -> DPUFlavorSpec {
    DPUFlavorSpec {
        config_files: files,
        ..Default::default()
    }
}

fn find<'a>(ign: &'a Config, path: &str) -> &'a File {
    files(ign)
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} not found"))
}

#[test]
fn merge_flavor_config_files_should_be_a_no_op_when_there_are_no_config_files() {
    let mut base = base_ign();
    merge_flavor_config_files(&mut base, &DPUFlavorSpec::default()).unwrap();
    assert_eq!(files(&base).len(), 3);
}

#[test]
fn merge_flavor_config_files_should_override_a_base_file_at_a_conflicting_path() {
    let mut base = base_ign();
    let s = spec(vec![cf(
        TEST_MLNX_BF_CONF_PATH,
        FILE_OVERRIDE,
        Some("ALLOW_SHARED_RQ=\"yes\"\nCUSTOM_SETTING=\"on\"\n"),
        "0644",
    )]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    // Total files: 3 base - 1 overridden + 0 new = still 3
    assert_eq!(files(&base).len(), 3);
    // The overridden file should have the DPUFlavor content
    let f = find(&base, TEST_MLNX_BF_CONF_PATH);
    let content = decode_flavor_file_content(source(f));
    assert!(content.contains("ALLOW_SHARED_RQ=\"yes\""));
    assert!(content.contains("CUSTOM_SETTING=\"on\""));
    assert!(!content.contains("DEFAULT_CONTENT"));
    assert!(f.overwrite.unwrap());
}

#[test]
fn merge_flavor_config_files_should_append_to_a_base_file_at_a_conflicting_path() {
    let mut base = base_ign();
    let s = spec(vec![cf(
        TEST_MLNX_BF_CONF_PATH,
        FILE_APPEND,
        Some("EXTRA_SETTING=\"yes\"\n"),
        "0644",
    )]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    // Total should still be 3 -- the append merges into the existing file entry
    assert_eq!(files(&base).len(), 3);
    let f = find(&base, TEST_MLNX_BF_CONF_PATH);
    // Base contents preserved
    assert!(ignition::file_source(f).is_some());
    assert_eq!(decode_flavor_file_content(source(f)), "DEFAULT_CONTENT");
    // Append entries added
    let append = f.append.as_deref().unwrap_or_default();
    assert_eq!(append.len(), 1);
    assert_eq!(
        decode_flavor_file_content(append[0].source.as_deref().unwrap()),
        "EXTRA_SETTING=\"yes\"\n"
    );
}

#[test]
fn merge_flavor_config_files_should_add_a_new_file_when_path_does_not_conflict() {
    let mut base = base_ign();
    let s = spec(vec![cf(
        "/etc/mellanox/mlnx-sf.conf",
        FILE_OVERRIDE,
        Some("SF_CONFIG=1\n"),
        "0644",
    )]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    // 3 base + 1 new = 4
    assert_eq!(files(&base).len(), 4);
    let mut found = false;
    for f in files(&base) {
        if f.path == "/etc/mellanox/mlnx-sf.conf" {
            found = true;
            assert_eq!(decode_flavor_file_content(source(f)), "SF_CONFIG=1\n");
        }
    }
    assert!(found, "new file should be present in ignition");
}

#[test]
fn merge_flavor_config_files_should_handle_a_mix_of_override_append_and_new_files() {
    let mut base = base_ign();
    let s = spec(vec![
        cf(
            TEST_MLNX_BF_CONF_PATH,
            FILE_OVERRIDE,
            Some("REPLACED_CONTENT\n"),
            "",
        ),
        cf(
            "/etc/mellanox/mlnx-ovs.conf",
            FILE_APPEND,
            Some("APPENDED_LINE\n"),
            "",
        ),
        cf(
            "/etc/dpf/custom.conf",
            FILE_OVERRIDE,
            Some("NEW_FILE\n"),
            "",
        ),
    ]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    // 3 base - 1 overridden + 1 new = 4
    assert_eq!(files(&base).len(), 4);
    // mlnx-bf.conf: overridden
    assert_eq!(
        decode_flavor_file_content(source(find(&base, TEST_MLNX_BF_CONF_PATH))),
        "REPLACED_CONTENT\n"
    );
    // mlnx-ovs.conf: base contents preserved, append added
    let ovs = find(&base, "/etc/mellanox/mlnx-ovs.conf");
    assert_eq!(decode_flavor_file_content(source(ovs)), "DEFAULT_CONTENT");
    assert_eq!(ovs.append.as_deref().unwrap_or_default().len(), 1);
    // custom.conf: new file
    assert_eq!(
        decode_flavor_file_content(source(find(&base, "/etc/dpf/custom.conf"))),
        "NEW_FILE\n"
    );
    // install-dpu-agent.sh: untouched base file
    assert_eq!(
        decode_flavor_file_content(source(find(&base, "/usr/local/bin/install-dpu-agent.sh"))),
        "DEFAULT_CONTENT"
    );
}

#[test]
fn merge_flavor_config_files_should_preserve_file_ordering_with_overridden_file_in_its_original_position()
 {
    let mut base = base_ign();
    let s = spec(vec![cf(
        "/etc/mellanox/mlnx-ovs.conf",
        FILE_OVERRIDE,
        Some("NEW_OVS\n"),
        "",
    )]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    // mlnx-ovs.conf was at index 1, should still be at index 1
    assert_eq!(files(&base)[0].path, TEST_MLNX_BF_CONF_PATH);
    assert_eq!(files(&base)[1].path, "/etc/mellanox/mlnx-ovs.conf");
    assert_eq!(files(&base)[2].path, "/usr/local/bin/install-dpu-agent.sh");
}

#[test]
fn merge_flavor_config_files_should_use_default_permissions_0644_when_not_specified() {
    let mut base = base_ign();
    let s = spec(vec![cf(
        "/etc/dpf/test.conf",
        FILE_OVERRIDE,
        Some("test"),
        "",
    )]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    assert_eq!(find(&base, "/etc/dpf/test.conf").mode.unwrap(), 0o644);
}

#[test]
fn merge_flavor_config_files_should_use_custom_permissions_when_specified() {
    let mut base = base_ign();
    let s = spec(vec![cf(
        "/etc/dpf/script.sh",
        FILE_OVERRIDE,
        Some("#!/bin/bash\n"),
        "0755",
    )]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    assert_eq!(find(&base, "/etc/dpf/script.sh").mode.unwrap(), 0o755);
}

#[test]
fn merge_flavor_config_files_should_return_an_error_for_invalid_permissions() {
    let mut base = base_ign();
    let s = spec(vec![cf(
        "/etc/dpf/bad.conf",
        FILE_OVERRIDE,
        Some("content"),
        "invalid",
    )]);
    let err = merge_flavor_config_files(&mut base, &s).unwrap_err();
    assert!(err.to_string().contains("invalid permissions"));
}

#[test]
fn merge_flavor_config_files_should_skip_entries_with_empty_paths() {
    let mut base = base_ign();
    let s = spec(vec![
        cf("", "", Some("should be skipped"), ""),
        cf("/etc/dpf/valid.conf", FILE_OVERRIDE, Some("valid"), ""),
    ]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    // 3 base + 1 new (empty path skipped) = 4
    assert_eq!(files(&base).len(), 4);
}

#[test]
fn merge_flavor_config_files_should_handle_empty_raw_content_for_override_creates_empty_file() {
    let mut base = base_ign();
    let s = spec(vec![cf(
        "/etc/mellanox/mlnx-sf.conf",
        FILE_OVERRIDE,
        Some(""),
        "",
    )]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    assert!(
        decode_flavor_file_content(source(find(&base, "/etc/mellanox/mlnx-sf.conf"))).is_empty()
    );
}

#[test]
fn merge_flavor_config_files_should_override_multiple_base_files_simultaneously() {
    let mut base = base_ign();
    let s = spec(vec![
        cf(TEST_MLNX_BF_CONF_PATH, FILE_OVERRIDE, Some("NEW_BF\n"), ""),
        cf(
            "/etc/mellanox/mlnx-ovs.conf",
            FILE_OVERRIDE,
            Some("NEW_OVS\n"),
            "",
        ),
    ]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    assert_eq!(files(&base).len(), 3);
    for f in files(&base) {
        match f.path.as_str() {
            TEST_MLNX_BF_CONF_PATH => assert_eq!(decode_flavor_file_content(source(f)), "NEW_BF\n"),
            "/etc/mellanox/mlnx-ovs.conf" => {
                assert_eq!(decode_flavor_file_content(source(f)), "NEW_OVS\n")
            }
            "/usr/local/bin/install-dpu-agent.sh" => {
                assert_eq!(decode_flavor_file_content(source(f)), "DEFAULT_CONTENT")
            }
            _ => {}
        }
    }
}

#[test]
fn merge_flavor_config_files_should_update_mode_on_append_if_flavor_specifies_different_permissions()
 {
    let mut base = base_ign();
    let s = spec(vec![cf(
        TEST_MLNX_BF_CONF_PATH,
        FILE_APPEND,
        Some("APPENDED\n"),
        "0600",
    )]);
    merge_flavor_config_files(&mut base, &s).unwrap();
    // Mode should be updated to the flavor's permissions
    assert_eq!(find(&base, TEST_MLNX_BF_CONF_PATH).mode.unwrap(), 0o600);
}
