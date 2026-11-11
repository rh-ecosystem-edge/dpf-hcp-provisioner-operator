# gocompat — test mapping

`gocompat` ports Go standard-library / vendored-library behaviour, so there are no operator Go
tests to mirror. Coverage is provided by golden differential tests: the Go program
`hack/golden/gocompat` runs fixed-seed random corpora and hand-picked edge cases through the
real Go implementations and records exact outputs in `tests/fixtures/*.json`
(`hack/golden/gocompat-tables` generates `src/unicode_tables.rs` from Go's Unicode tables).

| Go reference | Fixture | Rust test |
|---|---|---|
| `strconv.Quote` | `strconv.json` `quote` | `golden_strconv::quote_matches_go` |
| `strconv.ParseInt` / `ParseUint` / `Atoi` | `strconv.json` `ints` | `golden_strconv::parse_ints_match_go` |
| `strconv.FormatFloat` | `strconv.json` `format_float` | `golden_strconv::format_float_matches_go` |
| `strconv.ParseFloat` | `strconv.json` `parse_float` | `golden_strconv::parse_float_matches_go` |
| `json.Marshal` / `MarshalIndent` (`any`) | `json_yaml_marshal.json` | `golden_json::marshal_and_indent_match_go` |
| `json.Marshal` string encoding (arbitrary bytes) | `json_texts.json` `strings` | `golden_json::string_escaping_matches_go` |
| `json.Valid` / `SyntaxError` / `Unmarshal` (`any`, `map[string]any`) / `Compact` / `Indent` / `HTMLEscape` | `json_texts.json` `texts` | `golden_json::texts_match_go` |
| `json.Marshal` of a typed struct | inline | `golden_json::typed_struct_keeps_field_order_and_go_escaping` |
| `sigs.k8s.io/yaml.Marshal` | `json_yaml_marshal.json` | `golden_yaml::marshal_matches_go`, `marshal_typed_matches_marshal` |
| `sigs.k8s.io/yaml.Unmarshal` (`any`) of Marshal output | `json_yaml_marshal.json` `back` | `golden_yaml::unmarshal_of_marshal_output_matches_go` |
| `yaml.JSONToYAML` / `YAMLToJSON` | `json_to_yaml.json` | `golden_yaml::json_to_yaml_matches_go` |
| `yaml.YAMLToJSON` / `Unmarshal` of hand-written YAML | `yaml_to_json.json` | `golden_yaml::yaml_to_json_matches_go` |
| yaml.v2 depth limit | `yaml_deep.json` | `golden_yaml::depth_limit_matches_go` |
| `gzip.NewWriterLevel` + `Write`/`Flush`/`Close` (all levels) | `gzip.json` `compress` | `golden_gzip::compress_*_matches_go` |
| `gzip.NewWriterLevel` invalid level | `gzip.json` `bad_levels` | `golden_gzip::invalid_levels` |
| `gzip.NewReader` + read to EOF (valid, multistream, corrupted) | `gzip.json` `gunzip` | `golden_gzip::gunzip_matches_go` |
| `url.QueryEscape` / `PathEscape` / `QueryUnescape` / `PathUnescape` | `url_base64.json` `escape` | `golden_url_base64::url_escaping_matches_go` |
| `base64.StdEncoding` encode / `DecodeString` | `url_base64.json` `base64` | `golden_url_base64::base64_matches_go` |
| coreos `semver.NewVersion` / blang `Parse`, `ParseTolerant`, `FinalizeVersion` | `semver.json` `parse` | `golden_semver::parse_matches_go` |
| coreos `Version.Compare` / blang `Version.Compare` | `semver.json` `compare` | `golden_semver::compare_matches_go` |
