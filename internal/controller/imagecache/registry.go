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

import (
	"context"
	"crypto/tls"
	"fmt"
	"net/http"
	"strings"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	"github.com/google/go-containerregistry/pkg/v1/remote"
	"k8s.io/apimachinery/pkg/apis/meta/v1/unstructured"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// RegistryInfo contains information about the internal registry.
type RegistryInfo struct {
	Hostname string
}

// checkRegistryAvailability checks if the OpenShift internal registry is available.
// Uses unstructured client to avoid importing imageregistry and route APIs.
func checkRegistryAvailability(ctx context.Context, c client.Client) (*RegistryInfo, error) {
	log := logf.FromContext(ctx)

	// Check ImageRegistry config
	config := &unstructured.Unstructured{}
	config.SetGroupVersionKind(imageRegistryConfigGVK)
	err := c.Get(ctx, types.NamespacedName{Name: "cluster"}, config)
	if err != nil {
		return nil, &RegistryNotAvailableError{
			Reason:          fmt.Sprintf("ImageRegistry config not found: %v", err),
			ConditionReason: provisioningv1alpha1.ReasonRegistryNotAvailable,
		}
	}

	// Check managementState
	mgmtState, found, err := unstructured.NestedString(config.Object, "spec", "managementState")
	if err != nil || !found {
		return nil, &RegistryNotAvailableError{
			Reason:          "could not read managementState from ImageRegistry config",
			ConditionReason: provisioningv1alpha1.ReasonRegistryNotAvailable,
		}
	}
	if mgmtState != "Managed" {
		return nil, &RegistryNotAvailableError{
			Reason:          fmt.Sprintf("managementState is %s (expected Managed)", mgmtState),
			ConditionReason: provisioningv1alpha1.ReasonRegistryConfigNotManaged,
		}
	}

	// Check for default-route in openshift-image-registry namespace
	route := &unstructured.Unstructured{}
	route.SetGroupVersionKind(routeGVK)
	err = c.Get(ctx, types.NamespacedName{
		Name:      "default-route",
		Namespace: "openshift-image-registry",
	}, route)
	if err != nil {
		return nil, &RegistryNotAvailableError{
			Reason:          fmt.Sprintf("default-route not found in openshift-image-registry: %v", err),
			ConditionReason: provisioningv1alpha1.ReasonRegistryRouteNotExposed,
		}
	}

	// Extract hostname from route spec
	hostname, found, err := unstructured.NestedString(route.Object, "spec", "host")
	if err != nil || !found || hostname == "" {
		return nil, &RegistryNotAvailableError{
			Reason:          "default-route has empty or missing hostname",
			ConditionReason: provisioningv1alpha1.ReasonRegistryRouteNotExposed,
		}
	}

	log.Info("Internal registry is available", "hostname", hostname)
	return &RegistryInfo{Hostname: hostname}, nil
}

// internalRegistryTransport is a shared http.Transport that skips TLS verification.
// Used for the OpenShift internal registry whose route uses the cluster's
// self-signed ingress certificate.
// Package-level singleton to allow connection pooling across calls.
var internalRegistryTransport = &http.Transport{
	TLSClientConfig: &tls.Config{InsecureSkipVerify: true}, // #nosec G402 -- internal registry only
}

// compareImageDigests compares the digests of the source and cached images.
// Returns true if digests match (cache is valid), false otherwise.
func compareImageDigests(ctx context.Context, sourceURL, cachedURL string, externalKeychain, internalKeychain authn.Keychain) (bool, error) {
	log := logf.FromContext(ctx)

	// Get upstream image digest
	sourceRef, err := name.ParseReference(sourceURL)
	if err != nil {
		return false, &InvalidImageReferenceError{URL: sourceURL, Err: err}
	}

	sourceDesc, err := remote.Head(sourceRef, remote.WithAuthFromKeychain(externalKeychain), remote.WithContext(ctx))
	if err != nil {
		return false, fmt.Errorf("failed to fetch upstream image digest for %s: %w", sourceURL, err)
	}
	sourceDigest := sourceDesc.Digest.String()

	// Get cached image digest
	cachedRef, err := name.ParseReference(cachedURL)
	if err != nil {
		log.V(1).Info("Cached URL is invalid, will re-cache", "cachedURL", cachedURL)
		return false, nil
	}

	cachedDesc, err := remote.Head(cachedRef, remote.WithAuthFromKeychain(internalKeychain), remote.WithContext(ctx), remote.WithTransport(internalRegistryTransport))
	if err != nil {
		log.V(1).Info("Failed to fetch cached image digest, will re-cache", "error", err)
		return false, nil
	}
	cachedDigest := cachedDesc.Digest.String()

	match := sourceDigest == cachedDigest
	log.V(1).Info("Digest comparison", "sourceDigest", sourceDigest, "cachedDigest", cachedDigest, "match", match)
	return match, nil
}

// mirrorImage pulls an image from sourceURL and pushes it to the internal registry.
// Returns the full target URL in the internal registry.
func mirrorImage(ctx context.Context, sourceURL string, registry *RegistryInfo, operatorNamespace string, externalKeychain, internalKeychain authn.Keychain) (string, error) {
	log := logf.FromContext(ctx)
	log.Info("Starting image mirror operation", "source", sourceURL)

	// Parse source image reference
	sourceRef, err := name.ParseReference(sourceURL)
	if err != nil {
		return "", &InvalidImageReferenceError{URL: sourceURL, Err: err}
	}

	// Build target image reference
	// Extract repository name (last path segment) and identifier (tag or digest)
	repoPath := sourceRef.Context().RepositoryStr()
	parts := strings.Split(repoPath, "/")
	repoName := parts[len(parts)-1]

	// Determine the correct separator: digests use "@", tags use ":"
	identifier := sourceRef.Identifier()
	separator := ":"
	if strings.HasPrefix(identifier, "sha256:") || strings.HasPrefix(identifier, "sha512:") {
		separator = "@"
	}

	targetURL := fmt.Sprintf("%s/%s/%s%s%s",
		registry.Hostname,
		operatorNamespace,
		repoName,
		separator,
		identifier)

	targetRef, err := name.ParseReference(targetURL)
	if err != nil {
		return "", &InvalidImageReferenceError{URL: targetURL, Err: err}
	}

	log.Info("Target image reference constructed", "target", targetURL)

	// Pull image from external registry
	log.Info("Pulling image from external registry")
	img, err := remote.Image(sourceRef,
		remote.WithAuthFromKeychain(externalKeychain),
		remote.WithContext(ctx))
	if err != nil {
		return "", &ImagePullError{URL: sourceURL, Err: err}
	}

	// Push image to internal registry
	log.Info("Pushing image to internal registry")
	err = remote.Write(targetRef, img,
		remote.WithAuthFromKeychain(internalKeychain),
		remote.WithContext(ctx),
		remote.WithTransport(internalRegistryTransport))
	if err != nil {
		return "", &ImagePushError{URL: targetURL, Err: err}
	}

	log.Info("Image mirrored successfully", "targetURL", targetURL)
	return targetURL, nil
}
