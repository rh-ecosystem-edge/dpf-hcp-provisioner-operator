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

// IsOwnedByProvisioner checks if a resource's labels indicate ownership by the given DPFHCPProvisioner.
// Used for cross-namespace resources where OwnerReferences cannot be used.
func IsOwnedByProvisioner(labels map[string]string, provisionerName, provisionerNamespace string) bool {
	if len(labels) == 0 {
		return false
	}
	return labels[LabelDPFHCPProvisionerName] == provisionerName &&
		labels[LabelDPFHCPProvisionerNamespace] == provisionerNamespace
}
