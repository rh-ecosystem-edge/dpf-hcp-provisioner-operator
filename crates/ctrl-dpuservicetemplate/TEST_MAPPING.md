# internal/controller/dpuservicetemplate test mapping

Go suites:

* `release_image_test.go` (package `dpuservicetemplate`, internal) → `src/release_image_test.rs`;
* `dpuservicetemplate_test.go` (package `dpuservicetemplate_test`, external) → `tests/dpuservicetemplate_test.rs`.

`suite_test.go` only bootstraps Ginkgo (`TestDPUServiceTemplate` → `RunSpecs("DPUServiceTemplate Suite")`); it has no
Rust counterpart — each spec is a plain `#[test]` / `#[tokio::test]`. The controller-runtime fake client is replaced by
`ctrlrt-fake` with the same scheme kinds and `WithStatusSubresource(&DPFOperatorConfig{})`.

All 39 Go specs are ported (8 + 31, DescribeTable entries counted individually).

## release_image_test.go

| Go spec (`Release Image Utilities / ...`) | Rust test (`src/release_image_test.rs`) |
|---|---|
| findComponentInImageReferences / should find a component by name | `release_image_test::release_image_utilities_find_component_in_image_references_should_find_a_component_by_name` |
| findComponentInImageReferences / should return error when component is not found | `release_image_test::release_image_utilities_find_component_in_image_references_should_return_error_when_component_is_not_found` |
| findComponentInImageReferences / should return error when image-references file is missing | `release_image_test::release_image_utilities_find_component_in_image_references_should_return_error_when_image_references_file_is_missing` |
| findComponentInImageReferences / should return error when component has empty image reference | `release_image_test::release_image_utilities_find_component_in_image_references_should_return_error_when_component_has_empty_image_reference` |
| splitImage / should split image with tag | `release_image_test::release_image_utilities_split_image_should_split_image_with_tag` |
| splitImage / should split image with sha256 digest | `release_image_test::release_image_utilities_split_image_should_split_image_with_sha256_digest` |
| splitImage / should return error when no tag or digest present | `release_image_test::release_image_utilities_split_image_should_return_error_when_no_tag_or_digest_present` |
| splitImage / should return error for empty image | `release_image_test::release_image_utilities_split_image_should_return_error_for_empty_image` |

`buildTestTar` writes a raw tar stream; the Rust test builds the same ustar archive and reads it through
`mutate::extract` of a one-layer `StaticImage` (the production reader type).

## dpuservicetemplate_test.go

