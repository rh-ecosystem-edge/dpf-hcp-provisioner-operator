# ctrl-ovshugepages — Go test → Rust test mapping

Go package: `internal/controller/ovs-hugepages` (Ginkgo suite `TestHugepages`, "Hugepages Suite",
`suite_test.go`). The Ginkgo specs are in-package (they call unexported functions), so the Rust
ports are unit tests in `src/ovs_hugepages_test.rs`; the suite runner itself has no Rust
counterpart (each spec is a `#[test]`/`#[tokio::test]`).

## ovs-hugepages_test.go

| Go spec | Rust test (`src/ovs_hugepages_test.rs`) |
|---|---|
| buildDaemonSet / reserves the configured hugepages amount on every node with no nodeSelector | `build_daemon_set_reserves_the_configured_hugepages_amount_on_every_node_with_no_node_selector` |
| buildDaemonSet / computes amount x size for a custom size and page count | `build_daemon_set_computes_amount_x_size_for_a_custom_size_and_page_count` |
| ensureNamespace / creates the reservation namespace when missing and is idempotent | `ensure_namespace_creates_the_reservation_namespace_when_missing_and_is_idempotent` |
| ensureDaemonSet / creates the DaemonSet on first call | `ensure_daemon_set_creates_the_daemon_set_on_first_call` |
| ensureDaemonSet / does not modify the DaemonSet on an unchanged subsequent call | `ensure_daemon_set_does_not_modify_the_daemon_set_on_an_unchanged_subsequent_call` |
| ensureDaemonSet / updates the DaemonSet when the reservation amount changes | `ensure_daemon_set_updates_the_daemon_set_when_the_reservation_amount_changes` |
| ensureDaemonSet / deletes the DaemonSet when amount is zero | `ensure_daemon_set_deletes_the_daemon_set_when_amount_is_zero` |
| ensureDaemonSet / is a no-op when amount is zero and nothing exists | `ensure_daemon_set_is_a_no_op_when_amount_is_zero_and_nothing_exists` |
| resolvePauseImage / resolves the pause image from the aarch64 release payload and caches it | `resolve_pause_image_resolves_the_pause_image_from_the_aarch64_release_payload_and_caches_it` |
| resolvePauseImage / resolves the pause image directly from a multi-arch release | `resolve_pause_image_resolves_the_pause_image_directly_from_a_multi_arch_release` |
| resolvePauseImage / returns an error when the aarch64 release lookup fails | `resolve_pause_image_returns_an_error_when_the_aarch64_release_lookup_fails` |

Fixtures are identical (`testPauseImage`, `newTestManager` pull secret `{"auths":{}}`,
`newTestCR` release `quay.io/openshift-release-dev/ocp-release:4.17.6-x86_64`). As in Go, the
two aarch64 specs make `IsMultiArchReleaseImage` HEAD quay.io (any failure, e.g. no network,
yields "not multi-arch"). `Quantity.Equal` is compared via the canonical quantity string.

## Additional Rust tests (no Go counterpart)

| Rust test | Purpose |
|---|---|
| `tests/golden.rs` — 24 scenario tests + `every_fixture_is_tested` | Golden differential tests of `Manager::reconcile_hugepages_daemon_set` against Go (`hack/golden/ovshugepages`): errors, stale-client invalidation, reader calls, V-level log records, byte-exact `encoding/json` of the hosted Namespace/DaemonSet after every step |
| `src/quantity_golden_test.rs::hugepages_quantity_matches_go` | `hugepagesQuantity(size, amount)` vs Go for 38 sizes x 7 amounts (incl. `MustParse` panic messages) |
| `src/quantity.rs::tests::{values, binary_si_strings}` | `resource.MustParse(..).Value()` / `NewQuantity(v, BinarySI).String()` spot checks |
