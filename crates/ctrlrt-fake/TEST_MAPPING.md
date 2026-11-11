# ctrlrt-fake — test mapping

This crate ports vendored library code, not a package of this repository, so there are no
Go tests in the repo to map one-to-one. Each Rust test pins down one behaviour of the vendored
Go source that it ports:

| Go source (vendor/…) | Rust tests |
|---|---|
| `controller-runtime/pkg/client/fake/client.go` `versionedTracker.Add` | `fake_client::initial_objects_*`, `obj_mut_sets_resource_version_on_caller_object`, `initial_object_with_deletion_timestamp_but_no_finalizers_is_refused`, `initial_object_being_deleted_with_finalizers_is_accepted`, `duplicate_initial_objects_fail`, `initial_object_of_unregistered_kind_fails`, `with_list_and_runtime_objects` |
| `ClientBuilder.Build` (default scheme, WithStatusSubresource) | `default_scheme_is_client_go`, `status_subresource_for_unregistered_kind_fails`, `scheme_named_after_call_site` |
| `apiutil.GVKForObject` / `runtime.notRegisteredErr` | `unregistered_kind_errors_on_every_call` |
| RESTMapper `NoKindMatchError` (knob) | `unserved_kind_returns_no_kind_match` |
| `fakeClient.Get` / `ensureTypeMeta` / managedFields | `get_not_found_message`, `resource_names_are_guessed_from_kind`, `get_sets_type_meta_and_strips_managed_fields` |
| `fakeClient.List` / `filterByNamespace` | `list_namespace_and_labels` |
| `fakeClient.Create` / `versionedTracker.Create` | `create_sets_resource_version_1`, `create_with_resource_version_is_bad_request`, `create_existing_is_already_exists`, `create_without_name_is_invalid`, `create_with_generate_name`, `create_ignores_deletion_timestamp`, `cluster_scoped_objects` |
| `versionedTracker.updateObject` | `update_increments_resource_version`, `update_with_stale_resource_version_conflicts`, `update_with_empty_resource_version`, `update_missing_is_not_found`, `update_missing_service_creates_it`, `update_without_name_is_invalid`, `update_with_unparsable_stored_resource_version`, `update_cannot_change_deletion_timestamp` |
| status subresource (`copyStatusFrom`, `inTreeResourcesWithStatus`, `fakeSubResourceClient.Update`) | `update_without_status_subresource_writes_status`, `status_update_without_status_subresource_is_not_found`, `status_subresource_semantics`, `status_update_can_clear_status`, `in_tree_kinds_have_status_subresource`, `csr_approval_subresource_updates_status_only`, `scale_subresource_update_without_body` |
| `fakeClient.Delete` / `deleteObjectLocked` | `delete_without_finalizers_removes`, `delete_with_finalizers_sets_deletion_timestamp`, `delete_with_resource_version_precondition`, `delete_ignores_uid_precondition_and_propagation` |
| `fakeClient.patch` / `dryPatch` / `fakeSubResourceClient.statusPatch` | `merge_patch`, `patch_resource_version_handling`, `patch_missing_is_not_found`, `patch_and_status_patch_with_status_subresource`, `patch_cannot_change_deletion_timestamp`, `patch_removing_last_finalizer_deletes`, `strategic_merge_patch` |
| `interceptor.Funcs` analogue, `trackerWriteLock` | `reactor_injects_errors`, `concurrent_use`, `stored_readback`, `create_or_update_round_trip` |
| `client-go/testing/fixture.go` `tracker` | `tracker::tests::*` |
| `apimachinery/pkg/api/meta.UnsafeGuessKindToResource` | `tracker::tests::guess_kind_to_resource_matches_apimachinery` |
| `gopkg.in/evanphx/json-patch.v4` `MergePatch` | `patch::tests::merge_patch_rfc7386_and_evanphx_quirks` |
| `apimachinery/pkg/util/strategicpatch` (approximation) | `patch::tests::strategic_merge_by_name_and_finalizers` |
| `utilrand.String`, `strconv.ParseUint`, `deletionTimestampEqual` | `util::tests::*` |
| `client-go/kubernetes/fake` `NewSimpleClientset`, `gentype/fake.go`, `testing/fake.go` | `clientset::*` (15 tests) |
