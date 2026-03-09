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

package bluefield

import (
	"encoding/json"
	"fmt"

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

// Resolve looks up credentials for the given registry in the docker config
func (k *dockerConfigKeychain) Resolve(target authn.Resource) (authn.Authenticator, error) {
	if authConfig, ok := k.auths[target.RegistryStr()]; ok {
		return authn.FromConfig(authConfig), nil
	}
	return authn.Anonymous, nil
}
