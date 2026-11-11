# e2e — Go → Rust test mapping

The Go suite is one `go test` test (`TestE2E`) that runs Ginkgo. The Rust suite is likewise one
libtest test, `test_e2e` in `tests/e2e_suite/main.rs` (`#[ignore]`d), which drives the
Ginkgo-compatible runner in `src/ginkgo.rs`. Every Ginkgo node is a variant of a container's
`Node` enum and is dispatched by `Container::run`. Specs are reported with their full Ginkgo
text, e.g. `DPFHCPProvisioner E2E HostedCluster Lifecycle should have generated valid ignition [ocp-required]`.

## Suite (`e2e_suite_test.go` → `tests/e2e_suite/main.rs`)

| Go | Rust |
|---|---|
| `TestE2E` (`RunSpecs(t, "e2e suite")`) | `test_e2e` |
| `BeforeSuite` | `before_suite` (run via `Runner::run_suite_node("BeforeSuite", ...)`) |
| `AfterSuite` | `after_suite` |
| `imageRepository` / `imageTag` / `imageDigest` | `image_repository` / `image_tag` / `image_digest` (unit test `tests::image_reference_helpers`) |
| helm args built in `BeforeSuite` | `helm_deploy_args` (unit test `tests::helm_args`) |
| globals `k8sClient`, `k8sConfig`, `testingOnOCP` | `Env { client, backend, config, testing_on_ocp }` |

## `Describe("DPFHCPProvisioner E2E", Ordered, Label("ocp-required"))` (`e2e_ci_test.go` → `tests/e2e_suite/ci.rs`)

| Go node / spec | Rust (`ci::Node::…` → method) |
|---|---|
| `ensureHostedClusterAPI` closure | `DpfhcpProvisionerE2e::ensure_hosted_cluster_api` |
| `BeforeAll` | `BeforeAll` → `before_all` |
| `AfterAll` | `AfterAll` → `after_all` |
| `AfterEach` (dump status on failure) | `AfterEach` |
| HostedCluster Lifecycle / should create DPFHCPProvisioner and reach Ready state | `CreateAndReachReady` → `create_and_reach_ready` |
| HostedCluster Lifecycle / should have generated valid ignition | `ValidIgnition` → `valid_ignition` |
| HostedCluster Lifecycle / should include DPUFlavor config files in target ignition with correct merge behavior | `DpuFlavorConfigFiles` → `dpu_flavor_config_files` |
| HostedCluster Lifecycle / should self-heal when ignition ConfigMap is deleted | `SelfHealIgnitionConfigMap` → `self_heal_ignition_config_map` |
| HostedCluster Lifecycle / should regenerate ignition when DPUDeployment flavor changes | `RegenerateOnFlavorChange` → `regenerate_on_flavor_change` |
| HostedCluster Lifecycle / should update ConfigMap BFB annotation when DPUDeployment BFB changes | `BfbAnnotationUpdate` → `bfb_annotation_update` |
| HostedCluster Lifecycle / should not regenerate ignition while DPUDeployment is missing | `NoRegenerationWhileDeploymentMissing` → `no_regeneration_while_deployment_missing` |
| HostedCluster Lifecycle / should have injected kubeconfig into DPUCluster namespace | `KubeconfigInjected` → `kubeconfig_injected` |
| HostedCluster Lifecycle / should have correct status conditions | `StatusConditions` → `status_conditions` |
| HostedCluster Upgrade / should upgrade HostedCluster when ocpReleaseImage is updated | `Upgrade` → `upgrade` |
| CSR Auto-Approval / `BeforeEach` | `CsrBeforeEach` → `csr_before_each` |
| CSR Auto-Approval / `AfterEach` | `CsrAfterEach` → `csr_after_each` |
| CSR Auto-Approval / should approve bootstrap CSRs for valid DPU nodes | `CsrBootstrap` → `csr_bootstrap` |
| CSR Auto-Approval / should approve serving CSRs for valid DPU nodes | `CsrServing` → `csr_serving` |
| CSR Auto-Approval / should not approve CSRs for unknown hostnames | `CsrUnknownHostname` → `csr_unknown_hostname` |
| Cleanup Verification / `AfterEach` | `CleanupAfterEach` → `cleanup_after_each` |
| Cleanup Verification / should clean up all resources on CR deletion | `CleanupOnDeletion` → `cleanup_on_deletion` |
| Validation Error Cases / `AfterEach` | `ValidationAfterEach` |
| Validation Error Cases / should fail with missing DPUCluster | `MissingDpuCluster` → `missing_dpu_cluster` |
| Validation Error Cases / should fail with invalid secrets | `InvalidSecrets` → `invalid_secrets` |
| (tree shape) | unit test `ci::tests::spec_tree_mirrors_go_container` |

