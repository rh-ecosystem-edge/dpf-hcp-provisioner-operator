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

	"github.com/go-logr/logr"
	"github.com/google/go-containerregistry/pkg/authn"
	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	configv1 "github.com/openshift/api/config/v1"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/bfocplookup"
)

const (
	dpfOperatorConfigName      = "dpfoperatorconfig"
	dpfOperatorConfigNamespace = "dpf-operator-system"

	clusterVersionName = "version"

	clusterPullSecretName      = "pull-secret"
	clusterPullSecretNamespace = "openshift-config"
	clusterPullSecretKey       = ".dockerconfigjson"

	ovnDPUDaemonSetName      = "ovnkube-node-dpu-host"
	ovnDPUDaemonSetNamespace = "openshift-ovn-kubernetes"
	ovnControllerName        = "ovnkube-controller"

	// AnnotationSourceOVNImage records the x86 OVN image from the DaemonSet that was used
	// to derive the current aarch64 OVN image. When this differs from the DaemonSet's
	// current image, the aarch64 image is re-resolved from the release payload.
	AnnotationSourceOVNImage = "dpfhcpprovisioner.dpu.hcp.io/source-x86-ovn-image"
)

// ServiceNames are the fixed DPUServiceTemplate names managed per DPUCluster namespace.
var ServiceNames = []string{"ovn", "doca-telemetry-service", "hbn"}

// DPUServiceTemplateManager handles DPUServiceTemplate resource creation and deletion.
type DPUServiceTemplateManager struct {
	client             client.Client
	recorder           record.EventRecorder
	ReleaseImageReader ReleaseImageReader
}

// NewDPUServiceTemplateManager creates a new DPUServiceTemplate manager
func NewDPUServiceTemplateManager(c client.Client, recorder record.EventRecorder, reader ReleaseImageReader) *DPUServiceTemplateManager {
	return &DPUServiceTemplateManager{
		client:             c,
		recorder:           recorder,
		ReleaseImageReader: reader,
	}
}

