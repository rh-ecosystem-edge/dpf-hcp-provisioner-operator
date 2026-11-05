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
	"errors"
	"fmt"
	"strings"

	"github.com/blang/semver/v4"
	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	"github.com/google/go-containerregistry/pkg/v1/remote"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/dpfdefaults"
)

const (
	// OCPReleaseLabel is the OCI image label that contains the OCP version string on an OCP release image
	OCPReleaseLabel = "io.openshift.release"

	// OCPLayerOCPVersionLabel is the label that contains the OCP version string
	// on a BlueField OCP layer image
	OCPLayerOCPVersionLabel = "OCP.version"

	// OCPLayerDOCAVersionLabel is the label that contains the full DOCA version
	// string on a BlueField OCP layer image
	OCPLayerDOCAVersionLabel = "NVIDIA.DOCA.version"

	// Reason codes
	reasonImageFound                  = "ImageFound"
	reasonRegistryAccessError         = "RegistryAccessError"
	reasonRegistryAuthError           = "RegistryAuthError"
	reasonInvalidImageFormat          = "InvalidImageFormat"
	reasonVersionNotFound             = "VersionNotFound"
	reasonInvalidBlueFieldOCPLayerURL = "InvalidBlueFieldOCPLayerImageURL"
	reasonDPFVersionNotFound          = "DPFVersionNotFound"
	reasonUnsupportedDPFVersion       = "UnsupportedDPFVersion"
	reasonLabelMismatch               = "LabelMismatch"
)

// Registry abstracts the container registry calls used by the image lookup
type Registry interface {
	ListTags(ctx context.Context, repo name.Repository, keychain authn.Keychain) ([]string, error)
	FetchLabels(ctx context.Context, image string, keychain authn.Keychain) (map[string]string, error)
}

// RemoteRegistry is the production implementation that talks to a real container registry
type RemoteRegistry struct{}

// ListTags lists all tags in the repository using the registry tags list API.
// Errors are classified with registryError.
func (r *RemoteRegistry) ListTags(ctx context.Context, repo name.Repository, keychain authn.Keychain) ([]string, error) {
	tags, err := remote.List(repo, remote.WithAuthFromKeychain(keychain), remote.WithContext(ctx))
	if err != nil {
		return nil, registryError(repo.String(), err)
	}
	return tags, nil
}

// FetchLabels fetches the image config (manifest + config blob) and returns its labels.
// Errors are classified with registryError.
func (r *RemoteRegistry) FetchLabels(ctx context.Context, image string, keychain authn.Keychain) (map[string]string, error) {
	ref, err := name.ParseReference(image)
	if err != nil {
		return nil, registryError(image, fmt.Errorf("failed to parse image reference %s: %w", image, err))
	}
	desc, err := remote.Get(ref, remote.WithAuthFromKeychain(keychain), remote.WithContext(ctx))
	if err != nil {
		return nil, registryError(image, err)
	}
	img, err := desc.Image()
	if err != nil {
		return nil, registryError(image, err)
	}
	cfg, err := img.ConfigFile()
	if err != nil {
		return nil, registryError(image, err)
	}
	return cfg.Config.Labels, nil
}

// ImageLookup handles BlueField OCP layer image lookup by querying a container registry
type ImageLookup struct {
	client.Client
	Recorder   record.EventRecorder
	Registry   Registry
	Repository string
}

// NewImageLookup creates a new ImageLookup
func NewImageLookup(c client.Client, recorder record.EventRecorder) *ImageLookup {
	return &ImageLookup{
		Client:   c,
		Recorder: recorder,
		Registry: &RemoteRegistry{},
	}
}

