//! Port of `internal/ignition/validate_test.go` (Describe "ValidateConfig").

use ignition::types::{Config, File, Ignition, Resource, Unit};
use ignition::{files_mut, new_empty_ignition, units_mut, validate_config};

const TEST_IGNITION_VERSION: &str = "3.4.0";
const TEST_DATA_SOURCE: &str = "data:,hello";
const UNIT_CONTENTS: &str = "[Unit]\nDescription=Test\n[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target";

fn file(path: &str, source: &str) -> File {
    let mut f = File::new(path.to_string());
    f.overwrite = Some(true);
    f.mode = Some(0o644);
    f.contents = Some(Resource {
        source: Some(source.to_string()),
        ..Default::default()
    });
    f
}

fn unit(name: &str) -> Unit {
    let mut u = Unit::new(name.to_string());
    u.enabled = Some(true);
    u.contents = Some(UNIT_CONTENTS.to_string());
    u
}

#[test]
fn validate_config_should_accept_a_valid_empty_ignition_config() {
    let cfg = new_empty_ignition(TEST_IGNITION_VERSION);
    validate_config(Some(&cfg)).unwrap();
}

#[test]
fn validate_config_should_accept_a_valid_ignition_config_with_files() {
    let mut cfg = new_empty_ignition(TEST_IGNITION_VERSION);
    files_mut(&mut cfg).push(file("/etc/test-file", TEST_DATA_SOURCE));
    validate_config(Some(&cfg)).unwrap();
}

#[test]
fn validate_config_should_reject_ignition_config_with_duplicate_file_paths() {
    let mut cfg = new_empty_ignition(TEST_IGNITION_VERSION);
    let f = file("/etc/duplicate-file", "data:,content1");
    files_mut(&mut cfg).extend([f.clone(), f]);
    let err = validate_config(Some(&cfg)).unwrap_err();
    assert!(err.to_string().contains("duplicate"));
}

#[test]
fn validate_config_should_reject_ignition_config_with_duplicate_systemd_unit_names() {
    let mut cfg = new_empty_ignition(TEST_IGNITION_VERSION);
    let u = unit("duplicate.service");
    units_mut(&mut cfg).extend([u.clone(), u]);
    let err = validate_config(Some(&cfg)).unwrap_err();
    assert!(err.to_string().contains("duplicate"));
}

#[test]
fn validate_config_should_accept_ignition_config_with_multiple_unique_files() {
    let mut cfg = new_empty_ignition(TEST_IGNITION_VERSION);
    for i in 0..10u8 {
        files_mut(&mut cfg).push(file(
            &format!("/etc/file-{}", (b'a' + i) as char),
            "data:,content",
        ));
    }
    validate_config(Some(&cfg)).unwrap();
}

#[test]
fn validate_config_should_accept_ignition_config_with_older_3_x_versions() {
    // ValidateConfig normalizes any 3.x version internally for validation,
    // since the v3_4 Go types are forward-compatible with older specs.
    for version in ["3.0.0", "3.1.0", "3.2.0", "3.3.0"] {
        let cfg = new_empty_ignition(version);
        assert!(
            validate_config(Some(&cfg)).is_ok(),
            "version {version} should be accepted"
        );
    }
}

#[test]
fn validate_config_should_validate_configs_with_files_and_units_across_ignition_versions_used_by_hypershift()
 {
    // Hypershift may produce ignition configs with spec versions 3.2.0, 3.3.0, or 3.4.0.
    for version in ["3.2.0", "3.3.0", "3.4.0"] {
        // By("validating a config with files and units at version " + version)
        let mut cfg = new_empty_ignition(version);
        files_mut(&mut cfg).push(file("/etc/test-file", TEST_DATA_SOURCE));
        units_mut(&mut cfg).push(unit("test.service"));
        assert!(
            validate_config(Some(&cfg)).is_ok(),
            "version {version} with files and units should be accepted"
        );
    }
}

#[test]
fn validate_config_should_detect_duplicate_files_across_ignition_versions_used_by_hypershift() {
    for version in ["3.2.0", "3.3.0", "3.4.0"] {
        // By("detecting duplicate files at version " + version)
        let mut cfg = new_empty_ignition(version);
        let f = file("/etc/duplicate", "data:,content");
        files_mut(&mut cfg).extend([f.clone(), f]);
        let err = validate_config(Some(&cfg))
            .expect_err(&format!("version {version} should reject duplicate files"));
        assert!(err.to_string().contains("duplicate"));
    }
}

#[test]
fn validate_config_should_not_modify_the_caller_s_config_version() {
    let cfg = new_empty_ignition("3.2.0");
    validate_config(Some(&cfg)).unwrap();
    assert_eq!(cfg.ignition.version, "3.2.0");
}

#[test]
fn validate_config_should_reject_ignition_config_with_unsupported_version() {
    let cfg = Config::new(Ignition::new("2.0.0".into()));
    assert!(validate_config(Some(&cfg)).is_err());
}

#[test]
fn validate_config_should_reject_ignition_config_with_garbage_version() {
    let cfg = Config::new(Ignition::new("not-a-version".into()));
    assert!(validate_config(Some(&cfg)).is_err());
}

#[test]
fn validate_config_should_reject_nil_ignition_config() {
    assert!(validate_config(None).is_err());
}
