# ignition — test mapping

> **Golden fixtures are frozen.** The fixtures referenced below were generated from the former Go
> implementation by the generators under `hack/golden/`, which were removed together with the Go
> code (still in the git history, commit `fead352`). They can no longer be regenerated; see
> `docs/rust-port/CONVENTIONS.md` (rule 6).

Go tests of `internal/ignition`, `internal/ignition/content` → Rust tests of `crates/ignition`.
Every Ginkgo `It` is ported with the same assertions; the Rust name is the Go spec path in
snake_case (`Describe` + `It`).

Run: `CARGO_TARGET_DIR=target/p2-ign cargo test -p ignition`.


## ignition_test.go (Ignition Suite)

| Go (`internal/ignition/ignition_test.go`) | Rust (`tests/ignition_test.rs`) |
|---|---|
| NewEmptyIgnition / should create ignition with the specified version | `new_empty_ignition_should_create_ignition_with_the_specified_version` |
| NewEmptyIgnition / should default to IgnitionVersion when version is empty | `new_empty_ignition_should_default_to_ignition_version_when_version_is_empty` |
| MarshalJSON / should strip empty objects from JSON output | `marshal_json_should_strip_empty_objects_from_json_output` |
| MarshalJSON / should preserve non-empty data | `marshal_json_should_preserve_non_empty_data` |
| EncodeIgnition / DecodeIgnition / should round-trip encode and decode an ignition config | `encode_ignition_decode_ignition_should_round_trip_encode_and_decode_an_ignition_config` |
| EncodeIgnition / DecodeIgnition / should fail to decode invalid base64 | `encode_ignition_decode_ignition_should_fail_to_decode_invalid_base64` |
| EncodeIgnition / DecodeIgnition / should fail to decode invalid gzip | `encode_ignition_decode_ignition_should_fail_to_decode_invalid_gzip` |
| EncodeGzipFile / should return a gzip-compressed base64-encoded Resource | `encode_gzip_file_should_return_a_gzip_compressed_base64_encoded_resource` |
| EncodeGzipFile / should produce decompressible content that matches the input | `encode_gzip_file_should_produce_decompressible_content_that_matches_the_input` |
| ReplaceMachineOSURL / should replace the osImageURL in the machine config | `replace_machine_os_url_should_replace_the_os_image_url_in_the_machine_config` |
| ReplaceMachineOSURL / should return error when the file is not found | `replace_machine_os_url_should_return_error_when_the_file_is_not_found` |
| ReplaceMachineOSURL / should return error when the data URI format is unexpected | `replace_machine_os_url_should_return_error_when_the_data_uri_format_is_unexpected` |
| ReplaceMachineOSURL / should return error when data URI contains invalid JSON | `replace_machine_os_url_should_return_error_when_data_uri_contains_invalid_json` |
| AddFlavorOVSScript / should add the OVS script file to ignition | `add_flavor_ovs_script_should_add_the_ovs_script_file_to_ignition` |
| AddFlavorOVSScript / should base64-encode the script content | `add_flavor_ovs_script_should_base64_encode_the_script_content` |
| AddFlavorOVSScript / should handle an empty script | `add_flavor_ovs_script_should_handle_an_empty_script` |
| EnableMTU / should add NM connection files for p0, p1, pf0hpf, pf1hpf | `enable_mtu_should_add_nm_connection_files_for_p0_p1_pf0hpf_pf1hpf` |
| EnableMTU / should set correct MTU value in the generated config | `enable_mtu_should_set_correct_mtu_value_in_the_generated_config` |
| EnableMTU / should set file mode 0600 for NM connection files | `enable_mtu_should_set_file_mode_0600_for_nm_connection_files` |
| EnableMTU / should modify existing pf0vf0.nmconnection to add MTU | `enable_mtu_should_modify_existing_pf0vf0_nmconnection_to_add_mtu` |
| GzipIgnitionFiles / should compress data URI files | `gzip_ignition_files_should_compress_data_uri_files` |
| GzipIgnitionFiles / should compress base64 files | `gzip_ignition_files_should_compress_base64_files` |
| GzipIgnitionFiles / should skip already compressed files | `gzip_ignition_files_should_skip_already_compressed_files` |
| GzipIgnitionFiles / should skip files with nil or empty source | `gzip_ignition_files_should_skip_files_with_nil_or_empty_source` |
| GzipIgnitionFiles / should produce content that decompresses to the original | `gzip_ignition_files_should_produce_content_that_decompresses_to_the_original` |
| cleanDPUFlavor / should keep only TypeMeta, name, namespace, and spec | `clean_dpu_flavor_should_keep_only_type_meta_name_namespace_and_spec` |
| AddDPUFlavorYAML / should add a YAML file at the correct path | `add_dpu_flavor_yaml_should_add_a_yaml_file_at_the_correct_path` |
| AddDPUFlavorYAML / should produce valid YAML content with only cleaned fields | `add_dpu_flavor_yaml_should_produce_valid_yaml_content_with_only_cleaned_fields` |
| AddDPUFlavorJSON / should add a JSON file at the correct path | `add_dpu_flavor_json_should_add_a_json_file_at_the_correct_path` |
| AddDPUFlavorJSON / should produce valid JSON content with only cleaned fields | `add_dpu_flavor_json_should_produce_valid_json_content_with_only_cleaned_fields` |
| MergeFlavorConfigFiles / should be a no-op when there are no config files | `merge_flavor_config_files_should_be_a_no_op_when_there_are_no_config_files` |
| MergeFlavorConfigFiles / should override a base file at a conflicting path | `merge_flavor_config_files_should_override_a_base_file_at_a_conflicting_path` |
| MergeFlavorConfigFiles / should append to a base file at a conflicting path | `merge_flavor_config_files_should_append_to_a_base_file_at_a_conflicting_path` |
| MergeFlavorConfigFiles / should add a new file when path does not conflict | `merge_flavor_config_files_should_add_a_new_file_when_path_does_not_conflict` |
| MergeFlavorConfigFiles / should handle a mix of override, append, and new files | `merge_flavor_config_files_should_handle_a_mix_of_override_append_and_new_files` |
| MergeFlavorConfigFiles / should preserve file ordering with overridden file in its original position | `merge_flavor_config_files_should_preserve_file_ordering_with_overridden_file_in_its_original_position` |
| MergeFlavorConfigFiles / should use default permissions (0644) when not specified | `merge_flavor_config_files_should_use_default_permissions_0644_when_not_specified` |
| MergeFlavorConfigFiles / should use custom permissions when specified | `merge_flavor_config_files_should_use_custom_permissions_when_specified` |
| MergeFlavorConfigFiles / should return an error for invalid permissions | `merge_flavor_config_files_should_return_an_error_for_invalid_permissions` |
| MergeFlavorConfigFiles / should skip entries with empty paths | `merge_flavor_config_files_should_skip_entries_with_empty_paths` |
| MergeFlavorConfigFiles / should handle empty raw content for override (creates empty file) | `merge_flavor_config_files_should_handle_empty_raw_content_for_override_creates_empty_file` |
| MergeFlavorConfigFiles / should override multiple base files simultaneously | `merge_flavor_config_files_should_override_multiple_base_files_simultaneously` |
| MergeFlavorConfigFiles / should update mode on append if flavor specifies different permissions | `merge_flavor_config_files_should_update_mode_on_append_if_flavor_specifies_different_permissions` |

