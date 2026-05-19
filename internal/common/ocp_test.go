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
	"testing"
)

func TestExtractOCPVersion(t *testing.T) {
	tests := []struct {
		name     string
		image    string
		want     string
		wantErr  bool
		errMatch string
	}{
		{
			name:  "strips -multi suffix",
			image: "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
			want:  "4.19.0-ec.5",
		},
		{
			name:  "strips -x86_64 suffix",
			image: "quay.io/openshift-release-dev/ocp-release:4.18.2-rc.1-x86_64",
			want:  "4.18.2-rc.1",
		},
		{
			name:  "strips -amd64 suffix",
			image: "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-amd64",
			want:  "4.19.0-ec.5",
		},
		{
			name:  "strips -arm64 suffix",
			image: "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-arm64",
			want:  "4.19.0-ec.5",
		},
		{
			name:  "strips -ppc64le suffix",
			image: "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-ppc64le",
			want:  "4.19.0-ec.5",
		},
		{
			name:  "strips -s390x suffix",
			image: "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-s390x",
			want:  "4.19.0-ec.5",
		},
		{
			name:  "no suffix",
			image: "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5",
			want:  "4.19.0-ec.5",
		},
		{
			name:     "no tag separator",
			image:    "quay.io/openshift-release-dev/ocp-release",
			wantErr:  true,
			errMatch: "missing tag separator",
		},
		{
			name:     "empty tag",
			image:    "quay.io/openshift-release-dev/ocp-release:",
			wantErr:  true,
			errMatch: "empty tag",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := ExtractOCPVersion(tt.image)
			if tt.wantErr {
				if err == nil {
					t.Fatalf("expected error containing %q, got nil", tt.errMatch)
				}
				return
			}
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if got != tt.want {
				t.Errorf("got %q, want %q", got, tt.want)
			}
		})
	}
}
