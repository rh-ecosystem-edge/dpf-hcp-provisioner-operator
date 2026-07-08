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
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"sort"

	"github.com/blang/semver/v4"
	"github.com/go-logr/logr"
	"github.com/google/go-containerregistry/pkg/authn"
	configv1 "github.com/openshift/api/config/v1"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
)

const (
	clusterVersionName = "version"

	clusterPullSecretName      = "pull-secret"
	clusterPullSecretNamespace = "openshift-config"

	ovnDPUDaemonSetName      = "ovnkube-node-dpu-host"
	ovnDPUDaemonSetNamespace = "openshift-ovn-kubernetes"
	ovnControllerName        = "ovnkube-controller"

	// TODO: This is an OVN-K template, not OVN, some day we should probably
	// change "ovn" to "ovn-k", but then we also have to touch the
	// DPUDeployment which refers to it, which resides in the openshift-dpf
	// automation repo...
	templateNameOVNK = "ovn"
	templateNameDTS  = "doca-telemetry-service"
	templateNameHBN  = "hbn"

	// AnnotationSourceOVNImage records the x86 OVN image from the DaemonSet that was used
	// to derive the current aarch64 OVN image. When this differs from the DaemonSet's
	// current image, the aarch64 image is re-resolved from the release payload.
	// On multi-arch release payloads, this is actually a manifest-list reference, but
	// we still refer to it as the "x86 OVN image" for clarity that it's what's running
	// on the x86 management cluster.
	AnnotationSourceOVNImage = "dpfhcpprovisioner.dpu.hcp.io/source-x86-ovn-image"
)

// DPUServiceTemplateManager handles DPUServiceTemplate resource creation and deletion.
type DPUServiceTemplateManager struct {
	client             client.Client
	apiReader          client.Reader
	ReleaseImageReader ReleaseImageReader
	OperatorNamespace  string
}

// NewDPUServiceTemplateManager creates a new DPUServiceTemplate manager
func NewDPUServiceTemplateManager(c client.Client, apiReader client.Reader, reader ReleaseImageReader, operatorNamespace string) *DPUServiceTemplateManager {
	return &DPUServiceTemplateManager{
		client:             c,
		apiReader:          apiReader,
		ReleaseImageReader: reader,
		OperatorNamespace:  operatorNamespace,
	}
}

// EnsureTemplates creates or updates the three DPUServiceTemplate resources
// (OVN, DTS, HBN) in the given namespace.
func (m *DPUServiceTemplateManager) EnsureTemplates(ctx context.Context, namespace string, operatorConfig *common.OperatorConfig) error {
	log := logf.FromContext(ctx).WithValues("namespace", namespace)

	log.Info("Ensuring DPUServiceTemplates")

	dpuServiceTemplateValues, err := m.getDPUServiceTemplateValuesForCurrentDPF(ctx)
	if err != nil {
		return fmt.Errorf("determining DPUServiceTemplate values: %w", err)
	}

	dpuServiceTemplateValues.ImagePullSecret = operatorConfig.DPUServicesImagePullSecret

	if err := m.ensureOVNTemplate(ctx, namespace, dpuServiceTemplateValues); err != nil {
		return fmt.Errorf("ensuring OVN template: %w", err)
	}

	if err := m.ensureDTSTemplate(ctx, namespace, dpuServiceTemplateValues); err != nil {
		return fmt.Errorf("ensuring DTS template: %w", err)
	}

	if err := m.ensureHBNTemplate(ctx, namespace, dpuServiceTemplateValues); err != nil {
		return fmt.Errorf("ensuring HBN template: %w", err)
	}

	log.Info("DPUServiceTemplate configuration complete")
	return nil
}

// DeleteTemplates removes the three DPUServiceTemplate resources from the given namespace.
func (m *DPUServiceTemplateManager) DeleteTemplates(ctx context.Context, namespace string) error {
	log := logf.FromContext(ctx).WithValues("namespace", namespace)

	log.Info("Deleting DPUServiceTemplates")

	var list dpuservicev1alpha1.DPUServiceTemplateList
	if err := m.client.List(ctx, &list,
		client.InNamespace(namespace),
		client.MatchingLabels{common.LabelManagedBy: "true"},
	); err != nil {
		return fmt.Errorf("listing managed DPUServiceTemplates in %s: %w", namespace, err)
	}

	for i := range list.Items {
		log.Info("Deleting DPUServiceTemplate", "name", list.Items[i].Name)
		if err := m.client.Delete(ctx, &list.Items[i]); err != nil && !apierrors.IsNotFound(err) {
			return fmt.Errorf("deleting DPUServiceTemplate %s/%s: %w", namespace, list.Items[i].Name, err)
		}
	}

	log.Info("DPUServiceTemplate cleanup complete")
	return nil
}

