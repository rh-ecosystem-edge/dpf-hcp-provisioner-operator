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
	"encoding/json"
	"fmt"
	"strings"

	"github.com/google/go-containerregistry/pkg/authn"
)

// dockerConfig represents the structure of a .dockerconfigjson file
type dockerConfig struct {
	Auths map[string]authn.AuthConfig `json:"auths"`
}

// dockerConfigKeychain implements authn.Keychain using a parsed .dockerconfigjson
type dockerConfigKeychain struct {
	auths map[string]authn.AuthConfig
}

// NewKeychainFromDockerConfig creates an authn.Keychain from raw .dockerconfigjson bytes
func NewKeychainFromDockerConfig(dockerConfigJSON []byte) (authn.Keychain, error) {
	var config dockerConfig
	if err := json.Unmarshal(dockerConfigJSON, &config); err != nil {
		return nil, fmt.Errorf("failed to unmarshal docker config: %w", err)
	}

	return &dockerConfigKeychain{auths: config.Auths}, nil
}

// Resolve looks up credentials for the given registry in the docker config.
// It tries the most specific match first (registry + path) then falls back to
// registry-only, matching Docker/podman behavior for scoped credentials.
func (k *dockerConfigKeychain) Resolve(target authn.Resource) (authn.Authenticator, error) {
	registry := target.RegistryStr()
	repo := target.String() // e.g. "quay.io/edge-infrastructure/bluefield-ocp"

	// Try most-specific match first (registry/org/repo, registry/org)
	// Walk up from the full repo path to just the registry
	for path := repo; path != ""; {
		if authConfig, ok := k.auths[path]; ok {
			return authn.FromConfig(authConfig), nil
		}
		// Strip last path component
		if i := strings.LastIndex(path, "/"); i >= 0 {
			path = path[:i]
		} else {
			break
		}
	}

	// Try exact registry match
	if authConfig, ok := k.auths[registry]; ok {
		return authn.FromConfig(authConfig), nil
	}

	return authn.Anonymous, nil
}
