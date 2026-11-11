# ctrl-dpucluster — Go test → Rust test

Go: `internal/controller/dpucluster/validator_test.go` → Rust: `src/validator_test.rs`.
`TestDPUClusterValidator` only bootstraps Ginkgo (`RunSpecs`) — no Rust counterpart needed.
`forbiddenClient` → a `ctrlrt-fake` reactor on `get dpuclusters` returning
`ApiError::forbidden("provisioning.dpu.nvidia.com", "dpuclusters", name, "<nil>")`.
Ginkgo `BeforeEach` → `before_each()`. `Eventually(recorder.Events).Should(Receive(ContainSubstring(s)))`
→ `receive_containing` (receives until a match); `Consistently(...).ShouldNot(Receive())` →
`recorder.try_recv().is_none()`. The "drain all events looking for the exact InUse event"
`Eventually` loop → `recorder.drain()` + exact-string `any`.

All Go specs are under `Describe("DPUCluster Validator") / Describe("ValidateDPUCluster")`; the
Rust names start with `validate_dpu_cluster_`.

| Go spec (Context / It) | Rust test |
|---|---|
| when DPUCluster exists / should set DPUClusterMissing=False | `validate_dpu_cluster_when_dpu_cluster_exists_should_set_dpu_cluster_missing_false` |
| when DPUCluster exists / should emit event only on first success (not on subsequent reconciliations) | `validate_dpu_cluster_when_dpu_cluster_exists_should_emit_event_only_on_first_success_not_on_subsequent_reconciliations` |
| when DPUCluster does not exist / should set DPUClusterMissing=True and not requeue | `validate_dpu_cluster_when_dpu_cluster_does_not_exist_should_set_dpu_cluster_missing_true_and_not_requeue` |
| when DPUCluster does not exist / should use 'Deleted' reason when DPUCluster was previously found | `validate_dpu_cluster_when_dpu_cluster_does_not_exist_should_use_deleted_reason_when_dpu_cluster_was_previously_found` |
| when RBAC permission is denied / should set DPUClusterMissing=True with AccessDenied reason and not requeue | `validate_dpu_cluster_when_rbac_permission_is_denied_should_set_dpu_cluster_missing_true_with_access_denied_reason_and_not_requeue` (also asserts the exact condition message) |
| when validation runs in different phases / should validate in Pending phase | `validate_dpu_cluster_when_validation_runs_in_different_phases_should_validate_in_pending_phase` |
| when validation runs in different phases / should validate in Ready phase (proves no phase gating) | `validate_dpu_cluster_when_validation_runs_in_different_phases_should_validate_in_ready_phase_proves_no_phase_gating` |
| when validation runs in different phases / should detect DPUCluster deletion when provisioner is Ready | `validate_dpu_cluster_when_validation_runs_in_different_phases_should_detect_dpu_cluster_deletion_when_provisioner_is_ready` |
| when clusterType validation is performed / should set ClusterTypeValid=True when type is not kamaji | `validate_dpu_cluster_when_cluster_type_validation_is_performed_should_set_cluster_type_valid_true_when_type_is_not_kamaji` |
| when clusterType validation is performed / should set ClusterTypeValid=False when type is kamaji | `validate_dpu_cluster_when_cluster_type_validation_is_performed_should_set_cluster_type_valid_false_when_type_is_kamaji` |
| when clusterType validation is performed / should recover when DPUCluster type changes from kamaji to supported type | `validate_dpu_cluster_when_cluster_type_validation_is_performed_should_recover_when_dpu_cluster_type_changes_from_kamaji_to_supported_type` |
| when DPUCluster exclusivity validation is performed / should set DPUClusterInUse=False when DPUCluster is available | `validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_set_dpu_cluster_in_use_false_when_dpu_cluster_is_available` |
| when DPUCluster exclusivity validation is performed / should set DPUClusterInUse=True when DPUCluster is already in use by another provisioner | `validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_set_dpu_cluster_in_use_true_when_dpu_cluster_is_already_in_use_by_another_provisioner` |
| when DPUCluster exclusivity validation is performed / should allow same provisioner to validate multiple times (skip self) | `validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_allow_same_provisioner_to_validate_multiple_times_skip_self` |
| when DPUCluster exclusivity validation is performed / should recover when conflicting provisioner is deleted | `validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_recover_when_conflicting_provisioner_is_deleted` |
| when DPUCluster exclusivity validation is performed / should handle provisioners in different namespaces referencing same DPUCluster | `validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_handle_provisioners_in_different_namespaces_referencing_same_dpu_cluster` |
| when DPUCluster exclusivity validation is performed / should not emit event on subsequent validations with same result | `validate_dpu_cluster_when_dpu_cluster_exclusivity_validation_is_performed_should_not_emit_event_on_subsequent_validations_with_same_result` |

17 Go specs → 17 Rust tests.