// getDPFVersion reads the DPF version from DPFOperatorConfig.status.version
func (m *DPUServiceTemplateManager) getDPFVersion(ctx context.Context) (string, error) {
	config, err := common.GetSingletonDPFOperatorConfig(ctx, m.client)
	if err != nil {
		return "", err
	}

	if config.Status.Version == nil || *config.Status.Version == "" {
		return "", fmt.Errorf("DPFOperatorConfig.status.version is not set")
	}

	return *config.Status.Version, nil
}

// getDPFMajorMinorVersion reads the DPF version and returns the major.minor component (e.g. "26.4").
func (m *DPUServiceTemplateManager) getDPFMajorMinorVersion(ctx context.Context) (string, error) {
	dpfVersion, err := m.getDPFVersion(ctx)
	if err != nil {
		return "", fmt.Errorf("determining DPF version: %w", err)
	}
	version := strings.TrimPrefix(dpfVersion, "v")
	parts := strings.SplitN(version, ".", 3)
	if len(parts) < 2 {
		return "", fmt.Errorf("DPF version %q does not contain a minor component", dpfVersion)
	}
	return parts[0] + "." + parts[1], nil
}

// getDPUServiceTemplateValuesForCurrentDPF determines the running DPF major.minor version and
// returns the matching template values with any user overrides applied.
func (m *DPUServiceTemplateManager) getDPUServiceTemplateValuesForCurrentDPF(ctx context.Context) (*DPUServiceTemplateValues, error) {
	version, err := m.getDPFMajorMinorVersion(ctx)
	if err != nil {
		return nil, err
	}
	return m.getDPUServiceTemplateValues(ctx, version)
}

func (m *DPUServiceTemplateManager) getDPUServiceTemplateValues(ctx context.Context, version string) (*DPUServiceTemplateValues, error) {
	values, err := DPUServiceTemplateValuesForVersion(version)
	if err != nil {
		return nil, err
	}
	if err := applyOverridesFromConfigMap(ctx, m.apiReader, m.OperatorNamespace, version, values); err != nil {
		return nil, fmt.Errorf("applying overrides from configmap: %w", err)
	}
	return values, nil
}

type ovnTemplateInfo struct {
	Repo             string
	Tag              string
	SourceImage      string
	DaemonsetVersion string
}

// existingOVNKTemplateValues extracts the OVN image repo, tag, and daemonset
// version from an existing template's values.
func (m *DPUServiceTemplateManager) existingOVNKTemplateValues(dpuServiceTemplate *dpuservicev1alpha1.DPUServiceTemplate, sourceImage string) (ovnTemplateInfo, error) {
	if dpuServiceTemplate.Spec.HelmChart.Values == nil {
		return ovnTemplateInfo{}, fmt.Errorf("existing OVN template has no values")
	}
	var helmValues map[string]any
	if err := json.Unmarshal(dpuServiceTemplate.Spec.HelmChart.Values.Raw, &helmValues); err != nil {
		return ovnTemplateInfo{}, fmt.Errorf("parsing existing OVN template values: %w", err)
	}
	dpuManifests, ok := helmValues["dpuManifests"].(map[string]any)
	if !ok {
		return ovnTemplateInfo{}, fmt.Errorf("existing OVN template missing dpuManifests")
	}
	dpuManifestsImage, ok := dpuManifests["image"].(map[string]any)
	if !ok {
		return ovnTemplateInfo{}, fmt.Errorf("existing OVN template missing dpuManifests.image")
	}
	ovnkImageRepo, _ := dpuManifestsImage["repository"].(string)
	ovnkImageTag, _ := dpuManifestsImage["tag"].(string)
	if ovnkImageRepo == "" || ovnkImageTag == "" {
		return ovnTemplateInfo{}, fmt.Errorf("existing OVN template has empty image repository or tag")
	}
	daemonsetVersion, _ := helmValues["ovnDaemonsetVersion"].(string)
	return ovnTemplateInfo{
		Repo:             ovnkImageRepo,
		Tag:              ovnkImageTag,
		SourceImage:      sourceImage,
		DaemonsetVersion: daemonsetVersion,
	}, nil
}

