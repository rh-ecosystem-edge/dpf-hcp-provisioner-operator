# controller — test mapping

> **Golden fixtures are frozen.** The fixtures referenced below were generated from the former Go
> implementation by the generators under `hack/golden/`, which were removed together with the Go
> code (still in the git history, commit `fead352`). They can no longer be regenerated; see
> `docs/rust-port/CONVENTIONS.md` (rule 6).

Go package: `internal/controller` (main `DPFHCPProvisionerReconciler`). All Go test files are
white-box tests (package `controller`), so they are ported as unit-test modules under `src/`
(`src/<go file name>.rs`), with access to the crate-private reconciler methods
(`update_phase_from_conditions`, `handle_upgrade`, `handle_dependency_changes`, ...).

Conventions: `Describe`/`Context` → nested module, `It` → test function (snake_case of the
spec text). `BeforeEach`/`AfterEach` → `before_each()`/`after_each()` run by the container's
`it()` helper (`AfterEach` runs even when the spec fails, like Ginkgo). Gomega
`Eventually`/`Consistently` → `suite_test::eventually`/`consistently` with the Go timeouts and
polling intervals.

## suite_test.go → `src/suite_test.rs`

`BeforeSuite` is ported 1:1: one envtest control plane (`ctrlrt-envtest`, CRDs from
`config/crd/bases` + `config/crd/external`, binaries from `bin/k8s/<first dir>`),
a direct `k8sClient`, and a real controller manager (`ctrlrt-runtime::Manager`, Go's
`k8sManager`) running the reconciler wired with exactly the Go suite's dependency subset
(image lookup with the fake image checker, DPUCluster/secrets validators, secret/HC/NodePool
managers, finalizer manager with kubeconfig-injection + HostedCluster cleanup handlers, status
syncer, kubeconfig injector; MetalLB manager, ignition generator and hugepages manager unset =
Go `nil`), plus the `clusters` / `dpf-operator-system` namespaces and the `default`
DPFHCPProvisionerConfig. The environment/manager are started lazily by the first envtest spec
on a process-wide tokio runtime, and envtest specs are serialized by a global lock — Ginkgo runs
the Go specs serially and they share fixed names (`default` namespace, cluster-wide cleanups in
`AfterEach`). Pure fake-client specs run as ordinary parallel `#[tokio::test]`s, except those
inside a container with an envtest `BeforeEach` (Phase Transitions), which keep it.
`fakeImageChecker` → `suite_test::FakeImageChecker`. `AfterSuite` (stop) is handled by
ctrlrt-envtest's watchdog at process exit. Set `CONTROLLER_TEST_LOG=1` to see controller logs
(Go writes them to `GinkgoWriter`).

Deviation (documented): the "DPFHCPProvisioner Integration Tests with envtest" container's
`AfterEach` (delete all provisioners, wait for none) is also run *before* each of its specs.
Ginkgo runs that container's specs consecutively, so "should list multiple DPFHCPProvisioner
resources" (expects exactly 3 cluster-wide) always starts after the previous spec's cleanup;
in Rust specs of different containers interleave (e.g. the DPUDeployment envtest specs leave
provisioners behind, as in Go), so the precondition is re-established explicitly.

All **107** Go specs are ported (plus 2 Rust-only nil-pointer tests and 12 golden scenarios).

## dpfhcpprovisioner_crd_test.go (19, envtest) — `dpfhcpprovisioner_crd_test::dpfhcpprovisioner_crd_schema_validation_tests::…`

