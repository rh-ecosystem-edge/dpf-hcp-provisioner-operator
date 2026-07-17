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
	"crypto/sha256"
	"embed"
	"encoding/hex"
	"fmt"
	"io/fs"
	"path/filepath"
	"sort"
)

// FileDefinition represents a file to be added to an ignition config
type FileDefinition struct {
	Path          string
	Mode          int
	ContentSource interface{} // Can be []byte for embedded files or string for data URIs
}

// SystemdUnitDefinition represents a systemd unit to be added
type SystemdUnitDefinition struct {
	Name     string
	Contents []byte
}

// ContentProvider interface for content modules
type ContentProvider interface {
	GetFiles() []FileDefinition
	GetSystemdUnits() ([]SystemdUnitDefinition, error)
}

// EmbeddedProvider is a reusable ContentProvider backed by embed.FS.
// It eliminates the boilerplate of per-file //go:embed vars and duplicated systemd loading.
type EmbeddedProvider struct {
	Files     []FileDefinition
	SystemdFS *embed.FS // nil if this provider has no systemd units
}

func (p *EmbeddedProvider) GetFiles() []FileDefinition {
	return p.Files
}

func (p *EmbeddedProvider) GetSystemdUnits() ([]SystemdUnitDefinition, error) {
	if p.SystemdFS == nil {
		return nil, nil
	}
	return LoadSystemdUnits(*p.SystemdFS, "systemd")
}

// LoadSystemdUnits reads all systemd unit files from an embedded filesystem directory.
func LoadSystemdUnits(fsys embed.FS, dir string) ([]SystemdUnitDefinition, error) {
	entries, err := fs.ReadDir(fsys, dir)
	if err != nil {
		return nil, err
	}

	sort.Slice(entries, func(i, j int) bool {
		return entries[i].Name() < entries[j].Name()
	})

	var units []SystemdUnitDefinition
	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}
		data, err := fs.ReadFile(fsys, filepath.Join(dir, entry.Name()))
		if err != nil {
			return nil, err
		}
		units = append(units, SystemdUnitDefinition{
			Name:     entry.Name(),
			Contents: data,
		})
	}
	return units, nil
}

// EmbedFile reads a file from an embed.FS and returns the bytes.
// Use this to build FileDefinition slices without per-file embed vars.
func EmbedFile(fsys embed.FS, path string) []byte {
	data, err := fs.ReadFile(fsys, path)
	if err != nil {
		panic(fmt.Sprintf("embedded file %q not found: %v", path, err))
	}
	return data
}

// ContentHash computes a SHA-256 hash over all files and systemd units in the
// provider. This can be used to detect when embedded content changes between
// operator versions.
func (p *EmbeddedProvider) ContentHash() string {
	h := sha256.New()
	for _, f := range p.Files {
		h.Write([]byte(f.Path))
		switch src := f.ContentSource.(type) {
		case []byte:
			h.Write(src)
		case string:
			h.Write([]byte(src))
		}
	}
	if p.SystemdFS != nil {
		units, _ := LoadSystemdUnits(*p.SystemdFS, "systemd")
		for _, u := range units {
			h.Write([]byte(u.Name))
			h.Write(u.Contents)
		}
	}
	return hex.EncodeToString(h.Sum(nil))[:12]
}
