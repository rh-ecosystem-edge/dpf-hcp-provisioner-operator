# ctrlrt-envtest — test mapping

`ctrlrt-envtest` ports `sigs.k8s.io/controller-runtime/pkg/envtest` (+ `pkg/internal/testing/...`).
The vendored Go package ships no tests of its own in this repository, so there is no
`Go test → Rust test` mapping to maintain. The Rust tests below exercise the port itself.

| Rust test | What it covers |
|---|---|
| `duration::tests::parses_go_durations` | `time.ParseDuration` for `KUBEBUILDER_CONTROLPLANE_*_TIMEOUT` (values and error messages) |
| `process::tests::arguments_render_sorted_with_defaults` | `process.Arguments.AsStrings` (Set/Append/Enable/Disable, sorted flags) |
| `process::tests::bin_path_finder_uses_asset_dir` | `process.BinPathFinder` fallbacks |
| `process::tests::help_flag_detection` | `State.CheckFlag` regexp (`(?m)^\s*--flag\b`) |
| `process::tests::watchdog_kills_tracked_processes_and_removes_dirs_on_eof` | orphan watchdog (process-exit cleanup) |
| `crd::tests::parse_crd_skips_non_crds` | `readCRDs` filter (kind / names.kind / group) |
| `crd::tests::missing_path_errors_like_go` | `ErrorIfPathMissing` error message |
| `crd::tests::reads_repo_crds` | multi-document YAML reading of `config/crd/bases` + `internal/controller/testdata/crds` |
| `tests/envtest.rs::installs_repo_crds_and_enforces_cel_validation` | full start, CRD install, namespace + `DPFHCPProvisionerConfig` "default" creation, CEL rejection of other names, kubeconfig, `AddUser` + RBAC, stop cleanup |
| `tests/envtest.rs::concurrent_environments_and_drop_cleanup` | two concurrent environments (distinct ports/dirs, isolated data), cleanup on drop |
| `tests/envtest.rs::missing_crd_path_is_an_error` | `Start()` error chain for a missing CRD path |
| `tests/envtest.rs::shared_environment_first_user` / `shared_environment_second_user` | `shared()` across tests/runtimes |