| Go spec | Rust test |
|---|---|
| BaseDomain Validation › should accept valid baseDomain patterns | `base_domain_validation::should_accept_valid_base_domain_patterns` |
| BaseDomain Validation › should reject invalid baseDomain patterns | `base_domain_validation::should_reject_invalid_base_domain_patterns` |
| BaseDomain Validation › should reject baseDomain exceeding max length (253 chars) | `base_domain_validation::should_reject_base_domain_exceeding_max_length_253_chars` |
| BaseDomain Validation › should reject CR missing required baseDomain field | `base_domain_validation::should_reject_cr_missing_required_base_domain_field` |
| Enum Validation › should accept valid controlPlaneAvailabilityPolicy values | `enum_validation::should_accept_valid_control_plane_availability_policy_values` |
| Enum Validation › should reject invalid controlPlaneAvailabilityPolicy values | `enum_validation::should_reject_invalid_control_plane_availability_policy_values` |
| Default Values Application › should apply default values when fields are omitted | `default_values_application::should_apply_default_values_when_fields_are_omitted` |
| VIP Requirement … › should reject HighlyAvailable without VIP | `vip_requirement_based_on_control_plane_availability_policy::should_reject_highly_available_without_vip` |
| VIP Requirement … › should accept HighlyAvailable with VIP | `vip_requirement_based_on_control_plane_availability_policy::should_accept_highly_available_with_vip` |
| VIP Requirement … › should accept SingleReplica without VIP | `vip_requirement_based_on_control_plane_availability_policy::should_accept_single_replica_without_vip` |
| VIP Requirement … › should accept SingleReplica with VIP | `vip_requirement_based_on_control_plane_availability_policy::should_accept_single_replica_with_vip` |
| Field Immutability Validation › should reject updates to dpuClusterRef | `field_immutability_validation::should_reject_updates_to_dpu_cluster_ref` |
| Field Immutability Validation › should reject updates to baseDomain | `field_immutability_validation::should_reject_updates_to_base_domain` |
| Field Immutability Validation › should allow updates to sshKeySecretRef (mutable for credential rotation) | `field_immutability_validation::should_allow_updates_to_ssh_key_secret_ref_mutable_for_credential_rotation` |
| Field Immutability Validation › should allow updates to pullSecretRef (mutable for credential rotation) | `field_immutability_validation::should_allow_updates_to_pull_secret_ref_mutable_for_credential_rotation` |
| Field Immutability Validation › should reject updates to etcdStorageClass | `field_immutability_validation::should_reject_updates_to_etcd_storage_class` |
| Field Immutability Validation › should reject updates to controlPlaneAvailabilityPolicy | `field_immutability_validation::should_reject_updates_to_control_plane_availability_policy` |
| Field Immutability Validation › should reject updates to virtualIP | `field_immutability_validation::should_reject_updates_to_virtual_ip` |
| Field Immutability Validation › should allow updates to ocpReleaseImage (mutable) | `field_immutability_validation::should_allow_updates_to_ocp_release_image_mutable` |

## dpfhcpprovisioner_integration_test.go (5, envtest) — `dpfhcpprovisioner_integration_test::dpfhcpprovisioner_integration_tests_with_envtest::…`

| Go spec | Rust test |
|---|---|
| Complete CR Lifecycle › should successfully create, read, update, and delete a DPFHCPProvisioner | `complete_cr_lifecycle::should_successfully_create_read_update_and_delete_a_dpfhcp_provisioner` |
| Complete CR Lifecycle › should handle status updates independently of spec | `complete_cr_lifecycle::should_handle_status_updates_independently_of_spec` |
| Complete CR Lifecycle › should support full status population | `complete_cr_lifecycle::should_support_full_status_population` |
| List Operations › should list multiple DPFHCPProvisioner resources | `list_operations::should_list_multiple_dpfhcp_provisioner_resources` |
| Resource Metadata › should preserve labels and annotations | `resource_metadata::should_preserve_labels_and_annotations` |

## dpfhcpprovisioner_phase_test.go (31)

`DPFHCPProvisioner Phase Transitions` (envtest `BeforeEach`/`AfterEach`) —
`dpfhcpprovisioner_phase_test::dpfhcpprovisioner_phase_transitions::controller_driven_phase_computation::…`

