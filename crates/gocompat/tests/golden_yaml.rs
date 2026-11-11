//! Golden tests: sigs.k8s.io/yaml (Marshal, JSONToYAML, YAMLToJSON, Unmarshal into any).
mod common;

use common::{dec, fixture, sha256_hex, unhex};
use gocompat::{json, yaml};
use serde_json::Value;

#[test]
fn marshal_matches_go() {
    let f = fixture("json_yaml_marshal.json");
    let cases = f.as_array().unwrap();
    let mut ok = 0;
    let mut failures = Vec::new();
    for c in cases {
        let v = dec(&c["input"]);
        let got = yaml::marshal(&v)
            .map(|y| String::from_utf8_lossy(&y).into_owned())
            .map_err(|e| e.to_string());
        let want = match c["yaml"].as_str() {
            Some(y) => Ok(y.to_string()),
            None => Err(c["yaml_err"].as_str().unwrap().to_string()),
        };
        if got == want {
            ok += 1;
        } else {
            failures.push(format!(
                "input={}\n  got={got:?}\n want={want:?}",
                c["json"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(ok > 2000);
}

#[test]
fn marshal_typed_matches_marshal() {
    let f = fixture("json_yaml_marshal.json");
    for c in f.as_array().unwrap().iter().take(500) {
        let v = dec(&c["input"]);
        let a = yaml::marshal(&v).map_err(|e| e.to_string());
        let b = yaml::marshal_typed(&v).map_err(|e| e.to_string());
        assert_eq!(a, b, "{c}");
    }
}

#[test]
fn unmarshal_of_marshal_output_matches_go() {
    let f = fixture("json_yaml_marshal.json");
    let mut checked = 0;
    for c in f.as_array().unwrap() {
        let Some(y) = c["yaml"].as_str() else {
            continue;
        };
        match yaml::unmarshal_any(y.as_bytes()) {
            Ok(v) => {
                let want = c["back"].as_str().unwrap_or_else(|| {
                    panic!("go failed ({}) but rust succeeded: {y:?}", c["back_err"])
                });
                assert_eq!(String::from_utf8_lossy(&json::marshal(&v)), want, "{y:?}");
                checked += 1;
            }
            Err(e) => assert!(
                c["back"].is_null(),
                "rust failed ({e}) but go succeeded: {y:?}"
            ),
        }
    }
    assert!(checked > 2000);
}

#[test]
fn json_to_yaml_matches_go() {
    let f = fixture("json_to_yaml.json");
    for c in f.as_array().unwrap() {
        let input = unhex(c["in"].as_str().unwrap());
        match yaml::json_to_yaml(&input) {
            Ok(y) => {
                assert_eq!(
                    Some(String::from_utf8_lossy(&y).as_ref()),
                    c["out"].as_str(),
                    "{c}"
                );
                if let Some(back) = c["y2j"].as_str() {
                    let got = yaml::yaml_to_json(&y).unwrap_or_else(|e| panic!("{e}: {c}"));
                    assert_eq!(String::from_utf8_lossy(&got), back, "y2j {c}");
                }
            }
            Err(e) => assert_eq!(Some(e.to_string().as_str()), c["err"].as_str(), "{c}"),
        }
    }
}

#[test]
fn yaml_to_json_matches_go() {
    let f = fixture("yaml_to_json.json");
    for c in f.as_array().unwrap() {
        let input = unhex(c["in"].as_str().unwrap());
        let ctx = String::from_utf8_lossy(&input).into_owned();
        // Supported subset: anchors/aliases/tags are rejected by the Rust port.
        let unsupported = ctx.contains('&') || ctx.contains("!!");
        match yaml::yaml_to_json(&input) {
            Ok(j) => assert_eq!(
                Some(String::from_utf8_lossy(&j).as_ref()),
                c["out"].as_str(),
                "{ctx:?}"
            ),
            Err(e) => {
                if c["out"].is_string() && !unsupported {
                    panic!("rust failed ({e}) but go succeeded: {ctx:?}");
                }
                if let Some(go_err) = c["err"].as_str()
                    && go_err.starts_with("json: unsupported value")
                {
                    assert_eq!(e.to_string(), go_err, "{ctx:?}");
                }
            }
        }
        match yaml::unmarshal_any(&input) {
            Ok(v) => assert_eq!(
                Some(String::from_utf8_lossy(&json::marshal(&v)).as_ref()),
                c["any"].as_str(),
                "{ctx:?}"
            ),
            Err(e) => {
                if c["any"].is_string() && !unsupported {
                    panic!("rust failed ({e}) but go succeeded: {ctx:?}");
                }
            }
        }
    }
}

fn nested(depth: usize, kind: &str) -> Value {
    let mut v = Value::from(1i64);
    for _ in 0..depth {
        v = if kind == "array" {
            Value::Array(vec![v])
        } else {
            let mut m = serde_json::Map::new();
            m.insert("a".to_string(), v);
            Value::Object(m)
        };
    }
    v
}

#[test]
fn depth_limit_matches_go() {
    common::big_stack(|| {
        let f = fixture("yaml_deep.json");
        for c in f.as_array().unwrap() {
            let v = nested(
                c["depth"].as_u64().unwrap() as usize,
                c["kind"].as_str().unwrap(),
            );
            match yaml::marshal(&v) {
                Ok(y) => {
                    assert_eq!(y.len() as u64, c["yaml_len"].as_u64().unwrap(), "{c}");
                    assert_eq!(sha256_hex(&y), c["yaml_sha256"].as_str().unwrap(), "{c}");
                }
                Err(e) => assert_eq!(Some(e.to_string().as_str()), c["yaml_err"].as_str(), "{c}"),
            }
            std::mem::forget(v); // avoid recursive drop of a 10000-deep value
        }
    });
}

#[test]
fn dpu_flavor_like_document() {
    // shape of internal/ignition/dpuflavor.go output (sigs.k8s.io/yaml.Marshal of a CR)
    let v: Value = serde_json::from_str(
        r##"{"apiVersion":"provisioning.dpu.nvidia.com/v1alpha1","kind":"DPUFlavor","metadata":{"name":"my-flavor","namespace":"ns","creationTimestamp":null},"spec":{"ovs":{"rawConfigScript":"#!/bin/bash\nset -e\n"},"grub":{"kernelParameters":["console=hvc0","hugepages=3072"]}}}"##,
    )
    .unwrap();
    let y = String::from_utf8(yaml::marshal(&v).unwrap()).unwrap();
    assert_eq!(
        y,
        "apiVersion: provisioning.dpu.nvidia.com/v1alpha1\nkind: DPUFlavor\nmetadata:\n  creationTimestamp: null\n  name: my-flavor\n  namespace: ns\nspec:\n  grub:\n    kernelParameters:\n    - console=hvc0\n    - hugepages=3072\n  ovs:\n    rawConfigScript: |\n      #!/bin/bash\n      set -e\n"
    );
}
