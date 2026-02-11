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

package v1alpha1

import (
	"encoding/json"
	"testing"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/client-go/kubernetes/scheme"
)

func TestDPFHCPProvisionerTypes(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecs(t, "DPFHCPProvisioner API Types Suite")
}

var _ = Describe("DPFHCPProvisioner API Types", func() {
	Context("JSON Serialization", func() {
		It("should successfully serialize and deserialize a complete DPFHCPProvisioner", func() {
			original := &DPFHCPProvisioner{
				TypeMeta: metav1.TypeMeta{
					APIVersion: "provisioning.dpu.hcp.io/v1alpha1",
					Kind:       "DPFHCPProvisioner",
				},
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "default",
				},
				Spec: DPFHCPProvisionerSpec{
					DPUClusterRef: DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:                     "clusters.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "ceph-rbd-retain",
					ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
					VirtualIP:                      "192.168.1.100",
					NodeSelector: map[string]string{
						"node-role.kubernetes.io/master": "",
						"topology.kubernetes.io/zone":    "us-east-1a",
					},
				},
				Status: DPFHCPProvisionerStatus{
					Phase: PhaseReady,
					Conditions: []metav1.Condition{
						{
							Type:               "Ready",
							Status:             metav1.ConditionTrue,
							LastTransitionTime: metav1.Now(),
							Reason:             "ProvisionerReady",
							Message:            "All components operational",
						},
					},
					HostedClusterRef: &corev1.ObjectReference{
						APIVersion: "hypershift.openshift.io/v1beta1",
						Kind:       "HostedCluster",
						Name:       "test-provisioner",
						Namespace:  "default",
					},
					KubeConfigSecretRef: &corev1.LocalObjectReference{
						Name: "test-provisioner-kubeconfig",
					},
				},
			}

			// Marshal to JSON
			jsonData, err := json.Marshal(original)
			Expect(err).NotTo(HaveOccurred())
			Expect(jsonData).NotTo(BeEmpty())

			// Unmarshal back
			var deserialized DPFHCPProvisioner
			err = json.Unmarshal(jsonData, &deserialized)
			Expect(err).NotTo(HaveOccurred())

			// Verify critical spec fields
			Expect(deserialized.Spec.BaseDomain).To(Equal(original.Spec.BaseDomain))
			Expect(deserialized.Spec.DPUClusterRef.Name).To(Equal(original.Spec.DPUClusterRef.Name))
			Expect(deserialized.Spec.DPUClusterRef.Namespace).To(Equal(original.Spec.DPUClusterRef.Namespace))
			Expect(deserialized.Spec.OCPReleaseImage).To(Equal(original.Spec.OCPReleaseImage))
			Expect(deserialized.Spec.EtcdStorageClass).To(Equal(original.Spec.EtcdStorageClass))
			Expect(deserialized.Spec.VirtualIP).To(Equal(original.Spec.VirtualIP))
			Expect(deserialized.Spec.ControlPlaneAvailabilityPolicy).To(Equal(original.Spec.ControlPlaneAvailabilityPolicy))
			Expect(deserialized.Spec.NodeSelector).To(Equal(original.Spec.NodeSelector))

			// Verify status fields
			Expect(deserialized.Status.Phase).To(Equal(original.Status.Phase))
			Expect(deserialized.Status.Conditions).To(HaveLen(len(original.Status.Conditions)))
		})
	})

	Context("DeepCopy", func() {
		It("should create an independent copy of DPFHCPProvisioner", func() {
			original := &DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "default",
				},
				Spec: DPFHCPProvisionerSpec{
					DPUClusterRef: DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:       "clusters.example.com",
					OCPReleaseImage:  "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:  corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:    corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass: "ceph-rbd-retain",
				},
				Status: DPFHCPProvisionerStatus{
					Phase: PhaseReady,
					HostedClusterRef: &corev1.ObjectReference{
						Name: "test-provisioner",
					},
					KubeConfigSecretRef: &corev1.LocalObjectReference{
						Name: "test-provisioner-kubeconfig",
					},
				},
			}

			copied := original.DeepCopy()

			// Verify it's a different object
			Expect(copied).NotTo(BeIdenticalTo(original))

			// Verify fields are equal
			Expect(copied.Name).To(Equal(original.Name))
			Expect(copied.Namespace).To(Equal(original.Namespace))
			Expect(copied.Spec.BaseDomain).To(Equal(original.Spec.BaseDomain))
			Expect(copied.Status.Phase).To(Equal(original.Status.Phase))

			// Modify copy and ensure original is unchanged
			copied.Spec.BaseDomain = "modified.example.com"
			Expect(original.Spec.BaseDomain).To(Equal("clusters.example.com"))
			Expect(copied.Spec.BaseDomain).To(Equal("modified.example.com"))

			// Verify pointer fields are deep copied
			Expect(copied.Status.HostedClusterRef).NotTo(BeIdenticalTo(original.Status.HostedClusterRef))
			Expect(copied.Status.KubeConfigSecretRef).NotTo(BeIdenticalTo(original.Status.KubeConfigSecretRef))

			// Modify pointer field in copy
			copied.Status.HostedClusterRef.Name = "modified-cluster"
			Expect(original.Status.HostedClusterRef.Name).To(Equal("test-provisioner"))
		})

		It("should create an independent copy of DPFHCPProvisionerList", func() {
			original := &DPFHCPProvisionerList{
				Items: []DPFHCPProvisioner{
					{
						ObjectMeta: metav1.ObjectMeta{Name: "provisioner-1"},
						Spec:       DPFHCPProvisionerSpec{BaseDomain: "test1.example.com"},
					},
					{
						ObjectMeta: metav1.ObjectMeta{Name: "provisioner-2"},
						Spec:       DPFHCPProvisionerSpec{BaseDomain: "test2.example.com"},
					},
				},
			}

			copied := original.DeepCopy()

			Expect(copied).NotTo(BeIdenticalTo(original))
			Expect(copied.Items).To(HaveLen(len(original.Items)))

			// Modify copy and ensure original is unchanged
			copied.Items[0].Spec.BaseDomain = "modified.example.com"
			Expect(original.Items[0].Spec.BaseDomain).To(Equal("test1.example.com"))
		})
	})

	Context("Scheme Registration", func() {
		It("should register DPFHCPProvisioner types in the scheme", func() {
			s := runtime.NewScheme()

			err := AddToScheme(s)
			Expect(err).NotTo(HaveOccurred())

			// Verify DPFHCPProvisioner is registered
			gvk := GroupVersion.WithKind("DPFHCPProvisioner")

			obj, err := s.New(gvk)
			Expect(err).NotTo(HaveOccurred())
			_, ok := obj.(*DPFHCPProvisioner)
			Expect(ok).To(BeTrue())
		})

		It("should register DPFHCPProvisionerList in the scheme", func() {
			s := runtime.NewScheme()

			err := AddToScheme(s)
			Expect(err).NotTo(HaveOccurred())

			// Verify DPFHCPProvisionerList is registered
			gvkList := GroupVersion.WithKind("DPFHCPProvisionerList")

			objList, err := s.New(gvkList)
			Expect(err).NotTo(HaveOccurred())
			_, ok := objList.(*DPFHCPProvisionerList)
			Expect(ok).To(BeTrue())
		})

		It("should integrate with client-go scheme", func() {
			err := AddToScheme(scheme.Scheme)
			Expect(err).NotTo(HaveOccurred())

			provisioner := &DPFHCPProvisioner{}
			provisioner.SetGroupVersionKind(GroupVersion.WithKind("DPFHCPProvisioner"))

			Expect(provisioner.GetObjectKind().GroupVersionKind().Kind).To(Equal("DPFHCPProvisioner"))
		})
	})

	Context("Phase Enum", func() {
		It("should have correct phase enum values", func() {
			Expect(string(PhasePending)).To(Equal("Pending"))
			Expect(string(PhaseProvisioning)).To(Equal("Provisioning"))
			Expect(string(PhaseReady)).To(Equal("Ready"))
			Expect(string(PhaseFailed)).To(Equal("Failed"))
			Expect(string(PhaseDeleting)).To(Equal("Deleting"))
		})

		It("should allow all valid phases to be set", func() {
			validPhases := []DPFHCPProvisionerPhase{
				PhasePending,
				PhaseProvisioning,
				PhaseReady,
				PhaseFailed,
				PhaseDeleting,
			}

			for _, phase := range validPhases {
				provisioner := &DPFHCPProvisioner{
					Status: DPFHCPProvisionerStatus{
						Phase: phase,
					},
				}
				Expect(provisioner.Status.Phase).To(Equal(phase))
			}
		})
	})

	Context("NodeSelector", func() {
		It("should allow empty/nil NodeSelector (optional field)", func() {
			provisioner := &DPFHCPProvisioner{
				Spec: DPFHCPProvisionerSpec{
					DPUClusterRef:   DPUClusterReference{Name: "test", Namespace: "default"},
					BaseDomain:      "test.example.com",
					OCPReleaseImage: "quay.io/test:4.19.0",
					SSHKeySecretRef: corev1.LocalObjectReference{Name: "ssh"},
					PullSecretRef:   corev1.LocalObjectReference{Name: "pull"},
				},
			}

			Expect(provisioner.Spec.NodeSelector).To(BeNil())
		})

		It("should support master node selector", func() {
			provisioner := &DPFHCPProvisioner{
				Spec: DPFHCPProvisionerSpec{
					NodeSelector: map[string]string{
						"node-role.kubernetes.io/master": "",
					},
				},
			}

			Expect(provisioner.Spec.NodeSelector).To(HaveLen(1))
			Expect(provisioner.Spec.NodeSelector["node-role.kubernetes.io/master"]).To(Equal(""))
		})

		It("should support worker node selector", func() {
			provisioner := &DPFHCPProvisioner{
				Spec: DPFHCPProvisionerSpec{
					NodeSelector: map[string]string{
						"node-role.kubernetes.io/worker": "",
					},
				},
			}

			Expect(provisioner.Spec.NodeSelector).To(HaveLen(1))
			Expect(provisioner.Spec.NodeSelector["node-role.kubernetes.io/worker"]).To(Equal(""))
		})

		It("should support multiple node selectors", func() {
			provisioner := &DPFHCPProvisioner{
				Spec: DPFHCPProvisionerSpec{
					NodeSelector: map[string]string{
						"node-role.kubernetes.io/master": "",
						"topology.kubernetes.io/zone":    "us-east-1a",
						"hardware":                       "gpu",
					},
				},
			}

			Expect(provisioner.Spec.NodeSelector).To(HaveLen(3))
			Expect(provisioner.Spec.NodeSelector["node-role.kubernetes.io/master"]).To(Equal(""))
			Expect(provisioner.Spec.NodeSelector["topology.kubernetes.io/zone"]).To(Equal("us-east-1a"))
			Expect(provisioner.Spec.NodeSelector["hardware"]).To(Equal("gpu"))
		})

		It("should deep copy NodeSelector map", func() {
			original := &DPFHCPProvisioner{
				Spec: DPFHCPProvisionerSpec{
					NodeSelector: map[string]string{
						"node-role.kubernetes.io/master": "",
						"zone":                           "us-east-1",
					},
				},
			}

			copied := original.DeepCopy()

			// Verify map is copied
			Expect(copied.Spec.NodeSelector).To(Equal(original.Spec.NodeSelector))

			// Modify copy and ensure original is unchanged
			copied.Spec.NodeSelector["zone"] = "us-west-2"
			copied.Spec.NodeSelector["new-key"] = "new-value"

			Expect(original.Spec.NodeSelector["zone"]).To(Equal("us-east-1"))
			Expect(original.Spec.NodeSelector).NotTo(HaveKey("new-key"))
			Expect(copied.Spec.NodeSelector["zone"]).To(Equal("us-west-2"))
			Expect(copied.Spec.NodeSelector["new-key"]).To(Equal("new-value"))
		})
	})
})
