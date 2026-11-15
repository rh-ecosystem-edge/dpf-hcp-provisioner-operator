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

	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

func TestIsRedfishInstall(t *testing.T) {
	t.Run("nil operator config", func(t *testing.T) {
		if isRedfishInstall(nil) {
			t.Fatal("nil operator config must not select Redfish")
		}
	})

	tests := []struct {
		name         string
		provisioning *operatorv1alpha1.ProvisioningControllerConfiguration
		want         bool
	}{
		{name: "missing provisioning configuration"},
		{name: "missing install interface", provisioning: &operatorv1alpha1.ProvisioningControllerConfiguration{}},
		{
			name: "empty install interface",
			provisioning: &operatorv1alpha1.ProvisioningControllerConfiguration{
				InstallInterface: &operatorv1alpha1.ProvisioningInstallInterface{},
			},
		},
		{
			name: "hostagent",
			provisioning: &operatorv1alpha1.ProvisioningControllerConfiguration{
				InstallInterface: &operatorv1alpha1.ProvisioningInstallInterface{InstallViaHostAgent: &operatorv1alpha1.InstallViaHostAgent{}},
			},
		},
		{
			name: "legacy GNOI",
			provisioning: &operatorv1alpha1.ProvisioningControllerConfiguration{
				InstallInterface: &operatorv1alpha1.ProvisioningInstallInterface{InstallViaGNOI: &operatorv1alpha1.InstallViaGNOI{}},
			},
		},
		{
			name: "Redfish",
			provisioning: &operatorv1alpha1.ProvisioningControllerConfiguration{
				InstallInterface: &operatorv1alpha1.ProvisioningInstallInterface{InstallViaRedfish: &operatorv1alpha1.InstallViaRedfish{}},
			},
			want: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &operatorv1alpha1.DPFOperatorConfig{
				Spec: operatorv1alpha1.DPFOperatorConfigSpec{ProvisioningController: tt.provisioning},
			}
			if got := isRedfishInstall(cfg); got != tt.want {
				t.Fatalf("isRedfishInstall: want %v, got %v", tt.want, got)
			}
		})
	}
}

func TestDPUModeFromFlavor(t *testing.T) {
	tests := []struct {
		name          string
		dpuFlavor     *dpuprovisioningv1alpha1.DPUFlavor
		expectedMode  dpuprovisioningv1alpha1.DpuModeType
		expectedError bool
	}{
		{
			name: "zero-trust mode",
			dpuFlavor: &dpuprovisioningv1alpha1.DPUFlavor{
				Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
					DpuMode: dpuprovisioningv1alpha1.ZeroTrustMode,
				},
			},
			expectedMode: dpuprovisioningv1alpha1.ZeroTrustMode,
		},
		{
			name: "dpu mode",
			dpuFlavor: &dpuprovisioningv1alpha1.DPUFlavor{
				Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
					DpuMode: dpuprovisioningv1alpha1.DpuMode,
				},
			},
			expectedMode: dpuprovisioningv1alpha1.DpuMode,
		},
		{
			name: "empty mode returns error",
			dpuFlavor: &dpuprovisioningv1alpha1.DPUFlavor{
				Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{DpuMode: ""},
			},
			expectedError: true,
		},
		{
			name: "nic mode returns error",
			dpuFlavor: &dpuprovisioningv1alpha1.DPUFlavor{
				Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
					DpuMode: dpuprovisioningv1alpha1.NicMode,
				},
			},
			expectedError: true,
		},
		{
			name: "invalid mode returns error",
			dpuFlavor: &dpuprovisioningv1alpha1.DPUFlavor{
				Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{DpuMode: "invalid-mode"},
			},
			expectedError: true,
		},
		{
			name:          "nil DPUFlavor returns error",
			dpuFlavor:     nil,
			expectedError: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			mode, err := dpuModeFromFlavor(tt.dpuFlavor)
			if tt.expectedError {
				if err == nil {
					t.Fatal("expected error")
				}
				return
			}
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if mode != tt.expectedMode {
				t.Fatalf("expected mode %s, got %s", tt.expectedMode, mode)
			}
		})
	}
}

