# ctrl-finalizer — Go test → Rust test

Go package `internal/controller/finalizer` (handler.go, manager.go) has **no Go tests**.

Rust-only tests (`src/manager_test.rs`) covering the documented `HandleFinalizerCleanup` behaviour:

| Rust test | Covers |
|---|---|
| `manager_test::handlers_run_in_registration_order_and_emit_cleanup_succeeded` | registration order, `Normal CleanupSucceeded ...` event |
| `manager_test::handler_error_stops_cleanup_and_emits_warning` | error short-circuit, `Warning CleanupHandlerFailed Cleanup handler '<name>' failed: <err>` |
| `manager_test::handler_requeue_after_stops_cleanup_without_event` | `RequeueAfter > 0` short-circuit, no event |
| `manager_test::no_handlers_completes_immediately` | empty handler list |

The manager is additionally covered by the Go-differential golden tests in
`crates/ctrl-hostedcluster/tests/golden.rs` (`golden_finalizer_manager_complete`,
`golden_finalizer_manager_in_progress`), which run Go's `finalizer.Manager` with the
hostedcluster `CleanupHandler` and compare results, events and API calls.
