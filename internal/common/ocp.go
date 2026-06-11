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
	"fmt"
	"strings"
)

// ExtractOCPVersion extracts the OCP version from an OCP release image URL.
// It strips architecture suffixes like -multi, -amd64, etc.
func ExtractOCPVersion(ocpReleaseImage string) (string, error) {
	parts := strings.Split(ocpReleaseImage, ":")
	if len(parts) < 2 {
		return "", fmt.Errorf("missing tag separator ':' in image URL")
	}
	tag := parts[len(parts)-1]

	if tag == "" {
		return "", fmt.Errorf("empty tag in image URL")
	}

	suffixes := []string{"-multi", "-amd64", "-arm64", "-ppc64le", "-s390x", "-x86_64"}
	version := tag
	for _, suffix := range suffixes {
		version = strings.TrimSuffix(version, suffix)
	}

	if version == "" {
		return "", fmt.Errorf("extracted version is empty after processing tag: %s", tag)
	}

	return version, nil
}
