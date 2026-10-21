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
	"fmt"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// fakeTagLister implements TagLister for testing
type fakeTagLister struct {
	tags []string
	err  error
}

func (f *fakeTagLister) ListTags(ctx context.Context, repo name.Repository, keychain authn.Keychain) ([]string, error) {
	return f.tags, f.err
}

// fakeLabelFetcher implements LabelFetcher for testing
type fakeLabelFetcher struct {
	labels map[string]string
	err    error
}

func (f *fakeLabelFetcher) FetchLabels(ctx context.Context, image string, keychain authn.Keychain) (map[string]string, error) {
	return f.labels, f.err
}

const testRepository = "quay.io/test-org/bluefield-os"

// sampleRegistryTags mirrors a real BlueField OCP layer registry with both bare
// OCP tags and DOCA-suffixed tags.
var sampleRegistryTags = []string{
	"4.22.0", "4.22.0-doca3.4.2",
	"4.22.1", "4.22.1-doca3.4.2",
	"4.22.10", "4.22.10-doca3.4.2",
	"4.22.11", "4.22.11-doca3.4.2",
	"4.22.12", "4.22.12-doca3.4.2",
	"4.22.2", "4.22.2-doca3.4.2",
	"4.22.3", "4.22.3-doca3.4.2",
}

// sampleImageLabels mirrors the version labels on real BlueField OCP layer images
var sampleImageLabels = map[string]string{
	"OCP.version":         "4.22.11",
	"NVIDIA.DOCA.version": "3.4.2",
}

