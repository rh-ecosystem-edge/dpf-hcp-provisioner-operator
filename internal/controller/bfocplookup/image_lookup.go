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
	"context"
	"fmt"
	"strings"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	"github.com/google/go-containerregistry/pkg/v1/remote"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

const (
	// Reason codes
	reasonImageFound                  = "ImageFound"
	reasonRegistryAccessError         = "RegistryAccessError"
	reasonRegistryAuthError           = "RegistryAuthError"
	reasonInvalidImageFormat          = "InvalidImageFormat"
	reasonVersionNotFound             = "VersionNotFound"
	reasonInvalidBlueFieldOCPLayerURL = "InvalidBlueFieldOCPLayerImageURL"
)

// ImageChecker abstracts checking whether a specific image tag exists in a registry
type ImageChecker interface {
	CheckTag(ctx context.Context, ref name.Reference, keychain authn.Keychain) error
}

// RemoteImageChecker is the production implementation that checks a real container registry
type RemoteImageChecker struct{}

// CheckTag checks if a specific tag exists by making a HEAD request to the registry.
// Returns nil if the tag exists, or an error if not found or inaccessible.
func (r *RemoteImageChecker) CheckTag(ctx context.Context, ref name.Reference, keychain authn.Keychain) error {
	_, err := remote.Head(ref, remote.WithAuthFromKeychain(keychain), remote.WithContext(ctx))
	return err
}

// ImageLookup handles BlueField OCP layer image lookup by querying a container registry
type ImageLookup struct {
	client.Client
	Recorder     record.EventRecorder
	ImageChecker ImageChecker
	Repository   string
}

// NewImageLookup creates a new ImageLookup
func NewImageLookup(c client.Client, recorder record.EventRecorder) *ImageLookup {
	return &ImageLookup{
		Client:       c,
		Recorder:     recorder,
		ImageChecker: &RemoteImageChecker{},
	}
}

// LookupBlueFieldOCPLayerImage is the main function for BlueField OCP layer image lookup.
// It extracts the OCP version from the ocpReleaseImage, queries the container registry
// to find a matching tag, and updates the CR status with the found image reference.
func (r *ImageLookup) LookupBlueFieldOCPLayerImage(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	logger := log.FromContext(ctx)
	logger = logger.WithValues("feature", "bluefield-ocp-layer-lookup")

	// Step 1: Read ocpReleaseImage from spec
	ocpReleaseImage := cr.Spec.OCPReleaseImage
	if ocpReleaseImage == "" {
		err := fmt.Errorf("ocpReleaseImage is required but empty")
		logger.Error(err, "Missing required field")
		return r.handleValidationError(ctx, cr, &InvalidImageFormatError{
			Message: "ocpReleaseImage is required but empty",
			URL:     "",
		})
	}

	// Step 2: Parse OCP version from image URL
	logger.V(1).Info("Extracting OCP version from image URL", "ocpReleaseImage", ocpReleaseImage)
	version, err := extractOCPVersion(ocpReleaseImage)
	if err != nil {
		logger.Error(err, "Failed to parse OCP version from image URL", "ocpReleaseImage", ocpReleaseImage)
		return r.handleValidationError(ctx, cr, &InvalidImageFormatError{
			Message: err.Error(),
			URL:     ocpReleaseImage,
		})
	}
	logger.V(1).Info("Extracted OCP version", "version", version)

	// Step 3: Use the repository from operator config
	repository := r.Repository

	// Step 4: Get authentication keychain from the CR's pullSecretRef
	keychain, err := r.getKeychain(ctx, cr)
	if err != nil {
		logger.Error(err, "Failed to get registry credentials")
		return r.handlePermanentError(ctx, cr, &RegistryAuthError{
			Repository: repository,
			Err:        err,
		}, version)
	}

	// Step 5: Query registry for matching tag
	logger.V(1).Info("Querying registry for BlueField OCP layer image", "repository", repository, "version", version)
	blueFieldOCPLayerImage, err := r.validateTagExists(ctx, repository, version, keychain)
	if err != nil {
		switch err.(type) {
		case *VersionNotFoundError:
			return r.handlePermanentError(ctx, cr, err, version)
		case *RegistryAuthError:
			return r.handlePermanentError(ctx, cr, err, version)
		default:
			// Registry access errors are transient
			return r.handleTransientError(ctx, cr, err)
		}
	}

	// Step 6: Validate the found image URL
	logger.V(1).Info("Validating BlueField OCP layer image URL", "blueFieldOCPLayerImage", blueFieldOCPLayerImage)
	if err := validateBlueFieldOCPLayerImageURL(blueFieldOCPLayerImage, version); err != nil {
		logger.Error(err, "BlueField OCP layer image URL validation failed", "blueFieldOCPLayerImage", blueFieldOCPLayerImage)
		return r.handlePermanentError(ctx, cr, err, version)
	}

	// Step 7: Update status on success
	logger.Info("BlueField OCP layer image found successfully",
		"version", version,
		"blueFieldOCPLayerImage", blueFieldOCPLayerImage)
	return r.updateStatusOnSuccess(ctx, cr, blueFieldOCPLayerImage, version)
}

