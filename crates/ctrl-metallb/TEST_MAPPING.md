# ctrl-metallb — test mapping

Go package: `internal/controller/metallb` (external test package `metallb_test`).

`suite_test.go` (`TestMetalLB` → `RunSpecs(t, "MetalLB Suite")`) is only the Ginkgo bootstrap; it
has no Rust counterpart because each spec is a plain `#[tokio::test]`. Ginkgo's `BeforeEach`
(scheme + `record.NewFakeRecorder(100)` + default `OperatorConfig`) is `before_each()` in
`tests/metallb_test.rs`. `Describe`/`Context` → nested modules, `It` → test function.
`Eventually(recorder.Events).Should(Receive(ContainSubstring(s)))` →
`Suite::eventually_receive_containing(s)` (pops events until one matches);
`Consistently(recorder.Events).ShouldNot(Receive())` → `recorder.try_recv() == None`.

All 18 Go specs are ported (file `tests/metallb_test.rs`, prefix `metal_lb_manager::`):

| Go spec (`MetalLB Manager` › …) | Rust test |
|---|---|
| ConfigureMetalLB › when ShouldExposeThroughLoadBalancer is false › should skip MetalLB configuration | `configure_metal_lb::when_should_expose_through_load_balancer_is_false::should_skip_metal_lb_configuration` |
| ConfigureMetalLB › when MetalLB resources do not exist › should create IPAddressPool and L2Advertisement | `configure_metal_lb::when_metal_lb_resources_do_not_exist::should_create_ip_address_pool_and_l2_advertisement` |
| ConfigureMetalLB › when MetalLB resources exist with correct spec › should not update resources | `configure_metal_lb::when_metal_lb_resources_exist_with_correct_spec::should_not_update_resources` |
| ConfigureMetalLB › when MetalLB resources exist with incorrect spec (drift) › should update IPAddressPool to correct drift | `configure_metal_lb::when_metal_lb_resources_exist_with_incorrect_spec_drift::should_update_ip_address_pool_to_correct_drift` |
| ConfigureMetalLB › when MetalLB resources exist with incorrect spec (drift) › should update L2Advertisement to correct drift | `configure_metal_lb::when_metal_lb_resources_exist_with_incorrect_spec_drift::should_update_l2_advertisement_to_correct_drift` |
| ConfigureMetalLB › when DisableMetalLB is true in operator config › should skip MetalLB configuration and set condition | `configure_metal_lb::when_disable_metal_lb_is_true_in_operator_config::should_skip_metal_lb_configuration_and_set_condition` |
| Cleanup Handler › when both MetalLB resources exist › should delete IPAddressPool and L2Advertisement | `cleanup_handler::when_both_metal_lb_resources_exist::should_delete_ip_address_pool_and_l2_advertisement` |
| Cleanup Handler › when MetalLB resources don't exist › should return nil (idempotent) | `cleanup_handler::when_metal_lb_resources_dont_exist::should_return_nil_idempotent` |
| Cleanup Handler › when only IPAddressPool exists › should delete IPAddressPool and return success | `cleanup_handler::when_only_ip_address_pool_exists::should_delete_ip_address_pool_and_return_success` |
| Cleanup Handler › when only L2Advertisement exists › should delete L2Advertisement and return success | `cleanup_handler::when_only_l2_advertisement_exists::should_delete_l2_advertisement_and_return_success` |
| Cleanup Handler › when cleanup is called multiple times (idempotency) › should succeed on subsequent calls | `cleanup_handler::when_cleanup_is_called_multiple_times_idempotency::should_succeed_on_subsequent_calls` |
| Helper Functions › buildIPAddressPool › should build correct IPAddressPool spec | `helper_functions::build_ip_address_pool::should_build_correct_ip_address_pool_spec` |
| Helper Functions › buildL2Advertisement › should build correct L2Advertisement spec | `helper_functions::build_l2_advertisement::should_build_correct_l2_advertisement_spec` |
| Edge Cases › when DPFHCPProvisioner has SingleReplica with VirtualIP › should configure MetalLB | `edge_cases::when_dpfhcp_provisioner_has_single_replica_with_virtual_ip::should_configure_metal_lb` |
| Edge Cases › when IPAddressPool exists without ownership labels (ownership conflict) › should return error and not take ownership | `edge_cases::when_ip_address_pool_exists_without_ownership_labels_ownership_conflict::should_return_error_and_not_take_ownership` |
| Edge Cases › when L2Advertisement exists without ownership labels (ownership conflict) › should return error and not take ownership | `edge_cases::when_l2_advertisement_exists_without_ownership_labels_ownership_conflict::should_return_error_and_not_take_ownership` |
| Status Condition Management › when MetalLB configuration succeeds › should set MetalLBConfigured condition to True with correct reason | `status_condition_management::when_metal_lb_configuration_succeeds::should_set_metal_lb_configured_condition_to_true_with_correct_reason` |
| Status Condition Management › when condition changes › should emit event only when condition actually changes | `status_condition_management::when_condition_changes::should_emit_event_only_when_condition_actually_changes` |

Extra Rust-only tests:

- `tests/metallb_test.rs::finalizer_cleanup_handler_trait_impl` — the `ctrl_finalizer::CleanupHandler`
  trait impl delegates to the inherent methods.
- `tests/golden.rs` — 13 golden differential scenarios against the Go code (fixtures from
  `go run -mod=vendor ./hack/golden/metallb crates/ctrl-metallb/tests/fixtures`): `configure_create`
  (2 runs: create, then no-op), `configure_single_replica_vip`, `configure_skip`,
  `configure_disabled` (2 runs), `configure_drift` (pool + advert drift incl. extra
  labels/annotations/spec fields), `configure_pool_not_owned`, `configure_advert_not_owned`,
  `configure_pool_crd_missing` / `configure_advert_crd_missing` (`NoKindMatchError`),
  `cleanup_both` (3 runs), `cleanup_none`, `cleanup_not_owned`, `cleanup_crd_missing`. Each compares
  per-step result / error string / events and the exact `encoding/json` bytes of the
  IPAddressPool, L2Advertisement and DPFHCPProvisioner (stored and in-memory). Only condition
  `lastTransitionTime` is normalized (to `2025-01-01T00:00:00Z`).