## validate_test.go

| Go (`internal/ignition/validate_test.go`) | Rust (`tests/validate_test.rs`) |
|---|---|
| ValidateConfig / should accept a valid empty ignition config | `validate_config_should_accept_a_valid_empty_ignition_config` |
| ValidateConfig / should accept a valid ignition config with files | `validate_config_should_accept_a_valid_ignition_config_with_files` |
| ValidateConfig / should reject ignition config with duplicate file paths | `validate_config_should_reject_ignition_config_with_duplicate_file_paths` |
| ValidateConfig / should reject ignition config with duplicate systemd unit names | `validate_config_should_reject_ignition_config_with_duplicate_systemd_unit_names` |
| ValidateConfig / should accept ignition config with multiple unique files | `validate_config_should_accept_ignition_config_with_multiple_unique_files` |
| ValidateConfig / should accept ignition config with older 3.x versions | `validate_config_should_accept_ignition_config_with_older_3_x_versions` |
| ValidateConfig / should validate configs with files and units across ignition versions used by hypershift | `validate_config_should_validate_configs_with_files_and_units_across_ignition_versions_used_by_hypershift` |
| ValidateConfig / should detect duplicate files across ignition versions used by hypershift | `validate_config_should_detect_duplicate_files_across_ignition_versions_used_by_hypershift` |
| ValidateConfig / should not modify the caller's config version | `validate_config_should_not_modify_the_caller_s_config_version` |
| ValidateConfig / should reject ignition config with unsupported version | `validate_config_should_reject_ignition_config_with_unsupported_version` |
| ValidateConfig / should reject ignition config with garbage version | `validate_config_should_reject_ignition_config_with_garbage_version` |
| ValidateConfig / should reject nil ignition config | `validate_config_should_reject_nil_ignition_config` |