// extractOCPVersion extracts the OCP version from the ocpReleaseImage URL.
// It strips architecture suffixes like -multi, -amd64, etc.
// Exported for testing.
func extractOCPVersion(ocpReleaseImage string) (string, error) {
	// Extract tag (everything after last ':')
	parts := strings.Split(ocpReleaseImage, ":")
	if len(parts) < 2 {
		return "", fmt.Errorf("missing tag separator ':' in image URL")
	}
	tag := parts[len(parts)-1]

	if tag == "" {
		return "", fmt.Errorf("empty tag in image URL")
	}

	// Strip known architecture suffixes
	suffixes := []string{"-multi", "-amd64", "-arm64", "-ppc64le", "-s390x", "-x86_64"}
	version := tag
	for _, suffix := range suffixes {
		version = strings.TrimSuffix(version, suffix)
	}

	if version == "" {
		return "", fmt.Errorf("extracted version is empty after processing tag: %s", tag)
	}

	return version, nil
}

// getKeychain returns an authn.Keychain for authenticating to the container registry.
// It reuses the CR's pullSecretRef which typically contains credentials for multiple registries.
func (r *ImageLookup) getKeychain(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (authn.Keychain, error) {
	secret := &corev1.Secret{}
	err := r.Get(ctx, types.NamespacedName{
		Name:      cr.Spec.PullSecretRef.Name,
		Namespace: cr.Namespace,
	}, secret)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch pull secret %s: %w", cr.Spec.PullSecretRef.Name, err)
	}

	dockerConfigJSON, ok := secret.Data[".dockerconfigjson"]
	if !ok {
		return nil, fmt.Errorf("pull secret %s does not contain key '.dockerconfigjson'", cr.Spec.PullSecretRef.Name)
	}

	keychain, err := NewKeychainFromDockerConfig(dockerConfigJSON)
	if err != nil {
		return nil, fmt.Errorf("failed to parse docker config from secret %s: %w", cr.Spec.PullSecretRef.Name, err)
	}

	return keychain, nil
}

// buildImageReference constructs a full image reference string and parses it into a name.Reference.
func buildImageReference(repository, version string) (string, name.Reference, error) {
	imageRef := fmt.Sprintf("%s:%s", repository, version)
	ref, err := name.ParseReference(imageRef)
	if err != nil {
		return "", nil, fmt.Errorf("invalid image reference %s: %w", imageRef, err)
	}
	return imageRef, ref, nil
}

// validateTagExists checks if a specific tag exists in the container registry via a HEAD request.
// Returns the full image reference (repository:tag) if found.
func (r *ImageLookup) validateTagExists(ctx context.Context, repository, version string, keychain authn.Keychain) (string, error) {
	imageRef, ref, err := buildImageReference(repository, version)
	if err != nil {
		return "", &RegistryAccessError{
			Repository: repository,
			Err:        err,
		}
	}

	err = r.ImageChecker.CheckTag(ctx, ref, keychain)
	if err != nil {
		if isAuthError(err) {
			return "", &RegistryAuthError{
				Repository: repository,
				Err:        err,
			}
		}
		if isNotFoundError(err) {
			return "", &VersionNotFoundError{
				Version:    version,
				Repository: repository,
			}
		}
		return "", &RegistryAccessError{
			Repository: repository,
			Err:        err,
		}
	}

	return imageRef, nil
}

// isAuthError checks if an error is an authentication/authorization error
func isAuthError(err error) bool {
	errStr := err.Error()
	return strings.Contains(errStr, "UNAUTHORIZED") ||
		strings.Contains(errStr, "unauthorized") ||
		strings.Contains(errStr, "DENIED") ||
		strings.Contains(errStr, "denied") ||
		strings.Contains(errStr, "403")
}

// isNotFoundError checks if an error indicates the image tag was not found
func isNotFoundError(err error) bool {
	errStr := err.Error()
	return strings.Contains(errStr, "NOT_FOUND") ||
		strings.Contains(errStr, "not found") ||
		strings.Contains(errStr, "MANIFEST_UNKNOWN") ||
		strings.Contains(errStr, "manifest unknown") ||
		strings.Contains(errStr, "404")
}

// validateBlueFieldOCPLayerImageURL validates the BlueField OCP layer image URL format.
// Exported for testing.
func validateBlueFieldOCPLayerImageURL(url string, version string) error {
	if url == "" {
		return &InvalidBlueFieldOCPLayerImageURLError{
			Version: version,
			URL:     url,
			Message: "BlueField OCP layer image URL is empty",
		}
	}

	// Basic validation: should contain a colon (registry/repo:tag format)
	if !strings.Contains(url, ":") {
		return &InvalidBlueFieldOCPLayerImageURLError{
			Version: version,
			URL:     url,
			Message: "BlueField OCP layer image URL is malformed (missing tag separator ':')",
		}
	}

	return nil
}

