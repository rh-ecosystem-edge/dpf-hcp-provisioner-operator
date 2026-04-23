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
	"testing"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

func TestDetectDPUMode(t *testing.T) {
	tests := []struct {
		name          string
		dpuFlavor     *dpuprovisioningv1alpha1.DPUFlavor
		expectedMode  dpuprovisioningv1alpha1.DpuModeType
		expectedError bool
	}{
		{
			name: "zero-trust mode",
			dpuFlavor: &dpuprovisioningv1alpha1.DPUFlavor{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-flavor-zt",
					Namespace: "test-namespace",
				},
				Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
					DpuMode: dpuprovisioningv1alpha1.ZeroTrustMode,
				},
			},
			expectedMode:  dpuprovisioningv1alpha1.ZeroTrustMode,
			expectedError: false,
		},
		{
			name: "dpu mode",
			dpuFlavor: &dpuprovisioningv1alpha1.DPUFlavor{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-flavor-dpu",
					Namespace: "test-namespace",
				},
				Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
					DpuMode: dpuprovisioningv1alpha1.DpuMode,
				},
			},
			expectedMode:  dpuprovisioningv1alpha1.DpuMode,
			expectedError: false,
		},
		{
			name: "empty mode defaults to dpu",
			dpuFlavor: &dpuprovisioningv1alpha1.DPUFlavor{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-flavor-empty",
					Namespace: "test-namespace",
				},
				Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
					DpuMode: "",
				},
			},
			expectedMode:  dpuprovisioningv1alpha1.DpuMode,
			expectedError: false,
		},
		{
			name: "nic mode returns error",
			dpuFlavor: &dpuprovisioningv1alpha1.DPUFlavor{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-flavor-nic",
					Namespace: "test-namespace",
				},
				Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
					DpuMode: dpuprovisioningv1alpha1.NicMode,
				},
			},
			expectedMode:  "",
			expectedError: true,
		},
		{
			name: "invalid mode returns error",
			dpuFlavor: &dpuprovisioningv1alpha1.DPUFlavor{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-flavor-invalid",
					Namespace: "test-namespace",
				},
				Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
					DpuMode: "invalid-mode",
				},
			},
			expectedMode:  "",
			expectedError: true,
		},
		{
			name:          "nil DPUFlavor returns error",
			dpuFlavor:     nil,
			expectedMode:  "",
			expectedError: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			mode, err := DetectDPUMode(tt.dpuFlavor)

			if tt.expectedError {
				if err == nil {
					t.Errorf("expected error but got none")
				}
			} else {
				if err != nil {
					t.Errorf("unexpected error: %v", err)
				}
			}

			if mode != tt.expectedMode {
				t.Errorf("expected mode %s, got %s", tt.expectedMode, mode)
			}
		})
	}
}

func TestIsZeroTrustMode(t *testing.T) {
	tests := []struct {
		name     string
		mode     dpuprovisioningv1alpha1.DpuModeType
		expected bool
	}{
		{
			name:     "zero-trust mode returns true",
			mode:     dpuprovisioningv1alpha1.ZeroTrustMode,
			expected: true,
		},
		{
			name:     "dpu mode returns false",
			mode:     dpuprovisioningv1alpha1.DpuMode,
			expected: false,
		},
		{
			name:     "empty mode returns false",
			mode:     "",
			expected: false,
		},
		{
			name:     "invalid mode returns false",
			mode:     "invalid",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := IsZeroTrustMode(tt.mode)
			if result != tt.expected {
				t.Errorf("expected %v, got %v", tt.expected, result)
			}
		})
	}
}

func TestGetModeDescription(t *testing.T) {
	tests := []struct {
		name        string
		mode        dpuprovisioningv1alpha1.DpuModeType
		expected    string
		shouldMatch bool // if false, just check it contains "unknown"
	}{
		{
			name:        "zero-trust mode",
			mode:        dpuprovisioningv1alpha1.ZeroTrustMode,
			expected:    "zero-trust (host untrusted, hardware-isolated)",
			shouldMatch: true,
		},
		{
			name:        "dpu mode",
			mode:        dpuprovisioningv1alpha1.DpuMode,
			expected:    "dpu (host-trusted, standard mode)",
			shouldMatch: true,
		},
		{
			name:        "unknown mode",
			mode:        "invalid-mode",
			expected:    "unknown",
			shouldMatch: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := GetModeDescription(tt.mode)
			if tt.shouldMatch {
				if result != tt.expected {
					t.Errorf("expected %q, got %q", tt.expected, result)
				}
			} else {
				// For unknown modes, just check it contains "unknown"
				if result != "unknown ("+string(tt.mode)+")" {
					t.Errorf("expected description to be 'unknown (%s)', got %q", tt.mode, result)
				}
			}
		})
	}
}
