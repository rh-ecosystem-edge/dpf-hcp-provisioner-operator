# Build the manager binary
# A musl-native Rust toolchain produces a fully static binary (RHEL/UBI Rust packages ship
# no musl target and UBI lacks glibc-static). The image is multi-arch (amd64/arm64).
FROM docker.io/library/rust:1.99-alpine3.24 AS builder
ARG TARGETOS
ARG TARGETARCH

# Run as root to ensure write permissions in workspace
USER 0
WORKDIR /workspace

# Cargo manifests and sources
COPY Cargo.toml Cargo.toml
COPY Cargo.lock Cargo.lock
COPY crates/ crates/
# Data embedded into the binary at compile time
COPY internal/dpfdefaults/dpfversions.json internal/dpfdefaults/dpfversions.json
COPY internal/ignition/resources/ internal/ignition/resources/

# Build
# The binary is built for the platform of the builder image (TARGETARCH), so the container
# and the binary shipped in it always match. On a musl host target `crt-static` is the
# default, so the result is a statically linked executable. Dependencies come from the
# prefetched cargo sources in hermetic builds (`--locked` enforces Cargo.lock).
RUN cargo build --release --locked --bin manager \
    && cp target/release/manager /workspace/manager \
    && strip /workspace/manager \
    && if readelf -d /workspace/manager | grep -q NEEDED; then \
         echo "manager binary is not statically linked" >&2; exit 1; \
       fi

# CA bundle for outbound TLS (e.g. quay.io when resolving the corresponding ARM OVN-K image
# tag); ubi-micro has no CA certs.
FROM registry.access.redhat.com/ubi10/ubi-minimal:latest AS certs

FROM registry.access.redhat.com/ubi10-micro:latest

WORKDIR /

# ubi-micro has no CA certs; needed for outbound TLS (e.g. quay.io when
# resolving the corresponding ARM OVN-K image tag)
COPY --from=certs /etc/pki/tls/certs/ca-bundle.crt /etc/pki/tls/certs/

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
