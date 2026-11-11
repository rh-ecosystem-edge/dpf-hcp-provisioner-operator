# ctrl-hostedcluster — Go test → Rust test

Go package `internal/controller/hostedcluster`. `suite_test.go` (`TestHostedCluster`) only
bootstraps Ginkgo (`RunSpecs`) — no Rust counterpart needed. The Go tests are internal
(package-level) tests, so the Rust ports are unit-test modules inside `src/` (access to
`build_hosted_cluster`, `build_node_pool`, ...). Ginkgo `BeforeEach` → `before_each()` /
`upgrade_scheme()` helpers. `findServiceStrategy` / `findServiceStrategyByType` → same-named helpers.
In `status_test.go` the `*fake.ClientBuilder` captured pointers to `cr`/`hc` and was built
inside each `It` after mutation; `Env::build` snapshots the (mutated) objects at build time.

## `hostedcluster_test.go` → `src/hostedcluster_test.rs` (37 specs)

| Go spec (Describe / Context / It) | Rust test |
|---|---|
| HostedCluster Builder / Basic HostedCluster Fields / should set correct metadata | `hostedcluster_test::hosted_cluster_builder_basic_hosted_cluster_fields_should_set_correct_metadata` |
| HostedCluster Builder / Basic HostedCluster Fields / should set release image from DPFHCPProvisioner spec | `hostedcluster_test::hosted_cluster_builder_basic_hosted_cluster_fields_should_set_release_image_from_dpfhcp_provisioner_spec` |
| HostedCluster Builder / Basic HostedCluster Fields / should reference correct secret names | `hostedcluster_test::hosted_cluster_builder_basic_hosted_cluster_fields_should_reference_correct_secret_names` |
| HostedCluster Builder / Basic HostedCluster Fields / should set DNS base domain | `hostedcluster_test::hosted_cluster_builder_basic_hosted_cluster_fields_should_set_dns_base_domain` |
| HostedCluster Builder / Basic HostedCluster Fields / should set platform to None | `hostedcluster_test::hosted_cluster_builder_basic_hosted_cluster_fields_should_set_platform_to_none` |
| HostedCluster Builder / ETCD Configuration / should configure managed ETCD with persistent volume | `hostedcluster_test::hosted_cluster_builder_etcd_configuration_should_configure_managed_etcd_with_persistent_volume` |
| HostedCluster Builder / ETCD Configuration / should use storage class from DPFHCPProvisioner spec | `hostedcluster_test::hosted_cluster_builder_etcd_configuration_should_use_storage_class_from_dpfhcp_provisioner_spec` |
| HostedCluster Builder / ETCD Configuration / should set ETCD volume size to 8Gi | `hostedcluster_test::hosted_cluster_builder_etcd_configuration_should_set_etcd_volume_size_to_8gi` |
| HostedCluster Builder / Network Configuration / should set network type to Other | `hostedcluster_test::hosted_cluster_builder_network_configuration_should_set_network_type_to_other` |
| HostedCluster Builder / Network Configuration / should set default service network CIDR | `hostedcluster_test::hosted_cluster_builder_network_configuration_should_set_default_service_network_cidr` |
| HostedCluster Builder / Network Configuration / should set default cluster network CIDR | `hostedcluster_test::hosted_cluster_builder_network_configuration_should_set_default_cluster_network_cidr` |
| HostedCluster Builder / Network Configuration / should have empty machine network | `hostedcluster_test::hosted_cluster_builder_network_configuration_should_have_empty_machine_network` |
| HostedCluster Builder / Network Configuration / should not set AllocateNodeCIDRs when FlannelEnabled is false | `hostedcluster_test::hosted_cluster_builder_network_configuration_should_not_set_allocate_node_cidrs_when_flannel_enabled_is_false` |
| HostedCluster Builder / Network Configuration / should set AllocateNodeCIDRs to Enabled when FlannelEnabled is true | `hostedcluster_test::hosted_cluster_builder_network_configuration_should_set_allocate_node_cidrs_to_enabled_when_flannel_enabled_is_true` |
| HostedCluster Builder / Network Configuration / should set AllocateNodeCIDRs to Enabled when FlannelEnabled is nil (default true) | `hostedcluster_test::hosted_cluster_builder_network_configuration_should_set_allocate_node_cidrs_to_enabled_when_flannel_enabled_is_nil_default_true` |
| HostedCluster Builder / Network Configuration / should keep network type as Other when FlannelEnabled is true | `hostedcluster_test::hosted_cluster_builder_network_configuration_should_keep_network_type_as_other_when_flannel_enabled_is_true` |
| HostedCluster Builder / Availability Policies / should set SingleReplica when specified | `hostedcluster_test::hosted_cluster_builder_availability_policies_should_set_single_replica_when_specified` |
| HostedCluster Builder / Availability Policies / should set HighlyAvailable when specified | `hostedcluster_test::hosted_cluster_builder_availability_policies_should_set_highly_available_when_specified` |
| HostedCluster Builder / Secret Encryption / should configure AESCBC encryption | `hostedcluster_test::hosted_cluster_builder_secret_encryption_should_configure_aescbc_encryption` |
| HostedCluster Builder / Secret Encryption / should reference ETCD encryption key secret | `hostedcluster_test::hosted_cluster_builder_secret_encryption_should_reference_etcd_encryption_key_secret` |
| HostedCluster Builder / Service Publishing Strategy / should configure 4 services in LoadBalancer mode | `hostedcluster_test::hosted_cluster_builder_service_publishing_strategy_should_configure_4_services_in_load_balancer_mode` |
| HostedCluster Builder / Service Publishing Strategy / should use LoadBalancer for APIServer | `hostedcluster_test::hosted_cluster_builder_service_publishing_strategy_should_use_load_balancer_for_api_server` |
| HostedCluster Builder / Service Publishing Strategy / should use Route for OAuthServer | `hostedcluster_test::hosted_cluster_builder_service_publishing_strategy_should_use_route_for_oauth_server` |
| HostedCluster Builder / Service Publishing Strategy / should use Route for Konnectivity | `hostedcluster_test::hosted_cluster_builder_service_publishing_strategy_should_use_route_for_konnectivity` |
| HostedCluster Builder / Service Publishing Strategy / should use Route for Ignition | `hostedcluster_test::hosted_cluster_builder_service_publishing_strategy_should_use_route_for_ignition` |
| HostedCluster Builder / Networking Configuration / should use defaults when networking field is nil | `hostedcluster_test::hosted_cluster_builder_networking_configuration_should_use_defaults_when_networking_field_is_nil` |
| HostedCluster Builder / Networking Configuration / should use defaults when networking is an empty struct | `hostedcluster_test::hosted_cluster_builder_networking_configuration_should_use_defaults_when_networking_is_an_empty_struct` |
| HostedCluster Builder / Networking Configuration / should override only service network when custom service network provided | `hostedcluster_test::hosted_cluster_builder_networking_configuration_should_override_only_service_network_when_custom_service_network_provided` |
| HostedCluster Builder / Networking Configuration / should override only cluster network when custom cluster network provided | `hostedcluster_test::hosted_cluster_builder_networking_configuration_should_override_only_cluster_network_when_custom_cluster_network_provided` |
| HostedCluster Builder / Networking Configuration / should set machine network when provided | `hostedcluster_test::hosted_cluster_builder_networking_configuration_should_set_machine_network_when_provided` |
| HostedCluster Builder / Networking Configuration / should override all networks when all provided | `hostedcluster_test::hosted_cluster_builder_networking_configuration_should_override_all_networks_when_all_provided` |
| HostedCluster Builder / Networking Configuration / should always set network type to Other regardless of networking config | `hostedcluster_test::hosted_cluster_builder_networking_configuration_should_always_set_network_type_to_other_regardless_of_networking_config` |
| HostedCluster Builder / InfraID Generation / should generate non-empty infraID | `hostedcluster_test::hosted_cluster_builder_infra_id_generation_should_generate_non_empty_infra_id` |
| HostedCluster Builder / InfraID Generation / should generate infraID with cluster name prefix | `hostedcluster_test::hosted_cluster_builder_infra_id_generation_should_generate_infra_id_with_cluster_name_prefix` |
| HostedCluster Builder / InfraID Generation / should generate infraID with random suffix | `hostedcluster_test::hosted_cluster_builder_infra_id_generation_should_generate_infra_id_with_random_suffix` |
| HostedCluster Upgrade / should not update HostedCluster release image when spec changes (upgrade is handled by handleUpgrade) | `hostedcluster_test::hosted_cluster_upgrade_should_not_update_hosted_cluster_release_image_when_spec_changes_upgrade_is_handled_by_handle_upgrade` |
| HostedCluster Upgrade / should not update HostedCluster when release image is unchanged | `hostedcluster_test::hosted_cluster_upgrade_should_not_update_hosted_cluster_when_release_image_is_unchanged` |

