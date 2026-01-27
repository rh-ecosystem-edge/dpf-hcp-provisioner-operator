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

import (
	"context"
	"fmt"
	"strings"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
)

const (
	// ConfigMap name and namespace
	configMapName      = "ocp-bluefield-images"
	configMapNamespace = "dpf-hcp-bridge-system"

	// Reason codes
	reasonImageResolved            = "ImageResolved"
	reasonConfigMapNotFound        = "ConfigMapNotFound"
	reasonConfigMapTransientError  = "ConfigMapTransientError"
	reasonInvalidImageFormat       = "InvalidImageFormat"
	reasonVersionNotFound          = "VersionNotFound"
	reasonConfigMapAccessDenied    = "ConfigMapAccessDenied"
	reasonInvalidBlueFieldImageURL = "InvalidBlueFieldImageURL"
)

// ImageResolver handles BlueField container image resolution
type ImageResolver struct {
	client.Client
	Recorder record.EventRecorder
}

// NewImageResolver creates a new ImageResolver
func NewImageResolver(client client.Client, recorder record.EventRecorder) *ImageResolver {
	return &ImageResolver{
		Client:   client,
		Recorder: recorder,
	}
}

// ResolveBlueFieldImage is the main reconciliation function for BlueField image mapping
// It extracts the OCP version from the ocpReleaseImage, looks up the corresponding
// BlueField image in the ConfigMap, validates it, and updates the CR status
func (r *ImageResolver) ResolveBlueFieldImage(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge) (ctrl.Result, error) {
	log := log.FromContext(ctx)
	log = log.WithValues("feature", "bluefield-image-mapping")

	// Step 1: Read ocpReleaseImage from spec
	ocpReleaseImage := cr.Spec.OCPReleaseImage
	if ocpReleaseImage == "" {
		err := fmt.Errorf("ocpReleaseImage is required but empty")
		log.Error(err, "Missing required field")
		return r.handleValidationError(ctx, cr, &InvalidImageFormatError{
			Message: "ocpReleaseImage is required but empty",
			URL:     "",
		})
	}

	// Step 2: Parse OCP version from image URL
	log.V(1).Info("Extracting OCP version from image URL", "ocpReleaseImage", ocpReleaseImage)
	version, err := extractOCPVersion(ocpReleaseImage)
	if err != nil {
		log.Error(err, "Failed to parse OCP version from image URL", "ocpReleaseImage", ocpReleaseImage)
		return r.handleValidationError(ctx, cr, &InvalidImageFormatError{
			Message: err.Error(),
			URL:     ocpReleaseImage,
		})
	}
	log.V(1).Info("Extracted OCP version", "version", version)

	// Step 3: Fetch ConfigMap
	log.V(1).Info("Fetching ConfigMap", "name", configMapName, "namespace", configMapNamespace)
	configMap, err := r.fetchConfigMap(ctx)
	if err != nil {
		// Check error type
		if _, ok := err.(*ConfigMapNotFoundError); ok {
			return r.handleTransientError(ctx, cr, err, version)
		}
		if _, ok := err.(*ConfigMapAccessDeniedError); ok {
			return r.handlePermanentError(ctx, cr, err, version)
		}
		// Other API errors - treat as transient
		log.Error(err, "Transient error fetching ConfigMap")
		return r.handleTransientError(ctx, cr, err, version)
	}

	// Step 4: Lookup version in ConfigMap
	log.V(1).Info("Looking up BlueField image in ConfigMap", "version", version)
	blueFieldImage, err := lookupBlueFieldImage(configMap, version)
	if err != nil {
		log.V(1).Info("BlueField image lookup failed", "version", version, "error", err.Error())
		return r.handlePermanentError(ctx, cr, err, version)
	}

	// Step 5: Validate BlueField image URL
	log.V(1).Info("Validating BlueField image URL", "blueFieldImage", blueFieldImage)
	if err := validateBlueFieldImageURL(blueFieldImage, version); err != nil {
		log.Error(err, "BlueField image URL validation failed", "blueFieldImage", blueFieldImage)
		return r.handlePermanentError(ctx, cr, err, version)
	}

	// Step 6: Update status on success
	log.Info("BlueField image resolved successfully",
		"version", version,
		"blueFieldImage", blueFieldImage)
	return r.updateStatusOnSuccess(ctx, cr, blueFieldImage, version)
}

// extractOCPVersion extracts the OCP version from the ocpReleaseImage URL
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

// fetchConfigMap fetches the ocp-bluefield-images ConfigMap
func (r *ImageResolver) fetchConfigMap(ctx context.Context) (*corev1.ConfigMap, error) {
	configMap := &corev1.ConfigMap{}
	err := r.Get(ctx, types.NamespacedName{
		Name:      configMapName,
		Namespace: configMapNamespace,
	}, configMap)

	if err != nil {
		if apierrors.IsNotFound(err) {
			return nil, &ConfigMapNotFoundError{Err: err}
		}
		if apierrors.IsForbidden(err) {
			return nil, &ConfigMapAccessDeniedError{Err: err}
		}
		// Other errors (network, API server issues)
		return nil, err
	}

	return configMap, nil
}

// lookupBlueFieldImage looks up the BlueField image in the ConfigMap by version
// Exported for testing.
func lookupBlueFieldImage(configMap *corev1.ConfigMap, version string) (string, error) {
	if configMap.Data == nil || len(configMap.Data) == 0 {
		availableVersions := []string{}
		return "", &VersionNotFoundError{
			Version:           version,
			AvailableVersions: availableVersions,
		}
	}

	blueFieldImage, found := configMap.Data[version]
	if !found {
		// Collect available versions for error message
		availableVersions := make([]string, 0, len(configMap.Data))
		for k := range configMap.Data {
			availableVersions = append(availableVersions, k)
		}
		return "", &VersionNotFoundError{
			Version:           version,
			AvailableVersions: availableVersions,
		}
	}

	return blueFieldImage, nil
}

