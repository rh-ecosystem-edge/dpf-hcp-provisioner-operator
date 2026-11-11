# Extract CLI tools from their official images
FROM quay.io/hypershift/hypershift-operator:latest as hypershift-cli
FROM quay.io/openshift/origin-cli:latest as oc-cli

# Main test image
FROM registry.access.redhat.com/ubi10/ubi:latest AS tester

USER root
WORKDIR /workspace

# Install required system tools and the Rust toolchain (the e2e suite is the `e2e` crate:
# `cargo test -p e2e`)
RUN dnf install -y \
    rust \
    cargo \
    gcc \
    make \
    openssl \
    openssh-clients \
    && dnf clean all

# Install Helm
RUN curl -L https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash

# Copy CLI tools from their official images
COPY --from=hypershift-cli /usr/bin/hypershift /usr/bin/hypershift
COPY --from=oc-cli /usr/bin/oc /usr/bin/oc

# Create kubectl symlink (oc provides kubectl functionality)
RUN ln -s /usr/bin/oc /usr/bin/kubectl

# Cargo writes its registry cache and build outputs here; keep them writable for the arbitrary
# UID the container runs as on OpenShift.
ENV CARGO_HOME=/workspace/.cargo

# Copy config directory (operator CRDs that the helm chart symlinks reference, and the
# external CRDs in config/crd/external installed by apply-crds-for-e2e)
COPY --chown=1001:0 config/ ./config/

# Copy Makefiles with e2e targets
COPY --chown=1001:0 Makefile Makefile.e2e ./

# Copy test data and scripts (committed external CRDs in test/e2e/testdata/crds, HyperShift
# deploy script)
COPY --chown=1001:0 test/ ./test/

# Copy the Cargo workspace (the e2e suite and the crates it depends on), then pre-build the
# suite so test runs start immediately.
COPY --chown=1001:0 Cargo.toml Cargo.lock ./
COPY --chown=1001:0 crates/ ./crates/
RUN cargo test --locked -p e2e --test e2e_suite --no-run

# Copy helm chart (used to deploy operator during tests)
COPY --chown=1001:0 helm/ ./helm/

# Set OpenShift-compatible permissions on entire workspace (arbitrary UID, group 0)
# This allows the container to run with any UID in OpenShift (always group 0)
RUN chgrp -R 0 /workspace && \
    chmod -R g=u /workspace

# Set user to non-root for running tests
USER 1001
