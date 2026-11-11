# ctrlrt-runtime — test mapping

This crate ports vendored library code (client-go / controller-runtime), not operator code, so
the mapping lists the upstream tests whose semantics were ported.

| Upstream test | Rust test |
|---|---|
| client-go `workqueue/queue_test.go` TestBasic | `workqueue::tests::test_basic` |
| client-go TestAddWhileProcessing | `workqueue::tests::test_add_while_processing` |
| client-go TestLen | `workqueue::tests::test_len` |
| client-go TestReinsert | `workqueue::tests::test_reinsert` |
| client-go TestQueueDrainageUsingShutDownWithDrain | `workqueue::tests::test_queue_drainage_using_shut_down_with_drain` |
| client-go TestForceQueueShutdownUsingShutDown | `workqueue::tests::test_force_queue_shutdown_using_shut_down` |
| client-go `delaying_queue_test.go` TestSimpleQueue | `workqueue::tests::test_simple_delaying_queue` |
| client-go TestDeduping | `workqueue::tests::test_deduping` |
| client-go TestAddTwoFireEarly | `workqueue::tests::test_add_two_fire_early` |
| client-go TestCopyShifting | `workqueue::tests::test_copy_shifting` |
| client-go `rate_limiting_queue_test.go` TestRateLimitingQueue | `workqueue::tests::test_rate_limiting_queue` |
| client-go `default_rate_limiters_test.go` TestItemExponentialFailureRateLimiter | `ratelimit::tests::test_item_exponential_failure_rate_limiter` |
| client-go TestItemExponentialFailureRateLimiterOverFlow | `ratelimit::tests::test_item_exponential_failure_rate_limiter_overflow` |
| client-go TestItemFastSlowRateLimiter | `ratelimit::tests::test_item_fast_slow_rate_limiter` |
| client-go TestMaxOfRateLimiter | `ratelimit::tests::test_max_of_rate_limiter` |
| x/time/rate Reserve/Delay semantics | `ratelimit::tests::token_bucket_burst_then_rate`, `default_controller_rate_limiter_is_max_of_both` |
| controller-runtime `internal/controller` "should requeue a Request if there is an error" | `controller::tests::reconcile_error_requeues_rate_limited` |
| "should not requeue if the error is a terminal error" | `controller::tests::terminal_error_is_not_requeued` |
| "should requeue a Request after a duration ... RequeueAfter" | `controller::tests::requeue_after_forgets_and_delays` |
| "should requeue a Request if the Result sets Requeue:true" | `controller::tests::requeue_true_is_rate_limited` |
| "should forget an item if it is successfully reconciled" | `controller::tests::success_forgets` |
| "should recover when reconciler panics" | `controller::tests::panic_is_recovered_as_error` |
| `controller/name_test` duplicate names | `controller::tests::duplicate_names_are_rejected` |
| controller-runtime `healthz_test.go` (all specs) | `healthz::tests::*` |
| client-go shared informer / DeltaFIFO event semantics | `informer::tests::initial_list_watch_relist_and_late_subscriber` |
| controller-runtime manager runnable groups | `manager::tests::runnable_group_starts_queued_and_late_runnables` |
| client-go LeaderElector validation | `leaderelection::tests::elector_validation` |
| envtest: client CRUD, errors, lazy RESTMapper | `tests/integration.rs::direct_client_crud_errors_and_rest_mapper` |
| envtest: manager + ByObject cache + controller + LE + probes + metrics + shutdown | `tests/integration.rs::manager_end_to_end` |
| envtest: lost leadership / standby | `tests/integration.rs::leader_election_lost_stops_manager_and_standby_is_not_elected` |
| hosted cluster client from kubeconfig bytes | `tests/integration.rs::kubeconfig_bytes_client`, `config::tests::kubeconfig_bytes_with_server_override` |
