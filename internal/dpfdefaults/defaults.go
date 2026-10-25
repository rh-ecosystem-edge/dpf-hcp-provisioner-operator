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

// Package dpfdefaults holds the embedded per-DPF-version table: the DOCA
// version used by BlueField OCP layer lookup, and the chart and image pins
// used to create DPUServiceTemplates. Callers pass a DPF major.minor (from
// common.DPFMajorMinorFromVersion); the table is parsed once at startup.
package dpfdefaults

import (
	"context"
	_ "embed"
	"encoding/json"
	"fmt"
	"sort"
	"sync"

	logf "sigs.k8s.io/controller-runtime/pkg/log"
)

//go:embed dpfversions.json
var dpfVersionsJSON []byte

// defaultKey is the JSON map key used when the running DPF major.minor
// version has no dedicated entry (e.g. DPF main or a new GA minor).
const defaultKey = "default"

// OVNTemplateValues holds the chart settings for the OVN DPUServiceTemplate.
type OVNTemplateValues struct {
	ChartRepoURL string `json:"chartRepoURL"`
	ChartName    string `json:"chartName"`
	ChartVersion string `json:"chartVersion"`
}

// DTSTemplateValues holds the chart and image settings for the DOCA Telemetry Service DPUServiceTemplate.
type DTSTemplateValues struct {
	ChartRepoURL   string `json:"chartRepoURL"`
	ChartName      string `json:"chartName"`
	ChartVersion   string `json:"chartVersion"`
	ImageDTS       string `json:"imageDTS"`
	PrometheusPort int32  `json:"prometheusPort"`
}

// HBNTemplateValues holds the chart and image settings for the HBN DPUServiceTemplate.
type HBNTemplateValues struct {
	ChartRepoURL string `json:"chartRepoURL"`
	ChartName    string `json:"chartName"`
	ChartVersion string `json:"chartVersion"`
	ImageRepo    string `json:"imageRepo"`
	ImageTag     string `json:"imageTag"`
	BFSFCount    int32  `json:"bfSFCount"`
}

// DPUServiceTemplateValues holds the chart and image pins for creating
// DPUServiceTemplates for one DPF version. ImagePullSecret is not part of
// the embedded table; the template manager sets it from operator config.
type DPUServiceTemplateValues struct {
	OVN             OVNTemplateValues `json:"ovn"`
	DTS             DTSTemplateValues `json:"dts"`
	HBN             HBNTemplateValues `json:"hbn"`
	ImagePullSecret string            `json:"-"`
}

// versionEntry is one row of dpfversions.json.
type versionEntry struct {
	DocaVersion string            `json:"docaVersion"`
	OVN         OVNTemplateValues `json:"ovn"`
	DTS         DTSTemplateValues `json:"dts"`
	HBN         HBNTemplateValues `json:"hbn"`
}

// byMajorMinor maps DPF major.minor versions (e.g. "26.4") to their row.
// When a new DPF version is released, add an entry to dpfversions.json and
// keep "default" pointing at the values to use for versions without one.
var byMajorMinor map[string]versionEntry

// loggedDefaults records DPF versions whose fallback to "default" was already
// logged. Several controllers look up values on every reconcile.
var loggedDefaults sync.Map

func init() {
	if err := json.Unmarshal(dpfVersionsJSON, &byMajorMinor); err != nil {
		panic(fmt.Sprintf("failed to parse embedded dpfversions.json: %v", err))
	}
	if _, ok := byMajorMinor[defaultKey]; !ok {
		panic(fmt.Sprintf("embedded dpfversions.json is missing %q", defaultKey))
	}
}

func entry(ctx context.Context, majorMinor string) (versionEntry, error) {
	if majorMinor == "" {
		return versionEntry{}, fmt.Errorf("unsupported DPF version %q", majorMinor)
	}
	if row, ok := byMajorMinor[majorMinor]; ok {
		return row, nil
	}
	row := byMajorMinor[defaultKey]
	if _, logged := loggedDefaults.LoadOrStore(majorMinor, struct{}{}); !logged {
		logf.FromContext(ctx).Info("No embedded values for DPF version; using default",
			"requestedVersion", majorMinor, "defaultDocaVersion", row.DocaVersion)
	}
	return row, nil
}

// DocaVersion returns the DOCA major.minor (e.g. "3.4") for a DPF major.minor.
// Versions without a dedicated row use the "default" row.
func DocaVersion(ctx context.Context, majorMinor string) (string, error) {
	row, err := entry(ctx, majorMinor)
	if err != nil {
		return "", err
	}
	if row.DocaVersion == "" {
		return "", fmt.Errorf("docaVersion is empty for DPF version %q", majorMinor)
	}
	return row.DocaVersion, nil
}

// TemplateValues returns a copy of the DPUServiceTemplate pins for a DPF
// major.minor. Versions without a dedicated row use the "default" row.
// The caller may mutate the result (overrides, image pull secret).
func TemplateValues(ctx context.Context, majorMinor string) (*DPUServiceTemplateValues, error) {
	row, err := entry(ctx, majorMinor)
	if err != nil {
		return nil, err
	}
	return &DPUServiceTemplateValues{
		OVN: row.OVN,
		DTS: row.DTS,
		HBN: row.HBN,
	}, nil
}

// SupportedDPFVersions returns the sorted DPF major.minor versions with a
// dedicated row (excluding "default").
func SupportedDPFVersions() []string {
	supported := make([]string, 0, len(byMajorMinor))
	for v := range byMajorMinor {
		if v == defaultKey {
			continue
		}
		supported = append(supported, v)
	}
	sort.Strings(supported)
	return supported
}
