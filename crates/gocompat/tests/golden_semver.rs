//! Golden tests: github.com/coreos/go-semver and github.com/blang/semver/v4.
mod common;

use common::fixture;
use gocompat::semver::{blang, coreos};

#[test]
fn parse_matches_go() {
    let f = fixture("semver.json");
    for c in f["parse"].as_array().unwrap() {
        let input = c["in"].as_str().unwrap();
        match coreos::Version::new(input) {
            Ok(v) => {
                assert_eq!(Some(v.to_string().as_str()), c["coreos"].as_str(), "{c}");
                assert_eq!(v.major, c["coreos_major"].as_i64().unwrap(), "{c}");
            }
            Err(e) => assert_eq!(
                Some(e.to_string().as_str()),
                c["coreos_err"].as_str(),
                "{c}"
            ),
        }
        match blang::parse(input) {
            Ok(v) => {
                assert_eq!(Some(v.to_string().as_str()), c["blang"].as_str(), "{c}");
                assert_eq!(
                    v.finalize_version(),
                    c["blang_final"].as_str().unwrap(),
                    "{c}"
                );
            }
            Err(e) => assert_eq!(Some(e.to_string().as_str()), c["blang_err"].as_str(), "{c}"),
        }
        match blang::parse_tolerant(input) {
            Ok(v) => assert_eq!(Some(v.to_string().as_str()), c["tolerant"].as_str(), "{c}"),
            Err(e) => assert_eq!(
                Some(e.to_string().as_str()),
                c["tolerant_err"].as_str(),
                "{c}"
            ),
        }
    }
}

#[test]
fn compare_matches_go() {
    let f = fixture("semver.json");
    for c in f["compare"].as_array().unwrap() {
        let (a, b) = (c["a"].as_str().unwrap(), c["b"].as_str().unwrap());
        if let Some(want) = c["coreos"].as_i64() {
            let va = coreos::Version::new(a).unwrap();
            let vb = coreos::Version::new(b).unwrap();
            assert_eq!(va.compare(&vb) as i64, want, "{c}");
        }
        if let Some(want) = c["blang"].as_i64() {
            let va = blang::parse_tolerant(a).unwrap();
            let vb = blang::parse_tolerant(b).unwrap();
            assert_eq!(va.compare(&vb) as i64, want, "{c}");
        }
    }
}

#[test]
fn operator_usage() {
    // internal/controller/dpuservicetemplate: ParseTolerant + Pre=nil + GTE(MustParse("4.22.2"))
    let min = blang::must_parse("4.22.2");
    let mut v = blang::parse_tolerant("4.22.2-rc.1").unwrap();
    v.pre.clear();
    assert!(v.gte(&min));
    // internal/ignition/validate.go
    let e = coreos::Version::new("3.x.0").unwrap_err();
    assert_eq!(
        e.to_string(),
        "strconv.ParseInt: parsing \"x\": invalid syntax"
    );
}
