//! Port of internal/dpfdefaults/defaults_test.go

use ctrlrt::Ctx;
use dpfdefaults::*;

#[test]
fn test_doca_version() {
    struct Case {
        name: &'static str,
        major_minor: &'static str,
        want: &'static str,
        want_err: bool,
    }
    let tests = [
        Case {
            name: "DPF 26.4 maps to DOCA 3.4",
            major_minor: "26.4",
            want: "3.4",
            want_err: false,
        },
        Case {
            name: "unknown DPF minor falls back to default row",
            major_minor: "26.12",
            want: "3.4",
            want_err: false,
        },
        Case {
            name: "empty version errors",
            major_minor: "",
            want: "",
            want_err: true,
        },
    ];
    for tt in tests {
        let got = doca_version(&Ctx::background(), tt.major_minor);
        if tt.want_err {
            assert!(got.is_err(), "{}: expected error, got {:?}", tt.name, got);
            continue;
        }
        let got = got.unwrap_or_else(|e| panic!("{}: unexpected error: {e}", tt.name));
        assert_eq!(
            got, tt.want,
            "{}: DocaVersion({:?})",
            tt.name, tt.major_minor
        );
    }
}

#[test]
fn test_template_values() {
    let ctx = Ctx::background();
    let known = template_values(&ctx, "26.4").expect("unexpected error");
    assert_eq!(
        known.ovn.chart_version, "v26.4.1-ocp-release-v4.22",
        "OVN chart"
    );
    assert_eq!(known.dts.chart_version, "1.25.5", "DTS chart");
    assert!(
        known.hbn.chart_version == "3.4.0" && known.hbn.image_tag == "3.4.0-doca3.4.0",
        "HBN = {:?}",
        known.hbn
    );

    let fallback = template_values(&ctx, "26.12").expect("unexpected error");
    assert_eq!(fallback, known, "fallback should equal the 26.4 row");

    assert!(
        template_values(&ctx, "").is_err(),
        "expected error for empty version"
    );
}

#[test]
fn test_template_values_copy() {
    let ctx = Ctx::background();
    let mut first = template_values(&ctx, "26.4").expect("unexpected error");
    first.ovn.chart_version = "mutated".into();
    first.image_pull_secret = "secret".into();

    let second = template_values(&ctx, "26.4").expect("unexpected error");
    assert!(
        second.ovn.chart_version != "mutated" && second.image_pull_secret.is_empty(),
        "lookup returned shared state: {second:?}"
    );
}

#[test]
fn test_supported_dpf_versions() {
    assert_eq!(supported_dpf_versions(), vec!["26.4".to_string()]);
}

#[test]
fn test_error_messages_match_go() {
    let err = doca_version(&Ctx::background(), "").unwrap_err();
    assert_eq!(err.to_string(), r#"unsupported DPF version """#);
}
