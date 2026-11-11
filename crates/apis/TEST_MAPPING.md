# Go test → Rust test mapping (`apis`)

## `api/v1alpha1/dpfhcpprovisioner_types_test.go` → `tests/dpfhcpprovisioner_types.rs`

| Go spec | Rust test |
|---|---|
| DPFHCPProvisioner API Types / JSON Serialization / should successfully serialize and deserialize a complete DPFHCPProvisioner | `json_serialization_should_successfully_serialize_and_deserialize_a_complete_dpfhcpprovisioner` |
| … / DeepCopy / should create an independent copy of DPFHCPProvisioner | `deep_copy_should_create_an_independent_copy_of_dpfhcpprovisioner` |
| … / DeepCopy / should create an independent copy of DPFHCPProvisionerList | `deep_copy_should_create_an_independent_copy_of_dpfhcpprovisioner_list` |
| … / Scheme Registration / should register DPFHCPProvisioner types in the scheme | `scheme_registration_should_register_dpfhcpprovisioner_types_in_the_scheme` |
| … / Scheme Registration / should register DPFHCPProvisionerList in the scheme | `scheme_registration_should_register_dpfhcpprovisioner_list_in_the_scheme` |
| … / Scheme Registration / should integrate with client-go scheme | `scheme_registration_should_integrate_with_client_go_scheme` |
| … / Phase Enum / should have correct phase enum values | `phase_enum_should_have_correct_phase_enum_values` |
| … / Phase Enum / should allow all valid phases to be set | `phase_enum_should_allow_all_valid_phases_to_be_set` |
| … / NodeSelector / should allow empty/nil NodeSelector (optional field) | `node_selector_should_allow_empty_nil_node_selector_optional_field` |
| … / NodeSelector / should support master node selector | `node_selector_should_support_master_node_selector` |
| … / NodeSelector / should support worker node selector | `node_selector_should_support_worker_node_selector` |
| … / NodeSelector / should support multiple node selectors | `node_selector_should_support_multiple_node_selectors` |
| … / NodeSelector / should deep copy NodeSelector map | `node_selector_should_deep_copy_node_selector_map` |

There is no Go scheme in Rust. "Scheme registration" is checked through
`ctrlrt::Object::resource_info()`, which `ctrlrt::impl_resource!` provides. A Go `DeepCopy` is a
Rust `clone()`.

## Additional Rust-only tests

| Test | Covers |
|---|---|
| `tests/dpfhcpprovisioner_types.rs::should_expose_through_load_balancer_and_is_vip_required` | `ShouldExposeThroughLoadBalancer` / `IsVIPRequired` |
| `tests/dpfhcpprovisioner_types.rs::constants_match_go` | constant values |
| `tests/golden.rs::<kind>` / `<kind>_list` (26 tests) | Go golden encodings: zero value, filled round trip (byte-exact), unknown-field preservation |
| `tests/golden.rs::dpu_flavor_clean_json` | `internal/ignition/dpuflavor.go` `cleanDPUFlavor` JSON, byte-exact |
| `tests/golden.rs::quantity_table` | `resource.ParseQuantity` + `String()` (Go table) |
| `tests/golden.rs::duration_table` | `time.ParseDuration` + `Duration.String()` (Go table) |
| `tests/golden.rs::ipnet_table` | `ipnet.ParseCIDR` + `MarshalJSON` (Go table) |
| `src/go.rs`, `src/resource.rs`, `src/ipnet.rs`, `src/infraid.rs` unit tests | helpers |
