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

package dpuservicetemplate

import (
	"archive/tar"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"strings"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	"github.com/google/go-containerregistry/pkg/v1/mutate"
	"github.com/google/go-containerregistry/pkg/v1/remote"
)

const (
	imageReferencesPath = "release-manifests/image-references"
	ovnKubernetesName   = "ovn-kubernetes"
)

// ReleaseImageReader extracts component images from an OCP release payload.
type ReleaseImageReader interface {
	GetComponentImage(ctx context.Context, releaseImageRef string, componentName string, keychain authn.Keychain) (string, error)
}

// RemoteReleaseImageReader pulls an OCP release image from a registry and
// extracts component image references from its image-references manifest.
type RemoteReleaseImageReader struct{}

func (r *RemoteReleaseImageReader) GetComponentImage(ctx context.Context, releaseImageRef string, componentName string, keychain authn.Keychain) (string, error) {
	ref, err := name.ParseReference(releaseImageRef)
	if err != nil {
		return "", fmt.Errorf("parsing release image ref %q: %w", releaseImageRef, err)
	}

	img, err := remote.Image(ref, remote.WithAuthFromKeychain(keychain), remote.WithContext(ctx))
	if err != nil {
		return "", fmt.Errorf("pulling release image %q: %w", releaseImageRef, err)
	}

	reader := mutate.Extract(img)
	defer reader.Close()

	imageRef, err := findComponentInImageReferences(reader, componentName)
	if err != nil {
		return "", fmt.Errorf("extracting %s from release image %q: %w", componentName, releaseImageRef, err)
	}

	return imageRef, nil
}

// imageReferences is a minimal representation of the OpenShift ImageStream
// stored at release-manifests/image-references in a release payload image.
type imageReferences struct {
	Spec struct {
		Tags []struct {
			Name string `json:"name"`
			From struct {
				Name string `json:"name"`
			} `json:"from"`
		} `json:"tags"`
	} `json:"spec"`
}

func findComponentInImageReferences(reader io.Reader, componentName string) (string, error) {
	tr := tar.NewReader(reader)
	for {
		header, err := tr.Next()
		if err == io.EOF {
			return "", fmt.Errorf("file %s not found in release image", imageReferencesPath)
		}
		if err != nil {
			return "", fmt.Errorf("reading tar: %w", err)
		}

		if header.Name != imageReferencesPath {
			continue
		}

		data, err := io.ReadAll(tr)
		if err != nil {
			return "", fmt.Errorf("reading %s: %w", imageReferencesPath, err)
		}

		var refs imageReferences
		if err := json.Unmarshal(data, &refs); err != nil {
			return "", fmt.Errorf("parsing %s: %w", imageReferencesPath, err)
		}

		for _, tag := range refs.Spec.Tags {
			if tag.Name == componentName {
				if tag.From.Name == "" {
					return "", fmt.Errorf("component %q has empty image reference", componentName)
				}
				return tag.From.Name, nil
			}
		}

		return "", fmt.Errorf("component %q not found in %s", componentName, imageReferencesPath)
	}
}

// splitImage splits a container image reference into repository and tag/digest.
func splitImage(image string) (repo, tag string, err error) {
	if image == "" {
		return "", "", fmt.Errorf("empty image reference")
	}

	if idx := strings.LastIndex(image, "@sha256:"); idx > 0 {
		return image[:idx] + "@sha256", image[idx+len("@sha256:"):], nil
	}

	lastColon := strings.LastIndex(image, ":")
	if lastColon > 0 && !strings.Contains(image[lastColon:], "/") {
		return image[:lastColon], image[lastColon+1:], nil
	}

	return image, "latest", nil
}
