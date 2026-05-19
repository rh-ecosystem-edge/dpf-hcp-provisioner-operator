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

package common

import (
	"context"
	"fmt"

	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"
)

// OperatorConfig holds operator-wide configuration.
type OperatorConfig struct {
	BlueFieldOCPLayerRepo      string
	DisableMetalLB             bool
	ManageDPUServiceTemplates  bool
	DPUServicesImagePullSecret string
}

// LoadOperatorConfigFromCR fetches the DPFHCPProvisionerConfig singleton CR.
// Returns nil when the CR does not exist - the operator should do nothing until a user creates the config CR.
// Fields defaults are handled by CRD defaulting (kubebuilder markers).
func LoadOperatorConfigFromCR(ctx context.Context, c client.Client) (*OperatorConfig, error) {
	logger := log.FromContext(ctx)

	var configCR provisioningv1alpha1.DPFHCPProvisionerConfig
	err := c.Get(ctx, types.NamespacedName{Name: provisioningv1alpha1.DefaultConfigName}, &configCR)
	if err != nil {
		if apierrors.IsNotFound(err) {
			return nil, nil
		}
		return nil, err
	}

	cfg := &OperatorConfig{
		BlueFieldOCPLayerRepo:      configCR.Spec.BlueFieldOCPLayerRepo,
		DisableMetalLB:             configCR.Spec.DisableMetalLB,
		ManageDPUServiceTemplates:  configCR.Spec.ManageDPUServiceTemplates,
		DPUServicesImagePullSecret: configCR.Spec.DPUServicesImagePullSecret,
	}

	logger.V(1).Info("Operator config loaded from CR",
		"blueFieldOCPLayerRepo", cfg.BlueFieldOCPLayerRepo,
		"disableMetalLB", cfg.DisableMetalLB,
		"manageDPUServiceTemplates", cfg.ManageDPUServiceTemplates,
		"dpuServicesImagePullSecret", cfg.DPUServicesImagePullSecret)
	return cfg, nil
}

// GetSingletonDPFOperatorConfig returns the single DPFOperatorConfig in the cluster.
// Only one instance exists, but its namespace depends on where the DPF operator is installed.
func GetSingletonDPFOperatorConfig(ctx context.Context, c client.Client) (*operatorv1alpha1.DPFOperatorConfig, error) {
	var list operatorv1alpha1.DPFOperatorConfigList
	if err := c.List(ctx, &list); err != nil {
		return nil, fmt.Errorf("listing DPFOperatorConfig: %w", err)
	}

	if len(list.Items) != 1 {
		return nil, fmt.Errorf("expected exactly one DPFOperatorConfig, found %d", len(list.Items))
	}

	return &list.Items[0], nil
}