| Go spec | Rust test (`tests/dpuservicetemplate_test.rs`) |
|---|---|
| DPUServiceTemplate Manager / EnsureTemplates / when all prerequisites are available / should create all three DPUServiceTemplates with the source annotation | `manager_ensure_templates_when_all_prerequisites_are_available_should_create_all_three_dpu_service_templates_with_the_source_annotation` |
| DPUServiceTemplate Manager / EnsureTemplates / when OVN DaemonSet image has not changed / should skip the release payload pull | `manager_ensure_templates_when_ovn_daemon_set_image_has_not_changed_should_skip_the_release_payload_pull` |
| DPUServiceTemplate Manager / EnsureTemplates / when OVN DaemonSet image has not changed / should not advance ovnDaemonsetVersion even when CVO desired version changes | `manager_ensure_templates_when_ovn_daemon_set_image_has_not_changed_should_not_advance_ovn_daemonset_version_even_when_cvo_desired_version_changes` |
| DPUServiceTemplate Manager / EnsureTemplates / on fresh install when DaemonSet pods are pending (bootstrap deadlock) / should create the OVN template even though rollout is not complete | `manager_ensure_templates_on_fresh_install_when_daemon_set_pods_are_pending_bootstrap_deadlock_should_create_the_ovn_template_even_though_rollout_is_not_complete` |
| DPUServiceTemplate Manager / EnsureTemplates / when OVN DaemonSet image changed but rollout is not complete / should keep current template values and not pull release payload | `manager_ensure_templates_when_ovn_daemon_set_image_changed_but_rollout_is_not_complete_should_keep_current_template_values_and_not_pull_release_payload` |
| DPUServiceTemplate Manager / EnsureTemplates / when OVN DaemonSet image changed and rollout is complete / should resolve the new aarch64 image from release payload | `manager_ensure_templates_when_ovn_daemon_set_image_changed_and_rollout_is_complete_should_resolve_the_new_aarch64_image_from_release_payload` |
| DPUServiceTemplate Manager / EnsureTemplates / when release image is multi-arch / should use the management cluster OVN image directly and skip release payload resolution | `manager_ensure_templates_when_release_image_is_multi_arch_should_use_the_management_cluster_ovn_image_directly_and_skip_release_payload_resolution` |
| DPUServiceTemplate Manager / EnsureTemplates / when DPFOperatorConfig is missing / should return error | `manager_ensure_templates_when_dpf_operator_config_is_missing_should_return_error` |
| DPUServiceTemplate Manager / EnsureTemplates / when DPF version has no dedicated embedded row / should use embedded default values | `manager_ensure_templates_when_dpf_version_has_no_dedicated_embedded_row_should_use_embedded_default_values` |
| DPUServiceTemplate Manager / EnsureTemplates / when OVN DaemonSet is missing / should return error | `manager_ensure_templates_when_ovn_daemon_set_is_missing_should_return_error` |
| DPUServiceTemplate Manager / EnsureTemplates / when ReleaseImageReader returns an error / should return error | `manager_ensure_templates_when_release_image_reader_returns_an_error_should_return_error` |
| DPUServiceTemplate Manager / EnsureTemplates / when a managed DPUServiceTemplate is being deleted / should not recreate or update the terminating template | `manager_ensure_templates_when_a_managed_dpu_service_template_is_being_deleted_should_not_recreate_or_update_the_terminating_template` |
| DPUServiceTemplate Manager / EnsureTemplates / when a DPUServiceTemplate already exists with different spec / should update it to the desired state | `manager_ensure_templates_when_a_dpu_service_template_already_exists_with_different_spec_should_update_it_to_the_desired_state` |
| DPUServiceTemplate Manager / DeleteTemplates / when all three templates exist / should delete all of them | `manager_delete_templates_when_all_three_templates_exist_should_delete_all_of_them` |
| DPUServiceTemplate Manager / DeleteTemplates / when no templates exist / should succeed immediately (idempotent) | `manager_delete_templates_when_no_templates_exist_should_succeed_immediately_idempotent` |
| DPUServiceTemplate Manager / OVNDaemonsetVersionForOCP / should return the correct daemonset version / Entry "old OCP" | `manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_old_ocp` |
| DPUServiceTemplate Manager / OVNDaemonsetVersionForOCP / should return the correct daemonset version / Entry "4.21.x" | `manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_4_21_x` |
| DPUServiceTemplate Manager / OVNDaemonsetVersionForOCP / should return the correct daemonset version / Entry "just before 4.22.0" | `manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_just_before_4_22_0` |
| DPUServiceTemplate Manager / OVNDaemonsetVersionForOCP / should return the correct daemonset version / Entry "4.22.0" | `manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_4_22_0` |
| DPUServiceTemplate Manager / OVNDaemonsetVersionForOCP / should return the correct daemonset version / Entry "4.22.1 boundary" | `manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_4_22_1_boundary` |
| DPUServiceTemplate Manager / OVNDaemonsetVersionForOCP / should return the correct daemonset version / Entry "4.22.z after" | `manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_4_22_z_after` |
| DPUServiceTemplate Manager / OVNDaemonsetVersionForOCP / should return the correct daemonset version / Entry "future 4.23" | `manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_future_4_23` |
| DPUServiceTemplate Manager / OVNDaemonsetVersionForOCP / should return the correct daemonset version / Entry "pre-release" | `manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_pre_release` |
| DPUServiceTemplate Manager / OVNDaemonsetVersionForOCP / should return the correct daemonset version / Entry "garbage" | `manager_ovn_daemonset_version_for_ocp_should_return_the_correct_daemonset_version_garbage` |
| DPUServiceTemplate Manager / GetDefaultsForVersion / should return defaults for supported version 26.4 | `manager_get_defaults_for_version_should_return_defaults_for_supported_version_26_4` |
| DPUServiceTemplate Manager / GetDefaultsForVersion / should return embedded default for unknown DPF minors | `manager_get_defaults_for_version_should_return_embedded_default_for_unknown_dpf_minors` |
| DPUServiceTemplate Reconciler / should delete DPUServiceTemplates when the referenced DPUDeployment is being deleted | `reconciler_should_delete_dpu_service_templates_when_the_referenced_dpu_deployment_is_being_deleted` |
| DPUServiceTemplate Reconciler / should create DPUServiceTemplates when the referenced DPUDeployment is active | `reconciler_should_create_dpu_service_templates_when_the_referenced_dpu_deployment_is_active` |
| DPUServiceTemplate Reconciler / should delete DPUServiceTemplates when the referenced DPUDeployment is not found | `reconciler_should_delete_dpu_service_templates_when_the_referenced_dpu_deployment_is_not_found` |
| DPUServiceTemplate Reconciler / should delete DPUServiceTemplates when no provisioner remains | `reconciler_should_delete_dpu_service_templates_when_no_provisioner_remains` |
| DPUServiceTemplate Reconciler / should still create DPUServiceTemplates when another DPUDeployment remains active | `reconciler_should_still_create_dpu_service_templates_when_another_dpu_deployment_remains_active` |