// validateBlueFieldImageURL validates the BlueField image URL format
// Exported for testing.
func validateBlueFieldImageURL(url string, version string) error {
	if url == "" {
		return &InvalidBlueFieldImageURLError{
			Version: version,
			URL:     url,
			Message: "BlueField image URL is empty",
		}
	}

	// Basic validation: should contain a colon (registry/repo:tag format)
	if !strings.Contains(url, ":") {
		return &InvalidBlueFieldImageURLError{
			Version: version,
			URL:     url,
			Message: "BlueField image URL is malformed (missing tag separator ':')",
		}
	}

	return nil
}

// updateStatusOnSuccess updates the CR status when image resolution succeeds
func (r *ImageResolver) updateStatusOnSuccess(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, blueFieldImage, version string) (ctrl.Result, error) {
	log := log.FromContext(ctx)

	// Get previous condition to check if we need to emit event

	// Update status field
	cr.Status.BlueFieldContainerImage = blueFieldImage

	// Update condition
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.BlueFieldImageResolved,
		Status:             metav1.ConditionTrue,
		Reason:             reasonImageResolved,
		Message:            fmt.Sprintf("BlueField container image resolved: %s", blueFieldImage),
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition status/reason changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		r.Recorder.Event(cr, corev1.EventTypeNormal, reasonImageResolved,
			fmt.Sprintf("BlueField container image resolved for OCP version %s: %s", version, blueFieldImage))
	}

	// Persist status
	if err := r.Status().Update(ctx, cr); err != nil {
		if apierrors.IsConflict(err) {
			// ResourceVersion conflict - controller-runtime will requeue automatically
			log.V(1).Info("Status update conflict, will retry")
			return ctrl.Result{}, err
		}
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	log.Info("Status updated successfully")
	return ctrl.Result{}, nil
}

// handleValidationError handles permanent validation errors
func (r *ImageResolver) handleValidationError(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, err error) (ctrl.Result, error) {
	log := log.FromContext(ctx)
	log.V(1).Info("Validation error - check CR spec", "error", err.Error())

	// Get previous condition

	// Clear status field
	cr.Status.BlueFieldContainerImage = ""

	// Determine reason and message based on error type
	var reason, message string
	switch e := err.(type) {
	case *InvalidImageFormatError:
		reason = reasonInvalidImageFormat
		message = e.Error()
	default:
		reason = reasonInvalidImageFormat
		message = err.Error()
	}

	// Update condition
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.BlueFieldImageResolved,
		Status:             metav1.ConditionFalse,
		Reason:             reason,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		r.Recorder.Event(cr, corev1.EventTypeWarning, reason, message)
	}

	// Update status
	if updateErr := r.Status().Update(ctx, cr); updateErr != nil {
		log.Error(updateErr, "Failed to update status after validation error")
	}

	// Do NOT requeue - permanent error
	return ctrl.Result{}, nil
}

// handlePermanentError handles permanent errors (version not found, access denied, invalid URL)
func (r *ImageResolver) handlePermanentError(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, err error, version string) (ctrl.Result, error) {
	log := log.FromContext(ctx)
	log.V(1).Info("Permanent error - user action required", "version", version, "error", err.Error())

	// Get previous condition

	// Clear status field
	cr.Status.BlueFieldContainerImage = ""

	// Determine reason based on error type
	var reason, message string
	switch err.(type) {
	case *VersionNotFoundError:
		reason = reasonVersionNotFound
		message = err.Error()
	case *ConfigMapAccessDeniedError:
		reason = reasonConfigMapAccessDenied
		message = err.Error()
	case *InvalidBlueFieldImageURLError:
		reason = reasonInvalidBlueFieldImageURL
		message = err.Error()
	default:
		reason = reasonVersionNotFound
		message = err.Error()
	}

	// Update condition
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.BlueFieldImageResolved,
		Status:             metav1.ConditionFalse,
		Reason:             reason,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		r.Recorder.Event(cr, corev1.EventTypeWarning, reason, message)
	}

	// Update status
	if updateErr := r.Status().Update(ctx, cr); updateErr != nil {
		log.Error(updateErr, "Failed to update status after permanent error")
	}

	// Do NOT requeue - permanent error
	return ctrl.Result{}, nil
}

// handleTransientError handles transient errors
func (r *ImageResolver) handleTransientError(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, err error, version string) (ctrl.Result, error) {
	log := log.FromContext(ctx)

	// Get previous condition

	// Determine reason based on error type
	var reason, message string
	switch err.(type) {
	case *ConfigMapNotFoundError:
		reason = reasonConfigMapNotFound
		message = fmt.Sprintf("ConfigMap %s not found in namespace %s", configMapName, configMapNamespace)
	default:
		reason = reasonConfigMapTransientError
		message = fmt.Sprintf("Transient error accessing ConfigMap: %v", err)
	}

	// Update condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.BlueFieldImageResolved,
		Status:             metav1.ConditionFalse,
		Reason:             reason,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		r.Recorder.Event(cr, corev1.EventTypeWarning, reason, message)
	}

	// Update status
	if updateErr := r.Status().Update(ctx, cr); updateErr != nil {
		log.Error(updateErr, "Failed to update status after transient error")
	}

	log.V(1).Info("Transient error, returning error for native retry with exponential backoff", "error", err)

	// Return error to trigger controller-runtime's native exponential backoff
	return ctrl.Result{}, err
}
