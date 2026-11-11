//! Golden tests: strconv (Quote, ParseInt/ParseUint/Atoi, FormatFloat, ParseFloat).
mod common;

use common::{fixture, unhex};
use gocompat::strconv;

#[test]
fn quote_matches_go() {
    let f = fixture("strconv.json");
    let cases = f["quote"].as_array().unwrap();
    for c in cases {
        let input = unhex(c["in"].as_str().unwrap());
        assert_eq!(
            strconv::quote(&input),
            c["out"].as_str().unwrap(),
            "quote({input:?})"
        );
    }
    assert!(cases.len() > 5000);
}

#[test]
fn parse_ints_match_go() {
    let f = fixture("strconv.json");
    for c in f["ints"].as_array().unwrap() {
        let s = unhex(c["s"].as_str().unwrap());
        let base = c["base"].as_i64().unwrap();
        let bits = c["bits"].as_i64().unwrap();
        let want_err = c["err"].as_str();
        let want_val = c["val"].as_str().unwrap();
        let (got_val, got_err) = match c["fn"].as_str().unwrap() {
            "ParseInt" => match strconv::parse_int(&s, base, bits) {
                Ok(v) => (v.to_string(), None),
                Err(e) => (String::new(), Some(e.to_string())),
            },
            "ParseUint" => match strconv::parse_uint(&s, base, bits) {
                Ok(v) => (v.to_string(), None),
                Err(e) => (String::new(), Some(e.to_string())),
            },
            "Atoi" => match strconv::atoi(&s) {
                Ok(v) => (v.to_string(), None),
                Err(e) => (String::new(), Some(e.to_string())),
            },
            other => panic!("{other}"),
        };
        assert_eq!(got_err.as_deref(), want_err, "{c}");
        if want_err.is_none() {
            assert_eq!(got_val, want_val, "{c}");
        }
    }
}

#[test]
fn format_float_matches_go() {
    let f = fixture("strconv.json");
    for c in f["format_float"].as_array().unwrap() {
        let bits = u64::from_str_radix(c["bits"].as_str().unwrap(), 16).unwrap();
        let fm = c["fmt"].as_str().unwrap().as_bytes()[0];
        let got = strconv::format_float(
            f64::from_bits(bits),
            fm,
            c["prec"].as_i64().unwrap(),
            c["bitsize"].as_i64().unwrap(),
        );
        assert_eq!(got, c["out"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn parse_float_matches_go() {
    let f = fixture("strconv.json");
    for c in f["parse_float"].as_array().unwrap() {
        let s = unhex(c["s"].as_str().unwrap());
        let got = strconv::parse_float(&s, c["bitsize"].as_i64().unwrap());
        match c["err"].as_str() {
            Some(e) => assert_eq!(
                got.map_err(|e| e.to_string()).err().as_deref(),
                Some(e),
                "{c}"
            ),
            None => {
                let want = u64::from_str_radix(c["bits"].as_str().unwrap(), 16).unwrap();
                let v = got.unwrap_or_else(|e| panic!("{c}: {e}"));
                if f64::from_bits(want).is_nan() {
                    assert!(v.is_nan(), "{c}");
                } else {
                    assert_eq!(v.to_bits(), want, "{c}");
                }
            }
        }
    }
}

#[test]
fn parse_permissions_examples() {
    // internal/ignition/configfiles.go: strconv.ParseInt(perms, 8, 32)
    assert_eq!(strconv::parse_int("0644", 8, 32).unwrap(), 0o644);
    assert_eq!(
        strconv::parse_int("9", 8, 32).unwrap_err().to_string(),
        "strconv.ParseInt: parsing \"9\": invalid syntax"
    );
    assert_eq!(
        strconv::parse_int("77777777777", 8, 32)
            .unwrap_err()
            .to_string(),
        "strconv.ParseInt: parsing \"77777777777\": value out of range"
    );
}
