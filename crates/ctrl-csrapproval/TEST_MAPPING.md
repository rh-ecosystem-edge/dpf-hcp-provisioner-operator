# internal/controller/csrapproval test mapping

Ginkgo suite "CSR Approval Suite". `suite_test.go` (`TestCSRApproval`, `BeforeSuite`,
`createFakeNode`) has no specs of its own; its bootstrap is ported in `src/test_util.rs`
(`scheme()`, `k8s_client()` = controller-runtime fake client with the DPU types,
`fake_clientset()` = `kubefake.NewSimpleClientset()` wrapped in a `HostedClient`,
`create_fake_node`). The CSR generators of `hostname_test.go` (`createTestCSR`,
`createTestCSRWithOrganization`, `createServingCSRWithDNS`) are in the same file and use
`rcgen` (ECDSA P-256 instead of RSA-2048; key type does not affect validation).

Tests live in-crate (`src/*_test.rs`, `#[cfg(test)]`) because the Go specs call unexported
functions (`getDPU`, `setCondition`). Rust test paths mirror the spec path
(Describe/Context → modules, It → fn). Go shares one fake client/clientset across the suite
(cleaned with `DeferCleanup`); the Rust specs build fresh fakes per spec.

## hostname_test.go — Describe "Hostname Extraction" (15 specs)

| Go spec | Rust test (`src/hostname_test.rs`, prefix `hostname_extraction::`) |
|---|---|
| ExtractHostname from Bootstrap CSR / When CSR has valid CN with system:node: prefix / should extract hostname correctly | `extract_hostname_from_bootstrap_csr::when_csr_has_valid_cn_with_system_node_prefix::should_extract_hostname_correctly` |
| ExtractHostname from Bootstrap CSR / When CSR has invalid CN format / should return error | `extract_hostname_from_bootstrap_csr::when_csr_has_invalid_cn_format::should_return_error` |
| ExtractHostname from Bootstrap CSR / When CSR has empty CN after prefix removal / should return error | `extract_hostname_from_bootstrap_csr::when_csr_has_empty_cn_after_prefix_removal::should_return_error` |
| ExtractHostname from Serving CSR / When CSR has valid username with system:node: prefix / should extract hostname correctly | `extract_hostname_from_serving_csr::when_csr_has_valid_username_with_system_node_prefix::should_extract_hostname_correctly` |
| ExtractHostname from Serving CSR / When CSR has invalid username format / should return error | `extract_hostname_from_serving_csr::when_csr_has_invalid_username_format::should_return_error` |
| ExtractHostname from Serving CSR / When CSR has empty username after prefix removal / should return error | `extract_hostname_from_serving_csr::when_csr_has_empty_username_after_prefix_removal::should_return_error` |
| ExtractHostname with unsupported signer / When CSR has unsupported signer name / should return error | `extract_hostname_with_unsupported_signer::when_csr_has_unsupported_signer_name::should_return_error` |
| ExtractHostname with unsupported signer / When CSR is nil / should return error | `extract_hostname_with_unsupported_signer::when_csr_is_nil::should_return_error` |
| IsPending / When CSR has no conditions / should return true | `is_pending_::when_csr_has_no_conditions::should_return_true` |
| IsPending / When CSR has Approved condition / should return false | `is_pending_::when_csr_has_approved_condition::should_return_false` |
| IsPending / When CSR has Denied condition / should return false | `is_pending_::when_csr_has_denied_condition::should_return_false` |
| IsPending / When CSR is nil / should return false | `is_pending_::when_csr_is_nil::should_return_false` |
| FilterBySignerName / When list contains mixed signer types / should filter to only bootstrap and serving CSRs | `filter_by_signer_name_::when_list_contains_mixed_signer_types::should_filter_to_only_bootstrap_and_serving_csrs` |
| FilterBySignerName / When list contains no matching signers / should return empty list | `filter_by_signer_name_::when_list_contains_no_matching_signers::should_return_empty_list` |
| FilterBySignerName / When list is empty / should return empty list | `filter_by_signer_name_::when_list_is_empty::should_return_empty_list` |

## csr_content_validation_test.go — Describe "CSR Validation" (10 specs)

| Go spec | Rust test (`src/csr_content_validation_test.rs`, prefix `csr_validation::`) |
|---|---|
| ValidateBootstrapCSR / When CSR has valid bootstrap configuration / should pass validation | `validate_bootstrap_csr_::when_csr_has_valid_bootstrap_configuration::should_pass_validation` |
| ValidateBootstrapCSR / When CSR has invalid username / should fail validation | `validate_bootstrap_csr_::when_csr_has_invalid_username::should_fail_validation` |
| ValidateBootstrapCSR / When CSR is missing required groups / should fail validation | `validate_bootstrap_csr_::when_csr_is_missing_required_groups::should_fail_validation` |
| ValidateBootstrapCSR / When CSR is missing required usages / should fail validation | `validate_bootstrap_csr_::when_csr_is_missing_required_usages::should_fail_validation` |
| ValidateBootstrapCSR / When CSR has invalid organization / should fail validation | `validate_bootstrap_csr_::when_csr_has_invalid_organization::should_fail_validation` |
| ValidateServingCSR / When CSR has valid serving configuration / should pass validation | `validate_serving_csr_::when_csr_has_valid_serving_configuration::should_pass_validation` |
| ValidateServingCSR / When CSR has invalid username / should fail validation | `validate_serving_csr_::when_csr_has_invalid_username::should_fail_validation` |
| ValidateServingCSR / When CSR is missing system:nodes group / should fail validation | `validate_serving_csr_::when_csr_is_missing_system_nodes_group::should_fail_validation` |
| ValidateServingCSR / When CSR is missing required usages / should fail validation | `validate_serving_csr_::when_csr_is_missing_required_usages::should_fail_validation` |
| ValidateServingCSR / When CSR has wrong hostname in DNS SAN / should fail validation | `validate_serving_csr_::when_csr_has_wrong_hostname_in_dns_san::should_fail_validation` |

