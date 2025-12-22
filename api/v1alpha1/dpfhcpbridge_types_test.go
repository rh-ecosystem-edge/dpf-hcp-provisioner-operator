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

func TestDPFHCPBridgeTypes(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecs(t, "DPFHCPBridge API Types Suite")
}

var _ = Describe("DPFHCPBridge API Types", func() {
	Context("JSON Serialization", func() {
		It("should successfully serialize and deserialize a complete DPFHCPBridge", func() {
			original := &DPFHCPBridge{
				TypeMeta: metav1.TypeMeta{
					APIVersion: "provisioning.dpu.hcp.io/v1alpha1",
					Kind:       "DPFHCPBridge",
				},
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge",
					Namespace: "default",
				},
				Spec: DPFHCPBridgeSpec{
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
				Status: DPFHCPBridgeStatus{
					Phase: PhaseReady,
					Conditions: []metav1.Condition{
						{
							Type:               "Ready",
							Status:             metav1.ConditionTrue,
							LastTransitionTime: metav1.Now(),
							Reason:             "BridgeReady",
							Message:            "All components operational",
						},
					},
					HostedClusterRef: &corev1.ObjectReference{
						APIVersion: "hypershift.openshift.io/v1beta1",
						Kind:       "HostedCluster",
						Name:       "test-bridge",
						Namespace:  "default",
					},
					KubeConfigSecretRef: &corev1.LocalObjectReference{
						Name: "test-bridge-kubeconfig",
					},
				},
			}

			// Marshal to JSON
			jsonData, err := json.Marshal(original)
			Expect(err).NotTo(HaveOccurred())
			Expect(jsonData).NotTo(BeEmpty())

			// Unmarshal back
			var deserialized DPFHCPBridge
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
		It("should create an independent copy of DPFHCPBridge", func() {
			original := &DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge",
					Namespace: "default",
				},
				Spec: DPFHCPBridgeSpec{
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
				Status: DPFHCPBridgeStatus{
					Phase: PhaseReady,
					HostedClusterRef: &corev1.ObjectReference{
						Name: "test-bridge",
					},
					KubeConfigSecretRef: &corev1.LocalObjectReference{
						Name: "test-bridge-kubeconfig",
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
			Expect(original.Status.HostedClusterRef.Name).To(Equal("test-bridge"))
		})

		It("should create an independent copy of DPFHCPBridgeList", func() {
			original := &DPFHCPBridgeList{
				Items: []DPFHCPBridge{
					{
						ObjectMeta: metav1.ObjectMeta{Name: "bridge-1"},
						Spec:       DPFHCPBridgeSpec{BaseDomain: "test1.example.com"},
					},
					{
						ObjectMeta: metav1.ObjectMeta{Name: "bridge-2"},
						Spec:       DPFHCPBridgeSpec{BaseDomain: "test2.example.com"},
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
		It("should register DPFHCPBridge types in the scheme", func() {
			s := runtime.NewScheme()

			err := AddToScheme(s)
			Expect(err).NotTo(HaveOccurred())

			// Verify DPFHCPBridge is registered
			gvk := GroupVersion.WithKind("DPFHCPBridge")

			obj, err := s.New(gvk)
			Expect(err).NotTo(HaveOccurred())
			_, ok := obj.(*DPFHCPBridge)
			Expect(ok).To(BeTrue())
		})

		It("should register DPFHCPBridgeList in the scheme", func() {
			s := runtime.NewScheme()

			err := AddToScheme(s)
			Expect(err).NotTo(HaveOccurred())

			// Verify DPFHCPBridgeList is registered
			gvkList := GroupVersion.WithKind("DPFHCPBridgeList")

			objList, err := s.New(gvkList)
			Expect(err).NotTo(HaveOccurred())
			_, ok := objList.(*DPFHCPBridgeList)
			Expect(ok).To(BeTrue())
		})

		It("should integrate with client-go scheme", func() {
			err := AddToScheme(scheme.Scheme)
			Expect(err).NotTo(HaveOccurred())

			bridge := &DPFHCPBridge{}
			bridge.SetGroupVersionKind(GroupVersion.WithKind("DPFHCPBridge"))

			Expect(bridge.GetObjectKind().GroupVersionKind().Kind).To(Equal("DPFHCPBridge"))
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
			validPhases := []DPFHCPBridgePhase{
				PhasePending,
				PhaseProvisioning,
				PhaseReady,
				PhaseFailed,
				PhaseDeleting,
			}

			for _, phase := range validPhases {
				bridge := &DPFHCPBridge{
					Status: DPFHCPBridgeStatus{
						Phase: phase,
					},
				}
				Expect(bridge.Status.Phase).To(Equal(phase))
			}
		})
	})

	Context("DPUClusterReference", func() {
		It("should correctly represent a cross-namespace reference", func() {
			ref := DPUClusterReference{
				Name:      "test-dpu",
				Namespace: "dpu-system",
			}

			Expect(ref.Name).To(Equal("test-dpu"))
			Expect(ref.Namespace).To(Equal("dpu-system"))
		})

		It("should be settable in DPFHCPBridge spec", func() {
			bridge := &DPFHCPBridge{
				Spec: DPFHCPBridgeSpec{
					DPUClusterRef: DPUClusterReference{
						Name:      "cross-ns-dpu",
						Namespace: "another-namespace",
					},
				},
			}

			Expect(bridge.Spec.DPUClusterRef.Name).To(Equal("cross-ns-dpu"))
			Expect(bridge.Spec.DPUClusterRef.Namespace).To(Equal("another-namespace"))
		})
	})

	Context("NodeSelector", func() {
		It("should allow empty/nil NodeSelector (optional field)", func() {
			bridge := &DPFHCPBridge{
				Spec: DPFHCPBridgeSpec{
					DPUClusterRef:   DPUClusterReference{Name: "test", Namespace: "default"},
					BaseDomain:      "test.example.com",
					OCPReleaseImage: "quay.io/test:4.19.0",
					SSHKeySecretRef: corev1.LocalObjectReference{Name: "ssh"},
					PullSecretRef:   corev1.LocalObjectReference{Name: "pull"},
				},
			}

			Expect(bridge.Spec.NodeSelector).To(BeNil())
		})

		It("should support master node selector", func() {
			bridge := &DPFHCPBridge{
				Spec: DPFHCPBridgeSpec{
					NodeSelector: map[string]string{
						"node-role.kubernetes.io/master": "",
					},
				},
			}

			Expect(bridge.Spec.NodeSelector).To(HaveLen(1))
			Expect(bridge.Spec.NodeSelector["node-role.kubernetes.io/master"]).To(Equal(""))
		})

		It("should support worker node selector", func() {
			bridge := &DPFHCPBridge{
				Spec: DPFHCPBridgeSpec{
					NodeSelector: map[string]string{
						"node-role.kubernetes.io/worker": "",
					},
				},
			}

			Expect(bridge.Spec.NodeSelector).To(HaveLen(1))
			Expect(bridge.Spec.NodeSelector["node-role.kubernetes.io/worker"]).To(Equal(""))
		})

		It("should support multiple node selectors", func() {
			bridge := &DPFHCPBridge{
				Spec: DPFHCPBridgeSpec{
					NodeSelector: map[string]string{
						"node-role.kubernetes.io/master": "",
						"topology.kubernetes.io/zone":    "us-east-1a",
						"hardware":                       "gpu",
					},
				},
			}

			Expect(bridge.Spec.NodeSelector).To(HaveLen(3))
			Expect(bridge.Spec.NodeSelector["node-role.kubernetes.io/master"]).To(Equal(""))
			Expect(bridge.Spec.NodeSelector["topology.kubernetes.io/zone"]).To(Equal("us-east-1a"))
			Expect(bridge.Spec.NodeSelector["hardware"]).To(Equal("gpu"))
		})

		It("should deep copy NodeSelector map", func() {
			original := &DPFHCPBridge{
				Spec: DPFHCPBridgeSpec{
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