// EnsureTemplates creates or updates the three DPUServiceTemplate resources
// (OVN, DTS, HBN) in the given namespace.
func (m *DPUServiceTemplateManager) EnsureTemplates(ctx context.Context, namespace string) error {
	log := logf.FromContext(ctx).WithValues("namespace", namespace)

	log.Info("Ensuring DPUServiceTemplates")

	dpfVersion, err := m.getDPFVersion(ctx)
	if err != nil {
		return fmt.Errorf("determining DPF version: %w", err)
	}

	dpuServiceTemplateValues, err := DPUServiceTemplateValuesForVersion(dpfVersion)
	if err != nil {
		return fmt.Errorf("unsupported DPF version %q: %w", dpfVersion, err)
	}

	ovnImageRepo, ovnImageTag, sourceImage, err := m.resolveOVNImageIfChanged(ctx, namespace)
	if err != nil {
		return fmt.Errorf("resolving OVN kubernetes image: %w", err)
	}

	if err := m.ensureOVNTemplate(ctx, namespace, dpuServiceTemplateValues, ovnImageRepo, ovnImageTag, sourceImage); err != nil {
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

	for _, svcName := range ServiceNames {
		template := &dpuservicev1alpha1.DPUServiceTemplate{}
		err := m.client.Get(ctx, client.ObjectKey{Name: svcName, Namespace: namespace}, template)
		if err != nil {
			if apierrors.IsNotFound(err) {
				continue
			}
			return fmt.Errorf("getting DPUServiceTemplate %s/%s: %w", namespace, svcName, err)
		}

		log.Info("Deleting DPUServiceTemplate", "name", svcName)
		if err := m.client.Delete(ctx, template); err != nil && !apierrors.IsNotFound(err) {
			return fmt.Errorf("deleting DPUServiceTemplate %s/%s: %w", namespace, svcName, err)
		}
	}

	log.Info("DPUServiceTemplate cleanup complete")
	return nil
}

// getDPFVersion reads the DPF version from DPFOperatorConfig.status.version
func (m *DPUServiceTemplateManager) getDPFVersion(ctx context.Context) (string, error) {
	var config operatorv1alpha1.DPFOperatorConfig
	if err := m.client.Get(ctx, types.NamespacedName{
		Name:      dpfOperatorConfigName,
		Namespace: dpfOperatorConfigNamespace,
	}, &config); err != nil {
		return "", fmt.Errorf("getting DPFOperatorConfig: %w", err)
	}

	if config.Status.Version == nil || *config.Status.Version == "" {
		return "", fmt.Errorf("DPFOperatorConfig.status.version is not set")
	}

	return *config.Status.Version, nil
}

// resolveOVNImageIfChanged reads the x86 OVN image from the DaemonSet and compares it
// against the annotation on the existing OVN template. If they match, the current template
// values are reused without an expensive release payload pull. If they differ (OCP upgrade)
// and the DaemonSet rollout is complete, the aarch64 OVN image is resolved from the release
// payload. If the rollout is still in progress, the existing template values are kept to
// avoid resolving from a release payload that hasn't fully landed yet.
func (m *DPUServiceTemplateManager) resolveOVNImageIfChanged(ctx context.Context, namespace string) (repo, tag, sourceImage string, err error) {
	log := logf.FromContext(ctx)

	ds, err := m.getOVNDaemonSet(ctx)
	if err != nil {
		return "", "", "", fmt.Errorf("reading OVN DaemonSet: %w", err)
	}

	currentImage, err := ovnImageFromDaemonSet(ds)
	if err != nil {
		return "", "", "", err
	}

	existing := &dpuservicev1alpha1.DPUServiceTemplate{}
	err = m.client.Get(ctx, client.ObjectKey{Name: "ovn", Namespace: namespace}, existing)
	if err == nil {
		if existing.Annotations[AnnotationSourceOVNImage] == currentImage {
			log.V(1).Info("OVN DaemonSet image unchanged, skipping release payload resolution")
			return m.ovnImageFromTemplate(existing, currentImage)
		}

		if !daemonSetRolloutComplete(ds) {
			log.Info("OVN DaemonSet image changed but rollout not complete, keeping current template values",
				"currentImage", currentImage,
				"annotatedImage", existing.Annotations[AnnotationSourceOVNImage])
			return m.ovnImageFromTemplate(existing, existing.Annotations[AnnotationSourceOVNImage])
		}
	} else if !apierrors.IsNotFound(err) {
		return "", "", "", fmt.Errorf("getting existing OVN template: %w", err)
	}

	log.Info("Resolving aarch64 OVN image from release payload", "sourceImage", currentImage)
	repo, tag, err = m.resolveARM64OVNImage(ctx)
	if err != nil {
		return "", "", "", err
	}
	return repo, tag, currentImage, nil
}

// ovnImageFromTemplate extracts the OVN image repo and tag from an existing template's values.
func (m *DPUServiceTemplateManager) ovnImageFromTemplate(tmpl *dpuservicev1alpha1.DPUServiceTemplate, sourceImage string) (repo, tag, source string, err error) {
	if tmpl.Spec.HelmChart.Values == nil {
		return "", "", "", fmt.Errorf("existing OVN template has no values")
	}
	var values map[string]any
	if err := json.Unmarshal(tmpl.Spec.HelmChart.Values.Raw, &values); err != nil {
		return "", "", "", fmt.Errorf("parsing existing OVN template values: %w", err)
	}
	dpuManifests, ok := values["dpuManifests"].(map[string]any)
	if !ok {
		return "", "", "", fmt.Errorf("existing OVN template missing dpuManifests")
	}
	img, ok := dpuManifests["image"].(map[string]any)
	if !ok {
		return "", "", "", fmt.Errorf("existing OVN template missing dpuManifests.image")
	}
	r, _ := img["repository"].(string)
	t, _ := img["tag"].(string)
	if r == "" || t == "" {
		return "", "", "", fmt.Errorf("existing OVN template has empty image repository or tag")
	}
	return r, t, sourceImage, nil
}

// getOVNDaemonSet reads the ovnkube-node-dpu-host DaemonSet.
func (m *DPUServiceTemplateManager) getOVNDaemonSet(ctx context.Context) (*appsv1.DaemonSet, error) {
	var ds appsv1.DaemonSet
	if err := m.client.Get(ctx, types.NamespacedName{
		Name:      ovnDPUDaemonSetName,
		Namespace: ovnDPUDaemonSetNamespace,
	}, &ds); err != nil {
		return nil, fmt.Errorf("getting DaemonSet %s/%s: %w", ovnDPUDaemonSetNamespace, ovnDPUDaemonSetName, err)
	}
	return &ds, nil
}

func ovnImageFromDaemonSet(ds *appsv1.DaemonSet) (string, error) {
	for _, c := range ds.Spec.Template.Spec.Containers {
		if c.Name == ovnControllerName {
			return c.Image, nil
		}
	}
	return "", fmt.Errorf("container %q not found in DaemonSet %s/%s", ovnControllerName, ovnDPUDaemonSetNamespace, ovnDPUDaemonSetName)
}

func daemonSetRolloutComplete(ds *appsv1.DaemonSet) bool {
	return ds.Status.ObservedGeneration >= ds.Generation &&
		ds.Status.UpdatedNumberScheduled == ds.Status.DesiredNumberScheduled &&
		ds.Status.NumberAvailable == ds.Status.DesiredNumberScheduled
}

// resolveARM64OVNImage reads the ClusterVersion to determine the OCP release,
// then extracts the aarch64 ovn-kubernetes image from the release payload.
func (m *DPUServiceTemplateManager) resolveARM64OVNImage(ctx context.Context) (repo, tag string, err error) {
	var cv configv1.ClusterVersion
	if err := m.client.Get(ctx, types.NamespacedName{Name: clusterVersionName}, &cv); err != nil {
		return "", "", fmt.Errorf("getting ClusterVersion: %w", err)
	}

	releaseImage := cv.Status.Desired.Image
	if releaseImage == "" {
		return "", "", fmt.Errorf("ClusterVersion status.desired.image is empty")
	}

	version := cv.Status.Desired.Version
	if version == "" {
		return "", "", fmt.Errorf("ClusterVersion status.desired.version is empty")
	}

	registry := releaseImage
	if idx := strings.Index(registry, "@"); idx > 0 {
		registry = registry[:idx]
	} else if idx := strings.LastIndex(registry, ":"); idx > 0 {
		registry = registry[:idx]
	}
	aarch64Ref := fmt.Sprintf("%s:%s-aarch64", registry, version)

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
	secret := &corev1.Secret{}
	if err := m.client.Get(ctx, types.NamespacedName{
		Name:      clusterPullSecretName,
		Namespace: clusterPullSecretNamespace,
	}, secret); err != nil {
		return nil, fmt.Errorf("getting secret %s/%s: %w", clusterPullSecretNamespace, clusterPullSecretName, err)
	}

	dockerConfigJSON, ok := secret.Data[clusterPullSecretKey]
	if !ok {
		return nil, fmt.Errorf("secret %s/%s missing key %q", clusterPullSecretNamespace, clusterPullSecretName, clusterPullSecretKey)
	}

	return bfocplookup.NewKeychainFromDockerConfig(dockerConfigJSON)
}

func (m *DPUServiceTemplateManager) ensureOVNTemplate(ctx context.Context, namespace string, defaults *DPUServiceTemplateValues, ovnImageRepo, ovnImageTag, sourceImage string) error {
	log := logf.FromContext(ctx)

	values := map[string]interface{}{
		"commonManifests": map[string]interface{}{"enabled": true},
		"dpuManifests": map[string]interface{}{
			"image": map[string]interface{}{
				"repository": ovnImageRepo,
				"tag":        ovnImageTag,
			},
			"enabled":    true,
			"cniBinDir":  "/var/lib/cni/bin/",
			"cniConfDir": "/run/multus/cni/net.d",
		},
		"leaseNamespace": "openshift-ovn-kubernetes",
		"gatewayOpts":    "--gateway-interface=br-dpu",
	}

	annotations := map[string]string{
		AnnotationSourceOVNImage: sourceImage,
	}

	return m.ensureTemplate(ctx, namespace, "ovn", "ovn", defaults.OVN.ChartRepoURL, defaults.OVN.ChartName, defaults.OVN.ChartVersion, values, nil, annotations, log)
}

func (m *DPUServiceTemplateManager) ensureDTSTemplate(ctx context.Context, namespace string, defaults *DPUServiceTemplateValues) error {
	log := logf.FromContext(ctx)

	values := map[string]interface{}{
		"configMapData": map[string]interface{}{
			"prometheus": map[string]interface{}{
				"port": 9189,
			},
		},
		"hostVolumePrefix": "/var/lib",
		"imageDTS":         defaults.DTS.ImageDTS,
		"imagePullSecrets": []map[string]interface{}{
			{"name": "dpf-pull-secret"},
		},
	}

	resourceReqs := corev1.ResourceList{
		corev1.ResourceCPU:     resource.MustParse("1"),
		corev1.ResourceMemory:  resource.MustParse("1Gi"),
		corev1.ResourceStorage: resource.MustParse("1Gi"),
	}

	return m.ensureTemplate(ctx, namespace, "doca-telemetry-service", "doca-telemetry-service", defaults.DTS.ChartRepoURL, defaults.DTS.ChartName, defaults.DTS.ChartVersion, values, resourceReqs, nil, log)
}

func (m *DPUServiceTemplateManager) ensureHBNTemplate(ctx context.Context, namespace string, defaults *DPUServiceTemplateValues) error {
	log := logf.FromContext(ctx)

	values := map[string]interface{}{
		"image": map[string]interface{}{
			"repository": defaults.HBN.ImageRepo,
			"tag":        defaults.HBN.ImageTag,
		},
		"imagePullSecrets": []map[string]interface{}{
			{"name": "dpf-pull-secret"},
		},
		"resources": map[string]interface{}{
			"memory":           "6Gi",
			"nvidia.com/bf_sf": 3,
		},
	}

	return m.ensureTemplate(ctx, namespace, "hbn", "hbn", defaults.HBN.ChartRepoURL, defaults.HBN.ChartName, defaults.HBN.ChartVersion, values, nil, nil, log)
}

func (m *DPUServiceTemplateManager) ensureTemplate(
	ctx context.Context,
	namespace string,
	name, deploymentServiceName string,
	repoURL, chartName, chartVersion string,
	values map[string]interface{},
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

	op, err := controllerutil.CreateOrUpdate(ctx, m.client, template, func() error {
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