## `nodepool_test.go` → `src/nodepool_test.rs` (8 specs)

| Go spec (Describe / Context / It) | Rust test |
|---|---|
| NodePool Builder / Basic NodePool Fields / should set correct metadata | `nodepool_test::node_pool_builder_basic_node_pool_fields_should_set_correct_metadata` |
| NodePool Builder / Basic NodePool Fields / should set cluster name to match HostedCluster | `nodepool_test::node_pool_builder_basic_node_pool_fields_should_set_cluster_name_to_match_hosted_cluster` |
| NodePool Builder / Basic NodePool Fields / should set replicas to 0 | `nodepool_test::node_pool_builder_basic_node_pool_fields_should_set_replicas_to_0` |
| NodePool Builder / Basic NodePool Fields / should set platform to None | `nodepool_test::node_pool_builder_basic_node_pool_fields_should_set_platform_to_none` |
| NodePool Builder / Basic NodePool Fields / should set release image from DPFHCPProvisioner spec | `nodepool_test::node_pool_builder_basic_node_pool_fields_should_set_release_image_from_dpfhcp_provisioner_spec` |
| NodePool Builder / Management Configuration / should set upgrade type to Replace | `nodepool_test::node_pool_builder_management_configuration_should_set_upgrade_type_to_replace` |
| NodePool Upgrade / should not update NodePool release image when spec changes (upgrade is handled by handleUpgrade) | `nodepool_test::node_pool_upgrade_should_not_update_node_pool_release_image_when_spec_changes_upgrade_is_handled_by_handle_upgrade` |
| NodePool Upgrade / should not update NodePool when release image is unchanged | `nodepool_test::node_pool_upgrade_should_not_update_node_pool_when_release_image_is_unchanged` |

