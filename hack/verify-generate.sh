#!/usr/bin/env bash
# Verifies that all generated files are up to date:
#   - `make manifests`: Helm RBAC templates synced from config/rbac/role.yaml
#   - `make generate`:  config/crd/external, test/e2e/testdata/crds (hack/gen-external-crds.sh)
#                       and crates/apis/src/generated (hack/gen-rust-apis.sh); needs Go
# Refuses to run on a dirty tree, runs all generation, and fails if anything changed.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${REPO_ROOT}"

if ! [[ -z "$(git status --porcelain --untracked-files=all)" ]]; then
    echo "Error: tree is dirty, cannot verify generation. Commit or stash changes first." >&2
    exit 1
fi

make manifests generate

if ! [[ -z "$(git status --porcelain --untracked-files=all)" ]]; then
    echo "Error: generated files are out of date. Run 'make manifests generate' and commit the result." >&2
    git diff --stat
    exit 1
fi
