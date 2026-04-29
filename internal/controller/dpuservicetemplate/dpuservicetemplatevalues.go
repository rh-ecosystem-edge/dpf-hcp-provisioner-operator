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
	"fmt"
	"strings"
)

// OVNTemplateValues holds the chart settings for the OVN DPUServiceTemplate.
type OVNTemplateValues struct {
	ChartRepoURL string
	ChartName    string
	ChartVersion string
}

// DTSTemplateValues holds the chart and image settings for the DOCA Telemetry Service DPUServiceTemplate.
type DTSTemplateValues struct {
	ChartRepoURL string
	ChartName    string
	ChartVersion string
	ImageDTS     string
}

// HBNTemplateValues holds the chart and image settings for the HBN DPUServiceTemplate.
type HBNTemplateValues struct {
	ChartRepoURL string
	ChartName    string
	ChartVersion string
	ImageRepo    string
	ImageTag     string
}

// DPUServiceTemplateValues holds all values for creating DPUServiceTemplates in a particular DPF version
type DPUServiceTemplateValues struct {
	OVN OVNTemplateValues
	DTS DTSTemplateValues
	HBN HBNTemplateValues
}

// dpfVersionToDPUServiceTemplateValuesMap maps DPF major versions to their corresponding DPUServiceTemplate values.
// When a new DPF version is released with different chart/image versions, add a new entry here.
var dpfVersionToDPUServiceTemplateValuesMap = map[string]DPUServiceTemplateValues{
	"26": {
		OVN: OVNTemplateValues{
			ChartRepoURL: "oci://ghcr.io/mellanox/charts",
			ChartName:    "ovn-kubernetes-chart",
			ChartVersion: "v26.4.0-ocpbeta",
		},
		DTS: DTSTemplateValues{
			ChartRepoURL: "https://helm.ngc.nvidia.com/nvidia/doca",
			ChartName:    "doca-telemetry",
			ChartVersion: "1.22.1",
			ImageDTS:     "nvcr.io/nvidia/doca/doca_telemetry:1.21.5-doca3.0.0",
		},
		HBN: HBNTemplateValues{
			ChartRepoURL: "https://helm.ngc.nvidia.com/nvidia/doca",
			ChartName:    "doca-hbn",
			ChartVersion: "1.0.3",
			ImageRepo:    "nvcr.io/nvidia/doca/doca_hbn",
			ImageTag:     "3.2.0-doca3.2.0",
		},
	},
}

// DPUServiceTemplateValuesForVersion returns the DPUServiceTemplateValues for the given DPF version string.
// The version string is expected to be in the format "major.minor.patch-hash" (e.g., "26.4.0-f314aa17").
// Lookup is by major version only.
func DPUServiceTemplateValuesForVersion(dpfVersion string) (*DPUServiceTemplateValues, error) {
	if dpfVersion == "" {
		return nil, fmt.Errorf("DPF version is empty")
	}

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
