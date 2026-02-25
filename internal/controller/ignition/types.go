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

// Ignition represents the top-level ignition configuration structure (v3.x).
type Ignition struct {
	Ignition        IgnitionConfig  `json:"ignition"`
	Storage         StorageFiles    `json:"storage,omitempty"`
	Systemd         SystemdUnits    `json:"systemd,omitempty"`
	KernelArguments KernelArguments `json:"kernelArguments,omitempty"`
	Passwd          Passwd          `json:"passwd,omitempty"`
}

// IgnitionConfig holds the ignition version metadata.
type IgnitionConfig struct {
	Version string `json:"version"`
}

// StorageFiles holds the list of files to be written by ignition.
type StorageFiles struct {
	Files []FileEntry `json:"files,omitempty"`
}

// SystemdUnits holds the list of systemd units to be configured by ignition.
type SystemdUnits struct {
	Units []SystemdUnit `json:"units,omitempty"`
}

// FileEntry describes a single file to be written by ignition.
type FileEntry struct {
	Path      string        `json:"path"`
	Mode      *int          `json:"mode,omitempty"`
	Contents  *FileContents `json:"contents,omitempty"`
	Overwrite *bool         `json:"overwrite,omitempty"`
}

// FileContents holds the encoded content and optional compression type for a file.
type FileContents struct {
	Source      string `json:"source,omitempty"`
	Compression string `json:"compression,omitempty"`
}

// SystemdUnit describes a systemd unit to be installed by ignition.
type SystemdUnit struct {
	Name     string  `json:"name"`
	Enabled  *bool   `json:"enabled,omitempty"`
	Contents *string `json:"contents,omitempty"`
}

// KernelArguments specifies kernel arguments to add or remove.
type KernelArguments struct {
	ShouldExist    []string `json:"shouldExist,omitempty"`
	ShouldNotExist []string `json:"shouldNotExist,omitempty"`
}

// Passwd holds the user account definitions.
type Passwd struct {
	Users []PasswdUser `json:"users,omitempty"`
}

// PasswdUser describes a user account to be configured by ignition.
type PasswdUser struct {
	Name              string   `json:"name"`
	Groups            []string `json:"groups,omitempty"`
	PasswordHash      *string  `json:"passwordHash,omitempty"`
	SSHAuthorizedKeys []string `json:"sshAuthorizedKeys,omitempty"`
}