// LookupBlueFieldOCPLayerImage is the main function for BlueField OCP layer image lookup.
// It extracts the OCP version from the ocpReleaseImage, resolves the compatible DOCA
// version from the DPF/DOCA compatibility matrix (based on the installed DPF version),
// queries the container registry for a tag matching both, and updates the CR status
// with the found image reference.
func (r *ImageLookup) LookupBlueFieldOCPLayerImage(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	logger := log.FromContext(ctx)
	logger = logger.WithValues("feature", "bluefield-ocp-layer-lookup")

	// Read ocpReleaseImage from spec
	ocpReleaseImage := cr.Spec.OCPReleaseImage
	if ocpReleaseImage == "" {
		logger.Info("ocpReleaseImage is required but empty")
		return r.fail(ctx, cr, permanentError(reasonInvalidImageFormat, "ocpReleaseImage is required but empty"))
	}

	// Use the repository from operator config
	repository := r.Repository

	// Get authentication keychain from the CR's pullSecretRef
	keychain, err := r.getKeychain(ctx, cr)
	if err != nil {
		logger.Error(err, "Failed to get registry credentials")
		return r.fail(ctx, cr, permanentError(reasonRegistryAuthError, "failed to get credentials for registry repository %s: %w", repository, err))
	}

	// Extract OCP version (tag parsing first, then registry label for digest images)
	logger.V(1).Info("Extracting OCP version", "ocpReleaseImage", ocpReleaseImage)
	version, err := ExtractOCPVersion(ctx, ocpReleaseImage, keychain)
	if err != nil {
		logger.Error(err, "Failed to extract OCP version", "ocpReleaseImage", ocpReleaseImage)
		if isAuthError(err) || strings.Contains(err.Error(), "failed to fetch") || strings.Contains(err.Error(), "failed to get") {
			return r.fail(ctx, cr, registryError(ocpReleaseImage, err))
		}
		return r.fail(ctx, cr, permanentError(reasonInvalidImageFormat, "invalid ocpReleaseImage %s: %w", ocpReleaseImage, err))
	}
	logger.V(1).Info("Extracted OCP version", "version", version)

	// Resolve the DOCA version compatible with the running DPF version from
	// the shared embedded per-DPF-version table. Unknown DPF versions (e.g.
	// DPF main) fall back to the table's default row.
	dpfVersion, err := common.GetDPFVersion(ctx, r.Client)
	if err != nil {
		logger.Error(err, "Failed to determine DPF version from DPFOperatorConfig")
		return r.fail(ctx, cr, transientError(reasonDPFVersionNotFound, "failed to determine DPF version: %w", err))
	}
	dpfMajorMinor, err := common.DPFMajorMinorFromVersion(dpfVersion)
	if err != nil {
		logger.Error(err, "Failed to parse DPF version", "dpfVersion", dpfVersion)
		return r.fail(ctx, cr, transientError(reasonDPFVersionNotFound, "failed to determine DPF version: %w", err))
	}

	docaMajorMinor, err := dpfdefaults.DocaVersion(ctx, dpfMajorMinor)
	if err != nil {
		logger.Error(err, "Failed to resolve DOCA version", "dpfVersion", dpfMajorMinor)
		return r.fail(ctx, cr, permanentError(reasonUnsupportedDPFVersion, "no DOCA version available for DPF version %s: %w", dpfMajorMinor, err))
	}
	logger.V(1).Info("Resolved compatible DOCA version", "dpfVersion", dpfMajorMinor, "docaVersion", docaMajorMinor)

	// Query registry for a tag matching the OCP version and DOCA version
	logger.V(1).Info("Querying registry for BlueField OCP layer image", "repository", repository, "version", version, "docaVersion", docaMajorMinor)
	blueFieldOCPLayerImage, err := r.selectImageTag(ctx, repository, version, docaMajorMinor, keychain)
	if err != nil {
		return r.fail(ctx, cr, err)
	}

	// Validate the found image URL
	logger.V(1).Info("Validating BlueField OCP layer image URL", "blueFieldOCPLayerImage", blueFieldOCPLayerImage)
	if err := validateBlueFieldOCPLayerImageURL(blueFieldOCPLayerImage, version); err != nil {
		logger.Error(err, "BlueField OCP layer image URL validation failed", "blueFieldOCPLayerImage", blueFieldOCPLayerImage)
		return r.fail(ctx, cr, err)
	}

	// Update status on success
	logger.Info("BlueField OCP layer image found successfully",
		"version", version,
		"docaVersion", docaMajorMinor,
		"blueFieldOCPLayerImage", blueFieldOCPLayerImage)
	return r.updateStatusOnSuccess(ctx, cr, blueFieldOCPLayerImage, version, docaMajorMinor)
}

// extractVersionFromTag is an internal helper that extracts OCP version from a tagged image URL.
// Returns empty string if not parseable (digest images, missing tag, etc.).
func extractVersionFromTag(imageURL string) string {
	if strings.Contains(imageURL, "@") {
		return ""
	}
	lastSlash := strings.LastIndex(imageURL, "/")
	lastColon := strings.LastIndex(imageURL, ":")
	if lastColon <= lastSlash {
		return ""
	}
	tag := imageURL[lastColon+1:]
	if tag == "" {
		return ""
	}
	suffixes := []string{"-multi", "-amd64", "-arm64", "-ppc64le", "-s390x", "-x86_64"}
	version := tag
	for _, suffix := range suffixes {
		version = strings.TrimSuffix(version, suffix)
	}
	return version
}

