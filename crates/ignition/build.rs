//! Generates the embedded directory listings of `internal/ignition/resources/*/{files,systemd}`
//! (Go `//go:embed files/*` / `//go:embed systemd/*`), sorted by name like `fs.ReadDir`.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../../internal/ignition/resources");
    let mut out = String::new();
    for provider in ["common", "live", "target"] {
        for dir in ["files", "systemd"] {
            let d = root.join(provider).join(dir);
            println!("cargo:rerun-if-changed={}", d.display());
            let mut names: Vec<(String, PathBuf)> = fs::read_dir(&d)
                .unwrap_or_else(|e| panic!("read_dir {}: {e}", d.display()))
                .map(|e| e.expect("dir entry"))
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                .collect();
            // fs.ReadDir sorts by name (bytewise)
            names.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            let _ = writeln!(
                out,
                "pub static {}_{}: crate::content::EmbeddedDir = crate::content::EmbeddedDir {{ entries: &[",
                provider.to_uppercase(),
                dir.to_uppercase()
            );
            for (name, path) in names {
                println!("cargo:rerun-if-changed={}", path.display());
                let abs = canonical(&path);
                let _ = writeln!(out, "    ({name:?}, include_bytes!({abs:?})),");
            }
            out.push_str("] };\n");
        }
    }
    let dest = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("embedded.rs");
    fs::write(dest, out).expect("write embedded.rs");
}

fn canonical(p: &Path) -> String {
    fs::canonicalize(p)
        .unwrap_or_else(|e| panic!("canonicalize {}: {e}", p.display()))
        .to_string_lossy()
        .into_owned()
}