| Go spec (› Controller-Driven Phase Computation › …) | Rust test |
|---|---|
| should transition to Pending when all validations pass | `should_transition_to_pending_when_all_validations_pass` |
| should transition to Failed when DPUCluster is missing | `should_transition_to_failed_when_dpu_cluster_is_missing` |
| should transition to Deleting when CR is being deleted | `should_transition_to_deleting_when_cr_is_being_deleted` |
| should transition to Provisioning when HostedCluster is created | `should_transition_to_provisioning_when_hosted_cluster_is_created` |
| should stay in Provisioning when HostedCluster is available BUT kubeconfig is NOT injected | `should_stay_in_provisioning_when_hosted_cluster_is_available_but_kubeconfig_is_not_injected` |
| should show WaitingForControlPlane when upgrading and control plane not ready | `should_show_waiting_for_control_plane_when_upgrading_and_control_plane_not_ready` |
| should transition to GeneratingIgnition when HostedClusterUpgrading is False | `should_transition_to_generating_ignition_when_hosted_cluster_upgrading_is_false` |
| should show GeneratingIgnition when upgrading and control plane is ready | `should_show_generating_ignition_when_upgrading_and_control_plane_is_ready` |
| isUpgrading should return true when HostedClusterUpgrading=True | `is_upgrading_should_return_true_when_hosted_cluster_upgrading_true` |
| isUpgrading should return false when HostedClusterUpgrading=False | `is_upgrading_should_return_false_when_hosted_cluster_upgrading_false` |
| isUpgrading should return false when condition is not set | `is_upgrading_should_return_false_when_condition_is_not_set` |
| isHostedClusterVersionReady should return false when ClusterVersionProgressing is True | `is_hosted_cluster_version_ready_should_return_false_when_cluster_version_progressing_is_true` |
| isHostedClusterVersionReady should return true when version matches and state is Completed | `is_hosted_cluster_version_ready_should_return_true_when_version_matches_and_state_is_completed` |
| isHostedClusterVersionReady should return false when version matches but state is Partial | `is_hosted_cluster_version_ready_should_return_false_when_version_matches_but_state_is_partial` |
| isHostedClusterVersionReady should return false when version does not match | `is_hosted_cluster_version_ready_should_return_false_when_version_does_not_match` |
| isHostedClusterVersionReady should return false when version history is empty | `is_hosted_cluster_version_ready_should_return_false_when_version_history_is_empty` |
| isHostedClusterVersionReady should return false when ControlPlaneVersion has no history | `is_hosted_cluster_version_ready_should_return_false_when_control_plane_version_has_no_history` |
| isHostedClusterVersionReady should return true for digest images when image matches and Completed | `is_hosted_cluster_version_ready_should_return_true_for_digest_images_when_image_matches_and_completed` |
| isHostedClusterVersionReady should return false for digest images when image does not match | `is_hosted_cluster_version_ready_should_return_false_for_digest_images_when_image_does_not_match` |
| should transition to Failed when ignition generation fails (IgnitionConfigured=False) | `should_transition_to_failed_when_ignition_generation_fails_ignition_configured_false` |
| should transition to Error when HostedCluster is blocked (Progressing=False, reason=Blocked) | `should_transition_to_error_when_hosted_cluster_is_blocked_progressing_false_reason_blocked` |
| should transition to Error when HostedCluster is degraded | `should_transition_to_error_when_hosted_cluster_is_degraded` |
| should stay in Provisioning when HostedCluster is progressing normally | `should_stay_in_provisioning_when_hosted_cluster_is_progressing_normally` |

`handleUpgrade` (fake client) — `dpfhcpprovisioner_phase_test::handle_upgrade::…`

| Go spec | Rust test |
|---|---|
| should recover when HC was updated but NP was not (partial failure) | `should_recover_when_hc_was_updated_but_np_was_not_partial_failure` |
| should NOT trigger upgrade recovery when in ClusterVersionProgressing phase (initial install) | `should_not_trigger_upgrade_recovery_when_in_cluster_version_progressing_phase_initial_install` |
| should recover and delete stale ignition ConfigMap during partial failure | `should_recover_and_delete_stale_ignition_config_map_during_partial_failure` |
| should detect upgrade and update both HC and NP | `should_detect_upgrade_and_update_both_hc_and_np` |
| should complete upgrade when version is ready | `should_complete_upgrade_when_version_is_ready` |
| should skip upgrade during Pending phase | `should_skip_upgrade_during_pending_phase` |
| should no-op when images match and version is ready | `should_no_op_when_images_match_and_version_is_ready` |
| should skip when HostedClusterRef is nil | `should_skip_when_hosted_cluster_ref_is_nil` |

## dpudeployment_watch_test.go (19) — `dpudeployment_watch_test::…`

