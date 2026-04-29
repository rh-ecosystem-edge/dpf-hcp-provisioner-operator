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

package dpuservicetemplate_test

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/google/go-containerregistry/pkg/authn"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	configv1 "github.com/openshift/api/config/v1"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/dpuservicetemplate"
)

type fakeReleaseImageReader struct {
	image     string
	err       error
	callCount int
}

func (f *fakeReleaseImageReader) GetComponentImage(_ context.Context, _ string, _ string, _ authn.Keychain) (string, error) {
	f.callCount++
	return f.image, f.err
}

func newDPFOperatorConfig(version string) *operatorv1alpha1.DPFOperatorConfig {
	return &operatorv1alpha1.DPFOperatorConfig{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "dpfoperatorconfig",
			Namespace: "dpf-operator-system",
		},
		Status: operatorv1alpha1.DPFOperatorConfigStatus{
			Version: &version,
		},
	}
}

func newClusterVersion(image string) *configv1.ClusterVersion {
	return &configv1.ClusterVersion{
		ObjectMeta: metav1.ObjectMeta{
			Name: "version",
		},
		Spec: configv1.ClusterVersionSpec{
			ClusterID: "test-cluster",
		},
		Status: configv1.ClusterVersionStatus{
			Desired: configv1.Release{
				Image: image,
			},
		},
	}
}

func newPullSecret() *corev1.Secret {
	return &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "pull-secret",
			Namespace: "openshift-config",
		},
		Data: map[string][]byte{
			".dockerconfigjson": []byte(`{"auths":{}}`),
		},
		Type: corev1.SecretTypeDockerConfigJson,
	}
}

