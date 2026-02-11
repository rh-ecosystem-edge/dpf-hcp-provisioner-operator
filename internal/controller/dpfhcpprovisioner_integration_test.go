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

package controller

import (
	"context"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

var _ = Describe("DPFHCPProvisioner Integration Tests with envtest", func() {
	const (
		timeout  = time.Second * 30
		interval = time.Second * 1
	)

	ctx := context.Background()

	AfterEach(func() {
		// Clean up all resources
		provisionerList := &provisioningv1alpha1.DPFHCPProvisionerList{}
		_ = k8sClient.List(ctx, provisionerList)
		for _, provisioner := range provisionerList.Items {
			_ = k8sClient.Delete(ctx, &provisioner)
		}

		// Wait for all deletions to complete to prevent race conditions with next test
		Eventually(func() int {
			provisionerList := &provisioningv1alpha1.DPFHCPProvisionerList{}
			_ = k8sClient.List(ctx, provisionerList)
			return len(provisionerList.Items)
		}, timeout, interval).Should(Equal(0))
	})

	Context("Complete CR Lifecycle", func() {
		It("should successfully create, read, update, and delete a DPFHCPProvisioner", func() {
			provisionerName := "lifecycle-test"

			// CREATE
			By("Creating a new DPFHCPProvisioner")
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      provisionerName,
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:       "lifecycle.example.com",
					OCPReleaseImage:  "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:  corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:    corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass: "test-storage-class",
					VirtualIP:        "192.168.1.100", // Required for HighlyAvailable default
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred())

			// READ
			By("Reading the created DPFHCPProvisioner")
			created := &provisioningv1alpha1.DPFHCPProvisioner{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())
			Expect(created.Spec.BaseDomain).To(Equal("lifecycle.example.com"))
			Expect(created.Spec.DPUClusterRef.Name).To(Equal("test-dpu"))
			Expect(created.Spec.DPUClusterRef.Namespace).To(Equal("dpu-system"))

			// Verify defaults were applied
			Expect(created.Spec.ControlPlaneAvailabilityPolicy).To(Equal(hyperv1.HighlyAvailable))

			// UPDATE
			By("Updating the DPFHCPProvisioner spec")
			// Wrap in Eventually to handle race with controller
			var initialGeneration int64
			Eventually(func() error {
				toUpdate := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: "default"}, toUpdate); err != nil {
					return err
				}
				initialGeneration = toUpdate.Generation
				// Update only mutable fields
				toUpdate.Spec.OCPReleaseImage = "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.6-multi"
				return k8sClient.Update(ctx, toUpdate)
			}, time.Second*5, time.Millisecond*100).Should(Succeed())

			// Verify update
			updated := &provisioningv1alpha1.DPFHCPProvisioner{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: "default"}, updated)
			Expect(err).NotTo(HaveOccurred())
			Expect(updated.Spec.OCPReleaseImage).To(Equal("quay.io/openshift-release-dev/ocp-release:4.19.0-ec.6-multi"))
			Expect(updated.Generation).To(BeNumerically(">", initialGeneration))

			// DELETE
			By("Deleting the DPFHCPProvisioner")
			err = k8sClient.Delete(ctx, updated)
			Expect(err).NotTo(HaveOccurred())

			// Verify deletion
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: "default"}, &provisioningv1alpha1.DPFHCPProvisioner{})
				return errors.IsNotFound(err)
			}, time.Second*5, time.Millisecond*500).Should(BeTrue())
		})

		It("should handle status updates independently of spec", func() {
			provisionerName := "status-update-test"

			// Create resource
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      provisionerName,
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:                     "status.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred())

			// Get the created resource
			created := &provisioningv1alpha1.DPFHCPProvisioner{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())

			initialGeneration := created.Generation

			// Update status with retry to avoid race with controller
			By("Updating status fields")
			Eventually(func() error {
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: "default"}, provisioner); err != nil {
					return err
				}
				provisioner.Status.Phase = provisioningv1alpha1.PhaseProvisioning
				return k8sClient.Status().Update(ctx, provisioner)
			}, time.Second*5, time.Millisecond*100).Should(Succeed())

			// Verify status was updated but generation didn't change
			updated := &provisioningv1alpha1.DPFHCPProvisioner{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: "default"}, updated)
			Expect(err).NotTo(HaveOccurred())

			Expect(updated.Status.Phase).To(Equal(provisioningv1alpha1.PhaseProvisioning))
			Expect(updated.Generation).To(Equal(initialGeneration), "Status update should not increment generation")
		})

		It("should support full status population", func() {
			provisionerName := "full-status-test"

			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      provisionerName,
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:                     "full-status.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred())

			created := &provisioningv1alpha1.DPFHCPProvisioner{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())

			// Populate all status fields
			By("Setting all status fields")
			// Wrap in Eventually to handle race with controller
			Eventually(func() error {
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: "default"}, provisioner); err != nil {
					return err
				}
				provisioner.Status = provisioningv1alpha1.DPFHCPProvisionerStatus{
					Phase: provisioningv1alpha1.PhaseReady,
					Conditions: []metav1.Condition{
						{
							Type:               "Ready",
							Status:             metav1.ConditionTrue,
							Reason:             "ProvisionerReady",
							Message:            "All components operational",
							LastTransitionTime: metav1.Now(),
						},
					},
					HostedClusterRef: &corev1.ObjectReference{
						APIVersion: "hypershift.openshift.io/v1beta1",
						Kind:       "HostedCluster",
						Name:       provisionerName,
						Namespace:  "default",
					},
					KubeConfigSecretRef: &corev1.LocalObjectReference{
						Name: provisionerName + "-kubeconfig",
					},
				}
				return k8sClient.Status().Update(ctx, provisioner)
			}, time.Second*5, time.Millisecond*100).Should(Succeed())

			// Verify all status fields
			retrieved := &provisioningv1alpha1.DPFHCPProvisioner{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: "default"}, retrieved)
			Expect(err).NotTo(HaveOccurred())

			Expect(retrieved.Status.Phase).To(Equal(provisioningv1alpha1.PhaseReady))
			Expect(retrieved.Status.Conditions).To(HaveLen(1))
			Expect(retrieved.Status.HostedClusterRef).NotTo(BeNil())
			Expect(retrieved.Status.KubeConfigSecretRef).NotTo(BeNil())
		})
	})

	Context("List Operations", func() {
		It("should list multiple DPFHCPProvisioner resources", func() {
			// Create multiple provisioners
			provisionerNames := []string{"provisioner-1", "provisioner-2", "provisioner-3"}

			for _, name := range provisionerNames {
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      name,
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu-" + name,
							Namespace: "dpu-system",
						},
						BaseDomain:                     name + ".example.com",
						OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
						SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
						PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
						EtcdStorageClass:               "test-storage-class",
						ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
					},
				}

				err := k8sClient.Create(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred())
			}

			// List all provisioners
			provisionerList := &provisioningv1alpha1.DPFHCPProvisionerList{}
			err := k8sClient.List(ctx, provisionerList)
			Expect(err).NotTo(HaveOccurred())
			Expect(provisionerList.Items).To(HaveLen(len(provisionerNames)))

			// Verify each provisioner exists
			foundNames := make(map[string]bool)
			for _, provisioner := range provisionerList.Items {
				foundNames[provisioner.Name] = true
			}

			for _, name := range provisionerNames {
				Expect(foundNames[name]).To(BeTrue(), "Provisioner %q should be in the list", name)
			}
		})
	})

	Context("Resource Metadata", func() {
		It("should preserve labels and annotations", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-metadata",
					Namespace: "default",
					Labels: map[string]string{
						"environment": "test",
						"team":        "platform",
					},
					Annotations: map[string]string{
						"description": "Test provisioner for metadata preservation",
					},
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:                     "metadata.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred())

			created := &provisioningv1alpha1.DPFHCPProvisioner{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: "test-metadata", Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())

			// Verify labels and annotations
			Expect(created.Labels).To(HaveKeyWithValue("environment", "test"))
			Expect(created.Labels).To(HaveKeyWithValue("team", "platform"))
			Expect(created.Annotations).To(HaveKeyWithValue("description", "Test provisioner for metadata preservation"))

			// Clean up
			_ = k8sClient.Delete(ctx, provisioner)
		})
	})
})
