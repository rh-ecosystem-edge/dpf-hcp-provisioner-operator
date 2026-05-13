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
	"errors"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/google/go-containerregistry/pkg/authn"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/bfocplookup"
)

const (
	// Exponential backoff duration for initial retry
	initialBackoff = 30 * time.Second
)

// resolveOperatorNamespace determines the namespace the operator is running in.
// Prefers POD_NAMESPACE (set via the downward API) and falls back to the
// in-cluster serviceaccount namespace file.
func resolveOperatorNamespace() (string, error) {
	if ns := os.Getenv("POD_NAMESPACE"); ns != "" {
		return ns, nil
	}
	data, err := os.ReadFile("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
	if err != nil {
		return "", fmt.Errorf("unable to determine operator namespace: %w", err)
	}
	return strings.TrimSpace(string(data)), nil
}

// GVKs for unstructured lookups (avoids importing operator and route APIs)
var (
	imageRegistryConfigGVK = schema.GroupVersionKind{
		Group:   "imageregistry.operator.openshift.io",
		Version: "v1",
		Kind:    "Config",
	}
	routeGVK = schema.GroupVersionKind{
		Group:   "route.openshift.io",
		Version: "v1",
		Kind:    "Route",
	}
)

// ImageCache handles caching container images to the OpenShift internal registry.
type ImageCache struct {
	Client   client.Client
	Recorder record.EventRecorder
}

// NewImageCache creates a new ImageCache manager.
func NewImageCache(c client.Client, recorder record.EventRecorder) *ImageCache {
	return &ImageCache{
		Client:   c,
		Recorder: recorder,
	}
}

// Reconcile performs the image caching workflow.
// It checks if the internal registry is available and mirrors the image if needed.
// This is an opportunistic feature - if the registry is not available, it skips gracefully.
func (ic *ImageCache) Reconcile(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "image-cache")

	// Step 1: Determine image source URL.
	// Spec.MachineOSURL is an optional user-provided override (e.g., custom build, private registry).
	// Status.BlueFieldOCPLayerImage is the auto-discovered URL from the bfocplookup controller.
	// The user override takes priority; auto-discovered URL is the fallback.
	sourceURL := cr.Spec.MachineOSURL
	if sourceURL == "" {
		sourceURL = cr.Status.BlueFieldOCPLayerImage
	}
	if sourceURL == "" {
		log.V(1).Info("No image URL available for caching, skipping")
		ic.setSkipCondition(cr, provisioningv1alpha1.ReasonNoUpstreamURL,
			"No image URL available for caching (machineOSURL and blueFieldOCPLayerImage are both empty)")
		if err := ic.Client.Status().Update(ctx, cr); err != nil {
			log.Error(err, "Failed to persist ImageCached skip condition")
			return ctrl.Result{}, err
		}
		return ctrl.Result{}, nil
	}

	// Step 2: Check internal registry availability (opportunistic)
	registryInfo, err := checkRegistryAvailability(ctx, ic.Client)
	if err != nil {
		var registryErr *RegistryNotAvailableError
		if errors.As(err, &registryErr) {
			// Registry not available - skip gracefully
			log.Info("Internal registry not available, skipping image caching", "reason", registryErr.Reason)
			ic.setSkipCondition(cr, registryErr.ConditionReason,
				fmt.Sprintf("Internal registry not available: %s. Using external URL directly.", registryErr.Reason))
			ic.Recorder.Event(cr, corev1.EventTypeNormal, "ImageCachingSkipped",
				fmt.Sprintf("Image caching skipped: %s", registryErr.Reason))
			if err := ic.Client.Status().Update(ctx, cr); err != nil {
				log.Error(err, "Failed to persist ImageCached skip condition")
				return ctrl.Result{}, err
			}
			return ctrl.Result{}, nil
		}
		// Unexpected error checking registry
		log.Error(err, "Unexpected error checking registry availability")
		return ctrl.Result{RequeueAfter: initialBackoff}, err
	}

	// Step 3: Check cache validity
	imageCachedCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.ImageCached)
	if imageCachedCond != nil && imageCachedCond.Status == metav1.ConditionTrue &&
		imageCachedCond.ObservedGeneration == cr.Generation &&
		cr.Status.CachedMachineOSURL != "" {

		// Step 4: Build keychains and compare digests
		externalKeychain, err := ic.buildExternalKeychain(ctx, cr)
		if err != nil {
			log.Error(err, "Failed to build external keychain for digest comparison")
			// Don't fail - just re-cache to be safe
		} else {
			internalKeychain := ic.buildInternalKeychain()

			match, err := compareImageDigests(ctx, sourceURL, cr.Status.CachedMachineOSURL, externalKeychain, internalKeychain)
			if err != nil {
				log.Error(err, "Failed to compare image digests")
				// Don't fail hard, proceed to re-cache
			} else if match {
				log.Info("Cached image is up-to-date, skipping mirror", "cachedURL", cr.Status.CachedMachineOSURL)
				return ctrl.Result{}, nil
			} else {
				log.Info("Upstream image changed, re-caching")
				ic.Recorder.Event(cr, corev1.EventTypeNormal, "UpstreamImageChanged",
					"Upstream image digest changed, re-caching to internal registry")
			}
		}
	}

	// Step 5: Set CachingInProgress condition
	meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.ImageCached,
		Status:             metav1.ConditionFalse,
		Reason:             provisioningv1alpha1.ReasonCachingInProgress,
		Message:            fmt.Sprintf("Caching image %s to internal registry", sourceURL),
		ObservedGeneration: cr.Generation,
	})
	if err := ic.Client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status for CachingInProgress")
		return ctrl.Result{}, err
	}

	// Step 6: Build keychains for mirror operation
	externalKeychain, err := ic.buildExternalKeychain(ctx, cr)
	if err != nil {
		log.Error(err, "Failed to build external keychain")
		ic.setErrorCondition(cr, provisioningv1alpha1.ReasonRegistryAuthFailed,
			fmt.Sprintf("Failed to build external registry keychain: %v", err))
		if updateErr := ic.Client.Status().Update(ctx, cr); updateErr != nil {
			log.Error(updateErr, "Failed to update status after keychain failure")
		}
		ic.Recorder.Event(cr, corev1.EventTypeWarning, "ImageCacheFailed",
			fmt.Sprintf("Failed to build external registry keychain: %v", err))
		return ctrl.Result{RequeueAfter: initialBackoff}, nil
	}
	internalKeychain := ic.buildInternalKeychain()

	// Step 6.5: Resolve operator namespace for target image reference
	opNamespace, err := resolveOperatorNamespace()
	if err != nil {
		log.Error(err, "Failed to resolve operator namespace")
		ic.setErrorCondition(cr, provisioningv1alpha1.ReasonCacheFailed,
			fmt.Sprintf("Failed to resolve operator namespace: %v", err))
		if updateErr := ic.Client.Status().Update(ctx, cr); updateErr != nil {
			log.Error(updateErr, "Failed to update status after namespace resolution failure")
		}
		return ctrl.Result{RequeueAfter: initialBackoff}, nil
	}

	// Step 7: Mirror image
	targetURL, err := mirrorImage(ctx, sourceURL, registryInfo, opNamespace, externalKeychain, internalKeychain)
	if err != nil {
		var pullErr *ImagePullError
		var pushErr *ImagePushError

		var reason string
		switch {
		case errors.As(err, &pullErr):
			reason = provisioningv1alpha1.ReasonImagePullFailed
		case errors.As(err, &pushErr):
			reason = provisioningv1alpha1.ReasonImagePushFailed
		default:
			reason = provisioningv1alpha1.ReasonCacheFailed
		}

		log.Error(err, "Image mirror failed", "reason", reason)
		ic.setErrorCondition(cr, reason, fmt.Sprintf("Image caching failed: %v", err))
		if err := ic.Client.Status().Update(ctx, cr); err != nil {
			log.Error(err, "Failed to update status after mirror failure")
		}
		ic.Recorder.Event(cr, corev1.EventTypeWarning, "ImageCacheFailed",
			fmt.Sprintf("Failed to cache image: %v", err))
		return ctrl.Result{RequeueAfter: initialBackoff}, nil
	}

	// Step 8: Update status with cached URL
	cr.Status.CachedMachineOSURL = targetURL
	meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.ImageCached,
		Status:             metav1.ConditionTrue,
		Reason:             provisioningv1alpha1.ReasonImageCached,
		Message:            fmt.Sprintf("Image successfully cached to internal registry: %s", targetURL),
		ObservedGeneration: cr.Generation,
	})

	if err := ic.Client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status after successful cache")
		return ctrl.Result{}, err
	}

	ic.Recorder.Event(cr, corev1.EventTypeNormal, "ImageCached",
		fmt.Sprintf("Image cached to internal registry: %s", targetURL))

	log.Info("Image caching completed successfully", "cachedURL", targetURL)
	return ctrl.Result{}, nil
}

