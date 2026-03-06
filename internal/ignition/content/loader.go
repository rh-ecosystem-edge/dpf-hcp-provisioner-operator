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

package content

import (
	"fmt"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition"
)

// AddFiles adds files from a content provider to an ignition config
func AddFiles(ign *igntypes.Config, provider ContentProvider) error {
	files := provider.GetFiles()

	for _, fileDef := range files {
		var contents igntypes.Resource

		switch source := fileDef.ContentSource.(type) {
		case string:
			s := source
			contents = igntypes.Resource{
				Source: &s,
			}
		case []byte:
			var err error
			contents, err = ignition.EncodeGzipFile(source)
			if err != nil {
				return fmt.Errorf("failed to encode content for %s: %w", fileDef.Path, err)
			}
		default:
			return fmt.Errorf("invalid content source type for %s", fileDef.Path)
		}

		mode := fileDef.Mode
		entry := igntypes.File{
			Node: igntypes.Node{
				Path:      fileDef.Path,
				Overwrite: ignition.Ptr(true),
			},
			FileEmbedded1: igntypes.FileEmbedded1{
				Contents: contents,
				Mode:     &mode,
			},
		}

		ign.Storage.Files = append(ign.Storage.Files, entry)
	}

	return nil
}

// AddSystemdUnits adds systemd units from a content provider to an ignition config
func AddSystemdUnits(ign *igntypes.Config, provider ContentProvider) error {
	units, err := provider.GetSystemdUnits()
	if err != nil {
		return fmt.Errorf("failed to get systemd units: %w", err)
	}

	for _, unitDef := range units {
		contents := string(unitDef.Contents)
		unit := igntypes.Unit{
			Name:     unitDef.Name,
			Enabled:  ignition.Ptr(true),
			Contents: &contents,
		}

		ign.Systemd.Units = append(ign.Systemd.Units, unit)
	}

	return nil
}

// AddKernelArgs adds kernel arguments templating to the ignition config
func AddKernelArgs(ign *igntypes.Config) {
	ign.KernelArguments.ShouldExist = append(
		ign.KernelArguments.ShouldExist,
		igntypes.KernelArgument("{{.KernelParameters}}"),
	)
}

// AddContent is a convenience function that adds both files and systemd units
func AddContent(ign *igntypes.Config, provider ContentProvider) error {
	if err := AddFiles(ign, provider); err != nil {
		return err
	}
	if err := AddSystemdUnits(ign, provider); err != nil {
		return err
	}
	return nil
}
