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

// Ptr returns a pointer to the given value.
func Ptr[T any](v T) *T {
	return &v
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
