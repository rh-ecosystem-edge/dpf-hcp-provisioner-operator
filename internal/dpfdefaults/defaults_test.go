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

package dpfdefaults

import (
	"context"
	"testing"
)

func TestDocaVersion(t *testing.T) {
	tests := []struct {
		name       string
		majorMinor string
		want       string
		wantErr    bool
	}{
		{name: "DPF 26.4 maps to DOCA 3.4", majorMinor: "26.4", want: "3.4"},
		{name: "unknown DPF minor falls back to default row", majorMinor: "26.12", want: "3.4"},
		{name: "empty version errors", majorMinor: "", wantErr: true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := DocaVersion(context.Background(), tt.majorMinor)
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
				t.Errorf("DocaVersion(%q) = %q, want %q", tt.majorMinor, got, tt.want)
			}
		})
	}
}

func TestTemplateValues(t *testing.T) {
	known, err := TemplateValues(context.Background(), "26.4")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if known.OVN.ChartVersion != "v26.4.1-ocp-release-v4.22" {
		t.Errorf("OVN chart = %q", known.OVN.ChartVersion)
	}
	if known.DTS.ChartVersion != "1.25.5" {
		t.Errorf("DTS chart = %q", known.DTS.ChartVersion)
	}
	if known.HBN.ChartVersion != "3.4.0" || known.HBN.ImageTag != "3.4.0-doca3.4.0" {
		t.Errorf("HBN = %+v", known.HBN)
	}

	fallback, err := TemplateValues(context.Background(), "26.12")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if *fallback != *known {
		t.Errorf("fallback = %+v, want 26.4 row %+v", *fallback, *known)
	}

	if _, err := TemplateValues(context.Background(), ""); err == nil {
		t.Fatal("expected error for empty version")
	}
}

func TestTemplateValuesCopy(t *testing.T) {
	first, err := TemplateValues(context.Background(), "26.4")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	first.OVN.ChartVersion = "mutated"
	first.ImagePullSecret = "secret"

	second, err := TemplateValues(context.Background(), "26.4")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if second.OVN.ChartVersion == "mutated" || second.ImagePullSecret != "" {
		t.Fatalf("lookup returned shared state: %+v", second)
	}
}

func TestSupportedDPFVersions(t *testing.T) {
	got := SupportedDPFVersions()
	want := []string{"26.4"}
	if len(got) != len(want) {
		t.Fatalf("SupportedDPFVersions() = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("SupportedDPFVersions() = %v, want %v", got, want)
		}
	}
}
