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
	"context"
	_ "embed"
	"encoding/json"
	"fmt"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
)

//go:embed dpuservicetemplate_values.json
var dpuServiceTemplateValuesJSON []byte

// OVNTemplateValues holds the chart settings for the OVN DPUServiceTemplate.
type OVNTemplateValues struct {
	ChartRepoURL string `json:"chartRepoURL"`
	ChartName    string `json:"chartName"`
	ChartVersion string `json:"chartVersion"`
}

// DTSTemplateValues holds the chart and image settings for the DOCA Telemetry Service DPUServiceTemplate.
type DTSTemplateValues struct {
	ChartRepoURL string `json:"chartRepoURL"`
	ChartName    string `json:"chartName"`
	ChartVersion string `json:"chartVersion"`
	ImageDTS     string `json:"imageDTS"`
}

// HBNTemplateValues holds the chart and image settings for the HBN DPUServiceTemplate.
type HBNTemplateValues struct {
	ChartRepoURL string `json:"chartRepoURL"`
	ChartName    string `json:"chartName"`
	ChartVersion string `json:"chartVersion"`
	ImageRepo    string `json:"imageRepo"`
	ImageTag     string `json:"imageTag"`
}

// DPUServiceTemplateValues holds all values for creating DPUServiceTemplates in a particular DPF version
type DPUServiceTemplateValues struct {
	OVN OVNTemplateValues `json:"ovn"`
	DTS DTSTemplateValues `json:"dts"`
	HBN HBNTemplateValues `json:"hbn"`
}

// dpfVersionToDPUServiceTemplateValuesMap maps DPF major versions to their corresponding DPUServiceTemplate values.
// When a new DPF version is released with different chart/image versions, add a new entry to dpuservicetemplate_values.json.
var dpfVersionToDPUServiceTemplateValuesMap map[string]DPUServiceTemplateValues

func init() {
	if err := json.Unmarshal(dpuServiceTemplateValuesJSON, &dpfVersionToDPUServiceTemplateValuesMap); err != nil {
		panic(fmt.Sprintf("failed to parse embedded dpuservicetemplate_values.json: %v", err))
	}
}

// DPUServiceTemplateValuesForVersion returns the DPUServiceTemplateValues for the given DPF major version
// (e.g., "26").
func DPUServiceTemplateValuesForVersion(majorVersion string) (*DPUServiceTemplateValues, error) {
	dpuServiceTemplateValues, ok := dpfVersionToDPUServiceTemplateValuesMap[majorVersion]
	if !ok {
		supported := make([]string, 0, len(dpfVersionToDPUServiceTemplateValuesMap))
		for k := range dpfVersionToDPUServiceTemplateValuesMap {
			supported = append(supported, k)
		}
		return nil, fmt.Errorf("unsupported DPF major version %q, supported: %v", majorVersion, supported)
	}

	return &dpuServiceTemplateValues, nil
}

const overridesConfigMapName = "dpuservicetemplate-overrides"

// applyOverridesFromConfigMap reads the [[overridesConfigMapName]] ConfigMap
// from the operator namespace and merges any matching major version overrides
// on top of the given hard-coded operator values. If the ConfigMap doesn't
// exist, the hard-coded values are returned unchanged.
func applyOverridesFromConfigMap(ctx context.Context, c client.Reader, operatorNamespace, majorVersion string, currentDPUServiceTemplateValues *DPUServiceTemplateValues) error {
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

	var allMajorVersionsOverrides map[string]json.RawMessage
	if err := json.Unmarshal([]byte(rawOverridesJSON), &allMajorVersionsOverrides); err != nil {
		return fmt.Errorf("parsing overrides configmap: %w", err)
	}

	currentMajorVersionOverride, ok := allMajorVersionsOverrides[majorVersion]
	if !ok {
		return nil
	}

	log.Info("Applying DPUServiceTemplate overrides from configmap", "major", majorVersion)

	// Unmarshal the override values into the currentDPUServiceTemplateValues,
	// which will merge the overrides on top of the existing values.
	return json.Unmarshal(currentMajorVersionOverride, currentDPUServiceTemplateValues)
}
