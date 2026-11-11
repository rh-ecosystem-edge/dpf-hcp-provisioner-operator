# VERSION defines the project version for the bundle.
# Update this value when you upgrade the version of your project.
# To re-generate a bundle for another specific version without changing the standard setup, you can:
# - use the VERSION as arg of the bundle target (e.g make bundle VERSION=0.0.2)
# - use environment variables to overwrite this value (e.g export VERSION=0.0.2)
VERSION ?= 0.0.1

# CHANNELS define the bundle channels used in the bundle.
# Add a new line here if you would like to change its default config. (E.g CHANNELS = "candidate,fast,stable")
# To re-generate a bundle for other specific channels without changing the standard setup, you can:
# - use the CHANNELS as arg of the bundle target (e.g make bundle CHANNELS=candidate,fast,stable)
# - use environment variables to overwrite this value (e.g export CHANNELS="candidate,fast,stable")
ifneq ($(origin CHANNELS), undefined)
BUNDLE_CHANNELS := --channels=$(CHANNELS)
endif

# DEFAULT_CHANNEL defines the default channel used in the bundle.
# Add a new line here if you would like to change its default config. (E.g DEFAULT_CHANNEL = "stable")
# To re-generate a bundle for any other default channel without changing the default setup, you can:
# - use the DEFAULT_CHANNEL as arg of the bundle target (e.g make bundle DEFAULT_CHANNEL=stable)
# - use environment variables to overwrite this value (e.g export DEFAULT_CHANNEL="stable")
ifneq ($(origin DEFAULT_CHANNEL), undefined)
BUNDLE_DEFAULT_CHANNEL := --default-channel=$(DEFAULT_CHANNEL)
endif
BUNDLE_METADATA_OPTS ?= $(BUNDLE_CHANNELS) $(BUNDLE_DEFAULT_CHANNEL)

# IMAGE_TAG_BASE defines the registry namespace and part of the image name for remote images.
# This variable is used to construct full image tags for bundle and catalog images.
#
# For example, running 'make bundle-build bundle-push catalog-build catalog-push' will build and push both
# dpu.hcp.io/dpf-hcp-provisioner-operator-bundle:$VERSION and dpu.hcp.io/dpf-hcp-provisioner-operator-catalog:$VERSION.
IMAGE_TAG_BASE ?= dpu.hcp.io/dpf-hcp-provisioner-operator

# BUNDLE_IMG defines the image:tag used for the bundle.
# You can use it as an arg. (E.g make bundle-build BUNDLE_IMG=<some-registry>/<project-name-bundle>:<tag>)
BUNDLE_IMG ?= $(IMAGE_TAG_BASE)-bundle:v$(VERSION)

# BUNDLE_GEN_FLAGS are the flags passed to the operator-sdk generate bundle command
BUNDLE_GEN_FLAGS ?= -q --overwrite --version $(VERSION) $(BUNDLE_METADATA_OPTS)

# USE_IMAGE_DIGESTS defines if images are resolved via tags or digests
# You can enable this value if you would like to use SHA Based Digests
# To enable set flag to true
USE_IMAGE_DIGESTS ?= false
ifeq ($(USE_IMAGE_DIGESTS), true)
	BUNDLE_GEN_FLAGS += --use-image-digests
endif

# Set the Operator SDK version to use. By default, what is installed on the system is used.
# This is useful for CI or a project to utilize a specific version of the operator-sdk toolkit.
OPERATOR_SDK_VERSION ?= v1.42.0
# Image URL to use all building/pushing image targets
IMG ?= controller:latest

# Host OS/architecture in Go/Kubernetes naming (linux|darwin, amd64|arm64), used to pick
# release binaries of the downloaded tools.
HOST_OS ?= $(shell uname -s | tr '[:upper:]' '[:lower:]')
HOST_ARCH ?= $(shell uname -m | sed -e 's/x86_64/amd64/' -e 's/aarch64/arm64/')

# RUST_TARGET is the musl target used for the static manager binary.
RUST_TARGET ?= $(shell uname -m | sed 's/arm64/aarch64/')-unknown-linux-musl

