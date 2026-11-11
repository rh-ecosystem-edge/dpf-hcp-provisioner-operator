//! Golden tests: net/url escaping and encoding/base64 StdEncoding.
mod common;

use common::{fixture, hex, unhex};
use gocompat::{base64, url};

#[test]
fn url_escaping_matches_go() {
    let f = fixture("url_base64.json");
    for c in f["escape"].as_array().unwrap() {
        let input = unhex(c["in"].as_str().unwrap());
        assert_eq!(url::query_escape(&input), c["query"].as_str().unwrap(), "{c}");
        assert_eq!(url::path_escape(&input), c["path"].as_str().unwrap(), "{c}");
        match url::query_unescape(&input) {
            Ok(v) => assert_eq!(Some(hex(&v).as_str()), c["qu"].as_str(), "{c}"),
            Err(e) => assert_eq!(Some(e.to_string().as_str()), c["qu_err"].as_str(), "{c}"),
        }
        match url::path_unescape(&input) {
            Ok(v) => assert_eq!(Some(hex(&v).as_str()), c["pu"].as_str(), "{c}"),
            Err(e) => assert_eq!(Some(e.to_string().as_str()), c["pu_err"].as_str(), "{c}"),
        }
    }
}

#[test]
fn base64_matches_go() {
    let f = fixture("url_base64.json");
    for c in f["base64"].as_array().unwrap() {
        let input = unhex(c["in"].as_str().unwrap());
        assert_eq!(base64::encode_to_string(&input), c["enc"].as_str().unwrap(), "{c}");
        match base64::decode_string(&input) {
            Ok(v) => assert_eq!(Some(hex(&v).as_str()), c["dec"].as_str(), "{c}"),
            Err(e) => assert_eq!(Some(e.to_string().as_str()), c["err"].as_str(), "{c}"),
        }
    }
}
