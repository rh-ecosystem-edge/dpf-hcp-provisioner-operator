# Extract oc CLI from its official image
FROM registry.redhat.io/openshift4/ose-cli:latest AS oc-cli

FROM registry.access.redhat.com/ubi10/ubi-minimal

LABEL \
    io.k8s.display-name="must-gather for dpf-hcp-provisioner-operator" \
    io.k8s.description="Collects diagnostics for the DPF HCP Provisioner Operator" \
    summary="must-gather for dpf-hcp-provisioner-operator"

# tar and rsync are required by oc adm must-gather
RUN microdnf install -y tar rsync && microdnf clean all

# Copy oc from the official CLI image
COPY --from=oc-cli /usr/bin/oc /usr/bin/oc
RUN ln -s /usr/bin/oc /usr/bin/kubectl

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
