# internal/hostedclient test mapping

The Go package `internal/hostedclient` has **no `_test.go` files**, so there is nothing to
port 1:1. The Rust crate carries Rust-only tests in `tests/hostedclient_test.rs` that pin the
ported behaviour:

| Rust test | Purpose |
|---|---|
| `replace_server_with_internal_endpoint_tests::rewrites_current_context_cluster_server` | rewrites only the current context's cluster to `https://kube-apiserver.<ns>-<name>.svc.cluster.local:6443` |
| `replace_server_with_internal_endpoint_tests::{nil_kubeconfig, no_current_context, context_not_found, cluster_not_found}` | Go error strings |
| `get_kubeconfig_data::{returns_kubeconfig_bytes, secret_not_found, key_not_found, empty_data}` | `GetKubeconfigData` result and error strings (`failed to get kubeconfig secret ns/name-admin-kubeconfig: ...` keeps `IsNotFound`) |
| `client_cache::missing_secret_error_is_wrapped` | `failed to get kubeconfig: ...` wrapping, nothing cached on error |
| `client_cache::invalid_kubeconfig_is_reported` | `failed to replace server endpoint: ...` |
| `client_cache::creates_and_caches_client_from_kubeconfig` | real client built from kubeconfig bytes, cached under `namespace/name`, `InvalidateClient` |
| `client_cache::injected_client_is_returned_from_cache` | test hook `set_hosted_cluster_client` (Go tests may write `hcClients` directly), fake discovery `TestConnection` |
| `client_cache::test_connection_wraps_discovery_error` | `failed to connect to hosted cluster API server: ...` |
| `create_or_update_tests::creates_when_missing_then_unchanged_then_updated` | `CreateOrUpdate` results `created` / `unchanged` / `updated` |
| `create_or_update_tests::nil_mutate_fn_creates_and_is_unchanged` | nil `mutateFn` |
| `create_or_update_tests::mutate_cannot_change_name` | `mutate function cannot change object name or namespace` on both paths |
| `create_or_update_tests::mutate_error_is_returned` | mutate errors returned as-is |
| `create_or_update_tests::get_error_is_returned_unwrapped` | non-NotFound Get errors returned as-is |
| `create_or_update_tests::namespaced_resource_client_scopes_requests` | `DaemonSets(ns)`-style scoped client (create/list/delete/get) |
| `create_or_update_tests::semantic_equality_treats_nil_and_empty_as_equal` | `equality.Semantic.DeepEqual` nil vs empty slices/maps |

Totals: 0 Go tests; 21 Rust-only tests.
