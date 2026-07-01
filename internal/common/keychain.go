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
	"context"
	"encoding/json"
	"fmt"

	"github.com/google/go-containerregistry/pkg/authn"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
)

type dockerConfig struct {
	Auths map[string]authn.AuthConfig `json:"auths"`
}

type dockerConfigKeychain struct {
	auths map[string]authn.AuthConfig
}

func (k *dockerConfigKeychain) Resolve(target authn.Resource) (authn.Authenticator, error) {
	if authConfig, ok := k.auths[target.RegistryStr()]; ok {
		return authn.FromConfig(authConfig), nil
	}
	return authn.Anonymous, nil
}

// NewKeychainFromDockerConfig creates an authn.Keychain from raw .dockerconfigjson bytes.
func NewKeychainFromDockerConfig(dockerConfigJSON []byte) (authn.Keychain, error) {
	var config dockerConfig
	if err := json.Unmarshal(dockerConfigJSON, &config); err != nil {
		return nil, fmt.Errorf("failed to unmarshal docker config: %w", err)
	}
	return &dockerConfigKeychain{auths: config.Auths}, nil
}

// KeychainFromPullSecret fetches a pull secret by name/namespace and returns an authn.Keychain.
func KeychainFromPullSecret(ctx context.Context, c client.Reader, name, namespace string) (authn.Keychain, error) {
	secret := &corev1.Secret{}
	if err := c.Get(ctx, types.NamespacedName{Name: name, Namespace: namespace}, secret); err != nil {
		return nil, fmt.Errorf("getting secret %s/%s: %w", namespace, name, err)
	}

	const key = ".dockerconfigjson"
	dockerConfigJSON, ok := secret.Data[key]
	if !ok {
		return nil, fmt.Errorf("secret %s/%s missing key %q", namespace, name, key)
	}

	return NewKeychainFromDockerConfig(dockerConfigJSON)
}
