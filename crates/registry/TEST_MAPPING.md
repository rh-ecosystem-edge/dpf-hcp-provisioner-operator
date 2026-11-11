# registry — Go test → Rust test mapping

The vendored `github.com/google/go-containerregistry` ships no tests, so behaviour is pinned
with golden fixtures generated from the vendored Go code:

| Go source of truth | Fixture | Rust test |
|---|---|---|
| `name.ParseReference` / `NewTag` / `NewDigest` / `NewRepository` / `NewRegistry` (5 option sets × ~140 inputs), `Repository.Tag/Digest`, `Registry.Repo` (`hack/golden/registry`) | `tests/fixtures/name_golden.json` | `tests/golden.rs::name_parsing_matches_go` |
| `authn.AuthConfig` `UnmarshalJSON` / `MarshalJSON` (`hack/golden/registry`) | `tests/fixtures/authn_golden.json` | `tests/golden.rs::auth_config_json_matches_go` |
| docker/distribution `challenge.ResponseChallenges` (`hack/golden/registry`) | `tests/fixtures/challenge_golden.json` | `tests/golden.rs::www_authenticate_parsing_matches_go` |
| `transport.CheckError` / `Error.Error()` / `Temporary()` (`hack/golden/registry`) | `tests/fixtures/transport_error_golden.json` | `tests/golden.rs::transport_error_strings_match_go` |
| `mutate.Extract` + `partial.CompressedToLayer().Uncompressed()` (`hack/golden/registry`) | `tests/fixtures/extract_golden.json`, `tests/fixtures/extract/*.bin` | `tests/golden.rs::mutate_extract_matches_go` |
| `remote.Head` / `Get` / `Image` / `ConfigFile` against scripted `httptest` registries (`hack/golden/registry-remote`) | `tests/fixtures/remote_golden.json` | `tests/remote_golden.rs::remote_flows_match_go` |

Regenerate with:

```sh
go run -mod=vendor ./hack/golden/registry
go run -mod=vendor ./hack/golden/registry-remote
```

## Operator Go tests

| Go test | Rust test |
|---|---|
| `internal/common/keychain_test.go` `TestKeychainPrefersRepositoryScopedAuth` (both `t.Run` cases) | `tests/remote.rs::test_keychain_prefers_repository_scoped_auth` |
| `internal/common/keychain_test.go` `TestKeychainFallsBackToRegistryAuth` | `tests/remote.rs::test_keychain_falls_back_to_registry_auth` |
| `internal/common/keychain_test.go` `TestKeychainAnonymousWhenNoMatch` | `tests/remote.rs::test_keychain_anonymous_when_no_match` |

(The keychain itself lives in the `common` crate; these use an equivalent test keychain to
exercise the `Keychain` / `Resource` / `AuthConfig` contract.)

## Additional Rust-only tests

`tests/remote.rs` (in-process fake registry with token / basic auth): descriptor fields,
User-Agent / Accept headers, token exchange query, repository-scoped credentials,
anonymous tokens, `access_token`, re-auth with extra scope on challenge, index → linux/amd64
resolution, digest references, config verification, extract of an OCP release image,
blob redirects to another host (no `Authorization` forwarded), retries, ping failures,
option validation. Unit tests in `src/` cover Go helpers (path cleaning, base64, `%v`
float formatting, URL handling, proxy env rules, tar parsing).
