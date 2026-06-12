#!/usr/bin/env bash
# Verifies that all generated files are up to date.
# Refuses to run on a dirty tree, runs all generation, and fails if anything changed.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${REPO_ROOT}"

if ! [[ -z "$(git status --porcelain --untracked-files=all)" ]]; then
    echo "Error: tree is dirty, cannot verify generation. Commit or stash changes first." >&2
    exit 1
fi

make manifests generate generate-test-crds

if ! [[ -z "$(git status --porcelain --untracked-files=all)" ]]; then
    echo "Error: generated files are out of date. Run 'make manifests generate generate-test-crds' and commit the result." >&2
    git diff --stat
    exit 1
fi
