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

package bfocplookup

import (
	_ "embed"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
)

//go:embed dpf_doca_compatibility.json
var dpfDocaCompatibilityJSON []byte

// dpfToDocaVersion maps DPF major.minor versions (e.g. "26.4") to the DOCA
// major.minor version (e.g. "3.4") that the corresponding BlueField OCP layer
// images are built with. When a new DPF release ships with a different DOCA
// version, add a new entry to dpf_doca_compatibility.json.
var dpfToDocaVersion map[string]string

func init() {
	if err := json.Unmarshal(dpfDocaCompatibilityJSON, &dpfToDocaVersion); err != nil {
		panic(fmt.Sprintf("failed to parse embedded dpf_doca_compatibility.json: %v", err))
	}
}

// DocaVersionForDPF returns the DOCA major.minor version (e.g. "3.4") compatible
// with the given DPF major.minor version (e.g. "26.4").
func DocaVersionForDPF(dpfMajorMinor string) (string, error) {
	docaMajorMinor, ok := dpfToDocaVersion[dpfMajorMinor]
	if !ok {
		return "", fmt.Errorf("DPF version %q has no compatible DOCA version, supported DPF versions: %s",
			dpfMajorMinor, strings.Join(SupportedDPFVersions(), ", "))
	}
	return docaMajorMinor, nil
}

// SupportedDPFVersions returns the sorted list of DPF major.minor versions in the
// compatibility matrix.
func SupportedDPFVersions() []string {
	supported := make([]string, 0, len(dpfToDocaVersion))
	for v := range dpfToDocaVersion {
		supported = append(supported, v)
	}
	sort.Strings(supported)
	return supported
}
