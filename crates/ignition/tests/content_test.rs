//! Port of `internal/ignition/content/content_test.go` ("Content Suite").

use std::borrow::Cow;

use ignition::content::{
    ContentProvider, ContentSource, EmbeddedProvider, FileDefinition, LinkDefinition,
    SystemdUnitDefinition, add_content, add_files, add_kernel_args, add_systemd_units,
};
use ignition::{files, new_empty_ignition, units};

/// mockProvider implements ContentProvider for testing.
#[derive(Default)]
struct MockProvider {
    files: Vec<FileDefinition>,
    links: Vec<LinkDefinition>,
    systemd_units: Vec<SystemdUnitDefinition>,
    systemd_err: Option<String>,
}

impl ContentProvider for MockProvider {
    fn get_files(&self) -> Vec<FileDefinition> {
        self.files.clone()
    }
    fn get_links(&self) -> Vec<LinkDefinition> {
        self.links.clone()
    }
    fn get_systemd_units(&self) -> anyhow::Result<Vec<SystemdUnitDefinition>> {
        match &self.systemd_err {
            Some(e) => Err(anyhow::anyhow!("{e}")),
            None => Ok(self.systemd_units.clone()),
        }
    }
}

fn fd(path: &str, mode: i64, src: ContentSource) -> FileDefinition {
    FileDefinition {
        path: path.into(),
        mode,
        content_source: src,
    }
}

fn data(s: &str) -> ContentSource {
    ContentSource::DataUri(s.into())
}

fn unit(name: &str, contents: &str) -> SystemdUnitDefinition {
    SystemdUnitDefinition {
        name: name.into(),
        contents: contents.as_bytes().to_vec(),
    }
}

#[test]
fn add_files_should_add_files_with_string_content_source_data_uri() {
    let mut ign = new_empty_ignition("3.4.0");
    let provider = MockProvider {
        files: vec![fd("/etc/test.conf", 0o644, data("data:,test-content"))],
        ..Default::default()
    };
    add_files(&mut ign, &provider).unwrap();
    assert_eq!(files(&ign).len(), 1);
    let file = &files(&ign)[0];
    assert_eq!(file.path, "/etc/test.conf");
    assert_eq!(file.mode.unwrap(), 0o644);
    assert_eq!(ignition::file_source(file).unwrap(), "data:,test-content");
    assert!(file.overwrite.unwrap());
}

#[test]
fn add_files_should_add_files_with_byte_content_source_gzip_base64_encoded() {
    let mut ign = new_empty_ignition("3.4.0");
    let provider = MockProvider {
        files: vec![fd(
            "/usr/local/bin/script.sh",
            0o755,
            ContentSource::Bytes(Cow::Borrowed(b"#!/bin/bash\necho hello")),
        )],
        ..Default::default()
    };
    add_files(&mut ign, &provider).unwrap();
    assert_eq!(files(&ign).len(), 1);
    let file = &files(&ign)[0];
    assert_eq!(file.path, "/usr/local/bin/script.sh");
    assert_eq!(file.mode.unwrap(), 0o755);
    assert!(
        ignition::file_source(file)
            .unwrap()
            .starts_with("data:;base64,")
    );
    let compression = file
        .contents
        .as_ref()
        .and_then(|c| c.compression.as_deref());
    assert_eq!(compression.expect("compression"), "gzip");
}

#[test]
fn add_files_should_return_error_for_invalid_content_source_type() {
    let mut ign = new_empty_ignition("3.4.0");
    let provider = MockProvider {
        files: vec![fd("/etc/bad", 0o644, ContentSource::Other("12345".into()))],
        ..Default::default()
    };
    let err = add_files(&mut ign, &provider).unwrap_err();
    assert!(err.to_string().contains("invalid content source type"));
}

