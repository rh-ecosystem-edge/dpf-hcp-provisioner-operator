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

package e2e

import (
	"context"
	"encoding/json"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	configv1 "github.com/openshift/api/config/v1"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
)

// createClusterVersionStub creates a ClusterVersion CR with the given version and release image.
func createClusterVersionStub(version, releaseImage string) {
	ctx := context.Background()
	cv := &configv1.ClusterVersion{
		ObjectMeta: metav1.ObjectMeta{
			Name: "version",
		},
		Spec: configv1.ClusterVersionSpec{
			ClusterID: "e2e-test-cluster",
		},
	}
	err := k8sClient.Create(ctx, cv)
	if err != nil && !apierrors.IsAlreadyExists(err) {
		ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create ClusterVersion stub")
	}

	// Patch status subresource
	err = k8sClient.Get(ctx, types.NamespacedName{Name: "version"}, cv)
	ExpectWithOffset(1, err).NotTo(HaveOccurred())
	cv.Status.Desired = configv1.Release{
		Version: version,
		Image:   releaseImage,
	}
	err = k8sClient.Status().Update(ctx, cv)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to update ClusterVersion status")
}

// createOVNDaemonSetStub creates the ovnkube-node-dpu-host DaemonSet with a
// rollout-complete status.
func createOVNDaemonSetStub(image string) {
	ctx := context.Background()

	By("creating openshift-ovn-kubernetes namespace")
	createNamespace("openshift-ovn-kubernetes")

	// Use a nodeSelector that matches nothing so the real DaemonSet controller
	// sets DesiredNumberScheduled=0, making 0==0==0 pass the rollout check.
	ds := &appsv1.DaemonSet{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "ovnkube-node-dpu-host",
			Namespace: "openshift-ovn-kubernetes",
		},
		Spec: appsv1.DaemonSetSpec{
			Selector: &metav1.LabelSelector{
				MatchLabels: map[string]string{"app": "ovnkube-node-dpu-host"},
			},
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{
					Labels: map[string]string{"app": "ovnkube-node-dpu-host"},
				},
				Spec: corev1.PodSpec{
					NodeSelector: map[string]string{"non-existent-label": "true"},
					Containers: []corev1.Container{
						{
							Name:  "ovnkube-controller",
							Image: image,
						},
					},
				},
			},
		},
	}
	err := k8sClient.Create(ctx, ds)
	if err != nil && !apierrors.IsAlreadyExists(err) {
		ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create OVN DaemonSet stub")
	}
}

// createPullSecretStub creates a dummy pull secret in openshift-config.
func createPullSecretStub() {
	ctx := context.Background()

	By("creating openshift-config namespace")
	createNamespace("openshift-config")

	dockerConfig := map[string]any{
		"auths": map[string]any{
			"quay.io": map[string]any{
				"auth": "ZTJlLXRlc3Q6cGFzcw==",
			},
		},
	}
	dockerConfigJSON, err := json.Marshal(dockerConfig)
	ExpectWithOffset(1, err).NotTo(HaveOccurred())

	secret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "pull-secret",
			Namespace: "openshift-config",
		},
		Type: corev1.SecretTypeDockerConfigJson,
		Data: map[string][]byte{
			".dockerconfigjson": dockerConfigJSON,
		},
	}
	err = k8sClient.Create(ctx, secret)
	if err != nil && !apierrors.IsAlreadyExists(err) {
		ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create pull secret stub")
	}
}

// createDummySecrets creates placeholder secrets for use on Kind where real secrets don't exist.
func createDummySecrets(ns string, names ...string) {
	ctx := context.Background()
	for _, name := range names {
		secret := &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      name,
				Namespace: ns,
			},
			Data: map[string][]byte{"dummy": []byte("data")},
		}
		err := k8sClient.Create(ctx, secret)
		if err != nil && !apierrors.IsAlreadyExists(err) {
			ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create secret %s", name)
		}
	}
}