## `servicepublishing_test.go` → `src/servicepublishing_test.rs` (12 specs)

| Go spec (Describe / Context / It) | Rust test |
|---|---|
| Service Publishing Strategy Builder / LoadBalancer Mode / should return 4 service publishing strategies | `servicepublishing_test::service_publishing_strategy_builder_load_balancer_mode_should_return_4_service_publishing_strategies` |
| Service Publishing Strategy Builder / LoadBalancer Mode / should use LoadBalancer for APIServer | `servicepublishing_test::service_publishing_strategy_builder_load_balancer_mode_should_use_load_balancer_for_api_server` |
| Service Publishing Strategy Builder / LoadBalancer Mode / should use Route for OAuthServer | `servicepublishing_test::service_publishing_strategy_builder_load_balancer_mode_should_use_route_for_oauth_server` |
| Service Publishing Strategy Builder / LoadBalancer Mode / should use Route for Konnectivity | `servicepublishing_test::service_publishing_strategy_builder_load_balancer_mode_should_use_route_for_konnectivity` |
| Service Publishing Strategy Builder / LoadBalancer Mode / should use Route for Ignition | `servicepublishing_test::service_publishing_strategy_builder_load_balancer_mode_should_use_route_for_ignition` |
| Service Publishing Strategy Builder / NodePort Mode / should return 5 service publishing strategies including OIDC | `servicepublishing_test::service_publishing_strategy_builder_node_port_mode_should_return_5_service_publishing_strategies_including_oidc` |
| Service Publishing Strategy Builder / NodePort Mode / should use NodePort for APIServer with correct address | `servicepublishing_test::service_publishing_strategy_builder_node_port_mode_should_use_node_port_for_api_server_with_correct_address` |
| Service Publishing Strategy Builder / NodePort Mode / should use NodePort for OAuthServer | `servicepublishing_test::service_publishing_strategy_builder_node_port_mode_should_use_node_port_for_oauth_server` |
| Service Publishing Strategy Builder / NodePort Mode / should use NodePort for OIDC | `servicepublishing_test::service_publishing_strategy_builder_node_port_mode_should_use_node_port_for_oidc` |
| Service Publishing Strategy Builder / NodePort Mode / should use NodePort for Konnectivity | `servicepublishing_test::service_publishing_strategy_builder_node_port_mode_should_use_node_port_for_konnectivity` |
| Service Publishing Strategy Builder / NodePort Mode / should use NodePort for Ignition | `servicepublishing_test::service_publishing_strategy_builder_node_port_mode_should_use_node_port_for_ignition` |
| Service Publishing Strategy Builder / NodePort Mode / should sort services alphabetically | `servicepublishing_test::service_publishing_strategy_builder_node_port_mode_should_sort_services_alphabetically` |

## `status_test.go` → `src/status_test.rs` (5 specs)

