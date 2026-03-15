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

// RegistryAccessError indicates the operator failed to access the container registry
// This is a transient error - the registry might be temporarily unavailable
type RegistryAccessError struct {
	Repository string
	Err        error
}

func (e *RegistryAccessError) Error() string {
	return fmt.Sprintf("failed to access registry repository %s: %v", e.Repository, e.Err)
}

func (e *RegistryAccessError) Unwrap() error {
	return e.Err
}

// RegistryAuthError indicates the operator lacks credentials to access the registry
// This is a permanent error - user must provide valid credentials
type RegistryAuthError struct {
	Repository string
	Err        error
}

func (e *RegistryAuthError) Error() string {
	return fmt.Sprintf("authentication failed for registry repository %s: %v", e.Repository, e.Err)
}

func (e *RegistryAuthError) Unwrap() error {
	return e.Err
}

// VersionNotFoundError indicates the OCP version tag was not found in the registry
// This is a permanent error - the image for this version hasn't been pushed yet
type VersionNotFoundError struct {
	Version    string
	Repository string
}

func (e *VersionNotFoundError) Error() string {
	return fmt.Sprintf("BlueField image not found for OCP version %s in repository %s", e.Version, e.Repository)
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

// InvalidBlueFieldImageURLError indicates the constructed BlueField image reference is invalid
// This is a permanent error
type InvalidBlueFieldImageURLError struct {
	Version string
	URL     string
	Message string
}

func (e *InvalidBlueFieldImageURLError) Error() string {
	return fmt.Sprintf("BlueField image URL is invalid for OCP version %s: %s (URL: %s)", e.Version, e.Message, e.URL)
}
