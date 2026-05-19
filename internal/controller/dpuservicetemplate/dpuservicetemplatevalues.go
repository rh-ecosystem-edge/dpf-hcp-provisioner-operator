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
	_ "embed"
	"encoding/json"
	"fmt"
	"strings"
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

// DPUServiceTemplateValuesForVersion returns the DPUServiceTemplateValues for the given DPF version string.
// The version string is expected to be in the format "major.minor.patch-hash" (e.g., "26.4.0-f314aa17").
// Lookup is by major version only.
func DPUServiceTemplateValuesForVersion(dpfVersion string) (*DPUServiceTemplateValues, error) {
	// Extract major version: "26.4.0-f314aa17" -> "26"
	major := dpfVersion
	if idx := strings.Index(major, "."); idx > 0 {
		major = major[:idx]
	}

	dpuServiceTemplateValues, ok := dpfVersionToDPUServiceTemplateValuesMap[major]
	if !ok {
		supported := make([]string, 0, len(dpfVersionToDPUServiceTemplateValuesMap))
		for k := range dpfVersionToDPUServiceTemplateValuesMap {
			supported = append(supported, k)
		}
		return nil, fmt.Errorf("unsupported DPF version %q (major: %q), supported major versions: %v", dpfVersion, major, supported)
	}

	return &dpuServiceTemplateValues, nil
}