// resolveARM64OVNImageForCurrentDaemonSet resolves the aarch64 OVN image from
// the release payload for the current DaemonSet image.
func (m *DPUServiceTemplateManager) resolveARM64OVNImageForCurrentDaemonSet(ctx context.Context, currentImage string, releaseImage, ocpVersion string) (ovnTemplateInfo, error) {
	log := logf.FromContext(ctx)
	log.Info("Resolving aarch64 OVN image from release payload", "sourceImage", currentImage)
	repo, tag, err := m.resolveARM64OVNImage(ctx, releaseImage, ocpVersion)
	if err != nil {
		return ovnTemplateInfo{}, err
	}
	return ovnTemplateInfo{
		Repo:        repo,
		Tag:         tag,
		SourceImage: currentImage,
	}, nil
}

// getWorkerOVNKDaemonSet reads the ovnkube-node-dpu-host DaemonSet.
func (m *DPUServiceTemplateManager) getWorkerOVNKDaemonSet(ctx context.Context) (*appsv1.DaemonSet, error) {
	var ds appsv1.DaemonSet
	if err := m.client.Get(ctx, types.NamespacedName{
		Name:      ovnDPUDaemonSetName,
		Namespace: ovnDPUDaemonSetNamespace,
	}, &ds); err != nil {
		return nil, fmt.Errorf("getting DaemonSet %s/%s: %w", ovnDPUDaemonSetNamespace, ovnDPUDaemonSetName, err)
	}
	return &ds, nil
}

func ovnkImageFromDaemonSet(daemonSet *appsv1.DaemonSet) (string, error) {
	for _, container := range daemonSet.Spec.Template.Spec.Containers {
		if container.Name == ovnControllerName {
			return container.Image, nil
		}
	}
	return "", fmt.Errorf("container %q not found in DaemonSet %s/%s", ovnControllerName, ovnDPUDaemonSetNamespace, ovnDPUDaemonSetName)
}

func daemonSetRolloutComplete(ds *appsv1.DaemonSet) bool {
	return ds.Status.ObservedGeneration >= ds.Generation &&
		ds.Status.UpdatedNumberScheduled == ds.Status.DesiredNumberScheduled &&
		ds.Status.NumberAvailable == ds.Status.DesiredNumberScheduled
}

// ovnDaemonsetVersionEntry maps a minimum OCP version to the OVN daemonset version
// that was introduced at that release. Entries are sorted ascending by version at init.
type ovnDaemonsetVersionEntry struct {
	minVersion       semver.Version
	daemonsetVersion string
}

// ovnDaemonsetVersionTable lists the OCP versions where OVN_DAEMONSET_VERSION changed.
// Each stream (4.21, 4.22, …) can have independent entries because OCP backports
// newer OVN builds into older minor releases at arbitrary z-stream versions.
var ovnDaemonsetVersionTable []ovnDaemonsetVersionEntry

func init() {
	raw := []struct {
		v  string
		dv string
	}{
		// We only really support OCP versions starting from 4.22.6 but leaving
		// 4.22.0 and 4.22.1 here for completeness in case someone tries to run
		// on those versions, and to serve as an example of how to add future
		// entries for new OCP versions.
		{"4.22.0", "1.2.0"},
		{"4.22.1", "1.3.0"},
	}
	for _, e := range raw {
		ovnDaemonsetVersionTable = append(ovnDaemonsetVersionTable, ovnDaemonsetVersionEntry{
			minVersion:       semver.MustParse(e.v),
			daemonsetVersion: e.dv,
		})
	}
	sort.Slice(ovnDaemonsetVersionTable, func(i, j int) bool {
		return ovnDaemonsetVersionTable[i].minVersion.LT(ovnDaemonsetVersionTable[j].minVersion)
	})
}

// OVNKDaemonsetVersionForOCP returns the OVN_DAEMONSET_VERSION value for a given
// OCP version string. It walks the table in reverse and returns the daemonset
// version from the highest entry whose minVersion <= ocpVersion.
func OVNKDaemonsetVersionForOCP(ocpVersion string) string {
	const defaultVersion = "1.3.0"

	v, err := semver.ParseTolerant(ocpVersion)
	if err != nil {
		return defaultVersion
	}
	// Strip pre-release so that e.g. "4.22.1-rc.1" matches >= 4.22.1
	v.Pre = nil

	result := defaultVersion
	for _, e := range ovnDaemonsetVersionTable {
		if v.GTE(e.minVersion) {
			result = e.daemonsetVersion
		}
	}
	return result
}

