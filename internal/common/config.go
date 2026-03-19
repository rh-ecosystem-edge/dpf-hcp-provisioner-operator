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

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"
)

// OperatorConfig holds operator-wide configuration.
type OperatorConfig struct {
	BlueFieldOCPRepo          string
	EnableBlueFieldValidation bool
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
		BlueFieldOCPRepo:          configCR.Spec.BlueFieldOCPRepo,
		EnableBlueFieldValidation: configCR.Spec.EnableBlueFieldValidation,
	}

	logger.V(1).Info("Operator config loaded from CR",
		"blueFieldOCPRepo", cfg.BlueFieldOCPRepo,
		"enableBlueFieldValidation", cfg.EnableBlueFieldValidation)
	return cfg, nil
}
