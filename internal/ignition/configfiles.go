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

package ignition

import (
	"encoding/base64"
	"fmt"
	"strconv"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
)

const defaultFilePermissions = 0644

// AddFlavorConfigFiles adds the DPUFlavor ConfigFiles to the ignition config.
// Each ConfigFile is added as an ignition file entry at the specified path with
// the specified permissions. The operation field controls whether the file
// content overrides or appends to the target file.
func AddFlavorConfigFiles(ign *igntypes.Config, flavorSpec *dpuprovisioningv1alpha1.DPUFlavorSpec) error {
	for _, cf := range flavorSpec.ConfigFiles {
		if cf.Path == "" {
			continue
		}

		mode, err := parsePermissions(cf.Permissions)
		if err != nil {
			return fmt.Errorf("invalid permissions %q for config file %s: %w", cf.Permissions, cf.Path, err)
		}

		encoded := base64.StdEncoding.EncodeToString([]byte(cf.Raw))
		source := fmt.Sprintf("data:text/plain;charset=utf-8;base64,%s", encoded)
		resource := igntypes.Resource{Source: &source}

		switch cf.Operation {
		case dpuprovisioningv1alpha1.FileAppend:
			file := igntypes.File{
				Node: igntypes.Node{Path: cf.Path},
				FileEmbedded1: igntypes.FileEmbedded1{
					Mode:   Ptr(mode),
					Append: []igntypes.Resource{resource},
				},
			}
			ign.Storage.Files = append(ign.Storage.Files, file)

		default:
			file := igntypes.File{
				Node: igntypes.Node{
					Path:      cf.Path,
					Overwrite: Ptr(true),
				},
				FileEmbedded1: igntypes.FileEmbedded1{
					Mode:     Ptr(mode),
					Contents: resource,
				},
			}
			ign.Storage.Files = append(ign.Storage.Files, file)
		}
	}
	return nil
}

func parsePermissions(perms string) (int, error) {
	if perms == "" {
		return defaultFilePermissions, nil
	}
	mode, err := strconv.ParseInt(perms, 8, 32)
	if err != nil {
		return 0, fmt.Errorf("failed to parse octal permissions: %w", err)
	}
	return int(mode), nil
}