// getOCPVersionFromClusterVersion reads the OCP version and release image from ClusterVersion.
func (m *DPUServiceTemplateManager) getOCPVersionFromClusterVersion(ctx context.Context) (version, releaseImage string, err error) {
	var cv configv1.ClusterVersion
	if err := m.client.Get(ctx, types.NamespacedName{Name: clusterVersionName}, &cv); err != nil {
		return "", "", fmt.Errorf("getting ClusterVersion: %w", err)
	}
	if cv.Status.Desired.Version == "" {
		return "", "", fmt.Errorf("ClusterVersion status.desired.version is empty")
	}
	if cv.Status.Desired.Image == "" {
		return "", "", fmt.Errorf("ClusterVersion status.desired.image is empty")
	}
	return cv.Status.Desired.Version, cv.Status.Desired.Image, nil
}

// ponytail: tag-based check only; digest-based multiarch refs fall back to aarch64 resolution
func isMultiArchReleaseImage(image string) bool {
	return strings.HasSuffix(image, "-multi")
}

// resolveARM64OVNImage extracts the aarch64 ovn-kubernetes image from the release payload.
func (m *DPUServiceTemplateManager) resolveARM64OVNImage(ctx context.Context, releaseImage, ocpVersion string) (repo, tag string, err error) {
	registry := releaseImage
	if idx := strings.Index(registry, "@"); idx > 0 {
		registry = registry[:idx]
	} else if idx := strings.LastIndex(registry, ":"); idx > 0 {
		registry = registry[:idx]
	}
	aarch64Ref := fmt.Sprintf("%s:%s-aarch64", registry, ocpVersion)

	keychain, err := m.getClusterPullSecretKeychain(ctx)
	if err != nil {
		return "", "", fmt.Errorf("getting cluster pull secret: %w", err)
	}

	ovnImage, err := m.ReleaseImageReader.GetComponentImage(ctx, aarch64Ref, ovnKubernetesName, keychain)
	if err != nil {
		return "", "", fmt.Errorf("resolving aarch64 OVN image from release %q: %w", aarch64Ref, err)
	}

	return splitImage(ovnImage)
}

// getClusterPullSecretKeychain reads the global cluster pull secret and returns a keychain.
func (m *DPUServiceTemplateManager) getClusterPullSecretKeychain(ctx context.Context) (authn.Keychain, error) {
	return common.KeychainFromPullSecret(ctx, m.client, clusterPullSecretName, clusterPullSecretNamespace)
}

