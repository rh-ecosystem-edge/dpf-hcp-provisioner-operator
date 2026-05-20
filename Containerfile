# Build the manager binary
# Use Red Hat UBI instead of Docker Hub to avoid rate limits
FROM registry.access.redhat.com/ubi10/go-toolset:1.25 AS builder
ARG TARGETOS
ARG TARGETARCH

# Run as root to ensure write permissions in workspace
USER 0
WORKDIR /workspace
# Copy the Go Modules manifests
COPY go.mod go.mod
COPY go.sum go.sum
COPY vendor/ vendor/

# Copy the go source
COPY cmd/main.go cmd/main.go
COPY api/ api/
COPY internal/ internal/

# Build
# the GOARCH has not a default value to allow the binary be built according to the host where the command
# was called. For example, if we call make container-build in a local env which has the Apple Silicon M1 SO
# the container build tool's BUILDPLATFORM arg will be linux/arm64 when for Apple x86 it will be linux/amd64. Therefore,
# by leaving it empty we can ensure that the container and binary shipped on it will have the same platform.
RUN CGO_ENABLED=0 GOOS=${TARGETOS:-linux} GOARCH=${TARGETARCH} go build -a -ldflags="-s -w" -o manager cmd/main.go


FROM registry.access.redhat.com/ubi10-micro:latest
WORKDIR /
COPY --from=builder /workspace/manager .
USER 65532:65532

ARG release=22
ARG version=v4

LABEL com.redhat.component="dpf-hcp-provisioner-rhel10-operator" \
      name="dpu-kit-for-nvidia-operator/dpf-hcp-provisioner-rhel10-operator" \
      version="${version}" \
      upstream-ref="${version}" \
      upstream-url="https://github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator" \
      url="https://github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator" \
      summary="DPU Kit for NVIDIA Operator - DPF HCP Provisioner" \
      io.k8s.display-name="DPU Kit for NVIDIA Operator - DPF HCP Provisioner" \
      description="DPU Kit for NVIDIA Operator - DPF HCP Provisioner" \
      io.k8s.description="DPU Kit for NVIDIA Operator - DPF HCP Provisioner" \
      distribution-scope="public" \
      release="${release}" \
      cpe="cpe:/a:redhat:dpu_kit:4.22::el10"

ENTRYPOINT ["/manager"]
