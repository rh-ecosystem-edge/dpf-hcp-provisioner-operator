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

// Package ovshugepages reconciles a DaemonSet inside the hosted (guest) cluster that
// reserves hugepages on every node. Kubelet does not report hugepages utilized by
// the system, so the DaemonSet runs one idle "dummy" pod per node to occupy the
// hugepages needed by OVS and keep them from being consumed by other workloads.
// See https://github.com/kubernetes/enhancements/pull/6253.
package ovshugepages

import (
	"context"
	"fmt"
	"strings"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	"github.com/google/go-containerregistry/pkg/v1/remote"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/bfocplookup"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/dpuservicetemplate"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/hostedclient"
)

const (
	pausePayloadImage      = "pod" // resolves to the "ose-pod" image in the release payload
	DefaultHugepagesSize   = "2Mi" // hugepage size selecting the "hugepages-2Mi" resource
	DefaultHugepagesAmount = 250   // pages per node; 250 x 2Mi = 500Mi reserved for OVS
)

const (
	// OVSHugepagesNamespace holds the reservation DaemonSet in the hosted cluster.
	OVSHugepagesNamespace = "openshift-doca-hugepages-holder"
	// DaemonSetName is the name of the reservation DaemonSet in the hosted cluster.
	DaemonSetName = "ovs-hugepages-reservation"
	// containerName is the name of the reservation container.
	containerName = "allocate"
	// reservationPriorityClassName ranks the reservation pods above ordinary
	// workloads so they reliably schedule (and, if needed, preempt) to hold the
	// OVS-reserved hugepages on every node. It is a built-in OpenShift priority
	// class that always exists on the hosted cluster, so referencing it never
	// blocks pod admission.
	reservationPriorityClassName = "system-node-critical"
)

// Manager reconciles the hugepages reservation DaemonSet inside hosted clusters.
type Manager struct {
	mgmtClient    client.Client
	clientManager *hostedclient.ClientManager
	releaseReader dpuservicetemplate.ReleaseImageReader

	// pauseImages caches the resolved pause image per release image ref so we don't
	// re-pull the release payload on every reconcile. No locking needed — reconciles
	// run serially (default concurrency of 1).
	pauseImages map[string]string
}

func NewManager(mgmtClient client.Client, clientManager *hostedclient.ClientManager, releaseReader dpuservicetemplate.ReleaseImageReader) *Manager {
	return &Manager{
		mgmtClient:    mgmtClient,
		clientManager: clientManager,
		releaseReader: releaseReader,
		pauseImages:   make(map[string]string),
	}
}

// ReconcileHugepagesDaemonSet ensures the hugepages reservation namespace and
// DaemonSet exist in the hosted cluster for the given DPFHCPProvisioner. It is
// idempotent and safe to call on every reconcile.
//
// size is the hugepage size (selects the "hugepages-<size>" resource, e.g. "2Mi")
// and amount is the number of pages reserved per node (e.g. 250). The reserved
// quantity is amount x size (e.g. 250 x 2Mi = 500Mi). An empty size falls back to
// DefaultHugepagesSize; an amount of zero disables the reservation. An omitted
// amount is defaulted to DefaultHugepagesAmount by the DPFHCPProvisionerConfig CRD.
func (m *Manager) ReconcileHugepagesDaemonSet(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, size string, amount int32) error {
	if size == "" {
		size = DefaultHugepagesSize
	}
	if amount < 0 {
		amount = DefaultHugepagesAmount
	}

	hcClient, err := m.clientManager.GetHostedClusterClient(ctx, cr.Namespace, cr.Name)
	if err != nil {
		return fmt.Errorf("failed to get hosted cluster client: %w", err)
	}

	// When disabled (amount == 0) no image is needed: ensureDaemonSet just removes
	// any existing reservation.
	if amount == 0 {
		return ensureDaemonSet(ctx, hcClient, "", size, amount)
	}

	if err := ensureNamespace(ctx, hcClient); err != nil {
		return err
	}

	// Resolve the pause image from the hosted cluster's own release payload so the
	// reservation pods run the exact image their nodes already have. On failure we
	// return an error so the reconcile requeues rather than deploying a guessed image.
	pauseImage, err := m.resolvePauseImage(ctx, cr)
	if err != nil {
		return err
	}

	return ensureDaemonSet(ctx, hcClient, pauseImage, size, amount)
}