// ExtractOCPVersion resolves the OCP version from a release image.
// For tagged images, parses the version from the tag (no registry call).
// For digest images, reads the [OCPReleaseLabel] label from the image config via registry.
func ExtractOCPVersion(ctx context.Context, releaseImage string, keychain authn.Keychain) (string, error) {
	// Try tag-based extraction first (fast, no registry call)
	if version := extractVersionFromTag(releaseImage); version != "" {
		return version, nil
	}

	// Digest image: fetch the image config from the registry
	ref, err := name.ParseReference(releaseImage)
	if err != nil {
		return "", fmt.Errorf("failed to parse image reference %s: %w", releaseImage, err)
	}

	desc, err := remote.Get(ref, remote.WithAuthFromKeychain(keychain), remote.WithContext(ctx))
	if err != nil {
		return "", fmt.Errorf("failed to fetch image descriptor for %s: %w", releaseImage, err)
	}

	img, err := desc.Image()
	if err != nil {
		return "", fmt.Errorf("failed to get image from descriptor for %s: %w", releaseImage, err)
	}

	cfg, err := img.ConfigFile()
	if err != nil {
		return "", fmt.Errorf("failed to get config file for %s: %w", releaseImage, err)
	}

	version, ok := cfg.Config.Labels[OCPReleaseLabel]
	if !ok || version == "" {
		return "", fmt.Errorf("image %s does not have label %s", releaseImage, OCPReleaseLabel)
	}

	return version, nil
}

