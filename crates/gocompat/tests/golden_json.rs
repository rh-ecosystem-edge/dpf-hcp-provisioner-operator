//! Golden tests: encoding/json (Marshal, MarshalIndent, Indent, Compact, HTMLEscape, Valid,
//! Unmarshal into any / map[string]any, string escaping of arbitrary bytes).
mod common;

use common::{dec, fixture, hex, same, unhex};
use gocompat::json;
use serde::Serialize;

#[test]
fn marshal_and_indent_match_go() {
    let f = fixture("json_yaml_marshal.json");
    let cases = f.as_array().unwrap();
    assert!(cases.len() > 2000);
    for c in cases {
        let v = dec(&c["input"]);
        let got = json::marshal(&v);
        assert_eq!(String::from_utf8_lossy(&got), c["json"].as_str().unwrap(), "marshal {c}");
        let ind = json::marshal_indent(&v, ">", "\t ").unwrap();
        assert_eq!(String::from_utf8_lossy(&ind), c["indent"].as_str().unwrap(), "indent {c}");
        // the typed path through serde must agree for the same value
        let typed = json::to_go_json(&v).unwrap();
        assert_eq!(String::from_utf8_lossy(&typed), c["json"].as_str().unwrap(), "to_go_json {c}");
    }
}

#[test]
fn string_escaping_matches_go() {
    let f = fixture("json_texts.json");
    for c in f["strings"].as_array().unwrap() {
        let input = unhex(c["in"].as_str().unwrap());
        let mut out = Vec::new();
        json::append_string(&mut out, &input, true);
        assert_eq!(String::from_utf8_lossy(&out), c["out"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn texts_match_go() {
    let f = fixture("json_texts.json");
    let texts = f["texts"].as_array().unwrap();
    assert!(texts.len() > 2000);
    for c in texts {
        let input = unhex(c["in"].as_str().unwrap());
        let ctx = String::from_utf8_lossy(&input).chars().take(200).collect::<String>();
        assert_eq!(json::valid(&input), c["valid"].as_bool().unwrap(), "valid {ctx:?}");
        match json::check_valid(&input) {
            Ok(()) => assert!(c["syn_err"].is_null(), "{ctx:?}"),
            Err(e) => {
                assert_eq!(Some(e.msg.as_str()), c["syn_err"].as_str(), "{ctx:?}");
                assert_eq!(e.offset, c["syn_off"].as_i64().unwrap(), "{ctx:?}");
            }
        }
        let light = input.len() > 15000;
        match json::unmarshal_any(&input) {
            Ok(v) => {
                assert!(c["any_err"].is_null(), "{ctx:?}: {}", c["any_err"]);
                if !light {
                    assert!(same(&v, &dec(&c["any"])), "{ctx:?}");
                }
            }
            Err(e) => assert_eq!(Some(e.to_string().as_str()), c["any_err"].as_str(), "{ctx:?}"),
        }
        match json::unmarshal_object(&input) {
            Ok(None) => assert!(c["map_nil"].as_bool().unwrap(), "{ctx:?}"),
            Ok(Some(m)) => {
                assert!(c["map_err"].is_null(), "{ctx:?}");
                if !light {
                    assert!(same(&serde_json::Value::Object(m), &dec(&c["map"])), "{ctx:?}");
                }
            }
            Err(e) => assert_eq!(Some(e.to_string().as_str()), c["map_err"].as_str(), "{ctx:?}"),
        }
        if light {
            continue;
        }
        assert_eq!(
            json::compact(&input, false).ok().map(|b| hex(&b)),
            c["compact"].as_str().map(str::to_string),
            "compact {ctx:?}"
        );
        assert_eq!(
            json::indent(&input, "", "  ").ok().map(|b| hex(&b)),
            c["indent"].as_str().map(str::to_string),
            "indent {ctx:?}"
        );
        assert_eq!(hex(&json::html_escape(&input)), c["html"].as_str().unwrap(), "html {ctx:?}");
    }
}

#[derive(Serialize)]
struct Typed {
    #[serde(rename = "zKey")]
    z: String,
    #[serde(rename = "aKey")]
    a: f64,
    #[serde(rename = "f32")]
    f: f32,
    #[serde(rename = "data", with = "gocompat::json::go_bytes")]
    data: Vec<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    opt: Option<i32>,
    map: std::collections::BTreeMap<String, i64>,
}

#[test]
fn typed_struct_keeps_field_order_and_go_escaping() {
    let t = Typed {
        z: "<a&b>\u{2028}\"\\\n\u{1}\u{7f}é".to_string(),
        a: 1e21,
        f: 0.1,
        data: b"hello".to_vec(),
        opt: None,
        map: [("b".to_string(), 2), ("a".to_string(), 1)].into_iter().collect(),
    };
    let got = String::from_utf8(json::to_go_json(&t).unwrap()).unwrap();
    // Go: json.Marshal(struct{ZKey string `json:"zKey"`; AKey float64 `json:"aKey"`; F32 float32 ...})
    assert_eq!(
        got,
        concat!("{\"zKey\":\"", "\\u003ca", "\\u0026b", "\\u003e", "\\u2028", "\\\"", "\\\\", "\\n", "\\u0001", "\u{7f}é\",\"aKey\":1e+21,\"f32\":0.1,\"data\":\"aGVsbG8=\",\"map\":{\"a\":1,\"b\":2}}")
    );
}

#[test]
fn unmarshal_error_messages() {
    assert_eq!(json::unmarshal_any(b"").unwrap_err().to_string(), "unexpected end of JSON input");
    assert_eq!(
        json::unmarshal_any(b"{\"a\" 1}").unwrap_err().to_string(),
        "invalid character '1' after object key"
    );
    assert_eq!(
        json::unmarshal_object(b"[1]").unwrap_err().to_string(),
        "json: cannot unmarshal array into Go value of type map[string]interface {}"
    );
}