# ---------------------------------------------------------------------------------------------
# Rust toolchain
# ---------------------------------------------------------------------------------------------
# The operator is a Cargo workspace. When `cargo` is not on PATH (e.g. the OpenShift CI build
# root), the pinned toolchain below is installed into bin/ from the official release tarballs
# (sha256-verified) and put first on PATH, so `make lint`/`make test`/`make build` work
# without a pre-installed Rust. RUST_TOOLCHAIN_VERSION matches the UBI10 builder image.
RUST_TOOLCHAIN_VERSION ?= 1.92.0
RUST_HOST_TRIPLE ?= $(shell uname -m | sed 's/arm64/aarch64/')-unknown-linux-gnu
RUST_DIST_URL ?= https://static.rust-lang.org/dist
RUST_TOOLCHAIN_DIR ?= $(CURDIR)/bin/rust-$(RUST_TOOLCHAIN_VERSION)
ifeq ($(shell command -v cargo 2>/dev/null),)
CARGO ?= $(RUST_TOOLCHAIN_DIR)/bin/cargo
RUST_TOOLCHAIN := $(RUST_TOOLCHAIN_DIR)/bin/cargo
export PATH := $(RUST_TOOLCHAIN_DIR)/bin:$(PATH)
export CARGO_HOME ?= $(CURDIR)/bin/cargo-home
else
CARGO ?= cargo
RUST_TOOLCHAIN :=
endif

# CONTAINER_TOOL defines the container tool to be used for building images.
CONTAINER_TOOL ?= podman
CONTAINER_FILE ?= Containerfile

# Setting SHELL to bash allows bash commands to be executed by recipes.
# Options are set to exit when a recipe line exits non-zero or a piped command fails.
SHELL = /usr/bin/env bash -o pipefail
.SHELLFLAGS = -ec

.PHONY: all
all: build

##@ General

# The help target prints out all targets with their descriptions organized
# beneath their categories. The categories are represented by '##@' and the
# target descriptions by '##'. The awk command is responsible for reading the
# entire set of makefiles included in this invocation, looking for lines of the
# file as xyz: ## something, and then pretty-format the target and help. Then,
# if there's a line with ##@ something, that gets pretty-printed as a category.
# More info on the usage of ANSI control characters for terminal formatting:
# https://en.wikipedia.org/wiki/ANSI_escape_code#SGR_parameters
# More info on the awk command:
# http://linuxcommand.org/lc3_adv_awk.php

.PHONY: help
help: ## Display this help.
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_0-9-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

##@ Development

# The operator's own CRDs (config/crd/bases) and RBAC (config/rbac/role.yaml) are
# hand-maintained sources: keep them consistent with crates/apis/src/provisioning_types.rs and
# with the API calls the controllers make.
.PHONY: manifests
manifests: yq ## Sync the Helm chart RBAC templates from config/rbac/role.yaml.
	PATH="$(LOCALBIN):$$PATH" python3 ./hack/sync-helm-rbac.py

.PHONY: generate
generate: ## Regenerate the external CRDs and the Rust API types (needs Go and network access).
	./hack/gen-external-crds.sh
	./hack/gen-rust-apis.sh

.PHONY: generate-external-crds
generate-external-crds: ## Regenerate config/crd/external and test/e2e/testdata/crds (needs Go).
	./hack/gen-external-crds.sh

.PHONY: generate-rust-apis
generate-rust-apis: ## Regenerate the Rust API types (crates/apis/src/generated) with kopium (needs Go).
	./hack/gen-rust-apis.sh

.PHONY: verify-generate
verify-generate: ## Verify that all generated files are up to date.
	./hack/verify-generate.sh

.PHONY: fmt
fmt: $(RUST_TOOLCHAIN) ## Run cargo fmt against code.
	$(CARGO) fmt --all

.PHONY: vet
vet: $(RUST_TOOLCHAIN) ## Run cargo clippy against code.
	$(CARGO) clippy --workspace --all-targets

.PHONY: test
test: manifests fmt vet setup-envtest $(RUST_TOOLCHAIN) ## Run tests.
	KUBEBUILDER_ASSETS="$(ENVTEST_ASSETS_DIR)" $(CARGO) test --workspace --exclude e2e

include Makefile.e2e