func (m *DPUServiceTemplateManager) ensureOVNTemplate(ctx context.Context, namespace string, defaults *DPUServiceTemplateValues) error {
	log := logf.FromContext(ctx)

	managementWorkerOVNKDaemonSet, err := m.getWorkerOVNKDaemonSet(ctx)
	if err != nil {
		return fmt.Errorf("reading OVN DaemonSet: %w", err)
	}

	currentOVNKImage, err := ovnkImageFromDaemonSet(managementWorkerOVNKDaemonSet)
	if err != nil {
		return fmt.Errorf("reading current OVN image: %w", err)
	}

	// When the OVN image hasn't changed since the template was last updated,
	// keep the existing OVN image and daemonset version, as the daemonset
	// version is derived from the desired CVO version, which may update long
	// before the OVN DaemonSet image does. We only want to update the OVN
	// image and the ovnDaemonsetVersion identifier in lockstep when the OVN
	// image actually changes.
	// We don't return early because we still want to reconcile other changes,
	// such as user overrides.
	existing := &dpuservicev1alpha1.DPUServiceTemplate{}
	templateExists := false
	CNOOVNKDaemonSetImageChanged := true
	switch err := m.client.Get(ctx, client.ObjectKey{Name: templateNameOVNK, Namespace: namespace}, existing); {
	case err == nil:
		templateExists = true
		lastUpdatedOVNKImage := existing.Annotations[AnnotationSourceOVNImage]
		CNOOVNKDaemonSetImageChanged = (lastUpdatedOVNKImage != currentOVNKImage)
	case !apierrors.IsNotFound(err):
		return fmt.Errorf("getting existing OVN template: %w", err)
	}

	// Gate DaemonSet rollout check only for updates to existing templates to avoid races.
	// On first creation, proceed immediately to break the bootstrap deadlock where
	// DPU CRs can't be created without the template, and the template can't be created
	// because DPU pods (which provide SR-IOV VFs) haven't scheduled yet.
	if templateExists && !daemonSetRolloutComplete(managementWorkerOVNKDaemonSet) {
		log.Info("OVN DaemonSet rollout in progress, skipping template update")
		return nil
	}

	ocpVersion, releaseImage, err := m.getOCPVersionFromClusterVersion(ctx)
	if err != nil {
		return fmt.Errorf("determining OCP version: %w", err)
	}

	var ovnkTemplateInfo ovnTemplateInfo
	if CNOOVNKDaemonSetImageChanged {

		// TODO(NVIDIA-906): This will never work as the .status.desired.image
		// field is always a digest. We must do something more intelligent,
		// possible reading OCI labels off of the release image itself. Leaving
		// this as broken for now and will fix it in a follow-up commit.
		if isMultiArchReleaseImage(releaseImage) {
			log.Info("Multi-arch release image detected, using management cluster OVN image directly", "image", currentOVNKImage)
			repo, tag, err := splitImage(currentOVNKImage)
			if err != nil {
				return fmt.Errorf("splitting management cluster OVN image: %w", err)
			}
			ovnkTemplateInfo = ovnTemplateInfo{Repo: repo, Tag: tag, SourceImage: currentOVNKImage}
		} else {
			ovnkTemplateInfo, err = m.resolveARM64OVNImageForCurrentDaemonSet(ctx, currentOVNKImage, releaseImage, ocpVersion)
			if err != nil {
				return fmt.Errorf("resolving OVN kubernetes image: %w", err)
			}
		}
		ovnkTemplateInfo.DaemonsetVersion = OVNKDaemonsetVersionForOCP(ocpVersion)
	} else {
		// The OVN image hasn't changed, so we keep the existing template
		// values for the OVN image and daemonset version, only changing the
		// other values that may have been overridden by the user.
		ovnkTemplateInfo, err = m.existingOVNKTemplateValues(existing, currentOVNKImage)
		if err != nil {
			return fmt.Errorf("reading existing OVN template values: %w", err)
		}
		log.V(1).Info("OVN DaemonSet image unchanged, keeping existing template OVN values")
	}
	log.Info("Setting OVN daemonset version", "ovnDaemonsetVersion", ovnkTemplateInfo.DaemonsetVersion)

	dpuManifests := map[string]any{
		"image": map[string]any{
			"repository": ovnkTemplateInfo.Repo,
			"tag":        ovnkTemplateInfo.Tag,
		},
		"enabled":    true,
		"cniBinDir":  "/var/lib/cni/bin/",
		"cniConfDir": "/run/multus/cni/net.d",
	}
	values := map[string]any{
		"commonManifests":     map[string]any{"enabled": true},
		"dpuManifests":        dpuManifests,
		"ovnDaemonsetVersion": ovnkTemplateInfo.DaemonsetVersion,
		"leaseNamespace":      "openshift-ovn-kubernetes",
		"gatewayOpts":         "--gateway-interface=br-dpu",
	}

	minOVNFeatures := semver.MustParse("4.22.2")
	if v, err := semver.ParseTolerant(ocpVersion); err == nil {
		v.Pre = nil
		if v.GTE(minOVNFeatures) {
			values["global"] = map[string]any{
				"enableEgressIP":            "true",
				"egressIpHealthCheckPort":   "0",
				"enableEgressFirewall":      "true",
				"enableEgressQoS":           "true",
				"enableEgressService":       "true",
				"enableMultiNetwork":        "false",
				"enableMultiNetworkPolicy":  "true",
				"enableAdminNetworkPolicy":  "true",
				"enableNetworkSegmentation": "true",
			}
			dpuManifests["ovnMultiNetworkEnable"] = "true" //nolint:goconst
			log.Info("OCP >= 4.22.2: enabling OVN feature flags")
		}
	}

	annotations := map[string]string{
		AnnotationSourceOVNImage: ovnkTemplateInfo.SourceImage,
	}

	return m.ensureTemplate(ctx, namespace, templateNameOVNK, templateNameOVNK, defaults.OVN.ChartRepoURL, defaults.OVN.ChartName, defaults.OVN.ChartVersion, values, nil, annotations, log)
}

