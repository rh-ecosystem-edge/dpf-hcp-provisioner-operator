# Extract oc, gather scripts and their dependencies from the must-gather image
FROM registry.redhat.io/openshift4/ose-must-gather-rhel9:latest AS must-gather-builder

FROM registry.access.redhat.com/ubi10/ubi-minimal

LABEL \
    io.k8s.display-name="must-gather for dpf-hcp-provisioner-operator" \
    io.k8s.description="Collects diagnostics for the DPF HCP Provisioner Operator" \
    summary="must-gather for dpf-hcp-provisioner-operator"

# tar and rsync are required by oc adm must-gather — installed from RHEL10 repos
# (not copied from the rhel9 builder since they are dynamically linked)
RUN microdnf install -y tar rsync && microdnf clean all

# oc is a static Go binary — safe to copy directly
COPY --from=must-gather-builder /usr/bin/oc /usr/bin/oc
RUN ln -s /usr/bin/oc /usr/bin/kubectl

# Copy the standard OCP gather script and its dependencies.
# This can be removed once the official RHEL10-based must-gather image is available.
# At that point, the base image will be FROM ose-must-gather-rhel10
# all these files will be pre-installed, and only a single RUN mv /usr/bin/gather /usr/bin/gather_generic is needed.
COPY --from=must-gather-builder /usr/bin/gather /usr/bin/gather_generic
COPY --from=must-gather-builder /usr/bin/gather_* /usr/bin/
COPY --from=must-gather-builder /usr/bin/common.sh /usr/bin/common.sh
COPY --from=must-gather-builder /usr/bin/monitoring_common.sh /usr/bin/monitoring_common.sh
COPY --from=must-gather-builder /usr/bin/version /usr/bin/version
COPY --from=must-gather-builder /usr/bin/hostname /usr/bin/hostname
COPY --from=must-gather-builder /usr/bin/gzip /usr/bin/gzip
COPY --from=must-gather-builder /usr/bin/jq /usr/bin/jq
COPY --from=must-gather-builder /lib64/libjq.so.1 /lib64/libjq.so.1
COPY --from=must-gather-builder /lib64/libonig.so.5 /lib64/libonig.so.5

COPY must-gather/must-gather.sh /usr/bin/gather
RUN chmod +x /usr/bin/gather

ARG release=22
ARG version=v4

LABEL com.redhat.component="must-gather-rhel10" \
      name="dpu-kit-for-nvidia-operator/must-gather-rhel10" \
      version="${version}" \
      upstream-ref="${version}" \
      upstream-url="https://github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator" \
      url="https://github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator" \
      summary="DPU Kit for NVIDIA Operator - Must Gather" \
      io.k8s.display-name="DPU Kit for NVIDIA Operator - Must Gather" \
      description="DPU Kit for NVIDIA Operator - Must Gather" \
      io.k8s.description="DPU Kit for NVIDIA Operator - Must Gather" \
      distribution-scope="public" \
      release="${release}" \
      cpe="cpe:/a:redhat:dpu_kit:4.22::el10"

# oc adm must-gather runs /usr/bin/gather as the entrypoint
ENTRYPOINT ["/usr/bin/gather"]
