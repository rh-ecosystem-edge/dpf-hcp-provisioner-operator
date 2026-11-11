//! Port of internal/common/keychain_test.go

use common::new_keychain_from_docker_config;
use registry::authn::Resource;
use registry::name;

fn parse_auth_target(r: &str) -> Box<dyn Resource> {
    if !r.contains('/') {
        Box::new(name::new_registry(r, &[]).expect("parse registry"))
    } else {
        Box::new(name::new_repository(r, &[]).expect("parse repository"))
    }
}

#[test]
fn test_keychain_prefers_repository_scoped_auth() {
    let kc = new_keychain_from_docker_config(
        br#"{
		"auths": {
			"quay.io": {"username": "generic", "password": "generic-pass"},
			"quay.io/edge-infrastructure/bluefield-ocp": {"username": "repo", "password": "repo-pass"}
		}
	}"#,
    )
    .expect("NewKeychainFromDockerConfig");
    let tests = [
        (
            "repository-scoped entry wins for that repository",
            "quay.io/edge-infrastructure/bluefield-ocp",
            "repo",
        ),
        (
            "registry entry is used for the registry itself",
            "quay.io",
            "generic",
        ),
    ];
    for (name, target, want) in tests {
        let target = parse_auth_target(target);
        let auth = kc.resolve(target.as_ref()).expect("Resolve");
        let cfg = auth.authorization().expect("Authorization");
        assert_eq!(cfg.username, want, "{name}");
    }
}

#[test]
fn test_keychain_falls_back_to_registry_auth() {
    let kc = new_keychain_from_docker_config(
        br#"{"auths": {"quay.io": {"username": "generic", "password": "generic-pass"}}}"#,
    )
    .expect("NewKeychainFromDockerConfig");
    let repo = name::new_repository("quay.io/openshift-release-dev/ocp-release", &[]).unwrap();
    let cfg = kc.resolve(&repo).unwrap().authorization().unwrap();
    assert_eq!(cfg.username, "generic");
}

#[test]
fn test_keychain_anonymous_when_no_match() {
    let kc = new_keychain_from_docker_config(
        br#"{"auths": {"registry.redhat.io": {"username": "rh", "password": "rh-pass"}}}"#,
    )
    .expect("NewKeychainFromDockerConfig");
    let repo = name::new_repository("quay.io/edge-infrastructure/bluefield-ocp", &[]).unwrap();
    let cfg = kc.resolve(&repo).unwrap().authorization().unwrap();
    assert!(
        cfg.username.is_empty() && cfg.password.is_empty() && cfg.auth.is_empty(),
        "expected anonymous auth, got username {:?}",
        cfg.username
    );
}

#[test]
fn test_keychain_error_messages_match_go() {
    // Go: json.Unmarshal([]byte("not json")) → "invalid character 'o' in literal null (expecting 'u')"
    let e = new_keychain_from_docker_config(b"xyz").err().unwrap();
    assert_eq!(
        e.to_string(),
        "failed to unmarshal docker config: invalid character 'x' looking for beginning of value"
    );
    let e = new_keychain_from_docker_config(b"").err().unwrap();
    assert_eq!(
        e.to_string(),
        "failed to unmarshal docker config: unexpected end of JSON input"
    );
    let e = new_keychain_from_docker_config(br#"{"auths": []}"#)
        .err()
        .unwrap();
    assert_eq!(
        e.to_string(),
        "failed to unmarshal docker config: json: cannot unmarshal array into Go struct field dockerConfig.auths of type map[string]authn.AuthConfig"
    );
    let e = new_keychain_from_docker_config(b"[]").err().unwrap();
    assert_eq!(
        e.to_string(),
        "failed to unmarshal docker config: json: cannot unmarshal array into Go value of type common.dockerConfig"
    );
}
