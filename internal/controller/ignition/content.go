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

	corev1 "k8s.io/api/core/v1"
	"k8s.io/utils/ptr"
	"sigs.k8s.io/yaml"
)

// contentIndex is the parsed structure of the _index key in a content ConfigMap.
type contentIndex struct {
	Files   []fileIndexEntry `yaml:"files"`
	Systemd []string         `yaml:"systemd"`
}

// fileIndexEntry describes one file entry in the content index.
type fileIndexEntry struct {
	// Name is the ConfigMap key holding this file's content.
	Name string `yaml:"name"`
	// Path is the destination path on the target system.
	Path string `yaml:"path"`
	// Mode is the file permission mode. Write as YAML octal literal (e.g. 0755)
	// so the YAML parser converts it to the correct decimal integer for ignition.
	Mode int `yaml:"mode"`
}

// ContentProvider holds pre-parsed ignition content loaded from a ConfigMap.
type ContentProvider struct {
	files []FileEntry
	units []SystemdUnit
}

// loadContentFromConfigMap parses a content ConfigMap (with an _index key) and returns
// a ContentProvider ready to be merged into an Ignition config.
func loadContentFromConfigMap(cm *corev1.ConfigMap) (*ContentProvider, error) {
	indexData, ok := cm.Data["_index"]
	if !ok {
		return nil, fmt.Errorf("configmap %s/%s missing _index key", cm.Namespace, cm.Name)
	}

	var idx contentIndex
	if err := yaml.Unmarshal([]byte(indexData), &idx); err != nil {
		return nil, fmt.Errorf("parsing _index in %s/%s: %w", cm.Namespace, cm.Name, err)
	}

	provider := &ContentProvider{}

	for _, entry := range idx.Files {
		content, ok := cm.Data[entry.Name]
		if !ok {
			return nil, fmt.Errorf("configmap %s/%s missing key %q referenced in _index",
				cm.Namespace, cm.Name, entry.Name)
		}
		mode := entry.Mode
		fc := EncodeGzipFile([]byte(content))
		provider.files = append(provider.files, FileEntry{
			Path:     entry.Path,
			Mode:     &mode,
			Contents: &fc,
		})
	}

	for _, name := range idx.Systemd {
		content, ok := cm.Data[name]
		if !ok {
			return nil, fmt.Errorf("configmap %s/%s missing key %q referenced in _index",
				cm.Namespace, cm.Name, name)
		}
		provider.units = append(provider.units, SystemdUnit{
			Name:     name,
			Enabled:  ptr.To(true),
			Contents: ptr.To(content),
		})
	}

	return provider, nil
}

// addContentToIgnition appends the provider's files and systemd units to the ignition config.
func addContentToIgnition(ign *Ignition, provider *ContentProvider) {
	ign.Storage.Files = append(ign.Storage.Files, provider.files...)
	ign.Systemd.Units = append(ign.Systemd.Units, provider.units...)
}
