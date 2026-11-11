# Go test → Rust test mapping (`ctrlrt-cmd`)

> **Golden fixtures are frozen.** The fixtures referenced below were generated from the former Go
> implementation by the generators under `hack/golden/`, which were removed together with the Go
> code (still in the git history, commit `fead352`). They can no longer be regenerated; see
> `docs/rust-port/CONVENTIONS.md` (rule 6).

`cmd/` has no Go tests. This crate ports behaviour of the Go standard library `flag` package and
vendored controller-runtime / client-go / apiserver code, whose upstream tests are not part of
this repository. Coverage is provided by golden differential tests against the Go
implementation plus unit tests modelled on the upstream tests.

## Golden differential tests

| Go reference (generator) | Fixtures | Rust test |
|---|---|---|
| real `cmd/main.go` binary: `-h`, flag errors, exit codes (`hack/golden/ctrlrt-cmd-flags/gen.sh`) | `tests/fixtures/flags/*` | `tests/flags_golden.rs`: `manager_flags_match_go_output`, `exit_codes_and_stderr_match_go` |
| controller-runtime `zap.New(zap.UseFlagOptions(&opts))` with a fixed clock, 31 flag/encoder/level/time-encoding scenarios (`hack/golden/zaplog`) | `tests/fixtures/zaplog/*` | `tests/zaplog_golden.rs`: `zap_output_matches_go_byte_for_byte`, `zap_stacktraces_have_go_shape` |
| client-go `record.EventCorrelator` with a fake clock: dedup, aggregation, spam filter, window reset (`hack/golden/ctrlrt-cmd-events`) | `tests/fixtures/events/correlator.json` | `tests/events.rs`: `correlator_matches_client_go` |

Generated (before the Go code was removed) with:

```sh
hack/golden/ctrlrt-cmd-flags/gen.sh
go run -mod=vendor ./hack/golden/zaplog crates/ctrlrt-cmd/tests/fixtures/zaplog
go run -mod=vendor ./hack/golden/ctrlrt-cmd-events > crates/ctrlrt-cmd/tests/fixtures/events/correlator.json
```

## Upstream-modelled unit tests

| Go upstream test | Rust test |
|---|---|
| `flag/flag_test.go` TestParse / TestPrintDefaults / help handling | `flag::tests::{parse_basic, terminator_and_stop, errors, help, print_defaults, parse_int_like_go}` |
| `time/time_test.go` durationTests / parseDurationTests / parseDurationErrorTests | `godur::tests::{formats_like_go, parses_like_go}` |
| `crypto/tls` X509KeyPair error cases | `tlsutil::tests::{key_pair_ok_and_errors, read_file_errors_like_go}` |
| controller-runtime `certwatcher_test.go` (New error, reload on change, watch errors) | `certwatcher::tests::{new_errors_like_go, reloads_on_change, start_fails_when_files_vanish, aggregate_formats}` |
| controller-runtime metrics server / `filters` behaviour (401/403/500 responses, SAR attributes, caching, HTTP/2 toggle, self-signed cert, `0` disables) | `tests/metrics_server.rs` |
| client-go `record/event_test.go` (create, patch on duplicate, 404 → create, retry/no-retry, shutdown) | `tests/events.rs`: `records_creates_and_patches_events`, `patch_not_found_falls_back_to_create`, `retries_transport_errors_but_not_api_errors`, `stop_drops_later_events`, `cluster_scoped_objects_use_default_namespace_and_invalid_types_are_dropped` |
| `events_cache_test.go` token bucket / LRU | `events::tests::{token_bucket, lru_evicts_oldest, event_names}` |