## csr_owner_validation_test.go — Describe "CSR Owner Validation - DPU Existence Check" (13 specs)

| Go spec | Rust test (`src/csr_owner_validation_test.rs`, prefix `csr_owner_validation_dpu_existence_check::`) |
|---|---|
| DPU Existence Check / when DPU exists with matching name / should find DPU with exact name match | `dpu_existence_check::when_dpu_exists_with_matching_name::should_find_dpu_with_exact_name_match` |
| DPU Existence Check / when DPU does not exist / should not find DPU when none exists | `dpu_existence_check::when_dpu_does_not_exist::should_not_find_dpu_when_none_exists` |
| DPU Existence Check / when DPU does not exist / should not find DPU in different namespace | `dpu_existence_check::when_dpu_does_not_exist::should_not_find_dpu_in_different_namespace` |
| DPU Existence Check / when DPU does not exist / should not find DPU with different name | `dpu_existence_check::when_dpu_does_not_exist::should_not_find_dpu_with_different_name` |
| DPU Existence Check / when multiple DPUs exist / should find the correct DPU among multiple | `dpu_existence_check::when_multiple_dpus_exist::should_find_the_correct_dpu_among_multiple` |
| Bootstrap CSR Phase Validation / when DPU is in DPU Cluster Config phase / should approve bootstrap CSR | `bootstrap_csr_phase_validation::when_dpu_is_in_dpu_cluster_config_phase::should_approve_bootstrap_csr` |
| Bootstrap CSR Phase Validation / when DPU is in Ready phase / should reject bootstrap CSR | `bootstrap_csr_phase_validation::when_dpu_is_in_ready_phase::should_reject_bootstrap_csr` |
| Bootstrap CSR Phase Validation / when DPU is in Initializing phase / should reject bootstrap CSR | `bootstrap_csr_phase_validation::when_dpu_is_in_initializing_phase::should_reject_bootstrap_csr` |
| Serving CSR Phase Validation / when DPU is in DPU Cluster Config phase / should approve serving CSR for initial node join | `serving_csr_phase_validation::when_dpu_is_in_dpu_cluster_config_phase::should_approve_serving_csr_for_initial_node_join` |
| Serving CSR Phase Validation / when DPU is in Ready phase / should approve serving CSR | `serving_csr_phase_validation::when_dpu_is_in_ready_phase::should_approve_serving_csr` |
| Serving CSR Phase Validation / when DPU is in Node Effect Removal phase / should approve serving CSR | `serving_csr_phase_validation::when_dpu_is_in_node_effect_removal_phase::should_approve_serving_csr` |
| Serving CSR Phase Validation / when DPU is in Initializing phase / should reject serving CSR | `serving_csr_phase_validation::when_dpu_is_in_initializing_phase::should_reject_serving_csr` |
| Serving CSR Phase Validation / when DPU is in Ready phase but node does not exist / should reject serving CSR | `serving_csr_phase_validation::when_dpu_is_in_ready_phase_but_node_does_not_exist::should_reject_serving_csr` |

## setcondition_test.go — Describe "setCondition" (2 specs)

| Go spec | Rust test (`src/setcondition_test.rs`) |
|---|---|
| setCondition / succeeds when there is no conflict | `set_condition::succeeds_when_there_is_no_conflict` |
| setCondition / retries and succeeds after a single conflict | `set_condition::retries_and_succeeds_after_a_single_conflict` |

## Rust-only tests

| Rust test | Purpose |
|---|---|
| `x509_golden_test::csr_corpus_matches_go` | Golden differential test over 142 CSR request variants generated by `hack/golden/csrapproval` (real CSRs for RSA/P-224/P-256/P-384/P-521/Ed25519 keys, hand-built DER/PEM/subject/SAN/public-key/attribute corner cases): `pem.Decode`, `x509.ParseCertificateRequest` (fields and error strings), `ExtractHostname`, `ValidateBootstrapCSR`, `ValidateServingCSR` must match Go. Regenerate: `go run -mod=vendor ./hack/golden/csrapproval crates/ctrl-csrapproval/tests/fixtures` |
| `csr_owner_validation_test::reason_strings::all_reasons_match_go_format` | exact `ValidationResult.Reason` strings |
| `setcondition_test::set_condition_extra::*` (3) | injected conflicts (retry count, give-up after 5 attempts), condition/event side effects |
| `csrapproval_test::process_csrs::*` (5) | `ProcessCSRs`: kubeconfig missing, client creation failure, connection failure (cache invalidation), approval of bootstrap/serving CSRs (condition, events, approval condition, 30s requeue), transient list error |
| `csrapproval_test::reconcile::*` (5) | `Reconcile`: not found, missing kubeconfig ref / approver, deletion, happy path, ProcessCSRs error → 30s requeue |
| `csrapproval_test::setup_with_manager::registers_named_controller_for_dpfhcpprovisioner` | `SetupWithManager`: `For(DPFHCPProvisioner)`, `Named("csrapproval")` |
| `x509::asn1::tests::*` (4), `x509::pem::tests::*` (3), `x509::gourl::tests::*` (1) | unit tests of the Go stdlib ports |

Totals: 40 Go specs → 40 Rust tests; 24 additional Rust-only tests (64 total).
