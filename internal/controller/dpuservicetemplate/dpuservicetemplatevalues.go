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

package dpuservicetemplate

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/dpfdefaults"
)

const overridesConfigMapName = "dpuservicetemplate-overrides"

// applyOverridesFromConfigMap reads the [[overridesConfigMapName]] ConfigMap
// from the operator namespace and merges any matching version overrides
// on top of the given embedded values. If the ConfigMap doesn't
// exist, the embedded values are returned unchanged.
func applyOverridesFromConfigMap(ctx context.Context, c client.Reader, operatorNamespace, version string, currentDPUServiceTemplateValues *dpfdefaults.DPUServiceTemplateValues) error {
	log := logf.FromContext(ctx)

	var cm corev1.ConfigMap
	if err := c.Get(ctx, client.ObjectKey{Name: overridesConfigMapName, Namespace: operatorNamespace}, &cm); err != nil {
		if apierrors.IsNotFound(err) {
			return nil
		}
		return fmt.Errorf("getting overrides configmap: %w", err)
	}

	rawOverridesJSON, ok := cm.Data["overrides.json"]
	if !ok || rawOverridesJSON == "" {
		return nil
	}

	var allVersionOverrides map[string]json.RawMessage
	if err := json.Unmarshal([]byte(rawOverridesJSON), &allVersionOverrides); err != nil {
		return fmt.Errorf("parsing overrides configmap: %w", err)
	}

	currentVersionOverride, ok := allVersionOverrides[version]
	if !ok {
		return nil
	}

	log.Info("Applying DPUServiceTemplate overrides from configmap", "version", version)

	// Unmarshal the override values into the currentDPUServiceTemplateValues,
	// which will merge the overrides on top of the existing values.
	decoder := json.NewDecoder(bytes.NewReader(currentVersionOverride))
	decoder.DisallowUnknownFields()
	return decoder.Decode(currentDPUServiceTemplateValues)
}