## `e2e_ci_helpers_test.go` → `tests/e2e_suite/ci_helpers.rs`

All constants (`ciNamespace` … `pollingInterval`) → `CI_NAMESPACE` … `POLLING_INTERVAL` (same values).

| Go | Rust |
|---|---|
| `detectOCPReleaseImage` | `detect_ocp_release_image` (+ `nested_string` = `unstructured.NestedString`, unit test `nested_string_semantics`) |
| `getBaseDomain` | `get_base_domain` |
| `getControlPlaneAvailability` | `get_control_plane_availability` |
| `getMachineOSURL` | `get_machine_os_url` |
| `getEtcdStorageClass` | `get_etcd_storage_class` |
| `createNamespace` | `create_namespace` |
| `createDPUClusterStub` | `create_dpu_cluster_stub` |
| `generateSSHKeySecret` | `generate_ssh_key_secret` |
| `copyPullSecret` | `copy_pull_secret` |
| `createDPUDeploymentStub` | `create_dpu_deployment_stub` |
| `createDPUFlavorStub` | `create_dpu_flavor_stub` |
| `createDPFOperatorConfig` | `create_dpf_operator_config` (object: `dpf_operator_config_stub`, unit test `stub_objects_serialize_like_go`) |
| `createDPFHCPProvisioner` / `createDPFHCPProvisionerWithCluster` | `create_dpfhcp_provisioner` / `create_dpfhcp_provisioner_with_cluster` |
| `waitForCRPhase` | `wait_for_cr_phase` |
| `getCRPhase` | `get_cr_phase` |
| `getCRConditions` | `get_cr_conditions` |
| `getConditionStatus` | `get_condition_status` |
| `createDPUStub` / `deleteDPUStub` | `create_dpu_stub` / `delete_dpu_stub` |
| `getHostedClusterKubeconfig` | `get_hosted_cluster_kubeconfig` |
| `writeKubeconfigToFile` | `write_kubeconfig_to_file` |
| `warnError` | `warn_error` |
| `loadHCConfig` | `load_hc_config` |
| `waitForHostedClusterAPIReachable` | `wait_for_hosted_cluster_api_reachable` |
| `getHCClient` / `getHCClientWithImpersonation` | `get_hc_client` / `get_hc_client_with_impersonation` |
| `createBootstrapCSRInHostedCluster` | `create_bootstrap_csr_in_hosted_cluster` |
| `createServingCSRInHostedCluster` | `create_serving_csr_in_hosted_cluster` (config text: `serving_csr_openssl_config`, unit test `serving_csr_config`) |
| `base64EncodeFile` | `base64_encode_file` |
| `waitForCSRApproval` / `verifyCSRNotApproved` | `wait_for_csr_approval` / `verify_csr_not_approved` |
| `createNodeInHostedCluster` / `deleteNodeInHostedCluster` / `deleteCSRInHostedCluster` | `create_node_in_hosted_cluster` / `delete_node_in_hosted_cluster` / `delete_csr_in_hosted_cluster` |
| `waitForDeletion` | `wait_for_deletion` |
| `forceDeleteProvisioner` | `force_delete_provisioner` |
| `cleanupStaleResources` | `cleanup_stale_resources` |
| `dumpProvisionerStatus` | `dump_provisioner_status` |
| `getUpgradeOCPReleaseImage` / `updateDPFHCPProvisionerReleaseImage` | `get_upgrade_ocp_release_image` / `update_dpfhcp_provisioner_release_image` |
| `getHostedClusterReleaseImage` / `getNodePoolReleaseImage` | `get_hosted_cluster_release_image` / `get_node_pool_release_image` |
| `getIgnitionConfigMap` | `get_ignition_config_map` |
| `decodeTargetIgnition` | `decode_target_ignition` (unit tests `target_ignition_roundtrip`, `target_ignition_errors`) |
| `getTargetIgnitionFiles` | `get_target_ignition_files` (unit test `target_files_absent`) |
| `decodeIgnitionFileContent` | `decode_ignition_file_content` (unit test `file_content_errors`) |
| `k8sClient.DeleteAllOf(ctx, &T{}, InNamespace(ns))` | `delete_all_of::<T>` |

## `Describe("DPUServiceTemplate E2E", Ordered)` (`e2e_dpuservicetemplate_test.go` → `tests/e2e_suite/dpuservicetemplate.rs`)

