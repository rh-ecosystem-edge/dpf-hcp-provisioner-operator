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
	"strings"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition"
)

// AddFiles adds files from a content provider to an ignition config
func AddFiles(ign *ignition.Ignition, provider ContentProvider) error {
	files := provider.GetFiles()

	for _, fileDef := range files {
		var contents ignition.FileContents

		// Check if content source is a data URI (string) or binary data ([]byte)
		switch source := fileDef.ContentSource.(type) {
		case string:
			// Data URI - pass through directly
			contents = ignition.FileContents{
				Source: source,
			}
		case []byte:
			// Binary data - gzip and base64 encode
			var err error
			contents, err = ignition.EncodeGzipFile(source)
			if err != nil {
				return fmt.Errorf("failed to encode content for %s: %w", fileDef.Path, err)
			}
		default:
			return fmt.Errorf("invalid content source type for %s", fileDef.Path)
		}

		entry := ignition.FileEntry{
			Path:      fileDef.Path,
			Overwrite: true,
			Mode:      fileDef.Mode,
			Contents:  contents,
		}

		ign.Storage.Files = append(ign.Storage.Files, entry)
	}

	return nil
}

// AddSystemdUnits adds systemd units from a content provider to an ignition config
func AddSystemdUnits(ign *ignition.Ignition, provider ContentProvider) error {
	units, err := provider.GetSystemdUnits()
	if err != nil {
		return fmt.Errorf("failed to get systemd units: %w", err)
	}

	for _, unitDef := range units {
		unit := ignition.SystemdUnit{
			Name:     unitDef.Name,
			Enabled:  true,
			Contents: string(unitDef.Contents),
		}

		ign.Systemd.Units = append(ign.Systemd.Units, unit)
	}

	return nil
}

// AddKernelArgs adds kernel arguments templating to the ignition config
func AddKernelArgs(ign *ignition.Ignition) {
	if ign.KernelArguments == nil {
		ign.KernelArguments = &ignition.KernelArguments{
			ShouldExist:    []string{},
			ShouldNotExist: []string{},
		}
	}
	ign.KernelArguments.ShouldExist = append(ign.KernelArguments.ShouldExist, "{{.KernelParameters}}")
}

// AddContent is a convenience function that adds both files and systemd units
func AddContent(ign *ignition.Ignition, provider ContentProvider) error {
	if err := AddFiles(ign, provider); err != nil {
		return err
	}
	if err := AddSystemdUnits(ign, provider); err != nil {
		return err
	}
	return nil
}

// IsDataURI checks if a string is a data URI
func IsDataURI(s string) bool {
	return strings.HasPrefix(s, "data:")
}