.PHONY: verify-deps
verify-deps: $(RUST_TOOLCHAIN) ## Verify Cargo.lock matches the manifests and all locked dependencies resolve and build offline.
	$(CARGO) metadata --locked --format-version 1 >/dev/null
	$(CARGO) fetch --locked
	$(CARGO) check --workspace --all-targets --locked --offline

.PHONY: lint
lint: verify-deps $(RUST_TOOLCHAIN) ## Run clippy (warnings are errors) and check formatting
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) fmt --all -- --check

.PHONY: lint-fix
lint-fix: $(RUST_TOOLCHAIN) ## Run clippy and rustfmt and apply fixes
	$(CARGO) clippy --workspace --all-targets --fix --allow-dirty --allow-staged
	$(CARGO) fmt --all

.PHONY: lint-config
lint-config: $(RUST_TOOLCHAIN) ## Verify the Rust lint toolchain
	$(CARGO) clippy --version
	$(CARGO) fmt --version

##@ Build

.PHONY: build
build: manifests fmt vet $(RUST_TOOLCHAIN) ## Build the static manager binary (musl).
	$(CARGO) build --release --locked --target $(RUST_TARGET) --bin manager
	mkdir -p bin
	cp target/$(RUST_TARGET)/release/manager bin/manager

.PHONY: run
run: manifests fmt vet $(RUST_TOOLCHAIN) ## Run a controller from your host.
	$(CARGO) run --bin manager --

.PHONY: container-build
container-build: ## Build container image with the manager.
	$(CONTAINER_TOOL) build -f $(CONTAINER_FILE) -t ${IMG} .

.PHONY: container-push
container-push: ## Push container image with the manager.
	$(CONTAINER_TOOL) push ${IMG}

.PHONY: docker-build
docker-build: container-build ## Alias for container-build.

.PHONY: docker-push
docker-push: container-push ## Alias for container-push.

##@ Helm

# Helm chart configuration — all vars must be provided by the caller.
# HELM_CHART_DIR     : chart name under helm/ in this repo (e.g. dpu-worker-config).
# HELM_CHART_REGISTRY: full OCI destination including the chart name
#                      (e.g. oci://quay.io/myorg/charts/dpu-worker-config).
#                      Helm appends the chart name automatically, so the last path segment
#                      is stripped before invoking helm push.
HELM_CHART_DIR ?=
HELM_CHART_REGISTRY ?=
HELM_CHART_VERSION ?= $(shell test -n "$(HELM_CHART_DIR)" && grep '^version:' helm/$(HELM_CHART_DIR)/Chart.yaml | awk '{print $$2}')

.PHONY: helm-package
helm-package: ## Package a Helm chart. Requires: HELM_CHART_DIR (chart name under helm/).
	@test -n "$(HELM_CHART_DIR)" || (echo "ERROR: HELM_CHART_DIR is not set."; exit 1)
	@test -n "$(HELM_CHART_VERSION)" || (echo "ERROR: Failed to resolve HELM_CHART_VERSION from helm/$(HELM_CHART_DIR)/Chart.yaml"; exit 1)
	helm package helm/$(HELM_CHART_DIR) --version $(HELM_CHART_VERSION)

.PHONY: helm-push
helm-push: helm-package ## Package and push a Helm chart. Requires: HELM_CHART_DIR, HELM_CHART_REGISTRY (full OCI path).
	@test -n "$(HELM_CHART_REGISTRY)" || (echo "ERROR: HELM_CHART_REGISTRY is not set."; exit 1)
	helm push $(HELM_CHART_DIR)-$(HELM_CHART_VERSION).tgz $$(dirname $(HELM_CHART_REGISTRY))

.PHONY: build-installer
build-installer: manifests kustomize ## Generate a consolidated YAML with CRDs and deployment.
	mkdir -p dist
	cd config/manager && $(KUSTOMIZE) edit set image controller=${IMG}
	$(KUSTOMIZE) build config/default > dist/install.yaml

##@ Deployment

ifndef ignore-not-found
  ignore-not-found = false
endif

.PHONY: install
install: manifests kustomize ## Install CRDs into the K8s cluster specified in ~/.kube/config.
	$(KUSTOMIZE) build config/crd | $(KUBECTL) apply -f -