#[test]
fn add_files_should_set_correct_file_metadata() {
    let mut ign = new_empty_ignition("3.4.0");
    let provider = MockProvider {
        files: vec![
            fd("/etc/a.conf", 0o600, data("data:,a")),
            fd("/etc/b.conf", 0o755, data("data:,b")),
        ],
        ..Default::default()
    };
    add_files(&mut ign, &provider).unwrap();
    assert_eq!(files(&ign).len(), 2);
    assert_eq!(files(&ign)[0].path, "/etc/a.conf");
    assert_eq!(files(&ign)[0].mode.unwrap(), 0o600);
    assert_eq!(files(&ign)[1].path, "/etc/b.conf");
    assert_eq!(files(&ign)[1].mode.unwrap(), 0o755);
}

#[test]
fn add_systemd_units_should_add_systemd_units_to_ignition() {
    let mut ign = new_empty_ignition("3.4.0");
    let provider = MockProvider {
        systemd_units: vec![unit(
            "test.service",
            "[Unit]\nDescription=Test\n[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target",
        )],
        ..Default::default()
    };
    add_systemd_units(&mut ign, &provider).unwrap();
    assert_eq!(units(&ign).len(), 1);
    let u = &units(&ign)[0];
    assert_eq!(u.name, "test.service");
    assert!(u.contents.as_deref().unwrap().contains("Description=Test"));
}

#[test]
fn add_systemd_units_should_set_all_units_as_enabled() {
    let mut ign = new_empty_ignition("3.4.0");
    let provider = MockProvider {
        systemd_units: vec![
            unit("a.service", "[Unit]\nDescription=A"),
            unit("b.service", "[Unit]\nDescription=B"),
        ],
        ..Default::default()
    };
    add_systemd_units(&mut ign, &provider).unwrap();
    for u in units(&ign) {
        assert!(u.enabled.unwrap());
    }
}

#[test]
fn add_systemd_units_should_handle_nil_systemd_fs_no_units() {
    let mut ign = new_empty_ignition("3.4.0");
    let provider = EmbeddedProvider {
        files: Vec::new(),
        systemd_fs: None,
        ..Default::default()
    };
    add_systemd_units(&mut ign, &provider).unwrap();
    assert!(units(&ign).is_empty());
}

#[test]
fn add_content_should_add_both_files_and_systemd_units() {
    let mut ign = new_empty_ignition("3.4.0");
    let provider = MockProvider {
        files: vec![fd("/etc/test.conf", 0o644, data("data:,content"))],
        systemd_units: vec![unit("test.service", "[Unit]\nDescription=Test")],
        ..Default::default()
    };
    add_content(&mut ign, &provider).unwrap();
    assert_eq!(files(&ign).len(), 1);
    assert_eq!(units(&ign).len(), 1);
}

#[test]
fn add_kernel_args_should_add_kernel_argument_template() {
    let mut ign = new_empty_ignition("3.4.0");
    add_kernel_args(&mut ign);
    let ka = ign
        .kernel_arguments
        .as_ref()
        .and_then(|k| k.should_exist.as_deref())
        .unwrap_or_default();
    assert_eq!(ka.len(), 1);
    assert_eq!(ka[0], "{{.KernelParameters}}");
}

/// Verify EmbeddedProvider and the mock satisfy ContentProvider (Go `var _ ContentProvider = ...`).
#[test]
fn content_provider_interface_assertions() {
    let _: &dyn ContentProvider = &EmbeddedProvider::default();
    let _: &dyn ContentProvider = &MockProvider::default();
}

/// Extra: errors from `GetSystemdUnits` are wrapped like Go.
#[test]
fn add_systemd_units_wraps_provider_errors() {
    let mut ign = new_empty_ignition("3.4.0");
    let provider = MockProvider {
        systemd_err: Some("boom".into()),
        ..Default::default()
    };
    let err = add_systemd_units(&mut ign, &provider).unwrap_err();
    assert_eq!(err.to_string(), "failed to get systemd units: boom");
}