## content/content_test.go (Content Suite)

| Go (`internal/ignition/content/content_test.go`) | Rust (`tests/content_test.rs`) |
|---|---|
| AddFiles / should add files with string content source (data URI) | `add_files_should_add_files_with_string_content_source_data_uri` |
| AddFiles / should add files with byte content source (gzip+base64 encoded) | `add_files_should_add_files_with_byte_content_source_gzip_base64_encoded` |
| AddFiles / should return error for invalid content source type | `add_files_should_return_error_for_invalid_content_source_type` |
| AddFiles / should set correct file metadata | `add_files_should_set_correct_file_metadata` |
| AddSystemdUnits / should add systemd units to ignition | `add_systemd_units_should_add_systemd_units_to_ignition` |
| AddSystemdUnits / should set all units as enabled | `add_systemd_units_should_set_all_units_as_enabled` |
| AddSystemdUnits / should handle nil systemd FS (no units) | `add_systemd_units_should_handle_nil_systemd_fs_no_units` |
| AddContent / should add both files and systemd units | `add_content_should_add_both_files_and_systemd_units` |
| AddKernelArgs / should add kernel argument template | `add_kernel_args_should_add_kernel_argument_template` |
| `var _ ContentProvider = &EmbeddedProvider{}` / `&mockProvider{}` (compile-time) | `content_provider_interface_assertions` |
| (extra) `GetSystemdUnits` error wrapping | `add_systemd_units_wraps_provider_errors` |

`TestIgnition` / `TestContent` (Ginkgo suite runners) have no Rust counterpart: each `It` is a
plain `#[test]`.

## Golden differential tests (`tests/golden.rs`)

Fixtures in `tests/fixtures/` are produced by `hack/golden/ignition`
(run from the repository root; deterministic, fixed random seeds). Every output is compared
byte-for-byte, error strings included.

| Rust test | Fixture | Go functions covered |
|---|---|---|
| `configs_marshal_encode_gzip_validate` | `configs.json` (520 configs: 29 hand-written, 250 random, 241 one-field probes of the URL / data-URL / unit-file / proxy / tang / header validators) | `json.Unmarshal` + `json.Marshal` of `types.Config`, `MarshalJSON`, `EncodeIgnition`, `GzipIgnitionFiles` (result + error), `ValidateConfig` (error string), `validate.ValidateWithContext` (`rpt.String()` with line/column markers, and every entry's kind/context/message incl. warnings) |
| `unmarshal_config_matches_go` | `unmarshal.json` | `json.Unmarshal` into `types.Config`: syntax errors, `UnmarshalTypeError` messages (struct/field context incl. embedded `Node`/`FileEmbedded1`), case-insensitive keys, `null`, duplicate keys, unknown keys |
| `decode_ignition_matches_go` | `decode_ignition.json` | `DecodeIgnition` (base64/gzip/json errors) |
| `merge_struct_transcribe_matches_go` | `merge.json` (11 hand-written + 300 random pairs with colliding keys) | `merge.MergeStructTranscribe` |
| `merge_flavor_config_files_matches_go` | `merge_flavor.json` | `MergeFlavorConfigFiles` (override/append/new/link replacement/errors) |
| `replace_machine_os_url_matches_go` | `machineos.json` | `ReplaceMachineOSURL` |
| `enable_mtu_matches_go` | `mtu.json` | `EnableMTU` |
| `add_flavor_ovs_script_matches_go` | `ovs.json` | `AddFlavorOVSScript` |
| `encode_gzip_file_and_url_encode_match_go` | `encode.json` | `EncodeGzipFile`, `URLEncode` |
| `add_dpu_flavor_yaml_json_match_go` | `flavor.json` | `AddDPUFlavorYAML`, `AddDPUFlavorJSON` (`cleanDPUFlavor`) |
| `content_providers_match_go` | `content.json` | `content.AddContent/AddFiles/AddLinks/AddSystemdUnits/AddKernelArgs` with `common`, `live`, `target` providers (trusted and zero-trust), the full target composition, `MarshalJSON`, `EncodeIgnition`, `GzipIgnitionFiles` |
| `render_dpu_agent_service_unit_matches_go` | `dpu_agent_unit.json` | `target.RenderDPUAgentServiceUnit` |
| `embedded_systemd_dirs_match_filesystem` | (the `crates/ignition/resources/*/systemd` directories) | `//go:embed systemd/*` + `LoadSystemdUnits` order |

## Unit tests (`src/`)

`validate::gopath` (`path.Clean`/`path.Ext` tables from the Go test suite), `validate::gonet`,
`validate::dataurl`, `validate::systemd`, `template`.
