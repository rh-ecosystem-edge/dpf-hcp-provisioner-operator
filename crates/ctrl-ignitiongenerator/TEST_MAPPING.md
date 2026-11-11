# ctrl-ignitiongenerator — Go test → Rust test

Go package `internal/controller/ignitiongenerator`. `suite_test.go` (`TestIgnitionGenerator`)
only bootstraps Ginkgo (`RunSpecs`) — no Rust counterpart needed. The Go tests are internal
(package-level) tests, so the Rust ports are unit-test modules inside `src/` (access to
`build_target_ignition`, `get_ignition_tokens`, ...). None of the Go tests uses envtest: they use
the controller-runtime fake client → `ctrlrt-fake`.

Notes:
* `&IgnitionGenerator{}` (nil client) → `bare_generator()` (an empty fake client, never used).
* `fake.NewClientBuilder().WithObjects(cr).WithStatusSubresource(cr)` → `obj_mut(&mut cr)` (Go's
  `WithObjects` sets resourceVersion `"999"` on the caller's object, which the later
  `Status().Update` relies on) + `with_status_subresource::<DPFHCPProvisioner>()`.
* `httptest.NewTLSServer` → `testutil::IgnitionServer` (rustls + hyper on 127.0.0.1). Go puts the
  PEM of the self-signed server certificate in `ignition-server-ca-cert`; webpki rejects a CA
  certificate used as end entity, so the Rust server certificate is signed by a generated test
  CA (same SANs as httptest's: `example.com`, `*.example.com`, `127.0.0.1`, `::1`) whose PEM is
  stored instead.
* `ConsistOf(...)` → sorted comparison; `MatchRegexp(...)` → explicit path check.
* Ignition configs in fixtures are built with the `ignition` crate types and serialized with
  `ignition::marshal_config` (Go `json.Marshal`).

## `ignition_generator_test.go` → `src/ignition_generator_test.rs` (38 specs)

| Go spec (Describe / It) | Rust test |
|---|---|
| controlPlaneNamespace / should return namespace-name format | `ignition_generator_test::control_plane_namespace_should_return_namespace_name_format` |
| getIgnitionCACert / should return CA cert when secret exists with tls.crt key | `ignition_generator_test::get_ignition_ca_cert_should_return_ca_cert_when_secret_exists_with_tls_crt_key` |
| getIgnitionCACert / should return error when secret is not found | `ignition_generator_test::get_ignition_ca_cert_should_return_error_when_secret_is_not_found` |
| getIgnitionCACert / should return error when tls.crt key is missing | `ignition_generator_test::get_ignition_ca_cert_should_return_error_when_tls_crt_key_is_missing` |
| getIgnitionToken / should find token secret by prefix and return base64-encoded token | `ignition_generator_test::get_ignition_token_should_find_token_secret_by_prefix_and_return_base64_encoded_token` |
| getIgnitionToken / should return error when no matching secret exists | `ignition_generator_test::get_ignition_token_should_return_error_when_no_matching_secret_exists` |
| getIgnitionToken / should return error when token key is missing from all matching secrets | `ignition_generator_test::get_ignition_token_should_return_error_when_token_key_is_missing_from_all_matching_secrets` |
| getIgnitionToken / should return all matching tokens when multiple secrets exist | `ignition_generator_test::get_ignition_token_should_return_all_matching_tokens_when_multiple_secrets_exist` |
| getIgnitionEndpoint / should return the ignition endpoint from HostedCluster status | `ignition_generator_test::get_ignition_endpoint_should_return_the_ignition_endpoint_from_hosted_cluster_status` |
| getIgnitionEndpoint / should return error when HostedCluster is not found | `ignition_generator_test::get_ignition_endpoint_should_return_error_when_hosted_cluster_is_not_found` |
| getIgnitionEndpoint / should return error when ignition endpoint is empty | `ignition_generator_test::get_ignition_endpoint_should_return_error_when_ignition_endpoint_is_empty` |
| retrieveDPUFlavor / should return DPUFlavor when DPUDeployment and DPUFlavor exist | `ignition_generator_test::retrieve_dpu_flavor_should_return_dpu_flavor_when_dpu_deployment_and_dpu_flavor_exist` |
| retrieveDPUFlavor / should return error when DPUDeployment is not found | `ignition_generator_test::retrieve_dpu_flavor_should_return_error_when_dpu_deployment_is_not_found` |
| retrieveDPUFlavor / should return error when DPUDeployment has empty flavor | `ignition_generator_test::retrieve_dpu_flavor_should_return_error_when_dpu_deployment_has_empty_flavor` |
| retrieveDPUFlavor / should return error when DPUFlavor is not found | `ignition_generator_test::retrieve_dpu_flavor_should_return_error_when_dpu_flavor_is_not_found` |
| GetSingletonDPFOperatorConfig / should return the singleton DPFOperatorConfig | `ignition_generator_test::get_singleton_dpf_operator_config_should_return_the_singleton_dpf_operator_config` |
| GetSingletonDPFOperatorConfig / should return error when no DPFOperatorConfig exists | `ignition_generator_test::get_singleton_dpf_operator_config_should_return_error_when_no_dpf_operator_config_exists` |
| buildTargetIgnition / should produce a valid target ignition with DPF files | `ignition_generator_test::build_target_ignition_should_produce_a_valid_target_ignition_with_dpf_files` |
| buildTargetIgnition / should preserve HCP ignition version from upstream | `ignition_generator_test::build_target_ignition_should_preserve_hcp_ignition_version_from_upstream` |
| buildTargetIgnition / should return error for invalid JSON | `ignition_generator_test::build_target_ignition_should_return_error_for_invalid_json` |
| buildTargetIgnition / should skip MTU configuration when MTU is 1500 | `ignition_generator_test::build_target_ignition_should_skip_mtu_configuration_when_mtu_is_1500` |
| buildTargetIgnition / should apply MTU configuration when MTU is not 1500 | `ignition_generator_test::build_target_ignition_should_apply_mtu_configuration_when_mtu_is_not_1500` |
| buildTargetIgnition / should exclude pf-monitor.service from systemd units in zero-trust mode | `ignition_generator_test::build_target_ignition_should_exclude_pf_monitor_service_from_systemd_units_in_zero_trust_mode` |
| buildTargetIgnition / should exclude pf-monitor.sh file in zero-trust mode | `ignition_generator_test::build_target_ignition_should_exclude_pf_monitor_sh_file_in_zero_trust_mode` |
| buildTargetIgnition / should include pf-monitor in non-zero-trust mode | `ignition_generator_test::build_target_ignition_should_include_pf_monitor_in_non_zero_trust_mode` |
| buildTargetIgnition / should keep other systemd units in zero-trust mode | `ignition_generator_test::build_target_ignition_should_keep_other_systemd_units_in_zero_trust_mode` |
| buildLiveIgnition / should produce live ignition with embedded target at /var/target.ign | `ignition_generator_test::build_live_ignition_should_produce_live_ignition_with_embedded_target_at_var_target_ign` |
| buildLiveIgnition / should copy passwd from HCP ignition | `ignition_generator_test::build_live_ignition_should_copy_passwd_from_hcp_ignition` |
| buildLiveIgnition / should work when HCP ignition has no passwd | `ignition_generator_test::build_live_ignition_should_work_when_hcp_ignition_has_no_passwd` |
| buildLiveIgnition / should return error for invalid HCP JSON | `ignition_generator_test::build_live_ignition_should_return_error_for_invalid_hcp_json` |
| createOrUpdateConfigMap / should create a new ConfigMap when it does not exist | `ignition_generator_test::create_or_update_config_map_should_create_a_new_config_map_when_it_does_not_exist` |
| createOrUpdateConfigMap / should update an existing ConfigMap | `ignition_generator_test::create_or_update_config_map_should_update_an_existing_config_map` |
| createOrUpdateConfigMap / should set owner reference when CR and ConfigMap are in the same namespace | `ignition_generator_test::create_or_update_config_map_should_set_owner_reference_when_cr_and_config_map_are_in_the_same_namespace` |
| createOrUpdateConfigMap / should not set owner reference for cross-namespace ConfigMap | `ignition_generator_test::create_or_update_config_map_should_not_set_owner_reference_for_cross_namespace_config_map` |
| createOrUpdateConfigMap / should not include bfb annotations when DPUDeploymentRef is nil | `ignition_generator_test::create_or_update_config_map_should_not_include_bfb_annotations_when_dpu_deployment_ref_is_nil` |
| GenerateIgnition / should set IgnitionConfigured=False and RequeueAfter on failure | `ignition_generator_test::generate_ignition_should_set_ignition_configured_false_and_requeue_after_on_failure` |
| GenerateIgnition / should use status.blueFieldOCPLayerImage as fallback when spec.machineOSURL is empty | `ignition_generator_test::generate_ignition_should_use_status_blue_field_ocp_layer_image_as_fallback_when_spec_machine_osurl_is_empty` |
| GenerateIgnition / should set MachineOSURLMissing when both spec and status URLs are empty | `ignition_generator_test::generate_ignition_should_set_machine_osurl_missing_when_both_spec_and_status_urls_are_empty` |

Rust-only additions: `constants_match_go_definitions` (label/annotation constants vs. their Go
prefix expressions, `ConfigMapName`), `hash_pull_secret_matches_hypershift_hash_simple`.

## `ignition_generator_integration_test.go` → `src/ignition_generator_integration_test.rs` (8 specs)

| Go spec (Describe / It) | Rust test |
|---|---|
| generateIgnition integration / should run the full ignition generation pipeline end-to-end | `ignition_generator_integration_test::generate_ignition_integration_should_run_the_full_ignition_generation_pipeline_end_to_end` |
| generateIgnition integration / should produce validated ignition with MTU=1500 (no NM connections) | `ignition_generator_integration_test::generate_ignition_integration_should_produce_validated_ignition_with_mtu_1500_no_nm_connections` |
| generateIgnition integration / should produce validated ignition with MTU=9000 (with NM connections in target) | `ignition_generator_integration_test::generate_ignition_integration_should_produce_validated_ignition_with_mtu_9000_with_nm_connections_in_target` |
| generateIgnition integration / should produce idempotent results on re-run (update existing ConfigMap) | `ignition_generator_integration_test::generate_ignition_integration_should_produce_idempotent_results_on_re_run_update_existing_config_map` |
| generateIgnition integration / should set IgnitionConfigured=False when ignition endpoint is unavailable | `ignition_generator_integration_test::generate_ignition_integration_should_set_ignition_configured_false_when_ignition_endpoint_is_unavailable` |
| generateIgnition integration / should propagate machineOSURL from spec to ConfigMap annotation | `ignition_generator_integration_test::generate_ignition_integration_should_propagate_machine_osurl_from_spec_to_config_map_annotation` |
| generateIgnition integration / should reject HCP ignition with duplicate files via target validation | `ignition_generator_integration_test::generate_ignition_integration_should_reject_hcp_ignition_with_duplicate_files_via_target_validation` |
| generateIgnition integration / should produce live ignition that passes validation after template substitution | `ignition_generator_integration_test::generate_ignition_integration_should_produce_live_ignition_that_passes_validation_after_template_substitution` |

## `mode_test.go` → `src/mode_test.rs` (11 cases)

| Go test / t.Run | Rust test |
|---|---|
| TestDPUModeFromFlavor / zero-trust mode | `mode_test::test_dpu_mode_from_flavor_zero_trust_mode` |
| TestDPUModeFromFlavor / dpu mode | `mode_test::test_dpu_mode_from_flavor_dpu_mode` |
| TestDPUModeFromFlavor / empty mode returns error | `mode_test::test_dpu_mode_from_flavor_empty_mode_returns_error` |
| TestDPUModeFromFlavor / nic mode returns error | `mode_test::test_dpu_mode_from_flavor_nic_mode_returns_error` |
| TestDPUModeFromFlavor / invalid mode returns error | `mode_test::test_dpu_mode_from_flavor_invalid_mode_returns_error` |
| TestDPUModeFromFlavor / nil DPUFlavor returns error | `mode_test::test_dpu_mode_from_flavor_nil_dpu_flavor_returns_error` |
| TestIsZeroTrustModeResolution / flavor dpuMode wins over host-trusted deploymentMode | `mode_test::test_is_zero_trust_mode_resolution_flavor_dpu_mode_wins_over_host_trusted_deployment_mode` |
| TestIsZeroTrustModeResolution / 26.8 deploymentMode when flavor dpuMode unset | `mode_test::test_is_zero_trust_mode_resolution_26_8_deployment_mode_when_flavor_dpu_mode_unset` |
| TestIsZeroTrustModeResolution / legacy redfish when flavor dpuMode unset | `mode_test::test_is_zero_trust_mode_resolution_legacy_redfish_when_flavor_dpu_mode_unset` |
| TestIsZeroTrustModeResolution / 26.4 flavor zero-trust when deploymentMode unset | `mode_test::test_is_zero_trust_mode_resolution_26_4_flavor_zero_trust_when_deployment_mode_unset` |
| TestIsZeroTrustModeResolution / 26.4 flavor dpu when deploymentMode unset | `mode_test::test_is_zero_trust_mode_resolution_26_4_flavor_dpu_when_deployment_mode_unset` |

Rust-only addition: `detect_dpu_mode_error_messages_match_go` (exact error texts of
`detectDPUMode` / `dpuModeFromFlavor`, `dpuModeDescription`).

## Golden differential tests → `src/golden_test.rs` (81 fixtures + 1 completeness check)

Fixtures: `tests/fixtures/<scenario>.json`, generated by
`go run -mod=vendor ./hack/golden/ignitiongenerator` (scenario list and fixture format are
documented there). One `golden_test::golden::<scenario>` test per fixture;
`golden_test::golden::golden_fixture_list_is_complete` checks that every fixture has a test.
