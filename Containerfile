# Build the manager binary
# Use Red Hat UBI instead of Docker Hub to avoid rate limits
FROM registry.access.redhat.com/ubi10/ubi:latest AS builder
ARG TARGETOS
ARG TARGETARCH

# Rust toolchain from the UBI repositories (prefetched from rpm-prefetching/ in hermetic
# builds). glibc-static is required to link a fully static binary; gcc compiles ring's
# C/assembly sources.
RUN dnf install -y rust cargo gcc glibc-static && dnf clean all

# Run as root to ensure write permissions in workspace
USER 0
WORKDIR /workspace

# Cargo manifests and sources
COPY Cargo.toml Cargo.toml
COPY Cargo.lock Cargo.lock
COPY crates/ crates/

# Build
# The binary is built for the platform of the builder image (TARGETARCH), so the container
# and the binary shipped in it always match. `+crt-static` on the gnu target links glibc
# statically (static-pie); the explicit --target keeps build scripts and proc-macros
# dynamically linked as required. Dependencies come from the prefetched cargo sources in
# hermetic builds (`--locked` enforces Cargo.lock).
RUN RUST_TARGET="$(uname -m)-unknown-linux-gnu" \
    && RUSTFLAGS="-C target-feature=+crt-static" \
       cargo build --release --locked --target "${RUST_TARGET}" --bin manager \
    && cp "target/${RUST_TARGET}/release/manager" /workspace/manager \
    && strip /workspace/manager \
    && if readelf -d /workspace/manager | grep -q NEEDED; then \
         echo "manager binary is not statically linked" >&2; exit 1; \
       fi


FROM registry.access.redhat.com/ubi10-micro:latest

WORKDIR /

# ubi-micro has no CA certs; needed for outbound TLS (e.g. quay.io when
# resolving the corresponding ARM OVN-K image tag)
COPY --from=builder /etc/pki/tls/certs/ca-bundle.crt /etc/pki/tls/certs/

COPY --from=builder /workspace/manager .

USER 65532:65532

ARG release=22
ARG version=v4

LABEL com.redhat.component="dpf-hcp-provisioner-rhel10-operator" \
      name="dpu-kit-for-nvidia/dpf-hcp-provisioner-rhel10-operator" \
      version="${version}" \
      upstream-ref="${version}" \
      upstream-url="https://github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator" \
      url="https://github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator" \
      summary="DPU Kit for NVIDIA - DPF HCP Provisioner" \
      io.k8s.display-name="DPU Kit for NVIDIA - DPF HCP Provisioner" \
      description="DPU Kit for NVIDIA - DPF HCP Provisioner" \
      io.k8s.description="DPU Kit for NVIDIA - DPF HCP Provisioner" \
      distribution-scope="public" \
      release="${release}" \
      cpe="cpe:/a:redhat:dpu_kit:4.22::el10"

ENTRYPOINT ["/manager"]
