/*
Copyright 2025.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package ignitiongenerator

import (
	"fmt"

	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
)

// isRedfishInstall reports whether the configured installation transport is Redfish.
func isRedfishInstall(cfg *operatorv1alpha1.DPFOperatorConfig) bool {
	if cfg == nil || cfg.Spec.ProvisioningController == nil {
		return false
	}
	installInterface := cfg.Spec.ProvisioningController.InstallInterface
	return installInterface != nil && installInterface.InstallViaRedfish != nil
}

// dpuModeFromFlavor reads DPUFlavor.spec.dpuMode (26.4 GA path).
func dpuModeFromFlavor(dpuFlavor *dpuprovisioningv1alpha1.DPUFlavor) (dpuprovisioningv1alpha1.DpuModeType, error) {
	if dpuFlavor == nil {
		return "", fmt.Errorf("DPUFlavor is nil")
	}

	mode := dpuFlavor.Spec.DpuMode

	switch mode {
	case dpuprovisioningv1alpha1.ZeroTrustMode:
		return dpuprovisioningv1alpha1.ZeroTrustMode, nil
	case dpuprovisioningv1alpha1.DpuMode:
		return dpuprovisioningv1alpha1.DpuMode, nil
	case dpuprovisioningv1alpha1.NicMode:
		return "", fmt.Errorf("NIC mode is not supported")
	case "":
		return "", fmt.Errorf("DPU mode is empty: set DPFOperatorConfig.spec.deploymentMode or DPUFlavor.spec.dpuMode")
	default:
		return "", fmt.Errorf("invalid DPU mode: %s (expected one of: zero-trust, dpu)", mode)
	}
}

// detectDPUMode resolves the effective DPU mode for ignition.
// DPUFlavor.spec.dpuMode is checked first so a newer HCP operator works with older DPF (26.4 GA).
// When flavor dpuMode is unset (26.8+), DPFOperatorConfig.spec.deploymentMode and legacy Redfish apply.
func detectDPUMode(
	cfg *operatorv1alpha1.DPFOperatorConfig,
	flavor *dpuprovisioningv1alpha1.DPUFlavor,
) (dpuprovisioningv1alpha1.DpuModeType, error) {
	if cfg == nil {
		return "", fmt.Errorf("DPFOperatorConfig is nil")
	}

	if flavor != nil && flavor.Spec.DpuMode != "" {
		mode, err := dpuModeFromFlavor(flavor)
		if err != nil {
			return "", fmt.Errorf("failed to detect DPU mode from flavor: %w", err)
		}
		return mode, nil
	}

	switch cfg.Spec.DeploymentMode {
	case operatorv1alpha1.DeploymentModeZeroTrust:
		return dpuprovisioningv1alpha1.ZeroTrustMode, nil
	case operatorv1alpha1.DeploymentModeHostTrusted:
		return dpuprovisioningv1alpha1.DpuMode, nil
	case "":
		if isRedfishInstall(cfg) {
			return dpuprovisioningv1alpha1.ZeroTrustMode, nil
		}
		return "", fmt.Errorf("DPU mode is empty: set DPFOperatorConfig.spec.deploymentMode or DPUFlavor.spec.dpuMode")
	default:
		return "", fmt.Errorf("unsupported deploymentMode %q", cfg.Spec.DeploymentMode)
	}
}

// isZeroTrustMode reports whether ignition should use zero-trust resources for this cluster.
func isZeroTrustMode(
	cfg *operatorv1alpha1.DPFOperatorConfig,
	flavor *dpuprovisioningv1alpha1.DPUFlavor,
) (zeroTrust bool, mode dpuprovisioningv1alpha1.DpuModeType, err error) {
	mode, err = detectDPUMode(cfg, flavor)
	if err != nil {
		return false, "", err
	}
	return mode == dpuprovisioningv1alpha1.ZeroTrustMode, mode, nil
}

func dpuModeDescription(mode dpuprovisioningv1alpha1.DpuModeType) string {
	switch mode {
	case dpuprovisioningv1alpha1.ZeroTrustMode:
		return "zero-trust (host untrusted, hardware-isolated)"
	case dpuprovisioningv1alpha1.DpuMode:
		return "dpu (host-trusted, standard mode)"
	default:
		return fmt.Sprintf("unknown (%s)", mode)
	}
}
