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

// FileContents represents the contents of a file in an ignition config
type FileContents struct {
	Source      string `json:"source,omitempty"`
	Compression string `json:"compression,omitempty"`
}

// FileEntry represents a file to be created in the system
type FileEntry struct {
	Path      string       `json:"path"`
	Overwrite bool         `json:"overwrite"`
	Mode      int          `json:"mode"`
	Contents  FileContents `json:"contents"`
}

// SystemdUnit represents a systemd unit configuration
type SystemdUnit struct {
	Name     string `json:"name"`
	Enabled  bool   `json:"enabled"`
	Contents string `json:"contents,omitempty"`
}

// IgnitionConfig represents the ignition metadata
type IgnitionConfig struct {
	Version string                 `json:"version"`
	Config  map[string]interface{} `json:"config,omitempty"`
}

// StorageFiles contains the list of files to create
type StorageFiles struct {
	Files []FileEntry `json:"files"`
}

// SystemdUnits contains the list of systemd units
type SystemdUnits struct {
	Units []SystemdUnit `json:"units"`
}

// KernelArguments represents kernel parameters
type KernelArguments struct {
	ShouldExist    []string `json:"shouldExist"`
	ShouldNotExist []string `json:"shouldNotExist"`
}

// PasswdUser represents a user configuration
type PasswdUser struct {
	Name              string   `json:"name"`
	SSHAuthorizedKeys []string `json:"sshAuthorizedKeys,omitempty"`
	PasswordHash      string   `json:"passwordHash,omitempty"`
}

// Passwd contains user configurations
type Passwd struct {
	Users []PasswdUser `json:"users"`
}

// Ignition represents a complete ignition configuration
type Ignition struct {
	Ignition        IgnitionConfig   `json:"ignition"`
	Storage         StorageFiles     `json:"storage"`
	Systemd         SystemdUnits     `json:"systemd"`
	KernelArguments *KernelArguments `json:"kernelArguments,omitempty"`
	Passwd          *Passwd          `json:"passwd,omitempty"`
}

// Grub represents grub configuration
type Grub struct {
	KernelParameters []string `json:"kernelParameters"`
}

// NVConfig represents NVIDIA configuration
type NVConfig struct {
	Device     string   `json:"device"`
	Parameters []string `json:"parameters"`
}

// OVS represents Open vSwitch configuration
type OVS struct {
	RawConfigScript string `json:"rawConfigScript"`
}

// Flavor represents a DPU flavor configuration
type Flavor struct {
	BFCFGParameters []string   `json:"bfcfgParameters"`
	Grub            Grub       `json:"grub"`
	NVConfig        []NVConfig `json:"nvconfig"`
	OVS             OVS        `json:"ovs"`
}

// FlavorResponse represents the API response containing a flavor
type FlavorResponse struct {
	Spec Flavor `json:"spec"`
}
