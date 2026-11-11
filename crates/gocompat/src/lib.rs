//! `gocompat` — byte-exact Rust ports of the Go standard-library (and library) behaviour
//! whose output the operator depends on.
//!
//! | Module | Go reference |
//! |---|---|
//! | [`json`] | `encoding/json` (`Marshal`, `MarshalIndent`, `Indent`, `Valid`, `Unmarshal` into `any`) |
//! | [`yaml`] | `sigs.k8s.io/yaml` v1.6.0 (`Marshal`, `JSONToYAML`, `Unmarshal`/`YAMLToJSON`) over `go.yaml.in/yaml/v2` v2.4.3 |
//! | [`gzip`], [`flate`] | `compress/gzip`, `compress/flate` (all levels, byte-identical output; reader) |
//! | [`url`] | `net/url` `QueryEscape`/`QueryUnescape`/`PathEscape`/`PathUnescape` |
//! | [`base64`] | `encoding/base64` `StdEncoding` |
//! | [`strconv`] | `strconv` `Quote`, `ParseInt`, `ParseUint`, `Atoi`, `ParseFloat`, `FormatFloat` |
//! | [`semver`] | `github.com/coreos/go-semver/semver` and `github.com/blang/semver/v4` |
//!
//! Go strings are byte strings; APIs that accept Go strings take `&[u8]`/`impl AsRef<[u8]>`
//! where invalid UTF-8 can matter.

pub mod base64;
pub mod crc32;
pub mod flate;
pub mod gzip;
pub mod json;
pub mod semver;
pub mod strconv;
pub mod unicode;
mod unicode_tables;
pub mod url;
pub mod utf8;
pub mod yaml;