| Go spec | Rust test |
|---|---|
| DPUDeployment Watch › dpuDeploymentToRequests › should map to the DPFHCPProvisioner referencing the DPUDeployment | `dpu_deployment_watch::dpu_deployment_to_requests::should_map_to_the_dpfhcp_provisioner_referencing_the_dpu_deployment` |
| … › should return empty when no provisioner references the DPUDeployment | `dpu_deployment_watch::dpu_deployment_to_requests::should_return_empty_when_no_provisioner_references_the_dpu_deployment` |
| … › should return empty when provisioner has nil DPUDeploymentRef | `dpu_deployment_watch::dpu_deployment_to_requests::should_return_empty_when_provisioner_has_nil_dpu_deployment_ref` |
| … › should not match when DPUDeployment namespace differs | `dpu_deployment_watch::dpu_deployment_to_requests::should_not_match_when_dpu_deployment_namespace_differs` |
| DPUDeployment Watch › handleDependencyChanges › should return false when IgnitionConfigured is not True | `dpu_deployment_watch::handle_dependency_changes::should_return_false_when_ignition_configured_is_not_true` |
| … › should return false when DPUDeploymentRef is nil | `dpu_deployment_watch::handle_dependency_changes::should_return_false_when_dpu_deployment_ref_is_nil` |
| … › should return false when Flavor matches ConfigMap annotation | `dpu_deployment_watch::handle_dependency_changes::should_return_false_when_flavor_matches_config_map_annotation` |
| … › should return true when Flavor changed | `dpu_deployment_watch::handle_dependency_changes::should_return_true_when_flavor_changed` |
| … › should return true when ConfigMap has nil annotations | `dpu_deployment_watch::handle_dependency_changes::should_return_true_when_config_map_has_nil_annotations` |
| … › should delete ConfigMap and set DependencyDeleted when DPUDeployment is not found | `dpu_deployment_watch::handle_dependency_changes::should_delete_config_map_and_set_dependency_deleted_when_dpu_deployment_is_not_found` |
| … › should return false when ConfigMap is not found | `dpu_deployment_watch::handle_dependency_changes::should_return_false_when_config_map_is_not_found` |
| DPUDeployment Watch › Phase computation with condition reasons › should transition to IgnitionGenerating (not Failed) when reason is DPUDeploymentChanged | `dpu_deployment_watch::phase_computation_with_condition_reasons::should_transition_to_ignition_generating_not_failed_when_reason_is_dpu_deployment_changed` |
| … › should set Failed phase for IgnitionGenerationFailed reason | `dpu_deployment_watch::phase_computation_with_condition_reasons::should_set_failed_phase_for_ignition_generation_failed_reason` |
| handleDependencyChanges dependency readiness › should wait instead of failing when DPUDeployment is not found | `handle_dependency_changes_dependency_readiness::should_wait_instead_of_failing_when_dpu_deployment_is_not_found` |
| … › should wait instead of retrying when recovering from IgnitionGenerationFailed due to missing DPUDeployment | `handle_dependency_changes_dependency_readiness::should_wait_instead_of_retrying_when_recovering_from_ignition_generation_failed_due_to_missing_dpu_deployment` |
| … › should not block reconciliation before ignition prerequisites are met | `handle_dependency_changes_dependency_readiness::should_not_block_reconciliation_before_ignition_prerequisites_are_met` |
| computeReadyCondition with DPUDeploymentChanged › should set Ready=False when IgnitionConfigured is False due to DPUDeploymentChanged | `compute_ready_condition_with_dpu_deployment_changed::should_set_ready_false_when_ignition_configured_is_false_due_to_dpu_deployment_changed` |
| DPUDeployment flavor change integration (envtest) › should detect flavor change and set IgnitionConfigured=False via controller reconcile | `dpu_deployment_flavor_change_integration_envtest::should_detect_flavor_change_and_set_ignition_configured_false_via_controller_reconcile` |
| DPUFlavor deletion integration (envtest) › should delete ConfigMap and set IgnitionConfigured=False when DPUFlavor is deleted | `dpu_flavor_deletion_integration_envtest::should_delete_config_map_and_set_ignition_configured_false_when_dpu_flavor_is_deleted` |

## dpuflavor_watch_test.go (7) — `dpuflavor_watch_test::dpu_flavor_watch::…`

