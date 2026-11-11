# ctrl-secrets — Go test → Rust test

Go: `internal/controller/secrets/validator_test.go` → Rust: `src/validator_test.rs`.
`TestSecretsValidator` only bootstraps Ginkgo (`RunSpecs`) — no Rust counterpart needed.
`forbiddenSecretsClient` → a `ctrlrt-fake` reactor on `get secrets` returning
`ApiError::forbidden("", "secrets", name, "<nil>")` (Go: `apierrors.NewForbidden(..., nil)`).
Ginkgo `BeforeEach` → `before_each()`. `Eventually(recorder.Events).Should(Receive(ContainSubstring(s)))`
→ `receive_containing` (receives until a match); `Consistently(...).ShouldNot(Receive())` →
`recorder.try_recv().is_none()` (fake client and recorder are synchronous).

All Go specs are under `Describe("Secrets Validator") / Describe("ValidateSecrets")`; the Rust
names start with `validate_secrets_`.

| Go spec (Context / It) | Rust test |
|---|---|
| when both secrets exist and are valid / should set SecretsValid=True | `validate_secrets_when_both_secrets_exist_and_are_valid_should_set_secrets_valid_true` |
| when both secrets exist and are valid / should emit event only on first success (not on subsequent reconciliations) | `validate_secrets_when_both_secrets_exist_and_are_valid_should_emit_event_only_on_first_success_not_on_subsequent_reconciliations` |
| when SSH key secret is missing / should set SecretsValid=False with SSHKeySecretMissing reason and not requeue | `validate_secrets_when_ssh_key_secret_is_missing_should_set_secrets_valid_false_with_ssh_key_secret_missing_reason_and_not_requeue` |
| when SSH key secret is missing required key / should set SecretsValid=False with SSHKeySecretInvalid reason and not requeue | `validate_secrets_when_ssh_key_secret_is_missing_required_key_should_set_secrets_valid_false_with_ssh_key_secret_invalid_reason_and_not_requeue` |
| when pull secret is missing / should set SecretsValid=False with PullSecretMissing reason and not requeue | `validate_secrets_when_pull_secret_is_missing_should_set_secrets_valid_false_with_pull_secret_missing_reason_and_not_requeue` |
| when pull secret is missing required key / should set SecretsValid=False with PullSecretInvalid reason and not requeue | `validate_secrets_when_pull_secret_is_missing_required_key_should_set_secrets_valid_false_with_pull_secret_invalid_reason_and_not_requeue` |
| when RBAC permission is denied / should set SecretsValid=False with AccessDenied reason and not requeue | `validate_secrets_when_rbac_permission_is_denied_should_set_secrets_valid_false_with_access_denied_reason_and_not_requeue` (also asserts the exact condition message and event) |
| when secrets are recovered / should transition from SSHKeySecretMissing to SecretsValid | `validate_secrets_when_secrets_are_recovered_should_transition_from_ssh_key_secret_missing_to_secrets_valid` |
| when secrets are recovered / should transition from PullSecretInvalid to SecretsValid | `validate_secrets_when_secrets_are_recovered_should_transition_from_pull_secret_invalid_to_secrets_valid` |

9 Go specs → 9 Rust tests.
