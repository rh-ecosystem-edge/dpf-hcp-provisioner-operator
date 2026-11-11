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

import "testing"

func TestDPFMajorMinorFromVersion(t *testing.T) {
	tests := []struct {
		name    string
		version string
		want    string
		wantErr bool
	}{
		{
			name:    "strips v prefix and patch",
			version: "v26.4.1",
			want:    "26.4",
		},
		{
			name:    "no v prefix",
			version: "26.4.0",
			want:    "26.4",
		},
		{
			name:    "strips git suffix",
			version: "26.4.0-f314aa17",
			want:    "26.4",
		},
		{
			name:    "prerelease suffix",
			version: "v26.4.0-rc.1",
			want:    "26.4",
		},
		{
			name:    "major only",
			version: "v26",
			wantErr: true,
		},
		{
			name:    "empty",
			version: "",
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := DPFMajorMinorFromVersion(tt.version)
			if tt.wantErr {
				if err == nil {
					t.Fatalf("expected error, got %q", got)
				}
				return
			}
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if got != tt.want {
				t.Fatalf("got %q, want %q", got, tt.want)
			}
		})
	}
}
