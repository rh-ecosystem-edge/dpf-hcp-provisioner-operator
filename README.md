# dpf-hcp-provisioner-operator
A Kubernetes controller that abstracts Hypershift complexity for DPF. It orchestrates the lifecycle of HostedClusters, manages CSR approvals for DPUs, and syncs status to the DPUCluster API, treating the control plane as a "black box" for the end user.

# Rename

This operator was initially called "DPF HCP Bridge Operator" and may still appear as such in old documents, tickets and git history.