.PHONY: uninstall
uninstall: manifests kustomize ## Uninstall CRDs from the K8s cluster specified in ~/.kube/config. Call with ignore-not-found=true to ignore resource not found errors during deletion.
	$(KUSTOMIZE) build config/crd | $(KUBECTL) delete --ignore-not-found=$(ignore-not-found) -f -

.PHONY: deploy
deploy: manifests kustomize ## Deploy controller to the K8s cluster specified in ~/.kube/config.
	cd config/manager && $(KUSTOMIZE) edit set image controller=${IMG}
	$(KUSTOMIZE) build config/default | $(KUBECTL) apply -f -

.PHONY: undeploy
undeploy: kustomize ## Undeploy controller from the K8s cluster specified in ~/.kube/config. Call with ignore-not-found=true to ignore resource not found errors during deletion.
	$(KUSTOMIZE) build config/default | $(KUBECTL) delete --ignore-not-found=$(ignore-not-found) -f -

##@ Dependencies

## Location to install dependencies to
LOCALBIN ?= $(shell pwd)/bin
$(LOCALBIN):
	mkdir -p $(LOCALBIN)

## Rust toolchain installer (used only when cargo is not on PATH; see "Rust toolchain" above).
## Components: rustc, cargo, std for the host and the musl target (static `make build`),
## clippy and rustfmt. Each tarball is verified against its published sha256.
RUST_COMPONENTS ?= rustc cargo rust-std clippy rustfmt
$(RUST_TOOLCHAIN_DIR)/bin/cargo:
	@echo "cargo not found on PATH; installing Rust $(RUST_TOOLCHAIN_VERSION) into $(RUST_TOOLCHAIN_DIR)"
	@set -euo pipefail; \
	tmp="$$(mktemp -d)"; trap 'rm -rf "$$tmp"' EXIT; \
	musl="$$(echo $(RUST_HOST_TRIPLE) | sed 's/-gnu$$/-musl/')"; \
	for pkg in rustc-$(RUST_TOOLCHAIN_VERSION)-$(RUST_HOST_TRIPLE) \
	           cargo-$(RUST_TOOLCHAIN_VERSION)-$(RUST_HOST_TRIPLE) \
	           rust-std-$(RUST_TOOLCHAIN_VERSION)-$(RUST_HOST_TRIPLE) \
	           rust-std-$(RUST_TOOLCHAIN_VERSION)-$$musl \
	           clippy-$(RUST_TOOLCHAIN_VERSION)-$(RUST_HOST_TRIPLE) \
	           rustfmt-$(RUST_TOOLCHAIN_VERSION)-$(RUST_HOST_TRIPLE); do \
	  curl -sSfL -o "$$tmp/$$pkg.tar.xz" "$(RUST_DIST_URL)/$$pkg.tar.xz"; \
	  curl -sSfL -o "$$tmp/$$pkg.tar.xz.sha256" "$(RUST_DIST_URL)/$$pkg.tar.xz.sha256"; \
	  (cd "$$tmp" && echo "$$(cut -d' ' -f1 "$$pkg.tar.xz.sha256")  $$pkg.tar.xz" | sha256sum -c --quiet -); \
	  tar -xJf "$$tmp/$$pkg.tar.xz" -C "$$tmp"; \
	  "$$tmp/$$pkg/install.sh" --prefix="$(RUST_TOOLCHAIN_DIR)" --disable-ldconfig >/dev/null; \
	done
	@"$(RUST_TOOLCHAIN_DIR)/bin/cargo" --version

## Tool Binaries
KUBECTL ?= kubectl
KIND ?= kind
KUSTOMIZE ?= $(LOCALBIN)/kustomize
YQ ?= $(LOCALBIN)/yq

