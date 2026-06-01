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
	"strconv"
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

	// maxCacheRetries is the maximum number of mirror failures before
	// the caching feature gives up and lets provisioning proceed with
	// the external URL directly.  This preserves the "opportunistic"
	// design: caching is best-effort and must never block indefinitely.
	maxCacheRetries = 5

	// cacheRetryAnnotation tracks the number of consecutive mirror failures.
	// Using an annotation avoids an API-type change and is reset on success.
	cacheRetryAnnotation = "dpf.nvidia.com/image-cache-retries"
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

// EnsureCached performs the image caching workflow.
// It checks if the internal registry is available and mirrors the image if needed.
// This is an opportunistic feature - if the registry is not available, it skips gracefully.
func (ic *ImageCache) EnsureCached(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
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
		changed := ic.setConditionFalse(cr, provisioningv1alpha1.ReasonNoUpstreamURL,
			"No image URL available for caching (machineOSURL and blueFieldOCPLayerImage are both empty)")
		if changed {
			log.V(1).Info("No image URL available for caching, skipping")
			if err := ic.Client.Status().Update(ctx, cr); err != nil {
				log.Error(err, "Failed to persist ImageCached skip condition")
				return ctrl.Result{}, err
			}
		}
		return ctrl.Result{}, nil
	}

	// Step 2: Check internal registry availability (opportunistic)
	registryInfo, err := checkRegistryAvailability(ctx, ic.Client)
	if err != nil {
		var registryErr *RegistryNotAvailableError
		if errors.As(err, &registryErr) {
			// Registry not available - skip gracefully
			msg := fmt.Sprintf("Internal registry not available: %s. Using external URL directly.", registryErr.Reason)
			if changed := ic.setConditionFalse(cr, registryErr.ConditionReason, msg); changed {
				log.Info("Internal registry not available, skipping image caching", "reason", registryErr.Reason)
				ic.Recorder.Event(cr, corev1.EventTypeNormal, "ImageCachingSkipped",
					fmt.Sprintf("Image caching skipped: %s", registryErr.Reason))
			}
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
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.ImageCached,
		Status:             metav1.ConditionFalse,
		Reason:             provisioningv1alpha1.ReasonCachingInProgress,
		Message:            fmt.Sprintf("Caching image %s to internal registry", sourceURL),
		ObservedGeneration: cr.Generation,
	}); changed {
		log.Info("Image caching in progress", "sourceURL", sourceURL)
	}
	if err := ic.Client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status for CachingInProgress")
		return ctrl.Result{}, err
	}

	// Step 6: Build keychains for mirror operation
	externalKeychain, err := ic.buildExternalKeychain(ctx, cr)
	if err != nil {
		log.Error(err, "Failed to build external keychain")
		msg := fmt.Sprintf("Failed to build external registry keychain: %v", err)
		if changed := ic.setConditionFalse(cr, provisioningv1alpha1.ReasonRegistryAuthFailed, msg); changed {
			ic.Recorder.Event(cr, corev1.EventTypeWarning, "ImageCacheFailed", msg)
		}
		if updateErr := ic.Client.Status().Update(ctx, cr); updateErr != nil {
			log.Error(updateErr, "Failed to update status after keychain failure")
		}
		return ctrl.Result{RequeueAfter: initialBackoff}, nil
	}
	internalKeychain := ic.buildInternalKeychain()

	// Step 6.5: Resolve operator namespace for target image reference
	opNamespace, err := resolveOperatorNamespace()
	if err != nil {
		log.Error(err, "Failed to resolve operator namespace")
		msg := fmt.Sprintf("Failed to resolve operator namespace: %v", err)
		if changed := ic.setConditionFalse(cr, provisioningv1alpha1.ReasonCacheFailed, msg); changed {
			ic.Recorder.Event(cr, corev1.EventTypeWarning, "ImageCacheFailed", msg)
		}
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

		// Track retry count via annotation so we can give up after maxCacheRetries
		retries := getCacheRetryCount(cr) + 1
		log.Error(err, "Image mirror failed", "reason", reason, "attempt", retries, "maxRetries", maxCacheRetries)

		if retries >= maxCacheRetries {
			// Exceeded max retries — give up on caching and let provisioning
			// proceed with the external URL (opportunistic design).
			log.Info("Max cache retries exceeded, skipping image caching permanently for this generation",
				"retries", retries)
			msg := fmt.Sprintf("Image caching failed after %d attempts, proceeding with external URL: %v", retries, err)
			if changed := ic.setConditionFalse(cr, provisioningv1alpha1.ReasonCacheFailed, msg); changed {
				ic.Recorder.Event(cr, corev1.EventTypeWarning, "ImageCacheAbandoned",
					fmt.Sprintf("Image caching abandoned after %d failures, using external URL directly", retries))
			}
			clearCacheRetryCount(cr)
			if updateErr := ic.Client.Update(ctx, cr); updateErr != nil {
				log.Error(updateErr, "Failed to clear retry annotation after max retries")
			}
			if updateErr := ic.Client.Status().Update(ctx, cr); updateErr != nil {
				log.Error(updateErr, "Failed to update status after max retries")
			}
			return ctrl.Result{}, nil
		}

		setCacheRetryCount(cr, retries)
		msg := fmt.Sprintf("Image caching failed (attempt %d/%d): %v", retries, maxCacheRetries, err)
		if changed := ic.setConditionFalse(cr, reason, msg); changed {
			ic.Recorder.Event(cr, corev1.EventTypeWarning, "ImageCacheFailed", msg)
		}
		if updateErr := ic.Client.Update(ctx, cr); updateErr != nil {
			log.Error(updateErr, "Failed to persist retry annotation")
		}
		if updateErr := ic.Client.Status().Update(ctx, cr); updateErr != nil {
			log.Error(updateErr, "Failed to update status after mirror failure")
		}
		return ctrl.Result{RequeueAfter: initialBackoff}, nil
	}

	// Step 8: Update status with cached URL and clear retry counter
	clearCacheRetryCount(cr)
	if updateErr := ic.Client.Update(ctx, cr); updateErr != nil {
		log.Error(updateErr, "Failed to clear retry annotation on success")
	}
	cr.Status.CachedMachineOSURL = targetURL
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.ImageCached,
		Status:             metav1.ConditionTrue,
		Reason:             provisioningv1alpha1.ReasonImageCached,
		Message:            fmt.Sprintf("Image successfully cached to internal registry: %s", targetURL),
		ObservedGeneration: cr.Generation,
	}); changed {
		ic.Recorder.Event(cr, corev1.EventTypeNormal, "ImageCached",
			fmt.Sprintf("Image cached to internal registry: %s", targetURL))
		log.Info("Image caching completed successfully", "cachedURL", targetURL)
	}

	if err := ic.Client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status after successful cache")
		return ctrl.Result{}, err
	}

	return ctrl.Result{}, nil
}

