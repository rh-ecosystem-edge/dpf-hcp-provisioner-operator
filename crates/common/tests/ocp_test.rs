//! Port of internal/common/ocp_test.go and dpfversion_test.go

use common::{dpf_major_minor_from_version, extract_ocp_version};

#[test]
fn test_extract_ocp_version() {
    // (name, image, want, wantErr, errMatch)
    let tests: &[(&str, &str, &str, bool, &str)] = &[
        (
            "strips -multi suffix",
            "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
            "4.19.0-ec.5",
            false,
            "",
        ),
        (
            "strips -x86_64 suffix",
            "quay.io/openshift-release-dev/ocp-release:4.18.2-rc.1-x86_64",
            "4.18.2-rc.1",
            false,
            "",
        ),
        (
            "strips -amd64 suffix",
            "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-amd64",
            "4.19.0-ec.5",
            false,
            "",
        ),
        (
            "strips -arm64 suffix",
            "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-arm64",
            "4.19.0-ec.5",
            false,
            "",
        ),
        (
            "strips -ppc64le suffix",
            "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-ppc64le",
            "4.19.0-ec.5",
            false,
            "",
        ),
        (
            "strips -s390x suffix",
            "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-s390x",
            "4.19.0-ec.5",
            false,
            "",
        ),
        (
            "no suffix",
            "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5",
            "4.19.0-ec.5",
            false,
            "",
        ),
        (
            "no tag separator",
            "quay.io/openshift-release-dev/ocp-release",
            "",
            true,
            "missing tag separator",
        ),
        (
            "empty tag",
            "quay.io/openshift-release-dev/ocp-release:",
            "",
            true,
            "empty tag",
        ),
    ];
    for (name, image, want, want_err, err_match) in tests {
        let got = extract_ocp_version(image);
        if *want_err {
            match got {
                Err(e) => assert!(
                    e.to_string().contains(err_match),
                    "{name}: error {e} does not contain {err_match:?}"
                ),
                Ok(v) => panic!("{name}: expected error containing {err_match:?}, got {v:?}"),
            }
            continue;
        }
        assert_eq!(
            got.unwrap_or_else(|e| panic!("{name}: unexpected error: {e}")),
            *want,
            "{name}"
        );
    }
}

#[test]
fn test_dpf_major_minor_from_version() {
    // (name, version, want, wantErr)
    let tests: &[(&str, &str, &str, bool)] = &[
        ("strips v prefix and patch", "v26.4.1", "26.4", false),
        ("no v prefix", "26.4.0", "26.4", false),
        ("strips git suffix", "26.4.0-f314aa17", "26.4", false),
        ("prerelease suffix", "v26.4.0-rc.1", "26.4", false),
        ("major only", "v26", "", true),
        ("empty", "", "", true),
    ];
    for (name, version, want, want_err) in tests {
        let got = dpf_major_minor_from_version(version);
        if *want_err {
            assert!(got.is_err(), "{name}: expected error, got {got:?}");
            continue;
        }
        assert_eq!(
            got.unwrap_or_else(|e| panic!("{name}: unexpected error: {e}")),
            *want,
            "{name}"
        );
    }
}