| Go node / spec | Rust (`dpuservicetemplate::Node::…` → method) |
|---|---|
| `BeforeAll` (Skip on OCP) | `BeforeAll` → `before_all` |
| `AfterAll` | `AfterAll` → `after_all` |
| Template Lifecycle / should create all three DPUServiceTemplates when provisioner exists | `CreateAllTemplates` → `create_all_templates` |
| Template Lifecycle / should have correct template structure | `TemplateStructure` → `template_structure` |
| Template Lifecycle / should self-heal when a template is deleted | `SelfHealDeleted` → `self_heal_deleted` |
| Template Lifecycle / should self-heal when a template is modified | `SelfHealModified` → `self_heal_modified` |
| Template Lifecycle / should delete templates when DPUDeployment is deleted | `DeleteOnDeploymentDeletion` → `delete_on_deployment_deletion` |
| Template Lifecycle / should delete templates when provisioner is deleted | `DeleteOnProvisionerDeletion` → `delete_on_provisioner_deletion` |

## `e2e_dpuservicetemplate_helpers_test.go` → `tests/e2e_suite/dpuservicetemplate_helpers.rs`

| Go | Rust |
|---|---|
| `stubOCPVersion`, `stubReleaseImage`, `stubOVNImage`, `stubDPFVersion` | `STUB_OCP_VERSION`, `STUB_RELEASE_IMAGE`, `STUB_OVN_IMAGE`, `STUB_DPF_VERSION` |
| `enableManageDPUServiceTemplates` | `enable_manage_dpu_service_templates` |
| `setDPFOperatorConfigVersion` | `set_dpf_operator_config_version` |
| `cleanupTemplateTestResources` | `cleanup_template_test_resources` |

## `e2e_fake_ocp_on_kind_test.go` → `tests/e2e_suite/fake_ocp_on_kind.rs`

| Go | Rust |
|---|---|
| `createClusterVersionStub` | `create_cluster_version_stub` |
| `createOVNDaemonSetStub` | `create_ovn_daemon_set_stub` (object: `ovn_daemon_set_stub`, unit test `ovn_daemon_set_stub_shape`) |
| `createPullSecretStub` | `create_pull_secret_stub` (bytes: `stub_docker_config_json`, unit test `pull_secret_stub_bytes`) |
| `createDummySecrets` | `create_dummy_secrets` |

## `test/utils/utils.go` → `src/utils.rs`

| Go | Rust |
|---|---|
| `warnError` | `warn_error` (private) |
| `Run` | `run` (unit test `run_combines_output_and_formats_errors`) |
| `InstallPrometheusOperator` / `UninstallPrometheusOperator` | `install_prometheus_operator` / `uninstall_prometheus_operator` |
| `IsPrometheusCRDsInstalled` | `is_prometheus_crds_installed` (unit test `crd_detection`) |
| `UninstallCertManager` / `InstallCertManager` | `uninstall_cert_manager` / `install_cert_manager` |
| `IsCertManagerCRDsInstalled` | `is_cert_manager_crds_installed` |
| `LoadImageToKindClusterWithName` | `load_image_to_kind_cluster_with_name` |
| `GetNonEmptyLines` | `get_non_empty_lines` (unit test `non_empty_lines`) |
| `GetProjectDir` | `get_project_dir` / `project_dir_from` (unit test `project_dir`) |
| `UncommentCode` | `uncomment_code` / `uncomment_bytes` (unit tests `uncomment`, `uncomment_code_on_file`) |

## Ginkgo / Gomega semantics (`src/ginkgo.rs`, `src/gomega.rs`)

Unit tests pin the semantics the suite depends on: `ordered_container_happy_path`,
`failure_skips_remaining_and_runs_after_all`, `skip_in_before_all_skips_everything_but_runs_after_all`,
`skip_in_before_each_only_skips_that_spec`, `before_all_failure_runs_only_outer_after_nodes`,
`panics_are_recovered_as_failures`, `filtered_specs_do_not_run_and_after_all_follows_last_selected`,
`label_filter_excludes_container`, `context_labels_select_subset`, `before_suite_failure_skips_all`,
`suite_timeout_interrupts_and_cleans_up`, `label_filter_parsing_and_matching`, `go_durations`,
`eventually_polls_until_success`, `eventually_times_out_with_last_message`,
`eventually_aborts_on_global_failure`, `consistently_semantics`, `matcher_messages`, `conversions`.

## Not ported

* The `test/e2e/README.md` line about the `openshift-doca-hugepages-holder` namespace /
  hugepages DaemonSet describes a check that does not exist in the Go specs; there is nothing to port.