## Additional tests (no Go counterpart)

| Test | What |
|---|---|
| `release_image_test::image_references_unmarshal_go_error_messages` | Go `encoding/json` error strings for malformed image-references (checked against Go output) |
| `gojson::tests::go_messages` (`src/gojson.rs`) | Go `encoding/json` merge-decoding semantics and error messages |
| `controller_test::ovn_daemon_set_predicate_matches_only_the_ovn_daemon_set` | `SetupWithManager` wiring: watches, predicates, map functions |
| `controller_test::overrides_config_map_predicate_matches_only_the_overrides_config_map` | `SetupWithManager` wiring: watches, predicates, map functions |
| `controller_test::map_functions` | `SetupWithManager` wiring: watches, predicates, map functions |
| `controller_test::map_functions_return_nothing_when_listing_fails` | `SetupWithManager` wiring: watches, predicates, map functions |
| `controller_test::setup_with_manager_registers_the_watches` | `SetupWithManager` wiring: watches, predicates, map functions |

### Golden differential tests (`tests/golden.rs`)

Fixtures from `go run -mod=vendor ./hack/golden/dpuservicetemplate crates/ctrl-dpuservicetemplate/tests/fixtures`. Each
scenario drives the Go manager/reconciler against the controller-runtime fake client (fake `ReleaseImageReader`, release
images on an httptest rules registry) and the Rust test replays it against `ctrlrt-fake` + the in-process rules registry
(`tests/support/mod.rs`), comparing per-step errors and release-reader calls and the `encoding/json` bytes of the three
stored DPUServiceTemplates (OVN / DTS / HBN incl. helm values, labels, annotations, resourceVersion).

