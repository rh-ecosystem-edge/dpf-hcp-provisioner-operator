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
	"strings"
	"testing"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
)

func TestKeychainPrefersRepositoryScopedAuth(t *testing.T) {
	kc, err := NewKeychainFromDockerConfig([]byte(`{
		"auths": {
			"quay.io": {"username": "generic", "password": "generic-pass"},
			"quay.io/edge-infrastructure/bluefield-ocp": {"username": "repo", "password": "repo-pass"}
		}
	}`))
	if err != nil {
		t.Fatalf("NewKeychainFromDockerConfig: %v", err)
	}

	tests := []struct {
		name   string
		target string
		want   string
	}{
		{
			name:   "repository-scoped entry wins for that repository",
			target: "quay.io/edge-infrastructure/bluefield-ocp",
			want:   "repo",
		},
		{
			name:   "registry entry is used for the registry itself",
			target: "quay.io",
			want:   "generic",
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			target, err := parseAuthTarget(tt.target)
			if err != nil {
				t.Fatalf("parse auth target: %v", err)
			}
			auth, err := kc.Resolve(target)
			if err != nil {
				t.Fatalf("Resolve: %v", err)
			}
			cfg, err := auth.Authorization()
			if err != nil {
				t.Fatalf("Authorization: %v", err)
			}
			if cfg.Username != tt.want {
				t.Fatalf("username = %q, want %q", cfg.Username, tt.want)
			}
		})
	}
}

func parseAuthTarget(ref string) (authn.Resource, error) {
	if !strings.Contains(ref, "/") {
		return name.NewRegistry(ref)
	}
	return name.NewRepository(ref)
}

func TestKeychainFallsBackToRegistryAuth(t *testing.T) {
	kc, err := NewKeychainFromDockerConfig([]byte(`{
		"auths": {
			"quay.io": {"username": "generic", "password": "generic-pass"}
		}
	}`))
	if err != nil {
		t.Fatalf("NewKeychainFromDockerConfig: %v", err)
	}

	repo, err := name.NewRepository("quay.io/openshift-release-dev/ocp-release")
	if err != nil {
		t.Fatalf("NewRepository: %v", err)
	}
	auth, err := kc.Resolve(repo)
	if err != nil {
		t.Fatalf("Resolve: %v", err)
	}
	cfg, err := auth.Authorization()
	if err != nil {
		t.Fatalf("Authorization: %v", err)
	}
	if cfg.Username != "generic" {
		t.Fatalf("username = %q, want generic", cfg.Username)
	}
}

func TestKeychainAnonymousWhenNoMatch(t *testing.T) {
	kc, err := NewKeychainFromDockerConfig([]byte(`{"auths": {"registry.redhat.io": {"username": "rh", "password": "rh-pass"}}}`))
	if err != nil {
		t.Fatalf("NewKeychainFromDockerConfig: %v", err)
	}

	repo, err := name.NewRepository("quay.io/edge-infrastructure/bluefield-ocp")
	if err != nil {
		t.Fatalf("NewRepository: %v", err)
	}
	auth, err := kc.Resolve(repo)
	if err != nil {
		t.Fatalf("Resolve: %v", err)
	}
	cfg, err := auth.Authorization()
	if err != nil {
		t.Fatalf("Authorization: %v", err)
	}
	if cfg.Username != "" || cfg.Password != "" || cfg.Auth != "" {
		t.Fatalf("expected anonymous auth, got username %q", cfg.Username)
	}
}