// resolvePauseImage returns the pause/sandbox image for the reservation pods, resolved
// from the aarch64-specific release payload and cached per release image. DPU nodes are
// aarch64, so resolution fails rather than risking an image for another architecture.
func (m *Manager) resolvePauseImage(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (string, error) {
	log := logf.FromContext(ctx)
	releaseImage := cr.Spec.OCPReleaseImage

	if cached, ok := m.pauseImages[releaseImage]; ok {
		return cached, nil
	}

	keychain, err := common.KeychainFromPullSecret(ctx, m.mgmtClient, cr.Spec.PullSecretRef.Name, cr.Namespace)
	if err != nil {
		return "", fmt.Errorf("getting pull secret keychain: %w", err)
	}

	var resolved string
	if isMultiArchRelease(ctx, releaseImage, keychain) {
		// Multi-arch release: component images listed in the payload are
		// multi-arch manifest lists, so the kubelet on aarch64 DPU nodes
		// pulls the correct architecture automatically.
		resolved, err = m.releaseReader.GetComponentImage(ctx, releaseImage, pausePayloadImage, keychain)
		if err != nil {
			return "", fmt.Errorf("resolving pause image from release %q: %w", releaseImage, err)
		}
		if resolved == "" {
			return "", fmt.Errorf("component %q resolved to an empty image from release %q", pausePayloadImage, releaseImage)
		}
	} else {
		resolved, err = m.tryAarch64Release(ctx, releaseImage, keychain)
		if err != nil {
			return "", fmt.Errorf("resolving aarch64 pause image from release %q: %w", releaseImage, err)
		}
	}

	m.pauseImages[releaseImage] = resolved
	log.V(1).Info("Resolved hugepages reservation pause image",
		"releaseImage", releaseImage, "pauseImage", resolved)
	return resolved, nil
}

// isMultiArchRelease checks whether a release image is a multi-arch manifest
// list. Multi-arch releases contain component images that are themselves
// multi-arch, so the pause image can be resolved directly without constructing
// an architecture-specific tag.
func isMultiArchRelease(ctx context.Context, image string, keychain authn.Keychain) bool {
	if strings.HasSuffix(image, "-multi") {
		return true
	}
	ref, err := name.ParseReference(image)
	if err != nil {
		return false
	}
	desc, err := remote.Head(ref, remote.WithAuthFromKeychain(keychain), remote.WithContext(ctx))
	if err != nil {
		return false
	}
	return desc.MediaType.IsIndex()
}

// tryAarch64Release resolves the pause image from the aarch64-specific release payload.
func (m *Manager) tryAarch64Release(ctx context.Context, releaseImage string, keychain authn.Keychain) (string, error) {
	version, err := bfocplookup.ExtractOCPVersion(ctx, releaseImage, keychain)
	if err != nil {
		return "", fmt.Errorf("extracting OCP version: %w", err)
	}
	registry := releaseImage
	if idx := strings.Index(registry, "@"); idx > 0 {
		registry = registry[:idx]
	} else if idx := strings.LastIndex(registry, ":"); idx > 0 {
		registry = registry[:idx]
	}
	aarch64Release := fmt.Sprintf("%s:%s-aarch64", registry, version)
	resolved, err := m.releaseReader.GetComponentImage(ctx, aarch64Release, pausePayloadImage, keychain)
	if err != nil {
		return "", fmt.Errorf("reading release %q: %w", aarch64Release, err)
	}
	if resolved == "" {
		return "", fmt.Errorf("component %q resolved to an empty image from release %q", pausePayloadImage, aarch64Release)
	}
	return resolved, nil
}

// ensureNamespace creates the reservation namespace in the hosted cluster if missing.
func ensureNamespace(ctx context.Context, hcClient kubernetes.Interface) error {
	ns := &corev1.Namespace{
		ObjectMeta: metav1.ObjectMeta{Name: OVSHugepagesNamespace},
	}
	_, err := hostedclient.CreateOrUpdate(ctx, hcClient.CoreV1().Namespaces(), ns,
		func(ns *corev1.Namespace) error {
			if ns.Labels == nil {
				ns.Labels = map[string]string{}
			}
			for k, v := range labels() {
				ns.Labels[k] = v
			}
			return nil
		})
	if err != nil {
		return fmt.Errorf("failed to ensure namespace %s: %w", OVSHugepagesNamespace, err)
	}
	return nil
}

// ensureDaemonSet reconciles the reservation DaemonSet in the hosted cluster and logs what it
// did. An amount of zero removes any existing DaemonSet so its pods stop reserving pages.
func ensureDaemonSet(ctx context.Context, hcClient kubernetes.Interface, image, size string, amount int32) error {
	log := logf.FromContext(ctx)
	dsClient := hcClient.AppsV1().DaemonSets(OVSHugepagesNamespace)

	if amount == 0 {
		if err := dsClient.Delete(ctx, DaemonSetName, metav1.DeleteOptions{}); err != nil && !apierrors.IsNotFound(err) {
			return fmt.Errorf("failed to delete DaemonSet %s/%s: %w", OVSHugepagesNamespace, DaemonSetName, err)
		}
		log.V(1).Info("Hugepages reservation disabled in hosted cluster",
			"namespace", OVSHugepagesNamespace, "name", DaemonSetName)
		return nil
	}

	desired := buildDaemonSet(image, size, amount)
	result, err := hostedclient.CreateOrUpdate(ctx, dsClient, desired,
		func(ds *appsv1.DaemonSet) error {
			ds.Labels = desired.Labels
			ds.Spec.Template = desired.Spec.Template
			return nil
		})
	if err != nil {
		return fmt.Errorf("failed to ensure DaemonSet %s/%s: %w", OVSHugepagesNamespace, DaemonSetName, err)
	}

	switch result {
	case hostedclient.OperationResultCreated:
		log.V(1).Info("Created hugepages reservation DaemonSet in hosted cluster",
			"namespace", OVSHugepagesNamespace, "name", DaemonSetName,
			"hugepagesSize", size, "hugepagesCount", amount)
	case hostedclient.OperationResultUpdated:
		log.V(1).Info("Updated hugepages reservation DaemonSet in hosted cluster (drift corrected)",
			"namespace", OVSHugepagesNamespace, "name", DaemonSetName,
			"hugepagesSize", size, "hugepagesCount", amount)
	default:
		log.V(1).Info("Hugepages reservation DaemonSet up to date in hosted cluster",
			"namespace", OVSHugepagesNamespace, "name", DaemonSetName)
	}
	return nil
}

// buildDaemonSet builds the reservation DaemonSet. The pod does nothing but idle;
// its resource requests/limits reserve the hugepages on every node it lands on.
// image is the pause image the reservation pods run; size selects the
// "hugepages-<size>" resource and amount is the number of pages; the reserved
// quantity is amount x size.
func buildDaemonSet(image, size string, amount int32) *appsv1.DaemonSet {
	hugepagesResource := corev1.ResourceName("hugepages-" + size)
	hugepagesQty := hugepagesQuantity(size, amount)

	return &appsv1.DaemonSet{
		ObjectMeta: metav1.ObjectMeta{
			Name:      DaemonSetName,
			Namespace: OVSHugepagesNamespace,
			Labels:    labels(),
		},
		Spec: appsv1.DaemonSetSpec{
			Selector: &metav1.LabelSelector{MatchLabels: labels()},
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{
					Labels: labels(),
				},
				Spec: corev1.PodSpec{
					// Rank above ordinary workloads so the reservation reliably holds
					// the hugepages that must not be consumed by other pods.
					PriorityClassName: reservationPriorityClassName,
					// Every node of the hosted cluster is a DPU, so there is no
					// nodeSelector — we want one pod on every node. Tolerate all
					// taints so the reservation lands even on tainted/not-ready nodes.
					Tolerations: []corev1.Toleration{
						{Operator: corev1.TolerationOpExists},
					},
					Containers: []corev1.Container{
						{
							Name:  containerName,
							Image: image,
							Resources: corev1.ResourceRequirements{
								Requests: corev1.ResourceList{
									corev1.ResourceCPU:    resource.MustParse("1m"),
									corev1.ResourceMemory: resource.MustParse("32Mi"),
									hugepagesResource:     hugepagesQty,
								},
								Limits: corev1.ResourceList{
									corev1.ResourceCPU:    resource.MustParse("1m"),
									corev1.ResourceMemory: resource.MustParse("64Mi"),
									hugepagesResource:     hugepagesQty,
								},
							},
						},
					},
				},
			},
		},
	}
}

// hugepagesQuantity returns the total hugepages quantity to reserve: amount pages
// each of the given page size (e.g. size "2Mi", amount 250 -> 500Mi). The result is
// always a multiple of the page size, as Kubernetes requires for hugepage resources.
func hugepagesQuantity(size string, amount int32) resource.Quantity {
	pageSize := resource.MustParse(size)
	return *resource.NewQuantity(pageSize.Value()*int64(amount), resource.BinarySI)
}

// labels returns the common labels applied to the reservation resources.
func labels() map[string]string {
	return map[string]string{
		"app.kubernetes.io/name":       DaemonSetName,
		"app.kubernetes.io/managed-by": "dpf-hcp-provisioner-operator",
	}
}
