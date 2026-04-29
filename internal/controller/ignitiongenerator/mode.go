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

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
)

// DetectDPUMode extracts and validates the DPU mode from DPUFlavor.
// Returns the detected mode or an error if invalid.
//
// The mode is used to determine what type of ignition configuration to generate:
// - zero-trust: Host is untrusted, DPU acts as security barrier with hardware-level restrictions
// - dpu: Standard DPU mode (host-trusted) with normal host-DPU communication
// - nic: NIC mode is not supported and will return an error
//
// If the mode is empty (which shouldn't happen if the DOCA Platform webhook is working),
// it defaults to "dpu" mode following the same logic as the webhook.
func DetectDPUMode(dpuFlavor *dpuprovisioningv1alpha1.DPUFlavor) (dpuprovisioningv1alpha1.DpuModeType, error) {
	if dpuFlavor == nil {
		return "", fmt.Errorf("DPUFlavor is nil")
	}

	mode := dpuFlavor.Spec.DpuMode

	// Validate mode (should already be validated by webhook, but double-check)
	switch mode {
	case dpuprovisioningv1alpha1.ZeroTrustMode:
		return dpuprovisioningv1alpha1.ZeroTrustMode, nil
	case dpuprovisioningv1alpha1.DpuMode:
		return dpuprovisioningv1alpha1.DpuMode, nil
	case dpuprovisioningv1alpha1.NicMode:
		return "", fmt.Errorf("NIC mode is not supported")
	case "":
		// Should not happen if webhook is working, but default gracefully.
		// Following the same logic as the webhook: default to dpu mode.
		// See: github.com/nvidia/doca-platform/internal/provisioning/webhooks/dpuflavor_webhook.go
		return dpuprovisioningv1alpha1.DpuMode, nil
	default:
		return "", fmt.Errorf("invalid DPU mode: %s (expected one of: zero-trust, dpu)", mode)
	}
}

// IsZeroTrustMode returns true if the mode is zero-trust.
// Helper function for future use when we implement mode-specific logic.
func IsZeroTrustMode(mode dpuprovisioningv1alpha1.DpuModeType) bool {
	return mode == dpuprovisioningv1alpha1.ZeroTrustMode
}

// GetModeDescription returns a human-readable description of the mode.
// Useful for logging and status messages.
func GetModeDescription(mode dpuprovisioningv1alpha1.DpuModeType) string {
	switch mode {
	case dpuprovisioningv1alpha1.ZeroTrustMode:
		return "zero-trust (host untrusted, hardware-isolated)"
	case dpuprovisioningv1alpha1.DpuMode:
		return "dpu (host-trusted, standard mode)"
	default:
		return fmt.Sprintf("unknown (%s)", mode)
	}
}