| Go spec (DPUFlavor Watch › …) | Rust test |
|---|---|
| dpuFlavorToRequests › should map to the DPFHCPProvisioner when DPUDeployment references the flavor | `dpu_flavor_to_requests::should_map_to_the_dpfhcp_provisioner_when_dpu_deployment_references_the_flavor` |
| dpuFlavorToRequests › should return empty when no DPUDeployment references the flavor | `dpu_flavor_to_requests::should_return_empty_when_no_dpu_deployment_references_the_flavor` |
| dpuFlavorToRequests › should return empty when DPUDeployment references a different flavor | `dpu_flavor_to_requests::should_return_empty_when_dpu_deployment_references_a_different_flavor` |
| handleDependencyChanges with missing dependencies › should delete ConfigMap and set DependencyDeleted when DPUDeployment is deleted | `handle_dependency_changes_with_missing_dependencies::should_delete_config_map_and_set_dependency_deleted_when_dpu_deployment_is_deleted` |
| … › should delete ConfigMap and set DependencyDeleted when DPUFlavor is deleted | `handle_dependency_changes_with_missing_dependencies::should_delete_config_map_and_set_dependency_deleted_when_dpu_flavor_is_deleted` |
| … › should not trigger when DPUFlavor exists | `handle_dependency_changes_with_missing_dependencies::should_not_trigger_when_dpu_flavor_exists` |
| Phase computation with DependencyDeleted › should transition to IgnitionGenerating (not Failed) when reason is DependencyDeleted | `phase_computation_with_dependency_deleted::should_transition_to_ignition_generating_not_failed_when_reason_is_dependency_deleted` |

## hostedcluster_lifecycle_foundation_test.go (6, envtest) — `hostedcluster_lifecycle_foundation_test::hosted_cluster_lifecycle_foundation_secret_management::…`

| Go spec | Rust test |
|---|---|
| Finalizer Management › should add finalizer to DPFHCPProvisioner on creation | `finalizer_management::should_add_finalizer_to_dpfhcp_provisioner_on_creation` |
| Secret Copying to Same Namespace › should copy pull-secret with correct type and OwnerReference | `secret_copying_to_same_namespace::should_copy_pull_secret_with_correct_type_and_owner_reference` |
| Secret Copying to Same Namespace › should copy ssh-key with correct type and OwnerReference | `secret_copying_to_same_namespace::should_copy_ssh_key_with_correct_type_and_owner_reference` |
| ETCD Encryption Key Generation › should generate ETCD encryption key with correct format | `etcd_encryption_key_generation::should_generate_etcd_encryption_key_with_correct_format` |
| Idempotency › should not create duplicate secrets on multiple reconciliations | `idempotency::should_not_create_duplicate_secrets_on_multiple_reconciliations` |
| Error Handling › should handle missing user secret gracefully | `error_handling::should_handle_missing_user_secret_gracefully` |

## ignition_configmap_watch_test.go (20) — `ignition_configmap_watch_test::…`

| Go spec | Rust test |
|---|---|
| ConfigMapName › should return the expected name format | `config_map_name::should_return_the_expected_name_format` |
| ConfigMapName › should handle cluster names with special characters | `config_map_name::should_handle_cluster_names_with_special_characters` |
| Ignition ConfigMap Watch › ignitionConfigMapPredicate › should accept Delete events for bfcfg-template ConfigMaps | `ignition_config_map_watch::ignition_config_map_predicate::should_accept_delete_events_for_bfcfg_template_config_maps` |
| … › should reject Delete events for ConfigMaps with nil labels | `ignition_config_map_watch::ignition_config_map_predicate::should_reject_delete_events_for_config_maps_with_nil_labels` |
| … › should reject Delete events for non-bfcfg ConfigMaps | `ignition_config_map_watch::ignition_config_map_predicate::should_reject_delete_events_for_non_bfcfg_config_maps` |
| … › should reject Delete events when label value is not 'true' | `ignition_config_map_watch::ignition_config_map_predicate::should_reject_delete_events_when_label_value_is_not_true` |
| … › should reject Create events | `ignition_config_map_watch::ignition_config_map_predicate::should_reject_create_events` |
| … › should reject Update events | `ignition_config_map_watch::ignition_config_map_predicate::should_reject_update_events` |
| … › should reject Generic events | `ignition_config_map_watch::ignition_config_map_predicate::should_reject_generic_events` |
| Ignition ConfigMap Watch › ignitionConfigMapToRequests › should map to the DPFHCPProvisioner referencing the DPUCluster | `ignition_config_map_watch::ignition_config_map_to_requests::should_map_to_the_dpfhcp_provisioner_referencing_the_dpu_cluster` |
| … › should return empty when no matching provisioner exists | `ignition_config_map_watch::ignition_config_map_to_requests::should_return_empty_when_no_matching_provisioner_exists` |
| … › should return empty when annotations are missing | `ignition_config_map_watch::ignition_config_map_to_requests::should_return_empty_when_annotations_are_missing` |
| … › should return empty when only cluster name annotation is present | `ignition_config_map_watch::ignition_config_map_to_requests::should_return_empty_when_only_cluster_name_annotation_is_present` |
| … › should only match the provisioner with the correct DPUClusterRef | `ignition_config_map_watch::ignition_config_map_to_requests::should_only_match_the_provisioner_with_the_correct_dpu_cluster_ref` |
| … › should not match when cluster namespace differs | `ignition_config_map_watch::ignition_config_map_to_requests::should_not_match_when_cluster_namespace_differs` |
| Ignition ConfigMap Watch › verifyIgnitionConfigMap › should return false when no conditions exist | `ignition_config_map_watch::verify_ignition_config_map::should_return_false_when_no_conditions_exist` |
| … › should return false when IgnitionConfigured is False | `ignition_config_map_watch::verify_ignition_config_map::should_return_false_when_ignition_configured_is_false` |
| … › should return false when ConfigMap exists and ignition already reconciled | `ignition_config_map_watch::verify_ignition_config_map::should_return_false_when_config_map_exists_and_ignition_already_reconciled` |
| … › should clear IgnitionConfigured and return true when ConfigMap is missing | `ignition_config_map_watch::verify_ignition_config_map::should_clear_ignition_configured_and_return_true_when_config_map_is_missing` |
| … › should preserve ObservedGeneration from the CR when clearing condition | `ignition_config_map_watch::verify_ignition_config_map::should_preserve_observed_generation_from_the_cr_when_clearing_condition` |