// updateStatusOnSuccess updates the CR status when BlueField OCP layer image lookup succeeds
func (r *ImageLookup) updateStatusOnSuccess(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, blueFieldOCPLayerImage, version string) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	// Update status field
	cr.Status.BlueFieldOCPLayerImage = blueFieldOCPLayerImage

	// Update condition
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.BlueFieldOCPLayerImageFound,
		Status:             metav1.ConditionTrue,
		Reason:             reasonImageFound,
		Message:            fmt.Sprintf("BlueField OCP layer image found: %s", blueFieldOCPLayerImage),
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition status/reason changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		r.Recorder.Event(cr, corev1.EventTypeNormal, reasonImageFound,
			fmt.Sprintf("BlueField OCP layer image found for OCP version %s: %s", version, blueFieldOCPLayerImage))
	}

	// Persist status
	if err := r.Status().Update(ctx, cr); err != nil {
		if apierrors.IsConflict(err) {
			logger.V(1).Info("Status update conflict, will retry")
			return ctrl.Result{}, err
		}
		logger.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	logger.Info("Status updated successfully")
	return ctrl.Result{}, nil
}

// handleValidationError handles permanent validation errors
func (r *ImageLookup) handleValidationError(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, err error) (ctrl.Result, error) {
	logger := log.FromContext(ctx)
	logger.V(1).Info("Validation error - check CR spec", "error", err.Error())

	// Clear status field
	cr.Status.BlueFieldOCPLayerImage = ""

	var reason, message string
	switch e := err.(type) {
	case *InvalidImageFormatError:
		reason = reasonInvalidImageFormat
		message = e.Error()
	default:
		reason = reasonInvalidImageFormat
		message = err.Error()
	}

	condition := metav1.Condition{
		Type:               provisioningv1alpha1.BlueFieldOCPLayerImageFound,
		Status:             metav1.ConditionFalse,
		Reason:             reason,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		r.Recorder.Event(cr, corev1.EventTypeWarning, reason, message)
	}

	if updateErr := r.Status().Update(ctx, cr); updateErr != nil {
		logger.Error(updateErr, "Failed to update status after validation error")
	}

	// Do NOT requeue - permanent error
	return ctrl.Result{}, nil
}

// handlePermanentError handles permanent errors (version not found, auth denied, invalid URL)
func (r *ImageLookup) handlePermanentError(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, err error, version string) (ctrl.Result, error) {
	logger := log.FromContext(ctx)
	logger.V(1).Info("Permanent error - user action required", "version", version, "error", err.Error())

	// Clear status field
	cr.Status.BlueFieldOCPLayerImage = ""

	var reason, message string
	switch err.(type) {
	case *VersionNotFoundError:
		reason = reasonVersionNotFound
		message = err.Error()
	case *RegistryAuthError:
		reason = reasonRegistryAuthError
		message = err.Error()
	case *InvalidBlueFieldOCPLayerImageURLError:
		reason = reasonInvalidBlueFieldOCPLayerURL
		message = err.Error()
	default:
		reason = reasonVersionNotFound
		message = err.Error()
	}

	condition := metav1.Condition{
		Type:               provisioningv1alpha1.BlueFieldOCPLayerImageFound,
		Status:             metav1.ConditionFalse,
		Reason:             reason,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		r.Recorder.Event(cr, corev1.EventTypeWarning, reason, message)
	}

	if updateErr := r.Status().Update(ctx, cr); updateErr != nil {
		logger.Error(updateErr, "Failed to update status after permanent error")
	}

	// Do NOT requeue - permanent error
	return ctrl.Result{}, nil
}

// handleTransientError handles transient errors
func (r *ImageLookup) handleTransientError(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, err error) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	var reason, message string
	switch e := err.(type) {
	case *RegistryAccessError:
		reason = reasonRegistryAccessError
		message = e.Error()
	default:
		reason = reasonRegistryAccessError
		message = fmt.Sprintf("Transient error accessing registry: %v", err)
	}

	condition := metav1.Condition{
		Type:               provisioningv1alpha1.BlueFieldOCPLayerImageFound,
		Status:             metav1.ConditionFalse,
		Reason:             reason,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		r.Recorder.Event(cr, corev1.EventTypeWarning, reason, message)
	}

	if updateErr := r.Status().Update(ctx, cr); updateErr != nil {
		logger.Error(updateErr, "Failed to update status after transient error")
	}

	logger.V(1).Info("Transient error, returning error for native retry with exponential backoff", "error", err)

	// Return error to trigger controller-runtime's native exponential backoff
	return ctrl.Result{}, err
}