var _ = Describe("BlueField OCP Layer Image Lookup", func() {
	Describe("OCP Version Extraction from Tag", func() {
		It("should strip -multi suffix", func() {
			Expect(extractVersionFromTag("quay.io/ocp-release:4.19.0-ec.5-multi")).To(Equal("4.19.0-ec.5"))
		})
		It("should strip -x86_64 suffix", func() {
			Expect(extractVersionFromTag("quay.io/ocp-release:4.18.2-rc.1-x86_64")).To(Equal("4.18.2-rc.1"))
		})
		It("should strip -amd64 suffix", func() {
			Expect(extractVersionFromTag("quay.io/ocp-release:4.19.0-ec.5-amd64")).To(Equal("4.19.0-ec.5"))
		})
		It("should strip -arm64 suffix", func() {
			Expect(extractVersionFromTag("quay.io/ocp-release:4.19.0-ec.5-arm64")).To(Equal("4.19.0-ec.5"))
		})
		It("should strip -ppc64le suffix", func() {
			Expect(extractVersionFromTag("quay.io/ocp-release:4.19.0-ec.5-ppc64le")).To(Equal("4.19.0-ec.5"))
		})
		It("should strip -s390x suffix", func() {
			Expect(extractVersionFromTag("quay.io/ocp-release:4.19.0-ec.5-s390x")).To(Equal("4.19.0-ec.5"))
		})
		It("should return version as-is when no arch suffix", func() {
			Expect(extractVersionFromTag("quay.io/ocp-release:4.19.0-ec.5")).To(Equal("4.19.0-ec.5"))
		})
		It("should return empty for digest images", func() {
			Expect(extractVersionFromTag("registry.ci/release@sha256:abc123")).To(BeEmpty())
		})
		It("should return empty for missing tag", func() {
			Expect(extractVersionFromTag("quay.io/ocp-release")).To(BeEmpty())
		})
		It("should return empty for empty tag", func() {
			Expect(extractVersionFromTag("quay.io/ocp-release:")).To(BeEmpty())
		})
	})

	Describe("DPF/DOCA Compatibility Matrix", func() {
		It("should map DPF 26.4 to DOCA 3.4", func() {
			doca, err := DocaVersionForDPF("26.4")
			Expect(err).NotTo(HaveOccurred())
			Expect(doca).To(Equal("3.4"))
		})

		It("should map DPF 26.8 to DOCA 3.5", func() {
			doca, err := DocaVersionForDPF("26.8")
			Expect(err).NotTo(HaveOccurred())
			Expect(doca).To(Equal("3.5"))
		})

		It("should return an error listing supported versions for an unknown DPF version", func() {
			_, err := DocaVersionForDPF("26.12")
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("26.12"))
			Expect(err.Error()).To(ContainSubstring("26.4"))
			Expect(err.Error()).To(ContainSubstring("26.8"))
		})

		It("should list supported DPF versions sorted", func() {
			Expect(SupportedDPFVersions()).To(Equal([]string{"26.4", "26.8"}))
		})
	})

	Describe("BlueField OCP Layer Tag Selection", func() {
		It("should select the DOCA-matched tag from a real-world tag list", func() {
			tag, found := findBlueFieldOCPLayerTag(sampleRegistryTags, "4.22.11", "3.4")
			Expect(found).To(BeTrue())
			Expect(tag).To(Equal("4.22.11-doca3.4.2"))
		})

		It("should not select the bare OCP tag (DOCA match is mandatory)", func() {
			tag, found := findBlueFieldOCPLayerTag([]string{"4.22.11"}, "4.22.11", "3.4")
			Expect(found).To(BeFalse())
			Expect(tag).To(BeEmpty())
		})

		It("should select the highest DOCA patch version when multiple tags match", func() {
			tags := []string{"4.22.11-doca3.4.1", "4.22.11-doca3.4.2", "4.22.11-doca3.4.10"}
			tag, found := findBlueFieldOCPLayerTag(tags, "4.22.11", "3.4")
			Expect(found).To(BeTrue())
			Expect(tag).To(Equal("4.22.11-doca3.4.10"))
		})

		It("should not match a different DOCA minor version", func() {
			tags := []string{"4.22.11", "4.22.11-doca3.5.0"}
			tag, found := findBlueFieldOCPLayerTag(tags, "4.22.11", "3.4")
			Expect(found).To(BeFalse())
			Expect(tag).To(BeEmpty())
		})

		It("should not match a DOCA minor version sharing a numeric prefix", func() {
			tags := []string{"4.22.11", "4.22.11-doca3.40.1"}
			tag, found := findBlueFieldOCPLayerTag(tags, "4.22.11", "3.4")
			Expect(found).To(BeFalse())
			Expect(tag).To(BeEmpty())
		})

		It("should handle OCP versions with a suffix", func() {
			tags := []string{"4.19.0-ec.5", "4.19.0-ec.5-doca3.4.2"}
			tag, found := findBlueFieldOCPLayerTag(tags, "4.19.0-ec.5", "3.4")
			Expect(found).To(BeTrue())
			Expect(tag).To(Equal("4.19.0-ec.5-doca3.4.2"))
		})

		It("should not match a longer OCP version sharing the same prefix", func() {
			tags := []string{"4.22.11-doca3.4.2"}
			tag, found := findBlueFieldOCPLayerTag(tags, "4.22.1", "3.4")
			Expect(found).To(BeFalse())
			Expect(tag).To(BeEmpty())
		})

		It("should skip tags with an unparseable DOCA version", func() {
			tags := []string{"4.22.11-doca", "4.22.11-doca-latest", "4.22.11-doca3.4.2"}
			tag, found := findBlueFieldOCPLayerTag(tags, "4.22.11", "3.4")
			Expect(found).To(BeTrue())
			Expect(tag).To(Equal("4.22.11-doca3.4.2"))
		})

		It("should return not found for an empty tag list", func() {
			tag, found := findBlueFieldOCPLayerTag(nil, "4.22.11", "3.4")
			Expect(found).To(BeFalse())
			Expect(tag).To(BeEmpty())
		})
	})

	Describe("Registry Tag Selection via selectImageTag", func() {
		var lookup *ImageLookup

		BeforeEach(func() {
			lookup = &ImageLookup{
				Labels: &fakeLabelFetcher{labels: sampleImageLabels},
			}
		})

		Context("When a matching tag exists in registry", func() {
			It("should return the full image reference", func() {
				lookup.TagLister = &fakeTagLister{tags: sampleRegistryTags}
				image, err := lookup.selectImageTag(context.Background(), testRepository, "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).NotTo(HaveOccurred())
				Expect(image).To(Equal(testRepository + ":4.22.11-doca3.4.2"))
			})
		})

		Context("When no matching tag exists in registry", func() {
			It("should return VersionNotFoundError", func() {
				lookup.TagLister = &fakeTagLister{tags: []string{"4.20.0-ec.1", "4.20.0-ec.1-doca3.4.2"}}
				_, err := lookup.selectImageTag(context.Background(), testRepository, "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var versionNotFoundErr *VersionNotFoundError
				Expect(err).To(BeAssignableToTypeOf(versionNotFoundErr))
			})
		})

		Context("When registry returns an auth error", func() {
			It("should return RegistryAuthError", func() {
				lookup.TagLister = &fakeTagLister{err: fmt.Errorf("UNAUTHORIZED: authentication required")}
				_, err := lookup.selectImageTag(context.Background(), testRepository, "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var authErr *RegistryAuthError
				Expect(err).To(BeAssignableToTypeOf(authErr))
			})
		})

		Context("When registry returns a transient error", func() {
			It("should return RegistryAccessError", func() {
				lookup.TagLister = &fakeTagLister{err: fmt.Errorf("connection refused")}
				_, err := lookup.selectImageTag(context.Background(), testRepository, "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var accessErr *RegistryAccessError
				Expect(err).To(BeAssignableToTypeOf(accessErr))
			})
		})

		Context("When the repository is malformed", func() {
			It("should return RegistryAccessError", func() {
				lookup.TagLister = &fakeTagLister{tags: sampleRegistryTags}
				_, err := lookup.selectImageTag(context.Background(), "quay.io/a b", "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var accessErr *RegistryAccessError
				Expect(err).To(BeAssignableToTypeOf(accessErr))
			})
		})

		Context("When the selected image's OCP label disagrees with its tag", func() {
			It("should return LabelMismatchError", func() {
				lookup.TagLister = &fakeTagLister{tags: sampleRegistryTags}
				lookup.Labels = &fakeLabelFetcher{labels: map[string]string{
					"OCP.version":         "4.22.10",
					"NVIDIA.DOCA.version": "3.4.2",
				}}
				_, err := lookup.selectImageTag(context.Background(), testRepository, "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var mismatchErr *LabelMismatchError
				Expect(err).To(BeAssignableToTypeOf(mismatchErr))
				Expect(err.Error()).To(ContainSubstring("OCP.version"))
			})
		})

		Context("When the selected image's DOCA label disagrees with its tag", func() {
			It("should return LabelMismatchError", func() {
				lookup.TagLister = &fakeTagLister{tags: sampleRegistryTags}
				lookup.Labels = &fakeLabelFetcher{labels: map[string]string{
					"OCP.version":         "4.22.11",
					"NVIDIA.DOCA.version": "3.5.0",
				}}
				_, err := lookup.selectImageTag(context.Background(), testRepository, "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var mismatchErr *LabelMismatchError
				Expect(err).To(BeAssignableToTypeOf(mismatchErr))
				Expect(err.Error()).To(ContainSubstring("NVIDIA.DOCA.version"))
			})
		})

		Context("When the selected image has no version labels", func() {
			It("should accept the tag (labels are advisory)", func() {
				lookup.TagLister = &fakeTagLister{tags: sampleRegistryTags}
				lookup.Labels = &fakeLabelFetcher{labels: map[string]string{}}
				image, err := lookup.selectImageTag(context.Background(), testRepository, "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).NotTo(HaveOccurred())
				Expect(image).To(Equal(testRepository + ":4.22.11-doca3.4.2"))
			})
		})

		Context("When label fetching fails with a transient error", func() {
			It("should return RegistryAccessError", func() {
				lookup.TagLister = &fakeTagLister{tags: sampleRegistryTags}
				lookup.Labels = &fakeLabelFetcher{err: fmt.Errorf("connection refused")}
				_, err := lookup.selectImageTag(context.Background(), testRepository, "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var accessErr *RegistryAccessError
				Expect(err).To(BeAssignableToTypeOf(accessErr))
			})
		})

		Context("When label fetching fails with an auth error", func() {
			It("should return RegistryAuthError", func() {
				lookup.TagLister = &fakeTagLister{tags: sampleRegistryTags}
				lookup.Labels = &fakeLabelFetcher{err: fmt.Errorf("UNAUTHORIZED: authentication required")}
				_, err := lookup.selectImageTag(context.Background(), testRepository, "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var authErr *RegistryAuthError
				Expect(err).To(BeAssignableToTypeOf(authErr))
			})
		})
	})

	Describe("DOCA Label Version Matching", func() {
		It("should match the same major.minor with any patch", func() {
			Expect(docaVersionMatches("3.4.2", "3.4")).To(BeTrue())
			Expect(docaVersionMatches("3.4.10", "3.4")).To(BeTrue())
		})
		It("should not match a different major or minor", func() {
			Expect(docaVersionMatches("3.5.0", "3.4")).To(BeFalse())
			Expect(docaVersionMatches("4.4.0", "3.4")).To(BeFalse())
		})
		It("should not match a version sharing a numeric prefix", func() {
			Expect(docaVersionMatches("3.40.1", "3.4")).To(BeFalse())
		})
		It("should not match an unparseable label", func() {
			Expect(docaVersionMatches("latest", "3.4")).To(BeFalse())
		})
	})

	Describe("LookupBlueFieldOCPLayerImage", func() {
		var (
			fc         client.Client
			lookup     *ImageLookup
			cr         *provisioningv1alpha1.DPFHCPProvisioner
			pullSecret *corev1.Secret
		)

		newTestScheme := func() *runtime.Scheme {
			s := runtime.NewScheme()
			Expect(clientgoscheme.AddToScheme(s)).To(Succeed())
			Expect(provisioningv1alpha1.AddToScheme(s)).To(Succeed())
			Expect(operatorv1alpha1.AddToScheme(s)).To(Succeed())
			return s
		}

		newDPFOperatorConfig := func(version *string) *operatorv1alpha1.DPFOperatorConfig {
			return &operatorv1alpha1.DPFOperatorConfig{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "dpfoperatorconfig",
					Namespace: "dpf-operator-system",
				},
				Status: operatorv1alpha1.DPFOperatorConfigStatus{
					Version: version,
				},
			}
		}

		BeforeEach(func() {
			version := "v26.4.1"
			cr = &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					OCPReleaseImage: "quay.io/openshift-release-dev/ocp-release:4.22.11-multi",
					PullSecretRef:   corev1.LocalObjectReference{Name: "pull-secret"},
				},
			}
			pullSecret = &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "pull-secret",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{".dockerconfigjson": []byte(`{"auths":{}}`)},
			}
			fc = fake.NewClientBuilder().
				WithScheme(newTestScheme()).
				WithObjects(cr, newDPFOperatorConfig(&version), pullSecret).
				WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
				Build()
			lookup = &ImageLookup{
				Client:     fc,
				Recorder:   record.NewFakeRecorder(10),
				TagLister:  &fakeTagLister{tags: sampleRegistryTags},
				Labels:     &fakeLabelFetcher{labels: sampleImageLabels},
				Repository: testRepository,
			}
		})

		It("should select the DOCA-matched image and update status", func() {
			result, err := lookup.LookupBlueFieldOCPLayerImage(context.Background(), cr)
			Expect(err).NotTo(HaveOccurred())
			Expect(result.RequeueAfter).To(BeZero())
			Expect(cr.Status.BlueFieldOCPLayerImage).To(Equal(testRepository + ":4.22.11-doca3.4.2"))

			cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionTrue))
			Expect(cond.Reason).To(Equal("ImageFound"))
		})

		It("should fail permanently when no tag matches the DOCA version", func() {
			lookup.TagLister = &fakeTagLister{tags: []string{"4.22.11"}}
			_, err := lookup.LookupBlueFieldOCPLayerImage(context.Background(), cr)
			Expect(err).NotTo(HaveOccurred())

			cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal("VersionNotFound"))
			Expect(cond.Message).To(ContainSubstring("4.22.11"))
			Expect(cond.Message).To(ContainSubstring("3.4"))
		})

		It("should fail permanently when the DPF version is not in the compatibility matrix", func() {
			unknown := "v26.12.0"
			fc = fake.NewClientBuilder().
				WithScheme(newTestScheme()).
				WithObjects(cr, newDPFOperatorConfig(&unknown), pullSecret).
				WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
				Build()
			lookup.Client = fc

			_, err := lookup.LookupBlueFieldOCPLayerImage(context.Background(), cr)
			Expect(err).NotTo(HaveOccurred())

			cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal("UnsupportedDPFVersion"))
			Expect(cond.Message).To(ContainSubstring("26.12"))
			Expect(cond.Message).To(ContainSubstring("26.4"))
			Expect(cond.Message).To(ContainSubstring("26.8"))
		})

		It("should retry when the DPF version is not available yet", func() {
			fc = fake.NewClientBuilder().
				WithScheme(newTestScheme()).
				WithObjects(cr, newDPFOperatorConfig(nil), pullSecret).
				WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
				Build()
			lookup.Client = fc

			_, err := lookup.LookupBlueFieldOCPLayerImage(context.Background(), cr)
			Expect(err).To(HaveOccurred())

			cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal("DPFVersionNotFound"))
		})

		It("should retry when the registry is unreachable", func() {
			lookup.TagLister = &fakeTagLister{err: fmt.Errorf("connection refused")}
			_, err := lookup.LookupBlueFieldOCPLayerImage(context.Background(), cr)
			Expect(err).To(HaveOccurred())

			cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal("RegistryAccessError"))
		})

		It("should fail permanently when the selected image's labels disagree with its tag", func() {
			lookup.Labels = &fakeLabelFetcher{labels: map[string]string{
				"OCP.version":         "4.22.10",
				"NVIDIA.DOCA.version": "3.4.2",
			}}
			_, err := lookup.LookupBlueFieldOCPLayerImage(context.Background(), cr)
			Expect(err).NotTo(HaveOccurred())

			cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal("LabelMismatch"))
			Expect(cond.Message).To(ContainSubstring("OCP.version"))
		})

		It("should retry when label fetching is unreachable", func() {
			lookup.Labels = &fakeLabelFetcher{err: fmt.Errorf("connection refused")}
			_, err := lookup.LookupBlueFieldOCPLayerImage(context.Background(), cr)
			Expect(err).To(HaveOccurred())

			cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal("RegistryAccessError"))
		})

		It("should fail permanently when ocpReleaseImage is empty", func() {
			cr.Spec.OCPReleaseImage = ""
			_, err := lookup.LookupBlueFieldOCPLayerImage(context.Background(), cr)
			Expect(err).NotTo(HaveOccurred())

			cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal("InvalidImageFormat"))
		})
	})

	Describe("BlueField OCP Layer Image URL Validation", func() {
		Context("When URL is valid", func() {
			It("should not return an error", func() {
				url := "quay.io/edge-infrastructure/bluefield-rhcos:4.19.0-ec.5"
				err := validateBlueFieldOCPLayerImageURL(url, "4.19.0-ec.5")
				Expect(err).NotTo(HaveOccurred())
			})
		})

		Context("When URL is valid with port", func() {
			It("should not return an error", func() {
				url := "registry.example.com:5000/bluefield-rhcos:4.19.0-ec.5"
				err := validateBlueFieldOCPLayerImageURL(url, "4.19.0-ec.5")
				Expect(err).NotTo(HaveOccurred())
			})
		})

		Context("When URL is empty", func() {
			It("should return InvalidBlueFieldOCPLayerImageURLError", func() {
				err := validateBlueFieldOCPLayerImageURL("", "4.19.0-ec.5")
				Expect(err).To(HaveOccurred())
				var invalidURLErr *InvalidBlueFieldOCPLayerImageURLError
				Expect(err).To(BeAssignableToTypeOf(invalidURLErr))
			})
		})

		Context("When URL is missing colon separator", func() {
			It("should return InvalidBlueFieldOCPLayerImageURLError", func() {
				url := "quay.io/edge-infrastructure/bluefield-rhcos"
				err := validateBlueFieldOCPLayerImageURL(url, "4.19.0-ec.5")
				Expect(err).To(HaveOccurred())
				var invalidURLErr *InvalidBlueFieldOCPLayerImageURLError
				Expect(err).To(BeAssignableToTypeOf(invalidURLErr))
			})
		})
	})

	Describe("Error Types", func() {
		Context("RegistryAccessError", func() {
			It("should have a descriptive error message", func() {
				err := &RegistryAccessError{
					Repository: testRepository,
					Err:        fmt.Errorf("connection refused"),
				}
				Expect(err.Error()).To(ContainSubstring("failed to access registry"))
				Expect(err.Error()).To(ContainSubstring(testRepository))
			})
		})

		Context("RegistryAuthError", func() {
			It("should have a descriptive error message", func() {
				err := &RegistryAuthError{
					Repository: testRepository,
					Err:        fmt.Errorf("UNAUTHORIZED"),
				}
				Expect(err.Error()).To(ContainSubstring("authentication failed"))
				Expect(err.Error()).To(ContainSubstring(testRepository))
			})
		})

		Context("VersionNotFoundError", func() {
			It("should include OCP version, DOCA version and repository in error message", func() {
				err := &VersionNotFoundError{
					Version:     "4.22.11",
					DocaVersion: "3.4",
					Repository:  testRepository,
				}
				Expect(err.Error()).To(ContainSubstring("4.22.11"))
				Expect(err.Error()).To(ContainSubstring("3.4"))
				Expect(err.Error()).To(ContainSubstring(testRepository))
			})
		})

		Context("LabelMismatchError", func() {
			It("should include the image, label, label value and expected value in error message", func() {
				err := &LabelMismatchError{
					Image:      testRepository + ":4.22.11-doca3.4.2",
					Label:      OCPLayerOCPVersionLabel,
					LabelValue: "4.22.10",
					Expected:   "4.22.11",
				}
				Expect(err.Error()).To(ContainSubstring("4.22.11-doca3.4.2"))
				Expect(err.Error()).To(ContainSubstring("OCP.version"))
				Expect(err.Error()).To(ContainSubstring("4.22.10"))
			})
		})

		Context("UnsupportedDPFVersionError", func() {
			It("should include the DPF version and supported versions in error message", func() {
				err := &UnsupportedDPFVersionError{
					DPFVersion: "26.12",
					Supported:  []string{"26.4", "26.8"},
				}
				Expect(err.Error()).To(ContainSubstring("26.12"))
				Expect(err.Error()).To(ContainSubstring("26.4"))
				Expect(err.Error()).To(ContainSubstring("26.8"))
			})
		})

		Context("DPFVersionError", func() {
			It("should wrap the underlying error", func() {
				cause := fmt.Errorf("DPFOperatorConfig.status.version is not set")
				err := &DPFVersionError{Err: cause}
				Expect(err.Error()).To(ContainSubstring("failed to determine DPF version"))
				Expect(err.Error()).To(ContainSubstring("DPFOperatorConfig.status.version is not set"))
				Expect(err.Unwrap()).To(Equal(cause))
			})
		})

		Context("InvalidImageFormatError", func() {
			It("should include the message and URL in error message", func() {
				err := &InvalidImageFormatError{
					Message: "missing tag separator",
					URL:     "quay.io/test/image",
				}
				Expect(err.Error()).To(ContainSubstring("missing tag separator"))
				Expect(err.Error()).To(ContainSubstring("quay.io/test/image"))
			})
		})

		Context("InvalidBlueFieldOCPLayerImageURLError", func() {
			It("should include version and message in error message", func() {
				err := &InvalidBlueFieldOCPLayerImageURLError{
					Version: "4.19.0-ec.5",
					URL:     "",
					Message: "BlueField OCP layer image URL is empty",
				}
				Expect(err.Error()).To(ContainSubstring("4.19.0-ec.5"))
				Expect(err.Error()).To(ContainSubstring("empty"))
			})
		})
	})

	Describe("isAuthError", func() {
		It("should detect UNAUTHORIZED errors", func() {
			Expect(isAuthError(fmt.Errorf("UNAUTHORIZED: authentication required"))).To(BeTrue())
		})

		It("should detect denied errors", func() {
			Expect(isAuthError(fmt.Errorf("denied: access forbidden"))).To(BeTrue())
		})

		It("should detect 403 errors", func() {
			Expect(isAuthError(fmt.Errorf("unexpected status code 403"))).To(BeTrue())
		})

		It("should not match non-auth errors", func() {
			Expect(isAuthError(fmt.Errorf("connection refused"))).To(BeFalse())
		})
	})
})