// setSkipCondition sets the ImageCached condition to False with a skip reason.
// This indicates the feature was skipped gracefully (not an error).
func (ic *ImageCache) setSkipCondition(cr *provisioningv1alpha1.DPFHCPProvisioner, reason, message string) {
	meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.ImageCached,
		Status:             metav1.ConditionFalse,
		Reason:             reason,
		Message:            message,
		ObservedGeneration: cr.Generation,
	})
}

// setErrorCondition sets the ImageCached condition to False with an error reason.
func (ic *ImageCache) setErrorCondition(cr *provisioningv1alpha1.DPFHCPProvisioner, reason, message string) {
	meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.ImageCached,
		Status:             metav1.ConditionFalse,
		Reason:             reason,
		Message:            message,
		ObservedGeneration: cr.Generation,
	})
}

// buildExternalKeychain creates an authn.Keychain from the CR's pull secret.
func (ic *ImageCache) buildExternalKeychain(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (authn.Keychain, error) {
	// Get pull secret
	var pullSecret corev1.Secret
	key := types.NamespacedName{
		Name:      cr.Spec.PullSecretRef.Name,
		Namespace: cr.Namespace,
	}
	if err := ic.Client.Get(ctx, key, &pullSecret); err != nil {
		return nil, fmt.Errorf("failed to get pull secret %s/%s: %w", cr.Namespace, cr.Spec.PullSecretRef.Name, err)
	}

	dockerConfigJSON, ok := pullSecret.Data[".dockerconfigjson"]
	if !ok {
		return nil, fmt.Errorf("pull secret %s/%s missing .dockerconfigjson key", cr.Namespace, cr.Spec.PullSecretRef.Name)
	}

	return bfocplookup.NewKeychainFromDockerConfig(dockerConfigJSON)
}

// buildInternalKeychain creates an authn.Keychain backed by the operator's
// ServiceAccount token mounted at the standard Kubernetes path.
func (ic *ImageCache) buildInternalKeychain() authn.Keychain {
	token, err := os.ReadFile("/var/run/secrets/kubernetes.io/serviceaccount/token")
	if err != nil {
		// Fallback if token cannot be read (non-fatal for graceful degradation)
		logf.Log.Error(err, "Failed to read serviceaccount token, falling back to DefaultKeychain")
		return authn.DefaultKeychain
	}
	auth := authn.FromConfig(authn.AuthConfig{
		Username: "serviceaccount",
		Password: strings.TrimSpace(string(token)),
	})
	return &staticKeychain{auth: auth}
}

// staticKeychain implements authn.Keychain with a single static Authenticator.
type staticKeychain struct {
	auth authn.Authenticator
}

func (s *staticKeychain) Resolve(_ authn.Resource) (authn.Authenticator, error) {
	return s.auth, nil
}