## Tool Versions
KUSTOMIZE_VERSION ?= v5.6.0
YQ_VERSION ?= v4.44.6
# ENVTEST_K8S_VERSION is the Kubernetes version of the envtest binaries (etcd, kube-apiserver,
# kubectl) used by the envtest suites; ENVTEST_VERSION is the matching controller-tools release.
ENVTEST_K8S_VERSION ?= 1.35.0
ENVTEST_VERSION ?= envtest-v$(ENVTEST_K8S_VERSION)
# The envtest suites (crates/ctrlrt-envtest) use $$KUBEBUILDER_ASSETS, or else the first
# directory under bin/k8s/, so the binaries are unpacked to bin/k8s/<version>-<os>-<arch>.
ENVTEST_ASSETS_DIR ?= $(LOCALBIN)/k8s/$(ENVTEST_K8S_VERSION)-$(HOST_OS)-$(HOST_ARCH)
ENVTEST_ARCHIVE ?= $(ENVTEST_VERSION)-$(HOST_OS)-$(HOST_ARCH).tar.gz
ENVTEST_RELEASE_URL ?= https://github.com/kubernetes-sigs/controller-tools/releases/download/$(ENVTEST_VERSION)/$(ENVTEST_ARCHIVE)
# Release index with the sha512 of every envtest archive (what setup-envtest verifies against).
ENVTEST_INDEX_URL ?= https://raw.githubusercontent.com/kubernetes-sigs/controller-tools/HEAD/envtest-releases.yaml

.PHONY: kustomize
kustomize: $(KUSTOMIZE) ## Download kustomize locally if necessary.
$(KUSTOMIZE): $(LOCALBIN)
	$(call go-install-tool,$(KUSTOMIZE),sigs.k8s.io/kustomize/kustomize/v5,$(KUSTOMIZE_VERSION))

.PHONY: setup-envtest
setup-envtest: $(LOCALBIN) ## Download the envtest binaries (etcd, kube-apiserver, kubectl) into bin/k8s.
	@if [ -x "$(ENVTEST_ASSETS_DIR)/etcd" ] && [ -x "$(ENVTEST_ASSETS_DIR)/kube-apiserver" ] && [ -x "$(ENVTEST_ASSETS_DIR)/kubectl" ]; then \
		echo "envtest binaries for Kubernetes $(ENVTEST_K8S_VERSION) present in $(ENVTEST_ASSETS_DIR)"; \
	else \
		set -e; \
		echo "Downloading envtest binaries for Kubernetes $(ENVTEST_K8S_VERSION) from $(ENVTEST_RELEASE_URL)"; \
		index="$$(mktemp)"; archive="$$(mktemp)"; trap 'rm -f "$$index" "$$archive"' EXIT; \
		curl -sSfL -o "$$index" "$(ENVTEST_INDEX_URL)"; \
		want="$$(awk -v a="$(ENVTEST_ARCHIVE):" '$$1 == a { getline; print $$2; exit }' "$$index")"; \
		if [ -z "$$want" ]; then echo "Error: $(ENVTEST_ARCHIVE) is not listed in $(ENVTEST_INDEX_URL)" >&2; exit 1; fi; \
		curl -sSfL -o "$$archive" "$(ENVTEST_RELEASE_URL)"; \
		got="$$(sha512sum "$$archive" | awk '{print $$1}')"; \
		if [ "$$got" != "$$want" ]; then echo "Error: sha512 mismatch for $(ENVTEST_ARCHIVE)" >&2; exit 1; fi; \
		mkdir -p "$(ENVTEST_ASSETS_DIR)"; \
		chmod u+w "$(ENVTEST_ASSETS_DIR)"; \
		tar -xzf "$$archive" -C "$(ENVTEST_ASSETS_DIR)" --strip-components=2 \
			controller-tools/envtest/etcd controller-tools/envtest/kube-apiserver controller-tools/envtest/kubectl; \
		echo "envtest binaries installed in $(ENVTEST_ASSETS_DIR)"; \
	fi

.PHONY: yq
yq: $(YQ) ## Download yq locally if necessary.
$(YQ): $(LOCALBIN)
	@[ -f "$(YQ)-$(YQ_VERSION)" ] || { \
		set -e; \
		echo "Downloading yq $(YQ_VERSION)"; \
		curl -sSfL -o "$(YQ)-$(YQ_VERSION)" "https://github.com/mikefarah/yq/releases/download/$(YQ_VERSION)/yq_$(HOST_OS)_$(HOST_ARCH)"; \
		chmod +x "$(YQ)-$(YQ_VERSION)"; \
	}
	ln -sf "$(YQ)-$(YQ_VERSION)" "$(YQ)"

