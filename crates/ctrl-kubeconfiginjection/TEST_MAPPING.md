# ctrl-kubeconfiginjection — test mapping

> **Golden fixtures are frozen.** The fixtures referenced below were generated from the former Go
> implementation by the generators under `hack/golden/`, which were removed together with the Go
> code (still in the git history, commit `fead352`). They can no longer be regenerated; see
> `docs/rust-port/CONVENTIONS.md` (rule 6).

Go package: `internal/controller/kubeconfiginjection`. `injector_test.go` and `watch_test.go` are
white-box tests (package `kubeconfiginjection`), so they are ported as unit-test modules
`src/injector_test.rs` and `src/watch_test.rs` (with access to the crate-private
`is_hosted_cluster_kubeconfig_secret`). `suite_test.go` (`TestKubeconfigInjection` →
`RunSpecs(t, "Kubeconfig Injection Suite")`) is only the Ginkgo bootstrap; each spec is a plain
`#[tokio::test]`/`#[test]`. `BeforeEach` → `before_each()`; `Describe` → nested module, `It` →
test function. The Go helper `findCondition` → `find_condition`.
`Eventually(recorder.Events).Should(Receive(ContainSubstring(s)))` →
`Suite::eventually_receive_containing(s)`.

All 23 Go specs are ported.

## injector_test.go (7) — `injector_test::kubeconfig_injection_reconciler::…`

| Go spec (`Kubeconfig Injection Reconciler` › …) | Rust test |
|---|---|
| Happy Path - Full Injection Flow › should successfully inject kubeconfig when all prerequisites are met | `happy_path_full_injection_flow::should_successfully_inject_kubeconfig_when_all_prerequisites_are_met` |
| Secret Not Ready Scenario › should set condition to pending when HC kubeconfig secret doesn't exist | `secret_not_ready_scenario::should_set_condition_to_pending_when_hc_kubeconfig_secret_doesnt_exist` |
| Idempotency - Scenario A: Drift Detection › should detect and correct content drift between source and destination secrets | `idempotency_scenario_a_drift_detection::should_detect_and_correct_content_drift_between_source_and_destination_secrets` |
| Idempotency - Scenario B: Secret Exists, DPUCluster Not Updated › should update DPUCluster without recreating secret | `idempotency_scenario_b_secret_exists_dpu_cluster_not_updated::should_update_dpu_cluster_without_recreating_secret` |
| Idempotency - Scenario C: Secret Missing, DPUCluster Updated › should recreate secret without modifying DPUCluster | `idempotency_scenario_c_secret_missing_dpu_cluster_updated::should_recreate_secret_without_modifying_dpu_cluster` |
| Skip When DPUCluster Not Found › should skip injection without error when DPUCluster is missing | `skip_when_dpu_cluster_not_found::should_skip_injection_without_error_when_dpu_cluster_is_missing` |
| Skip When HostedCluster Not Created › should skip injection when HostedClusterRef is nil | `skip_when_hosted_cluster_not_created::should_skip_injection_when_hosted_cluster_ref_is_nil` |

## watch_test.go (16) — `watch_test::watch_functions::…`

| Go spec (`Watch Functions` › …) | Rust test |
|---|---|
| isHostedClusterKubeconfigSecret › should return true for valid HC kubeconfig secret | `is_hosted_cluster_kubeconfig_secret::should_return_true_for_valid_hc_kubeconfig_secret` |
| isHostedClusterKubeconfigSecret › should return false for non-kubeconfig secret | `is_hosted_cluster_kubeconfig_secret::should_return_false_for_non_kubeconfig_secret` |
| isHostedClusterKubeconfigSecret › should return false for secret with partial match | `is_hosted_cluster_kubeconfig_secret::should_return_false_for_secret_with_partial_match` |
| isHostedClusterKubeconfigSecret › should return false for non-secret object | `is_hosted_cluster_kubeconfig_secret::should_return_false_for_non_secret_object` |
| IsHostedClusterKubeconfigSecretPredicate › should accept Create event with valid kubeconfig secret | `is_hosted_cluster_kubeconfig_secret_predicate::should_accept_create_event_with_valid_kubeconfig_secret` |
| IsHostedClusterKubeconfigSecretPredicate › should reject Create event with non-kubeconfig secret | `is_hosted_cluster_kubeconfig_secret_predicate::should_reject_create_event_with_non_kubeconfig_secret` |
| IsHostedClusterKubeconfigSecretPredicate › should accept Update event with valid kubeconfig secret | `is_hosted_cluster_kubeconfig_secret_predicate::should_accept_update_event_with_valid_kubeconfig_secret` (Go's nil `ObjectOld` → `Secret::default()`) |
| IsHostedClusterKubeconfigSecretPredicate › should accept Delete event with valid kubeconfig secret | `is_hosted_cluster_kubeconfig_secret_predicate::should_accept_delete_event_with_valid_kubeconfig_secret` |
| IsHostedClusterKubeconfigSecretPredicate › should reject Generic event | `is_hosted_cluster_kubeconfig_secret_predicate::should_reject_generic_event` |
| FindProvisionerForKubeconfigSecret › should return reconcile request when provisioner exists | `find_provisioner_for_kubeconfig_secret::should_return_reconcile_request_when_provisioner_exists` |
| FindProvisionerForKubeconfigSecret › should return empty when provisioner not found | `find_provisioner_for_kubeconfig_secret::should_return_empty_when_provisioner_not_found` |
| FindProvisionerForKubeconfigSecret › should return empty when provisioner exists but in different namespace | `find_provisioner_for_kubeconfig_secret::should_return_empty_when_provisioner_exists_but_in_different_namespace` |
| FindProvisionerForKubeconfigSecret › should handle name extraction correctly | `find_provisioner_for_kubeconfig_secret::should_handle_name_extraction_correctly` |
| FindProvisionerForKubeconfigSecret › should return empty for invalid object type | `find_provisioner_for_kubeconfig_secret::should_return_empty_for_invalid_object_type` |
| Watch Integration › should trigger reconciliation when HC kubeconfig secret is created | `watch_integration::should_trigger_reconciliation_when_hc_kubeconfig_secret_is_created` |
| Watch Integration › should trigger reconciliation when HC kubeconfig secret is updated (drift) | `watch_integration::should_trigger_reconciliation_when_hc_kubeconfig_secret_is_updated_drift` |

`cleanup_handler.go` has no Go tests; it is covered by the golden scenarios below.

## Extra Rust-only tests — `tests/golden.rs`

Golden differential scenarios against the Go code (fixtures from
`hack/golden/kubeconfiginjection`):
`inject_happy` (2 runs), `inject_drift`, `inject_no_drift`, `inject_scenario_b`,
`inject_scenario_c`, `inject_dpucluster_missing` (2 runs), `inject_secret_not_ready`,
`inject_hc_not_created`, `inject_source_missing_key`, `inject_dest_missing_key`,
`inject_other_kubeconfig`, `cleanup_secrets` (2 runs), `cleanup_none`, `map_requests`
(map-function results for 5 objects incl. a ConfigMap). Each compares per-step result / error
string / events and the exact `encoding/json` bytes of the DPFHCPProvisioner (stored and
in-memory), DPUCluster and the source/destination Secrets (k8s-openapi `Secret` re-encoded in Go
field order by the test helper `go_secret`). Only condition `lastTransitionTime` is normalized.
Plus `finalizer_cleanup_handler_trait_impl` (the `ctrl_finalizer::CleanupHandler` impl delegates
to the inherent methods).