// setConditionFalse sets the ImageCached condition to False.
// The semantic distinction between "skip" and "error" is encoded in the reason parameter.
// Returns true if the condition actually changed (callers should gate events/logs on this).
func (ic *ImageCache) setConditionFalse(cr *provisioningv1alpha1.DPFHCPProvisioner, reason, message string) bool {
	return meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.ImageCached,
		Status:             metav1.ConditionFalse,
		Reason:             reason,
		Message:            message,
		ObservedGeneration: cr.Generation,
	})
}

// getCacheRetryCount reads the retry counter from the CR's annotations.
func getCacheRetryCount(cr *provisioningv1alpha1.DPFHCPProvisioner) int {
	if cr.Annotations == nil {
		return 0
	}
	v, err := strconv.Atoi(cr.Annotations[cacheRetryAnnotation])
	if err != nil {
		return 0
	}
	return v
}

// setCacheRetryCount persists the retry counter as an annotation on the CR.
func setCacheRetryCount(cr *provisioningv1alpha1.DPFHCPProvisioner, count int) {
	if cr.Annotations == nil {
		cr.Annotations = make(map[string]string)
	}
	cr.Annotations[cacheRetryAnnotation] = strconv.Itoa(count)
}

// clearCacheRetryCount removes the retry counter annotation.
func clearCacheRetryCount(cr *provisioningv1alpha1.DPFHCPProvisioner) {
	if cr.Annotations != nil {
		delete(cr.Annotations, cacheRetryAnnotation)
	}
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
//
// PREREQUISITE: The operator's ServiceAccount must have the `registry-editor`
// (or `system:image-builder`) role in the target namespace for image pushes
// to succeed. This RoleBinding must be set up by the deployment mechanism
// (Helm chart, OLM, etc.) — it is not managed by the operator's own RBAC.
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