| Go spec (Describe / Context / It) | Rust test |
|---|---|
| Status Syncer / SyncStatusFromHostedCluster / should skip sync when hostedClusterRef is not set | `status_test::status_syncer_sync_status_from_hosted_cluster_should_skip_sync_when_hosted_cluster_ref_is_not_set` |
| Status Syncer / SyncStatusFromHostedCluster / should handle missing HostedCluster gracefully | `status_test::status_syncer_sync_status_from_hosted_cluster_should_handle_missing_hosted_cluster_gracefully` |
| Status Syncer / SyncStatusFromHostedCluster / should skip sync when HostedCluster status is not populated | `status_test::status_syncer_sync_status_from_hosted_cluster_should_skip_sync_when_hosted_cluster_status_is_not_populated` |
| Status Syncer / SyncStatusFromHostedCluster / should mirror all 7 HostedCluster conditions to DPFHCPProvisioner | `status_test::status_syncer_sync_status_from_hosted_cluster_should_mirror_all_7_hosted_cluster_conditions_to_dpfhcp_provisioner` |
| Status Syncer / SyncStatusFromHostedCluster / should set ObservedGeneration on mirrored conditions | `status_test::status_syncer_sync_status_from_hosted_cluster_should_set_observed_generation_on_mirrored_conditions` |

**62 Go specs → 62 Rust tests.**

## Golden differential tests (`tests/golden.rs`, Rust-only)

Fixtures in `tests/fixtures/*.json` are generated by `go run -mod=vendor ./hack/golden/hostedcluster`,
which runs the Go managers against a controller-runtime fake client with an interceptor
recording every create/update/delete (body with apiVersion/kind as the real client sends it),
plus result, error, events and conditions. The Rust test replays the same scenario on
`ctrlrt-fake` (reactor recording) and compares: call sequence exactly; bodies as JSON values;
HostedCluster/NodePool bodies also byte-for-byte against Go's `json.Marshal`; requeueAfter,
error strings and events exactly.

Normalizations: HostedCluster `spec.infraID` (checked against `<name>-[bcdfghjklmnpqrstvwxz2456789]{5}`)
→ `INFRAID`; ETCD key secret `data.key` (checked to be 32 bytes) → `KEY`. Status-sync
conditions are compared sorted by type (Go mirrors in random map order) without
`lastTransitionTime`.

| Rust test | Go code exercised |
|---|---|
| `golden_hc_create_loadbalancer` | HostedClusterManager.CreateOrUpdateHostedCluster |
| `golden_hc_create_nodeport` | HostedClusterManager.CreateOrUpdateHostedCluster |
| `golden_hc_create_nodeport_external_dns` | HostedClusterManager.CreateOrUpdateHostedCluster |
| `golden_hc_create_nodeport_no_nodes` | HostedClusterManager.CreateOrUpdateHostedCluster |
| `golden_hc_create_nodeport_no_address` | HostedClusterManager.CreateOrUpdateHostedCluster |
| `golden_hc_update_secret_refs` | HostedClusterManager.CreateOrUpdateHostedCluster |
| `golden_hc_noop` | HostedClusterManager.CreateOrUpdateHostedCluster |
| `golden_hc_conflict` | HostedClusterManager.CreateOrUpdateHostedCluster |
| `golden_np_create` | NodePoolManager.CreateNodePool |
| `golden_np_noop` | NodePoolManager.CreateNodePool |
| `golden_np_conflict` | NodePoolManager.CreateNodePool |
| `golden_secrets_copy_create` | SecretManager.CopySecrets (copy, relabel, stale cleanup, conflicts) |
| `golden_secrets_copy_relabel` | SecretManager.CopySecrets (copy, relabel, stale cleanup, conflicts) |
| `golden_secrets_copy_conflict` | SecretManager.CopySecrets (copy, relabel, stale cleanup, conflicts) |
| `golden_secrets_copy_missing_source` | SecretManager.CopySecrets (copy, relabel, stale cleanup, conflicts) |
| `golden_etcd_key_create` | SecretManager.GenerateETCDEncryptionKey |
| `golden_etcd_key_noop` | SecretManager.GenerateETCDEncryptionKey |
| `golden_etcd_key_conflict` | SecretManager.GenerateETCDEncryptionKey |
| `golden_etcd_key_is_random` | SecretManager.GenerateETCDEncryptionKey (randomness / 32-byte length; no fixture) |
| `golden_cleanup_hc_delete` | CleanupHandler.Cleanup |
| `golden_cleanup_np_deleting` | CleanupHandler.Cleanup |
| `golden_cleanup_secrets` | CleanupHandler.Cleanup |
| `golden_finalizer_manager_complete` | finalizer.Manager.HandleFinalizerCleanup + hostedcluster CleanupHandler |
| `golden_finalizer_manager_in_progress` | finalizer.Manager.HandleFinalizerCleanup + hostedcluster CleanupHandler |
| `golden_status_sync` | StatusSyncer.SyncStatusFromHostedCluster |
| `golden_copy_names` | PullSecretCopyName / SSHKeyCopyName (shortHash) |