// getKeychain returns an authn.Keychain for authenticating to the container registry.
// It reuses the CR's pullSecretRef which typically contains credentials for multiple registries.
func (r *ImageLookup) getKeychain(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (authn.Keychain, error) {
	return common.KeychainFromPullSecret(ctx, r.Client, cr.Spec.PullSecretRef.Name, cr.Namespace)
}

// selectImageTag lists the tags in the repository, selects the BlueField OCP layer
// image tag matching the OCP version and the compatible DOCA version, and verifies
// the selected image's version labels against its tag.
// Returns the full image reference (repository:tag) if found.
func (r *ImageLookup) selectImageTag(ctx context.Context, repository, ocpVersion, docaMajorMinor string, keychain authn.Keychain) (string, error) {
	repo, err := name.NewRepository(repository)
	if err != nil {
		return "", registryError(repository, fmt.Errorf("invalid repository: %w", err))
	}

	tags, err := r.Registry.ListTags(ctx, repo, keychain)
	if err != nil {
		return "", err
	}

	tag, found := findBlueFieldOCPLayerTag(tags, ocpVersion, docaMajorMinor)
	if !found {
		return "", permanentError(reasonVersionNotFound, "BlueField OCP layer image not found for OCP version %s with DOCA version %s.x in repository %s",
			ocpVersion, docaMajorMinor, repository)
	}

	image := repository + ":" + tag
	if err := r.verifyImageLabels(ctx, image, ocpVersion, docaMajorMinor, keychain); err != nil {
		return "", err
	}

	return image, nil
}

// verifyImageLabels fetches the selected image's config and checks its OCP and
// DOCA version labels against the versions the tag was selected by. Labels are
// only checked when present: a missing label does not fail the lookup, but a
// label that disagrees with the tag marks the image as mis-pushed.
func (r *ImageLookup) verifyImageLabels(ctx context.Context, image, ocpVersion, docaMajorMinor string, keychain authn.Keychain) error {
	labels, err := r.Registry.FetchLabels(ctx, image, keychain)
	if err != nil {
		return err
	}

	if ocp, ok := labels[OCPLayerOCPVersionLabel]; ok && ocp != ocpVersion {
		return labelMismatchError(image, OCPLayerOCPVersionLabel, ocp, ocpVersion)
	}

	if doca, ok := labels[OCPLayerDOCAVersionLabel]; ok && !docaVersionMatches(doca, docaMajorMinor) {
		return labelMismatchError(image, OCPLayerDOCAVersionLabel, doca, docaMajorMinor+".x")
	}

	return nil
}

func labelMismatchError(image, label, value, expected string) error {
	return permanentError(reasonLabelMismatch, "BlueField OCP layer image %s has label %s=%q which disagrees with expected %q",
		image, label, value, expected)
}

// docaVersionMatches reports whether the DOCA version in label (e.g. "3.4.2")
// belongs to the target DOCA major.minor (e.g. "3.4").
func docaVersionMatches(label, targetMajorMinor string) bool {
	target, err := semver.ParseTolerant(targetMajorMinor)
	if err != nil {
		return false
	}
	version, err := semver.ParseTolerant(label)
	if err != nil {
		return false
	}
	return version.Major == target.Major && version.Minor == target.Minor
}

// findBlueFieldOCPLayerTag selects the tag that matches the OCP version and the
// compatible DOCA major.minor version. Tags are expected in the format
// "<ocpVersion>-doca<docaVersion>" (e.g. "4.22.11-doca3.4.2").
// If multiple tags match (e.g. different DOCA patch releases), the highest
// DOCA version wins.
func findBlueFieldOCPLayerTag(tags []string, ocpVersion, docaMajorMinor string) (string, bool) {
	target, err := semver.ParseTolerant(docaMajorMinor)
	if err != nil {
		return "", false
	}

	// Multiple tags may match the same DOCA major.minor with different patch
	// releases (e.g. 4.22.11-doca3.4.1 and 4.22.11-doca3.4.2), so keep
	// scanning after the first match and keep the highest DOCA version.
	prefix := ocpVersion + "-doca"
	var bestTag string
	var bestDoca semver.Version
	for _, tag := range tags {
		if !strings.HasPrefix(tag, prefix) {
			continue
		}
		docaVersion, err := semver.ParseTolerant(tag[len(prefix):])
		if err != nil {
			continue
		}
		if docaVersion.Major != target.Major || docaVersion.Minor != target.Minor {
			continue
		}
		if docaVersion.GT(bestDoca) {
			bestTag, bestDoca = tag, docaVersion
		}
	}
	return bestTag, bestTag != ""
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

// validateBlueFieldOCPLayerImageURL validates the BlueField OCP layer image URL format.
// Exported for testing.
func validateBlueFieldOCPLayerImageURL(url string, version string) error {
	if url == "" {
		return permanentError(reasonInvalidBlueFieldOCPLayerURL, "BlueField OCP layer image URL is empty for OCP version %s", version)
	}

	// Basic validation: should contain a colon (registry/repo:tag format)
	if !strings.Contains(url, ":") {
		return permanentError(reasonInvalidBlueFieldOCPLayerURL, "BlueField OCP layer image URL %s for OCP version %s is missing tag separator ':'", url, version)
	}

	return nil
}

// updateStatusOnSuccess updates the CR status when BlueField OCP layer image lookup succeeds
func (r *ImageLookup) updateStatusOnSuccess(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, blueFieldOCPLayerImage, version, docaMajorMinor string) (ctrl.Result, error) {
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
			fmt.Sprintf("BlueField OCP layer image found for OCP version %s (DOCA %s.x): %s", version, docaMajorMinor, blueFieldOCPLayerImage))
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

// fail reports a lookup failure on the CR: it clears the resolved image, sets
// the BlueFieldOCPLayerImageFound condition to False with the error's reason,
// emits an event when the condition changes, and persists the status.
// Permanent failures are not requeued; anything else is returned so
// controller-runtime retries with backoff.
func (r *ImageLookup) fail(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, err error) (ctrl.Result, error) {
	lerr := &lookupError{reason: reasonRegistryAccessError, err: err}
	errors.As(err, &lerr)

	cr.Status.BlueFieldOCPLayerImage = ""

	condition := metav1.Condition{
		Type:               provisioningv1alpha1.BlueFieldOCPLayerImageFound,
		Status:             metav1.ConditionFalse,
		Reason:             lerr.reason,
		Message:            err.Error(),
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		r.Recorder.Event(cr, corev1.EventTypeWarning, condition.Reason, condition.Message)
	}

	if updateErr := r.Status().Update(ctx, cr); updateErr != nil {
		log.FromContext(ctx).Error(updateErr, "Failed to update status after lookup failure", "error", err)
	}

	if lerr.permanent {
		return ctrl.Result{}, nil
	}
	return ctrl.Result{}, err
}

// lookupError is a BlueField OCP layer lookup failure. reason is reported on the
// BlueFieldOCPLayerImageFound condition. Permanent failures need user action and
// are not requeued; transient ones are retried with backoff.
type lookupError struct {
	reason    string
	permanent bool
	err       error
}

func (e *lookupError) Error() string { return e.err.Error() }
func (e *lookupError) Unwrap() error { return e.err }

func permanentError(reason, format string, args ...any) error {
	return &lookupError{reason: reason, permanent: true, err: fmt.Errorf(format, args...)}
}

func transientError(reason, format string, args ...any) error {
	return &lookupError{reason: reason, err: fmt.Errorf(format, args...)}
}

// registryError classifies a failed registry call: auth failures are permanent,
// anything else is transient.
func registryError(repository string, err error) error {
	if isAuthError(err) {
		return permanentError(reasonRegistryAuthError, "authentication failed for registry repository %s: %w", repository, err)
	}
	return transientError(reasonRegistryAccessError, "failed to access registry repository %s: %w", repository, err)
}
