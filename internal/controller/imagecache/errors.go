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

package imagecache

import "fmt"

// PrerequisiteError indicates a prerequisite for image caching is not met.
// This is a skip condition, not a failure.
type PrerequisiteError struct {
	Message string
}

func (e *PrerequisiteError) Error() string {
	return fmt.Sprintf("prerequisite not met: %s", e.Message)
}

// RegistryNotAvailableError indicates the internal registry is not available.
// This is an opportunistic skip, not a failure.
type RegistryNotAvailableError struct {
	Reason          string
	ConditionReason string // maps to API reason constant
}

func (e *RegistryNotAvailableError) Error() string {
	return fmt.Sprintf("internal registry not available: %s", e.Reason)
}

// ImagePullError indicates a failure to pull an image from the external registry.
type ImagePullError struct {
	URL string
	Err error
}

func (e *ImagePullError) Error() string {
	return fmt.Sprintf("failed to pull image %s: %v", e.URL, e.Err)
}

func (e *ImagePullError) Unwrap() error { return e.Err }

// ImagePushError indicates a failure to push an image to the internal registry.
type ImagePushError struct {
	URL string
	Err error
}

func (e *ImagePushError) Error() string {
	return fmt.Sprintf("failed to push image %s: %v", e.URL, e.Err)
}

func (e *ImagePushError) Unwrap() error { return e.Err }

// InvalidImageReferenceError indicates an image reference could not be parsed.
type InvalidImageReferenceError struct {
	URL string
	Err error
}

func (e *InvalidImageReferenceError) Error() string {
	return fmt.Sprintf("invalid image reference %q: %v", e.URL, e.Err)
}

func (e *InvalidImageReferenceError) Unwrap() error { return e.Err }
