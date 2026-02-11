# dpf-hcp-provisioner-operator
A Kubernetes controller that abstracts Hypershift complexity for DPF. It orchestrates the lifecycle of HostedClusters, manages CSR approvals for DPUs, and syncs status to the DPUCluster API, treating the control plane as a "black box" for the end user.