| Fixture | Rust test |
|---|---|
| `tests/fixtures/create_single_arch.json` | `golden::golden_create_single_arch` |
| `tests/fixtures/create_multi_arch_tag_with_pull_secret_feature_flags.json` | `golden::golden_create_multi_arch_tag_with_pull_secret_feature_flags` |
| `tests/fixtures/create_multi_arch_index.json` | `golden::golden_create_multi_arch_index` |
| `tests/fixtures/single_arch_feature_flags_prerelease.json` | `golden::golden_single_arch_feature_flags_prerelease` |
| `tests/fixtures/overrides_applied.json` | `golden::golden_overrides_applied` |
| `tests/fixtures/overrides_other_version_only.json` | `golden::golden_overrides_other_version_only` |
| `tests/fixtures/overrides_unknown_field.json` | `golden::golden_overrides_unknown_field` |
| `tests/fixtures/overrides_unknown_nested_field.json` | `golden::golden_overrides_unknown_nested_field` |
| `tests/fixtures/overrides_type_errors.json` | `golden::golden_overrides_type_errors` |
| `tests/fixtures/overrides_int_overflow.json` | `golden::golden_overrides_int_overflow` |
| `tests/fixtures/overrides_struct_type_error.json` | `golden::golden_overrides_struct_type_error` |
| `tests/fixtures/overrides_version_not_object.json` | `golden::golden_overrides_version_not_object` |
| `tests/fixtures/overrides_version_null.json` | `golden::golden_overrides_version_null` |
| `tests/fixtures/overrides_invalid_json.json` | `golden::golden_overrides_invalid_json` |
| `tests/fixtures/overrides_not_object.json` | `golden::golden_overrides_not_object` |
| `tests/fixtures/overrides_null_document.json` | `golden::golden_overrides_null_document` |
| `tests/fixtures/drift_corrected_and_unmanaged_skipped.json` | `golden::golden_drift_corrected_and_unmanaged_skipped` |
| `tests/fixtures/deleting_template_skipped.json` | `golden::golden_deleting_template_skipped` |
| `tests/fixtures/image_unchanged_keeps_values_cv_changed.json` | `golden::golden_image_unchanged_keeps_values_cv_changed` |
| `tests/fixtures/existing_template_values_used_when_image_matches.json` | `golden::golden_existing_template_values_used_when_image_matches` |
| `tests/fixtures/existing_template_no_values.json` | `golden::golden_existing_template_no_values` |
| `tests/fixtures/existing_template_values_not_object.json` | `golden::golden_existing_template_values_not_object` |
| `tests/fixtures/existing_template_values_missing_image.json` | `golden::golden_existing_template_values_missing_image` |
| `tests/fixtures/rollout_incomplete_skips_update.json` | `golden::golden_rollout_incomplete_skips_update` |
| `tests/fixtures/rollout_complete_resolves_new_image.json` | `golden::golden_rollout_complete_resolves_new_image` |
| `tests/fixtures/reader_error.json` | `golden::golden_reader_error` |
| `tests/fixtures/reader_returns_untagged_image.json` | `golden::golden_reader_returns_untagged_image` |
| `tests/fixtures/multi_arch_untagged_daemonset_image.json` | `golden::golden_multi_arch_untagged_daemonset_image` |
| `tests/fixtures/missing_daemonset.json` | `golden::golden_missing_daemonset` |
| `tests/fixtures/daemonset_without_ovn_container.json` | `golden::golden_daemonset_without_ovn_container` |
| `tests/fixtures/missing_dpf_operator_config.json` | `golden::golden_missing_dpf_operator_config` |
| `tests/fixtures/unknown_dpf_version_uses_default.json` | `golden::golden_unknown_dpf_version_uses_default` |
| `tests/fixtures/bad_dpf_version.json` | `golden::golden_bad_dpf_version` |
| `tests/fixtures/missing_cluster_version.json` | `golden::golden_missing_cluster_version` |
| `tests/fixtures/cluster_version_empty_image.json` | `golden::golden_cluster_version_empty_image` |
| `tests/fixtures/missing_pull_secret.json` | `golden::golden_missing_pull_secret` |
| `tests/fixtures/delete_templates.json` | `golden::golden_delete_templates` |
| `tests/fixtures/reconcile_management_disabled.json` | `golden::golden_reconcile_management_disabled` |
| `tests/fixtures/reconcile_no_config.json` | `golden::golden_reconcile_no_config` |
| `tests/fixtures/reconcile_active_deployment.json` | `golden::golden_reconcile_active_deployment` |
| `tests/fixtures/reconcile_deleting_deployment.json` | `golden::golden_reconcile_deleting_deployment` |
| `tests/fixtures/reconcile_ensure_error.json` | `golden::golden_reconcile_ensure_error` |
| `tests/fixtures/release_reader.json` (10 `RemoteReleaseImageReader` cases) | `golden::golden_remote_release_image_reader` |
