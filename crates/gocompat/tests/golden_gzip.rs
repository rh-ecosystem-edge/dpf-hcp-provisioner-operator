//! Golden tests: compress/gzip + compress/flate (all levels, chunked writes, flushes,
//! decompression of valid and corrupted streams).
mod common;

use common::{fixture, hex, payload, sha256_hex, unhex};
use gocompat::gzip;

fn run(c: &serde_json::Value) {
    let data = payload(
        c["kind"].as_str().unwrap(),
        c["seed"].as_u64().unwrap(),
        c["size"].as_u64().unwrap() as usize,
    );
    let level = c["level"].as_i64().unwrap() as i32;
    let mut w = gzip::Writer::new(level).unwrap();
    let mut out = Vec::new();
    let mut pos = 0;
    if let Some(ops) = c["ops"].as_array() {
        for op in ops {
            match op["op"].as_str().unwrap() {
                "w" => {
                    let end = (pos + op["n"].as_u64().unwrap() as usize).min(data.len());
                    w.write(&data[pos..end]);
                    pos = end;
                }
                _ => w.flush(),
            }
            out.extend(w.take_output());
        }
    }
    w.write(&data[pos..]);
    out.extend(w.finish());
    let ctx = format!("kind={} seed={} size={} level={}", c["kind"], c["seed"], c["size"], level);
    if let Some(h) = c["hex"].as_str() {
        assert_eq!(hex(&out), h, "{ctx}");
    }
    assert_eq!(out.len() as u64, c["len"].as_u64().unwrap(), "{ctx}");
    assert_eq!(sha256_hex(&out), c["sha256"].as_str().unwrap(), "{ctx}");
    // and it must round-trip through the reader
    assert_eq!(gzip::decompress(&out).unwrap(), data, "{ctx}");
}

fn run_filtered(pred: impl Fn(&serde_json::Value) -> bool) {
    let f = fixture("gzip.json");
    let cases: Vec<_> = f["compress"].as_array().unwrap().iter().filter(|c| pred(c)).cloned().collect();
    assert!(!cases.is_empty());
    // spread over threads to keep the (unoptimized) test fast
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let chunks: Vec<Vec<serde_json::Value>> = cases.chunks(cases.len().div_ceil(n)).map(|c| c.to_vec()).collect();
    let handles: Vec<_> = chunks
        .into_iter()
        .map(|chunk| std::thread::spawn(move || chunk.iter().for_each(run)))
        .collect();
    for h in handles {
        h.join().expect("worker panicked");
    }
}

#[test]
fn compress_small_matches_go() {
    run_filtered(|c| c["size"].as_u64().unwrap() <= 4097 && c["ops"].is_null());
}

#[test]
fn compress_medium_matches_go() {
    run_filtered(|c| {
        let s = c["size"].as_u64().unwrap();
        s > 4097 && s <= 100_000 && c["ops"].is_null()
    });
}

#[test]
fn compress_large_matches_go() {
    run_filtered(|c| c["size"].as_u64().unwrap() > 100_000 && c["ops"].is_null());
}

#[test]
fn compress_chunked_and_flushed_matches_go() {
    run_filtered(|c| !c["ops"].is_null());
}

#[test]
fn invalid_levels() {
    let f = fixture("gzip.json");
    for c in f["bad_levels"].as_array().unwrap() {
        let err = gzip::Writer::new(c["level"].as_i64().unwrap() as i32).err().unwrap();
        assert_eq!(err.to_string(), c["err"].as_str().unwrap());
    }
}

#[test]
fn gunzip_matches_go() {
    let f = fixture("gzip.json");
    for c in f["gunzip"].as_array().unwrap() {
        let input = unhex(c["in"].as_str().unwrap());
        let ctx = &c["in"].as_str().unwrap()[..c["in"].as_str().unwrap().len().min(80)];
        match gzip::Reader::new(&input) {
            Err(e) => assert_eq!(Some(e.to_string().as_str()), c["new_err"].as_str(), "{ctx}"),
            Ok(r) => {
                assert!(c["new_err"].is_null(), "{ctx}");
                match r.read_all() {
                    Ok(out) => {
                        assert!(c["read_err"].is_null(), "{ctx}: go error {}", c["read_err"]);
                        assert_eq!(out.len() as u64, c["out_len"].as_u64().unwrap(), "{ctx}");
                        assert_eq!(sha256_hex(&out), c["out_sha256"].as_str().unwrap(), "{ctx}");
                    }
                    Err(e) => assert_eq!(Some(e.to_string().as_str()), c["read_err"].as_str(), "{ctx}"),
                }
            }
        }
    }
}

#[test]
fn flate_raw_roundtrip() {
    for level in [-2, -1, 0, 1, 5, 9] {
        let data = payload("mixed", 42, 50_000);
        let z = gocompat::flate::compress(&data, level).unwrap();
        assert_eq!(gocompat::flate::decompress(&z).unwrap(), data);
    }
}
