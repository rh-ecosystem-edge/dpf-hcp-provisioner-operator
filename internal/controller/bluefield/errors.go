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

package bluefield

import "fmt"

// ConfigMapNotFoundError indicates the ocp-bluefield-images ConfigMap was not found
// This is a transient error - the ConfigMap might be deployed soon
type ConfigMapNotFoundError struct {
	Err error
}

func (e *ConfigMapNotFoundError) Error() string {
	return fmt.Sprintf("ConfigMap ocp-bluefield-images not found in namespace dpf-hcp-provisioner-system: %v", e.Err)
}

func (e *ConfigMapNotFoundError) Unwrap() error {
	return e.Err
}

// ConfigMapAccessDeniedError indicates the operator lacks RBAC permissions to read the ConfigMap
// This is a permanent error - admin must grant permissions
type ConfigMapAccessDeniedError struct {
	Err error
}

func (e *ConfigMapAccessDeniedError) Error() string {
	return fmt.Sprintf("Operator lacks RBAC permissions to read ConfigMap ocp-bluefield-images: %v", e.Err)
}

func (e *ConfigMapAccessDeniedError) Unwrap() error {
	return e.Err
}

// VersionNotFoundError indicates the OCP version was not found in the ConfigMap
// This is a permanent error - admin must add version mapping to ConfigMap
type VersionNotFoundError struct {
	Version           string
	AvailableVersions []string
}

func (e *VersionNotFoundError) Error() string {
	return fmt.Sprintf("BlueField image not found for OCP version %s (available versions: %v)", e.Version, e.AvailableVersions)
}

// InvalidImageFormatError indicates the ocpReleaseImage URL is malformed
// This is a permanent error - user must fix CR spec
type InvalidImageFormatError struct {
	Message string
	URL     string
}

func (e *InvalidImageFormatError) Error() string {
	return fmt.Sprintf("Invalid ocpReleaseImage format: %s (URL: %s)", e.Message, e.URL)
}

// InvalidBlueFieldImageURLError indicates the BlueField image URL in ConfigMap is malformed or empty
// This is a permanent error - admin must fix ConfigMap
type InvalidBlueFieldImageURLError struct {
	Version string
	URL     string
	Message string
}

func (e *InvalidBlueFieldImageURLError) Error() string {
	return fmt.Sprintf("BlueField image URL is invalid for OCP version %s: %s (URL: %s)", e.Version, e.Message, e.URL)
}
