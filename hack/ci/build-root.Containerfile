# OpenShift CI build root (openshift/release: build_root.project_image.dockerfile_path).
# Provides the toolchain for the `lint`, `unit-tests` and `verify-deps` jobs:
#   - Rust (cargo, clippy, rustfmt) and gcc/glibc-static for the operator crates,
#   - Go, for hack/gen-rust-apis (`make verify-generate` regenerates the API types),
#   - make, git, python3 for the Makefile targets and hack/ scripts.
FROM registry.access.redhat.com/ubi10/ubi:latest

RUN dnf install -y \
        rust cargo clippy rustfmt \
        gcc glibc-static \
        golang \
        make git python3 diffutils findutils tar gzip which \
    && dnf clean all

# CI runs as an arbitrary UID without a writable home directory.
ENV HOME=/tmp \
    CARGO_HOME=/tmp/.cargo \
    GOPATH=/tmp/go \
    GOCACHE=/tmp/.cache/go-build \
    XDG_CACHE_HOME=/tmp/.cache

RUN cargo --version && cargo clippy --version && cargo fmt --version && go version
