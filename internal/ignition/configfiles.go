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
	"fmt"
	"strconv"

	"github.com/coreos/ignition/v2/config/merge"
	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
)

const defaultFilePermissions = 0644

// MergeFlavorConfigFiles builds an ignition config from DPUFlavor ConfigFiles
// and merges it into the base ignition using Ignition's native merge logic
// (merge.MergeStructTranscribe). This is the same merge mechanism used by
// assisted-service's MergeIgnitionConfig.
//
// Files are keyed by path; when paths collide, the DPUFlavor version wins:
//   - override: replaces the base file's Contents
//   - append: preserves base Contents, concatenates Append entries
func MergeFlavorConfigFiles(base *igntypes.Config, flavorSpec *dpuprovisioningv1alpha1.DPUFlavorSpec) error {
	if len(flavorSpec.ConfigFiles) == 0 {
		return nil
	}

	flavorIgn := NewEmptyIgnition(base.Ignition.Version)
	if err := addFlavorFiles(flavorIgn, flavorSpec.ConfigFiles); err != nil {
		return err
	}

	merged, _ := merge.MergeStructTranscribe(*base, *flavorIgn)
	*base = merged.(igntypes.Config)
	return nil
}

// addFlavorFiles converts DPUFlavor ConfigFiles into ignition file entries
// and appends them to the ignition config.
func addFlavorFiles(ign *igntypes.Config, configFiles []dpuprovisioningv1alpha1.ConfigFile) error {
	for _, cf := range configFiles {
		if cf.Path == "" {
			continue
		}

		mode, err := parsePermissions(cf.Permissions)
		if err != nil {
			return fmt.Errorf("invalid permissions %q for config file %s: %w", cf.Permissions, cf.Path, err)
		}

		resource, err := EncodeGzipFile([]byte(cf.Raw))
		if err != nil {
			return fmt.Errorf("failed to encode config file %s: %w", cf.Path, err)
		}

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
