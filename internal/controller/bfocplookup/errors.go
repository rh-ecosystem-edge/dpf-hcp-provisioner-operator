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

package bfocplookup

import (
	"fmt"
)

// RegistryAccessError indicates the operator failed to access the container registry
// This is a transient error - the registry might be temporarily unavailable
type RegistryAccessError struct {
	Repository string
	Err        error
}

func (e *RegistryAccessError) Reason() string {
	return reasonRegistryAccessError
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

func (e *RegistryAuthError) Reason() string {
	return reasonRegistryAuthError
}

func (e *RegistryAuthError) Error() string {
	return fmt.Sprintf("authentication failed for registry repository %s: %v", e.Repository, e.Err)
}

func (e *RegistryAuthError) Unwrap() error {
	return e.Err
}

// VersionNotFoundError indicates no BlueField OCP layer image tag was found in the
// registry matching the OCP version and the compatible DOCA version
// This is a permanent error - the image for this version hasn't been pushed yet
type VersionNotFoundError struct {
	Version     string
	DocaVersion string
	Repository  string
}

func (e *VersionNotFoundError) Reason() string {
	return reasonVersionNotFound
}

func (e *VersionNotFoundError) Error() string {
	return fmt.Sprintf("BlueField OCP layer image not found for OCP version %s with DOCA version %s.x in repository %s",
		e.Version, e.DocaVersion, e.Repository)
}

// UnsupportedDPFVersionError indicates the embedded per-DPF-version table has
// neither an entry for the running DPF version nor a default row
// This is a permanent error - the operator build needs an updated version table
type UnsupportedDPFVersionError struct {
	DPFVersion string
	Err        error
}

func (e *UnsupportedDPFVersionError) Reason() string {
	return reasonUnsupportedDPFVersion
}

func (e *UnsupportedDPFVersionError) Error() string {
	return fmt.Sprintf("no DOCA version available for DPF version %s: %v", e.DPFVersion, e.Err)
}

func (e *UnsupportedDPFVersionError) Unwrap() error {
	return e.Err
}

// DPFVersionError indicates the DPF version could not be determined from
// DPFOperatorConfig (missing CR, status.version not set, etc.)
// This is treated as a transient error - the DPF operator may still be updating its status
type DPFVersionError struct {
	Err error
}

func (e *DPFVersionError) Reason() string {
	return reasonDPFVersionNotFound
}

func (e *DPFVersionError) Error() string {
	return fmt.Sprintf("failed to determine DPF version: %v", e.Err)
}

func (e *DPFVersionError) Unwrap() error {
	return e.Err
}

// LabelMismatchError indicates the selected BlueField OCP layer image carries a
// version label that disagrees with the version its tag claims (e.g. a mis-pushed tag)
// This is a permanent error - the image content does not match its tag
type LabelMismatchError struct {
	Image      string
	Label      string
	LabelValue string
	Expected   string
}

func (e *LabelMismatchError) Reason() string {
	return reasonLabelMismatch
}

func (e *LabelMismatchError) Error() string {
	return fmt.Sprintf("BlueField OCP layer image %s has label %s=%q which disagrees with expected %q",
		e.Image, e.Label, e.LabelValue, e.Expected)
}

// InvalidImageFormatError indicates the ocpReleaseImage URL is malformed
// This is a permanent error - user must fix CR spec
type InvalidImageFormatError struct {
	Message string
	URL     string
}

func (e *InvalidImageFormatError) Reason() string {
	return reasonInvalidImageFormat
}

func (e *InvalidImageFormatError) Error() string {
	return fmt.Sprintf("Invalid ocpReleaseImage format: %s (URL: %s)", e.Message, e.URL)
}

// InvalidBlueFieldOCPLayerImageURLError indicates the constructed BlueField OCP layer image reference is invalid
// This is a permanent error
type InvalidBlueFieldOCPLayerImageURLError struct {
	Version string
	URL     string
	Message string
}

func (e *InvalidBlueFieldOCPLayerImageURLError) Reason() string {
	return reasonInvalidBlueFieldOCPLayerURL
}

func (e *InvalidBlueFieldOCPLayerImageURLError) Error() string {
	return fmt.Sprintf("BlueField OCP layer image URL is invalid for OCP version %s: %s (URL: %s)", e.Version, e.Message, e.URL)
}