# go-install-tool will 'go install' any package with custom target and name of binary, if it doesn't exist
# $1 - target path with name of binary
# $2 - package url which can be installed
# $3 - specific version of package
define go-install-tool
@[ -f "$(1)-$(3)" ] || { \
set -e; \
package=$(2)@$(3) ;\
echo "Downloading $${package}" ;\
rm -f $(1) || true ;\
GOBIN=$(LOCALBIN) go install $${package} ;\
mv $(1) $(1)-$(3) ;\
} ;\
ln -sf $(1)-$(3) $(1)
endef

.PHONY: operator-sdk
OPERATOR_SDK ?= $(LOCALBIN)/operator-sdk
operator-sdk: ## Download operator-sdk locally if necessary.
ifeq (,$(wildcard $(OPERATOR_SDK)))
ifeq (, $(shell which operator-sdk 2>/dev/null))
	@{ \
	set -e ;\
	mkdir -p $(dir $(OPERATOR_SDK)) ;\
	OS=$(HOST_OS) && ARCH=$(HOST_ARCH) && \
	curl -sSLo $(OPERATOR_SDK) https://github.com/operator-framework/operator-sdk/releases/download/$(OPERATOR_SDK_VERSION)/operator-sdk_$${OS}_$${ARCH} ;\
	chmod +x $(OPERATOR_SDK) ;\
	}
else
OPERATOR_SDK = $(shell which operator-sdk)
endif
endif

.PHONY: bundle
bundle: manifests kustomize operator-sdk ## Generate bundle manifests and metadata, then validate generated files.
	$(OPERATOR_SDK) generate kustomize manifests -q
	cd config/manager && $(KUSTOMIZE) edit set image controller=$(IMG)
	$(KUSTOMIZE) build config/manifests | $(OPERATOR_SDK) generate bundle $(BUNDLE_GEN_FLAGS)
	$(OPERATOR_SDK) bundle validate ./bundle

.PHONY: bundle-build
bundle-build: ## Build the bundle image.
	$(CONTAINER_TOOL) build -f bundle.Containerfile -t $(BUNDLE_IMG) .

.PHONY: bundle-push
bundle-push: ## Push the bundle image.
	$(MAKE) container-push IMG=$(BUNDLE_IMG)

.PHONY: opm
OPM = $(LOCALBIN)/opm
opm: ## Download opm locally if necessary.
ifeq (,$(wildcard $(OPM)))
ifeq (,$(shell which opm 2>/dev/null))
	@{ \
	set -e ;\
	mkdir -p $(dir $(OPM)) ;\
	OS=$(HOST_OS) && ARCH=$(HOST_ARCH) && \
	curl -sSLo $(OPM) https://github.com/operator-framework/operator-registry/releases/download/v1.55.0/$${OS}-$${ARCH}-opm ;\
	chmod +x $(OPM) ;\
	}
else
OPM = $(shell which opm)
endif
endif

# A comma-separated list of bundle images (e.g. make catalog-build BUNDLE_IMGS=example.com/operator-bundle:v0.1.0,example.com/operator-bundle:v0.2.0).
# These images MUST exist in a registry and be pull-able.
BUNDLE_IMGS ?= $(BUNDLE_IMG)

# The image tag given to the resulting catalog image (e.g. make catalog-build CATALOG_IMG=example.com/operator-catalog:v0.2.0).
CATALOG_IMG ?= $(IMAGE_TAG_BASE)-catalog:v$(VERSION)

# Set CATALOG_BASE_IMG to an existing catalog image tag to add $BUNDLE_IMGS to that image.
ifneq ($(origin CATALOG_BASE_IMG), undefined)
FROM_INDEX_OPT := --from-index $(CATALOG_BASE_IMG)
endif

# Build a catalog image by adding bundle images to an empty catalog using the operator package manager tool, 'opm'.
# This recipe invokes 'opm' in 'semver' bundle add mode. For more information on add modes, see:
# https://github.com/operator-framework/community-operators/blob/7f1438c/docs/packaging-operator.md#updating-your-existing-operator
.PHONY: catalog-build
catalog-build: opm ## Build a catalog image.
	$(OPM) index add --container-tool $(CONTAINER_TOOL) --mode semver --tag $(CATALOG_IMG) --bundles $(BUNDLE_IMGS) $(FROM_INDEX_OPT)

# Push the catalog image.
.PHONY: catalog-push
catalog-push: ## Push a catalog image.
	$(MAKE) container-push IMG=$(CATALOG_IMG)