func TestIsZeroTrustModeResolution(t *testing.T) {
	hostTrustedOperator := &operatorv1alpha1.DPFOperatorConfig{
		Spec: operatorv1alpha1.DPFOperatorConfigSpec{
			DeploymentMode: operatorv1alpha1.DeploymentModeHostTrusted,
		},
	}
	ztOperator := &operatorv1alpha1.DPFOperatorConfig{
		Spec: operatorv1alpha1.DPFOperatorConfigSpec{
			DeploymentMode: operatorv1alpha1.DeploymentModeZeroTrust,
		},
	}
	legacyRedfish := &operatorv1alpha1.DPFOperatorConfig{
		Spec: operatorv1alpha1.DPFOperatorConfigSpec{
			ProvisioningController: &operatorv1alpha1.ProvisioningControllerConfiguration{
				InstallInterface: &operatorv1alpha1.ProvisioningInstallInterface{
					InstallViaRedfish: &operatorv1alpha1.InstallViaRedfish{},
				},
			},
		},
	}
	legacyHostAgent := &operatorv1alpha1.DPFOperatorConfig{
		Spec: operatorv1alpha1.DPFOperatorConfigSpec{
			ProvisioningController: &operatorv1alpha1.ProvisioningControllerConfiguration{
				InstallInterface: &operatorv1alpha1.ProvisioningInstallInterface{
					InstallViaHostAgent: &operatorv1alpha1.InstallViaHostAgent{},
				},
			},
		},
	}
	ztFlavor := &dpuprovisioningv1alpha1.DPUFlavor{
		ObjectMeta: metav1.ObjectMeta{Name: "zt"},
		Spec:       dpuprovisioningv1alpha1.DPUFlavorSpec{DpuMode: dpuprovisioningv1alpha1.ZeroTrustMode},
	}
	dpuFlavor := &dpuprovisioningv1alpha1.DPUFlavor{
		ObjectMeta: metav1.ObjectMeta{Name: "dpu"},
		Spec:       dpuprovisioningv1alpha1.DPUFlavorSpec{DpuMode: dpuprovisioningv1alpha1.DpuMode},
	}
	emptyFlavor := &dpuprovisioningv1alpha1.DPUFlavor{
		ObjectMeta: metav1.ObjectMeta{Name: "empty"},
	}

	tests := []struct {
		name        string
		cfg         *operatorv1alpha1.DPFOperatorConfig
		flavor      *dpuprovisioningv1alpha1.DPUFlavor
		wantZT      bool
		wantMode    dpuprovisioningv1alpha1.DpuModeType
		expectError bool
	}{
		{
			name:     "flavor dpuMode wins over host-trusted deploymentMode",
			cfg:      hostTrustedOperator,
			flavor:   ztFlavor,
			wantZT:   true,
			wantMode: dpuprovisioningv1alpha1.ZeroTrustMode,
		},
		{
			name:     "26.8 deploymentMode when flavor dpuMode unset",
			cfg:      ztOperator,
			flavor:   emptyFlavor,
			wantZT:   true,
			wantMode: dpuprovisioningv1alpha1.ZeroTrustMode,
		},
		{
			name:     "legacy redfish when flavor dpuMode unset",
			cfg:      legacyRedfish,
			flavor:   emptyFlavor,
			wantZT:   true,
			wantMode: dpuprovisioningv1alpha1.ZeroTrustMode,
		},
		{
			name:     "26.4 flavor zero-trust when deploymentMode unset",
			cfg:      legacyHostAgent,
			flavor:   ztFlavor,
			wantZT:   true,
			wantMode: dpuprovisioningv1alpha1.ZeroTrustMode,
		},
		{
			name:     "26.4 flavor dpu when deploymentMode unset",
			cfg:      legacyHostAgent,
			flavor:   dpuFlavor,
			wantZT:   false,
			wantMode: dpuprovisioningv1alpha1.DpuMode,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			gotZT, gotMode, err := isZeroTrustMode(tt.cfg, tt.flavor)
			if tt.expectError {
				if err == nil {
					t.Fatal("expected error")
				}
				return
			}
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if gotZT != tt.wantZT {
				t.Fatalf("zeroTrust: want %v, got %v", tt.wantZT, gotZT)
			}
			if gotMode != tt.wantMode {
				t.Fatalf("mode: want %s, got %s", tt.wantMode, gotMode)
			}
		})
	}
}