`Eventually(recorder.Events).Should(Receive(ContainSubstring(...)))` → `FakeRecorder::try_recv`
(the event is recorded synchronously). `event.UpdateEvent{ObjectNew: cm}` → `pred.update(&default, &cm)`.

## Rust-only tests

* `nil_pointer_panic_test::generate_ignition_re_raises_ignition_generator_nil_pointer_dereference_as_panic`
  — a Go nil-pointer panic inside `GenerateIgnition` (DPFOperatorConfig without
  `spec.networking`, reached after a real HTTPS ignition download) is re-raised as a panic with
  Go's runtime message before any status update/event.
* `nil_pointer_panic_test::unset_dependency_panics_like_a_go_nil_pointer_dereference` — unset
  dependency fields behave like Go nil pointers (nil `MetalLBManager` still returns early when no
  LoadBalancer is needed).

## Golden differential tests — `tests/golden.rs`

Fixtures generated by `hack/golden/controller` (12 scenarios, 51 reconcile
steps, 333 recorded write calls). Each scenario runs the real Go reconciler with every
sub-feature wired like `cmd/main.go` on the controller-runtime fake client; the Rust test runs
the same driver steps and compares per step the result (`Requeue`, `RequeueAfter`), error,
write calls with normalized bodies, events, resulting status (conditions without
lastTransitionTime) and finalizers.

| Scenario | Covers |
|---|---|
| `lifecycle` | finalizer add, validations, BlueField lookup, secret copy, ETCD key, HC/NP creation, `setHostedClusterRefIfOwned`, status sync, kubeconfig injection, hugepages deferral (30s), ignition generation failure → Failed and retry, Ready, BFB annotation update, flavor change (DPUDeploymentChanged), upgrade detection/progress/completion, deletion with all cleanup handlers until the finalizer is removed |
| `no_config` | idle without the Config CR; CR not found |
| `dpucluster_missing` | DPUCluster missing → secrets missing → recovery |
| `invalid_release_image` | BlueField lookup with an invalid release image |
| `machine_os_url` | `LookupSkipped` condition; HC creation error (no nodes) |
| `metallb_highly_available` | MetalLB IPAddressPool/L2Advertisement, LoadBalancer HC, deletion incl. MetalLB cleanup |
| `metallb_disabled` | `DisableMetalLB` |
| `ready_restart` | operator-restart regeneration (`OperatorRestarted`) |
| `dependency_deleted` | DPUDeployment deleted / without flavor / flavor missing (`DependencyDeleted`) |
| `configmap_deleted` | ignition ConfigMap deleted (`ConfigMapDeleted`) and regeneration |
| `hc_blocked` | HostedCluster Blocked / Degraded → phase Error |
| `upgrade_recovery` | missed-upgrade recovery (HC already on target, NP stale) |
