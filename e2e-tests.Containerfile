# Extract CLI tools from their official images
FROM quay.io/hypershift/hypershift-operator:latest as hypershift-cli
FROM quay.io/openshift/origin-cli:latest as oc-cli

# Main test image
FROM registry.access.redhat.com/ubi10/go-toolset:1.25 AS tester

USER root
WORKDIR /workspace

# Install required system tools
RUN dnf install -y \
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

# Copy Go module files (required for go test to resolve module imports)
COPY --chown=1001:0 go.mod go.sum ./

# Copy vendor directory (contains all dependencies including ginkgo/gomega test frameworks and NVIDIA doca-platform for CRD generation)
COPY --chown=1001:0 vendor/ ./vendor/

# Copy API definitions (tests import api/v1alpha1 types)
COPY --chown=1001:0 api/ ./api/

# Copy Makefile (used to run e2e targets like e2e-deploy-hypershift, e2e-install-dpf-crds)
COPY --chown=1001:0 Makefile ./

# Build controller-gen inside the container (ensures correct OS/arch for CRD generation)
RUN make controller-gen

# Copy all test code (e2e tests, utils, scripts)
COPY --chown=1001:0 test/ ./test/

# Copy helm chart (used to deploy operator during tests)
COPY --chown=1001:0 helm/ ./helm/

# Create directory for generated CRDs (controller-gen writes here during test setup)
# Set proper permissions for non-root user to write CRDs
RUN mkdir -p test/e2e/testdata/crds && chown -R 1001:0 test/e2e/testdata

# Set user to non-root for running tests
USER 1001