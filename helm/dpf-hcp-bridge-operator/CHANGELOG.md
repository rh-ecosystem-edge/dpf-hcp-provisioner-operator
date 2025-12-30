# Changelog

All notable changes to the DPF-HCP Bridge Operator Helm chart will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2025-01-XX

### Added
- Initial Helm chart release for DPF-HCP Bridge Operator
- Support for deploying operator in `dpf-hcp-bridge-system` namespace
- CRD installation for `DPFHCPBridge` custom resource
- RBAC resources (ServiceAccount, ClusterRole, ClusterRoleBinding)
- Operator Deployment with configurable replicas
- ConfigMap `ocp-bluefield-images` for OCP-to-BlueField image mappings
- Comprehensive `values.yaml` with sensible defaults
- Template helpers for consistent labeling
- Health probe configuration (liveness and readiness)
- Security contexts following Pod Security Standards (restricted)
- Support for custom node selectors, tolerations, and affinity rules
- Post-installation notes (NOTES.txt)
- Production and development example values files
- Example DPFHCPBridge CRs (basic and HA configurations)
- Example secrets configuration
- Comprehensive README with installation and configuration guide
- Quick installation guide (INSTALL.md)
- .helmignore file for package optimization

### Configuration Options
- Configurable operator image repository and tag
- Configurable replica count (default: 1)
- Configurable resource limits and requests
- Configurable log level (debug, info, error)
- Leader election support
- BlueField image mappings via values.yaml
- Custom labels and annotations support
- Namespace customization

### Security Features
- Non-root container execution
- Seccomp profile (RuntimeDefault)
- Dropped all capabilities
- No privilege escalation

### Documentation
- Detailed README with prerequisites, installation, configuration, and troubleshooting
- Quick installation guide with step-by-step instructions
- Example configurations for different deployment scenarios
- Inline comments in values.yaml explaining all options

## [Unreleased]

### Planned
- Support for metrics Service and ServiceMonitor
- Support for webhooks (if operator adds webhook functionality)
- OLM bundle integration
- Network policies for enhanced security
- Additional deployment scenarios (air-gapped, multi-cluster)

---

## Version History

| Version | Release Date | Operator Version | Notes |
|---------|--------------|------------------|-------|
| 0.1.0   | 2025-01-XX   | v0.1.0          | Initial release |
