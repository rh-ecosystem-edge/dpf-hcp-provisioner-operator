# Extract oc CLI from its official image
FROM quay.io/openshift/origin-cli:4.22 AS oc-cli

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

COPY hack/must-gather.sh /usr/bin/gather
RUN chmod +x /usr/bin/gather

# oc adm must-gather runs /usr/bin/gather as the entrypoint
ENTRYPOINT ["/usr/bin/gather"]