func (m *DPUServiceTemplateManager) ensureDTSTemplate(ctx context.Context, namespace string, defaults *DPUServiceTemplateValues) error {
	log := logf.FromContext(ctx)

	values := map[string]any{
		"configMapData": map[string]any{
			"prometheus": map[string]any{
				"port": defaults.DTS.PrometheusPort,
			},
		},
		"hostVolumePrefix": "/var/lib",
		"imageDTS":         defaults.DTS.ImageDTS,
	}
	if defaults.ImagePullSecret != "" {
		values["imagePullSecrets"] = []map[string]any{
			{"name": defaults.ImagePullSecret},
		}
	}

	resourceReqs := corev1.ResourceList{
		corev1.ResourceCPU:     resource.MustParse("1"),
		corev1.ResourceMemory:  resource.MustParse("1Gi"),
		corev1.ResourceStorage: resource.MustParse("1Gi"),
	}

	return m.ensureTemplate(ctx, namespace, templateNameDTS, templateNameDTS, defaults.DTS.ChartRepoURL, defaults.DTS.ChartName, defaults.DTS.ChartVersion, values, resourceReqs, nil, log)
}

func (m *DPUServiceTemplateManager) ensureHBNTemplate(ctx context.Context, namespace string, defaults *DPUServiceTemplateValues) error {
	log := logf.FromContext(ctx)

	values := map[string]any{
		"image": map[string]any{
			"repository": defaults.HBN.ImageRepo,
			"tag":        defaults.HBN.ImageTag,
		},
		"resources": map[string]any{
			"memory":           "6Gi",
			"nvidia.com/bf_sf": defaults.HBN.BFSFCount,
		},
	}
	if defaults.ImagePullSecret != "" {
		values["imagePullSecrets"] = []map[string]any{
			{"name": defaults.ImagePullSecret},
		}
	}

	return m.ensureTemplate(ctx, namespace, "hbn", "hbn", defaults.HBN.ChartRepoURL, defaults.HBN.ChartName, defaults.HBN.ChartVersion, values, nil, nil, log)
}

func (m *DPUServiceTemplateManager) ensureTemplate(
	ctx context.Context,
	namespace string,
	name, deploymentServiceName string,
	repoURL, chartName, chartVersion string,
	values map[string]any,
	resourceReqs corev1.ResourceList,
	annotations map[string]string,
	log logr.Logger,
) error {
	valuesJSON, err := json.Marshal(values)
	if err != nil {
		return fmt.Errorf("marshalling values for %s: %w", name, err)
	}

	template := &dpuservicev1alpha1.DPUServiceTemplate{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: namespace,
		},
	}

	// Check if a user-created template with this name already exists
	if err := m.client.Get(ctx, client.ObjectKey{Name: name, Namespace: namespace}, template); err == nil {
		if template.Labels[common.LabelManagedBy] != "true" {
			log.Info("Skipping DPUServiceTemplate not managed by operator", "name", name)
			return nil
		}
	} else if !apierrors.IsNotFound(err) {
		return fmt.Errorf("checking existing DPUServiceTemplate %s: %w", name, err)
	}

	op, err := controllerutil.CreateOrUpdate(ctx, m.client, template, func() error {
		if template.Labels == nil {
			template.Labels = make(map[string]string)
		}
		template.Labels[common.LabelManagedBy] = "true"
		if len(annotations) > 0 {
			if template.Annotations == nil {
				template.Annotations = make(map[string]string)
			}
			for k, v := range annotations {
				template.Annotations[k] = v
			}
		}
		template.Spec = dpuservicev1alpha1.DPUServiceTemplateSpec{
			DeploymentServiceName: deploymentServiceName,
			HelmChart: dpuservicev1alpha1.HelmChart{
				Source: dpuservicev1alpha1.ApplicationSource{
					RepoURL: repoURL,
					Chart:   chartName,
					Version: chartVersion,
				},
				Values: &runtime.RawExtension{Raw: valuesJSON},
			},
			ResourceRequirements: resourceReqs,
		}
		return nil
	})

	if err != nil {
		return fmt.Errorf("creating or updating DPUServiceTemplate %s: %w", name, err)
	}

	switch op {
	case controllerutil.OperationResultCreated:
		log.Info("Created DPUServiceTemplate", "name", name, "namespace", namespace)
	case controllerutil.OperationResultUpdated:
		log.Info("Updated DPUServiceTemplate (drift corrected)", "name", name)
	case controllerutil.OperationResultNone:
		log.V(1).Info("DPUServiceTemplate already matches desired state", "name", name)
	}

	return nil
}