func newOVNDaemonSet(image string) *appsv1.DaemonSet {
	return &appsv1.DaemonSet{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "ovnkube-node-dpu-host",
			Namespace: "openshift-ovn-kubernetes",
		},
		Spec: appsv1.DaemonSetSpec{
			Selector: &metav1.LabelSelector{
				MatchLabels: map[string]string{"app": "ovnkube"},
			},
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{Labels: map[string]string{"app": "ovnkube"}},
				Spec: corev1.PodSpec{
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
}

func newTestScheme() *runtime.Scheme {
	scheme := runtime.NewScheme()
	Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
	Expect(dpuservicev1alpha1.AddToScheme(scheme)).To(Succeed())
	Expect(operatorv1alpha1.AddToScheme(scheme)).To(Succeed())
	Expect(configv1.Install(scheme)).To(Succeed())
	Expect(appsv1.AddToScheme(scheme)).To(Succeed())
	Expect(corev1.AddToScheme(scheme)).To(Succeed())
	return scheme
}

// allPrereqs returns the standard set of objects needed for EnsureTemplates.
func allPrereqs(ovnImage string) []client.Object {
	return []client.Object{
		newDPFOperatorConfig("26.4.0-f314aa17"),
		newClusterVersion("quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi"),
		newPullSecret(),
		newOVNDaemonSet(ovnImage),
	}
}

var _ = Describe("DPUServiceTemplate Manager", func() {
	var (
		ctx        context.Context
		fakeClient client.Client
		recorder   *record.FakeRecorder
		manager    *dpuservicetemplate.DPUServiceTemplateManager
		scheme     *runtime.Scheme
	)

	const (
		targetNamespace = "dpf-operator-system"
		x86OVNImage     = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:aaa111"
		arm64OVNImage   = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:bbb222"
	)

	BeforeEach(func() {
		ctx = context.TODO()
		scheme = newTestScheme()
		recorder = record.NewFakeRecorder(100)
	})

	Describe("EnsureTemplates", func() {
		Context("when all prerequisites are available", func() {
			It("should create all three DPUServiceTemplates with the source annotation", func() {
				prereqs := allPrereqs(x86OVNImage)

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(prereqs...).
					WithStatusSubresource(prereqs[0]).
					Build()

				reader := &fakeReleaseImageReader{image: arm64OVNImage}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				err := manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).NotTo(HaveOccurred())

				// Verify OVN template
				ovnTemplate := &dpuservicev1alpha1.DPUServiceTemplate{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "ovn",
					Namespace: targetNamespace,
				}, ovnTemplate)
				Expect(err).NotTo(HaveOccurred())
				Expect(ovnTemplate.Spec.DeploymentServiceName).To(Equal("ovn"))
				Expect(ovnTemplate.Spec.HelmChart.Source.Chart).To(Equal("ovn-kubernetes-chart"))

				// Verify the source annotation records the x86 DaemonSet image
				Expect(ovnTemplate.Annotations).To(HaveKeyWithValue(
					dpuservicetemplate.AnnotationSourceOVNImage, x86OVNImage,
				))

				// Verify OVN values contain the aarch64 image
				var ovnValues map[string]any
				Expect(json.Unmarshal(ovnTemplate.Spec.HelmChart.Values.Raw, &ovnValues)).To(Succeed())
				dpuManifests := ovnValues["dpuManifests"].(map[string]any)
				image := dpuManifests["image"].(map[string]any)
				Expect(image["repository"]).To(Equal("quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256"))
				Expect(image["tag"]).To(Equal("bbb222"))

				// Verify DTS template
				dtsTemplate := &dpuservicev1alpha1.DPUServiceTemplate{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "doca-telemetry-service",
					Namespace: targetNamespace,
				}, dtsTemplate)
				Expect(err).NotTo(HaveOccurred())
				Expect(dtsTemplate.Spec.DeploymentServiceName).To(Equal("doca-telemetry-service"))
				Expect(dtsTemplate.Spec.HelmChart.Source.Chart).To(Equal("doca-telemetry"))
				Expect(dtsTemplate.Spec.ResourceRequirements).NotTo(BeEmpty())

				// Verify HBN template
				hbnTemplate := &dpuservicev1alpha1.DPUServiceTemplate{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "hbn",
					Namespace: targetNamespace,
				}, hbnTemplate)
				Expect(err).NotTo(HaveOccurred())
				Expect(hbnTemplate.Spec.DeploymentServiceName).To(Equal("hbn"))
				Expect(hbnTemplate.Spec.HelmChart.Source.Chart).To(Equal("doca-hbn"))

				Expect(reader.callCount).To(Equal(1))
			})
		})

		Context("when OVN DaemonSet image has not changed", func() {
			It("should skip the release payload pull", func() {
				prereqs := allPrereqs(x86OVNImage)

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(prereqs...).
					WithStatusSubresource(prereqs[0]).
					Build()

				reader := &fakeReleaseImageReader{image: arm64OVNImage}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				// First call resolves from release payload
				err := manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).NotTo(HaveOccurred())
				Expect(reader.callCount).To(Equal(1))

				// Second call should skip the pull (annotation matches DaemonSet image)
				err = manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).NotTo(HaveOccurred())
				Expect(reader.callCount).To(Equal(1))
			})
		})

		Context("when OVN DaemonSet image changed but rollout is not complete", func() {
			It("should keep current template values and not pull release payload", func() {
				prereqs := allPrereqs(x86OVNImage)

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(prereqs...).
					WithStatusSubresource(prereqs[0]).
					Build()

				reader := &fakeReleaseImageReader{image: arm64OVNImage}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				// First call creates templates
				err := manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).NotTo(HaveOccurred())
				Expect(reader.callCount).To(Equal(1))

				// Simulate an OCP upgrade: DaemonSet spec changes to new image,
				// but rollout is not complete (UpdatedNumberScheduled < DesiredNumberScheduled)
				newX86Image := "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:ccc333"
				ds := newOVNDaemonSet(newX86Image)
				ds.Generation = 2
				ds.Status.ObservedGeneration = 2
				ds.Status.DesiredNumberScheduled = 3
				ds.Status.UpdatedNumberScheduled = 1
				ds.Status.NumberAvailable = 2
				err = fakeClient.Delete(ctx, newOVNDaemonSet(x86OVNImage))
				Expect(err).NotTo(HaveOccurred())
				err = fakeClient.Create(ctx, ds)
				Expect(err).NotTo(HaveOccurred())

				// Second call should detect image change but skip pull due to incomplete rollout
				err = manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).NotTo(HaveOccurred())
				Expect(reader.callCount).To(Equal(1))

				// Template should still have the original aarch64 image and original annotation
				ovnTemplate := &dpuservicev1alpha1.DPUServiceTemplate{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "ovn",
					Namespace: targetNamespace,
				}, ovnTemplate)
				Expect(err).NotTo(HaveOccurred())
				Expect(ovnTemplate.Annotations).To(HaveKeyWithValue(
					dpuservicetemplate.AnnotationSourceOVNImage, x86OVNImage,
				))
			})
		})

		Context("when OVN DaemonSet image changed and rollout is complete", func() {
			It("should resolve the new aarch64 image from release payload", func() {
				prereqs := allPrereqs(x86OVNImage)

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(prereqs...).
					WithStatusSubresource(prereqs[0]).
					Build()

				newArm64Image := "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:ddd444"
				reader := &fakeReleaseImageReader{image: arm64OVNImage}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				// First call creates templates
				err := manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).NotTo(HaveOccurred())
				Expect(reader.callCount).To(Equal(1))

				// Simulate completed OCP upgrade: DaemonSet has new image and rollout is done
				newX86Image := "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:ccc333"
				ds := newOVNDaemonSet(newX86Image)
				ds.Generation = 2
				ds.Status.ObservedGeneration = 2
				ds.Status.DesiredNumberScheduled = 3
				ds.Status.UpdatedNumberScheduled = 3
				ds.Status.NumberAvailable = 3
				err = fakeClient.Delete(ctx, newOVNDaemonSet(x86OVNImage))
				Expect(err).NotTo(HaveOccurred())
				err = fakeClient.Create(ctx, ds)
				Expect(err).NotTo(HaveOccurred())

				// Update reader to return new arm64 image
				reader.image = newArm64Image

				// Second call should resolve the new aarch64 image
				err = manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).NotTo(HaveOccurred())
				Expect(reader.callCount).To(Equal(2))

				// Template should have the new annotation
				ovnTemplate := &dpuservicev1alpha1.DPUServiceTemplate{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "ovn",
					Namespace: targetNamespace,
				}, ovnTemplate)
				Expect(err).NotTo(HaveOccurred())
				Expect(ovnTemplate.Annotations).To(HaveKeyWithValue(
					dpuservicetemplate.AnnotationSourceOVNImage, newX86Image,
				))

				// Verify the new aarch64 image is in the template values
				var ovnValues map[string]any
				Expect(json.Unmarshal(ovnTemplate.Spec.HelmChart.Values.Raw, &ovnValues)).To(Succeed())
				dpuManifests := ovnValues["dpuManifests"].(map[string]any)
				image := dpuManifests["image"].(map[string]any)
				Expect(image["repository"]).To(Equal("quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256"))
				Expect(image["tag"]).To(Equal("ddd444"))
			})
		})

		Context("when DPFOperatorConfig is missing", func() {
			It("should return error", func() {
				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(
						newClusterVersion("quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi"),
						newPullSecret(),
						newOVNDaemonSet(x86OVNImage),
					).
					Build()

				reader := &fakeReleaseImageReader{image: arm64OVNImage}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				err := manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).To(HaveOccurred())
			})
		})

		Context("when DPF version is unsupported", func() {
			It("should return error", func() {
				dpfConfig := newDPFOperatorConfig("99.0.0-unknown")

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(
						dpfConfig,
						newClusterVersion("quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi"),
						newPullSecret(),
						newOVNDaemonSet(x86OVNImage),
					).
					WithStatusSubresource(dpfConfig).
					Build()

				reader := &fakeReleaseImageReader{image: arm64OVNImage}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				err := manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).To(HaveOccurred())
			})
		})

		Context("when OVN DaemonSet is missing", func() {
			It("should return error", func() {
				dpfConfig := newDPFOperatorConfig("26.4.0-f314aa17")

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(
						dpfConfig,
						newClusterVersion("quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi"),
						newPullSecret(),
					).
					WithStatusSubresource(dpfConfig).
					Build()

				reader := &fakeReleaseImageReader{image: arm64OVNImage}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				err := manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).To(HaveOccurred())
			})
		})

		Context("when ReleaseImageReader returns an error", func() {
			It("should return error", func() {
				prereqs := allPrereqs(x86OVNImage)

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(prereqs...).
					WithStatusSubresource(prereqs[0]).
					Build()

				reader := &fakeReleaseImageReader{
					err: fmt.Errorf("unable to pull release image"),
				}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				err := manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("resolving OVN kubernetes image"))
			})
		})

		Context("when a DPUServiceTemplate already exists with different spec", func() {
			It("should update it to the desired state", func() {
				existingTemplate := &dpuservicev1alpha1.DPUServiceTemplate{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "ovn",
						Namespace: targetNamespace,
					},
					Spec: dpuservicev1alpha1.DPUServiceTemplateSpec{
						DeploymentServiceName: "ovn",
						HelmChart: dpuservicev1alpha1.HelmChart{
							Source: dpuservicev1alpha1.ApplicationSource{
								RepoURL: "oci://example.com",
								Version: "1.0.0",
							},
						},
					},
				}

				prereqs := allPrereqs(x86OVNImage)
				objects := append(prereqs, existingTemplate)

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(objects...).
					WithStatusSubresource(prereqs[0]).
					Build()

				reader := &fakeReleaseImageReader{image: arm64OVNImage}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				err := manager.EnsureTemplates(ctx, targetNamespace)
				Expect(err).NotTo(HaveOccurred())

				ovnTemplate := &dpuservicev1alpha1.DPUServiceTemplate{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "ovn",
					Namespace: targetNamespace,
				}, ovnTemplate)
				Expect(err).NotTo(HaveOccurred())
				Expect(ovnTemplate.Spec.HelmChart.Source.RepoURL).To(Equal("oci://ghcr.io/mellanox/charts"))
			})
		})
	})

	Describe("DeleteTemplates", func() {
		Context("when all three templates exist", func() {
			It("should delete all of them", func() {
				templates := []client.Object{
					&dpuservicev1alpha1.DPUServiceTemplate{
						ObjectMeta: metav1.ObjectMeta{
							Name:      "ovn",
							Namespace: targetNamespace,
						},
						Spec: dpuservicev1alpha1.DPUServiceTemplateSpec{
							DeploymentServiceName: "ovn",
							HelmChart: dpuservicev1alpha1.HelmChart{
								Source: dpuservicev1alpha1.ApplicationSource{RepoURL: "oci://x", Version: "1"},
							},
						},
					},
					&dpuservicev1alpha1.DPUServiceTemplate{
						ObjectMeta: metav1.ObjectMeta{
							Name:      "doca-telemetry-service",
							Namespace: targetNamespace,
						},
						Spec: dpuservicev1alpha1.DPUServiceTemplateSpec{
							DeploymentServiceName: "doca-telemetry-service",
							HelmChart: dpuservicev1alpha1.HelmChart{
								Source: dpuservicev1alpha1.ApplicationSource{RepoURL: "https://x", Version: "1"},
							},
						},
					},
					&dpuservicev1alpha1.DPUServiceTemplate{
						ObjectMeta: metav1.ObjectMeta{
							Name:      "hbn",
							Namespace: targetNamespace,
						},
						Spec: dpuservicev1alpha1.DPUServiceTemplateSpec{
							DeploymentServiceName: "hbn",
							HelmChart: dpuservicev1alpha1.HelmChart{
								Source: dpuservicev1alpha1.ApplicationSource{RepoURL: "https://x", Version: "1"},
							},
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(templates...).
					Build()

				reader := &fakeReleaseImageReader{image: arm64OVNImage}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				err := manager.DeleteTemplates(ctx, targetNamespace)
				Expect(err).NotTo(HaveOccurred())

				for _, name := range dpuservicetemplate.ServiceNames {
					template := &dpuservicev1alpha1.DPUServiceTemplate{}
					err := fakeClient.Get(ctx, types.NamespacedName{
						Name:      name,
						Namespace: targetNamespace,
					}, template)
					Expect(apierrors.IsNotFound(err)).To(BeTrue(), "expected template %s to be deleted", name)
				}
			})
		})

		Context("when no templates exist", func() {
			It("should succeed immediately (idempotent)", func() {
				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					Build()

				reader := &fakeReleaseImageReader{image: arm64OVNImage}
				manager = dpuservicetemplate.NewDPUServiceTemplateManager(fakeClient, recorder, reader)

				err := manager.DeleteTemplates(ctx, targetNamespace)
				Expect(err).NotTo(HaveOccurred())
			})
		})
	})

	Describe("GetDefaultsForVersion", func() {
		It("should return defaults for supported version", func() {
			defaults, err := dpuservicetemplate.DPUServiceTemplateValuesForVersion("26.4.0-f314aa17")
			Expect(err).NotTo(HaveOccurred())
			Expect(defaults.OVN.ChartVersion).To(Equal("v26.4.0-ocpbeta"))
			Expect(defaults.DTS.ChartVersion).To(Equal("1.22.1"))
			Expect(defaults.HBN.ChartVersion).To(Equal("1.0.3"))
		})

		It("should return error for unsupported version", func() {
			_, err := dpuservicetemplate.DPUServiceTemplateValuesForVersion("99.0.0")
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("unsupported DPF version"))
		})

		It("should return error for empty version", func() {
			_, err := dpuservicetemplate.DPUServiceTemplateValuesForVersion("")
			Expect(err).To(HaveOccurred())
		})
	})
})
