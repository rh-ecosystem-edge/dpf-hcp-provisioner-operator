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
	"k8s.io/utils/ptr"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// fakeImageChecker implements ImageChecker for testing
type fakeImageChecker struct {
	tags []string
	err  error
}

func (f *fakeImageChecker) ListTags(ctx context.Context, repo name.Repository, keychain authn.Keychain) ([]string, error) {
	return f.tags, f.err
}

const testRepository = "quay.io/test-org/bluefield-os"

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

	Describe("Registry Tag Selection", func() {
		selectTag := func(tags []string, ocpVersion string) (string, error) {
			lookup := &ImageLookup{ImageChecker: &fakeImageChecker{tags: tags}}
			return lookup.selectImageTag(context.Background(), testRepository, ocpVersion, "3.4", authn.DefaultKeychain)
		}

		DescribeTable("should select the tag matching the OCP and DOCA versions",
			func(tags []string, ocpVersion, expected string) {
				image, err := selectTag(tags, ocpVersion)
				Expect(err).NotTo(HaveOccurred())
				Expect(image).To(Equal(testRepository + ":" + expected))
			},
			Entry("DOCA tag",
				[]string{"4.22.1-doca3.4.2", "4.22.11", "4.22.11-doca3.4.2", "4.22.110-doca3.4.2"},
				"4.22.11", "4.22.11-doca3.4.2"),
			Entry("4k build",
				[]string{"4.22.17-doca3.4.2", "4.22.17-4k-doca3.4.2", "4.22.17-4k-doca3.4.2-1792683047"},
				"4.22.17", "4.22.17-4k-doca3.4.2"),
			Entry("DOCA major.minor tag",
				[]string{"4.22.17", "4.22.17-4k-doca3.4"},
				"4.22.17", "4.22.17-4k-doca3.4"),
			Entry("highest DOCA version",
				[]string{"4.22.11-doca3.4.1", "4.22.11-doca3.4.10", "4.22.11-doca3.4.2"},
				"4.22.11", "4.22.11-doca3.4.10"),
			Entry("OCP version tag when no tag names the DOCA version",
				[]string{"4.22.7", "v4.22.7", "4.22.7-1787823170", "4.22.70-doca3.4.2"},
				"4.22.7", "4.22.7"),
			Entry("DOCA tag over the OCP version tag",
				[]string{"4.22.7", "4.22.7-doca3.4.2"},
				"4.22.7", "4.22.7-doca3.4.2"),
			Entry("engineering candidate",
				[]string{"4.23.0-doca3.4.2", "4.23.0-ec.5-doca3.4.2", "4.23.0-ec.5-4k-doca3.4.2", "4.23.0-ec.50-doca3.4.2"},
				"4.23.0-ec.5", "4.23.0-ec.5-4k-doca3.4.2"),
			Entry("release candidate",
				[]string{"4.23.0-doca3.4.2", "4.23.0-rc.2-doca3.4.2", "4.23.0-rc.20-4k-doca3.4.2"},
				"4.23.0-rc.2", "4.23.0-rc.2-doca3.4.2"),
			Entry("release over candidates",
				[]string{"4.23.0-ec.5-4k-doca3.4.2", "4.23.0-rc.2-4k-doca3.4.2", "4.23.0-doca3.4.2"},
				"4.23.0", "4.23.0-doca3.4.2"),
		)

		DescribeTable("should return VersionNotFoundError when no tag matches",
			func(tags []string) {
				_, err := selectTag(tags, "4.22.11")
				var versionNotFoundErr *VersionNotFoundError
				Expect(err).To(BeAssignableToTypeOf(versionNotFoundErr))
			},
			Entry("no DOCA or OCP version tag", []string{"v4.22.11", "4.22.11-1787823170", "4.22.110"}),
			Entry("different DOCA minor", []string{"4.22.11-doca3.5.0", "4.22.11-doca3.40.1"}),
			Entry("timestamped alias only", []string{"4.22.11-4k-doca3.4.2-1792683047"}),
			Entry("only candidates of the release", []string{"4.22.11-ec.5-doca3.4.2", "4.22.11-rc.2-4k-doca3.4.2"}),
		)

		DescribeTable("should classify tag listing errors",
			func(listErr error, expected error) {
				lookup := &ImageLookup{ImageChecker: &fakeImageChecker{err: listErr}}
				_, err := lookup.selectImageTag(context.Background(), testRepository, "4.22.11", "3.4", authn.DefaultKeychain)
				Expect(err).To(BeAssignableToTypeOf(expected))
			},
			Entry("unauthorized", fmt.Errorf("UNAUTHORIZED: authentication required"), &RegistryAuthError{}),
			Entry("unreachable", fmt.Errorf("connection refused"), &RegistryAccessError{}),
		)
	})

	Describe("LookupBlueFieldOCPLayerImage", func() {
		var cr *provisioningv1alpha1.DPFHCPProvisioner

		// newLookup returns an ImageLookup whose cluster holds cr, a pull secret
		// and a DPFOperatorConfig reporting dpfVersion.
		newLookup := func(dpfVersion *string) *ImageLookup {
			s := runtime.NewScheme()
			Expect(clientgoscheme.AddToScheme(s)).To(Succeed())
			Expect(provisioningv1alpha1.AddToScheme(s)).To(Succeed())
			Expect(operatorv1alpha1.AddToScheme(s)).To(Succeed())
			return &ImageLookup{
				Client: fake.NewClientBuilder().
					WithScheme(s).
					WithObjects(cr,
						&operatorv1alpha1.DPFOperatorConfig{
							ObjectMeta: metav1.ObjectMeta{Name: "dpfoperatorconfig", Namespace: "dpf-operator-system"},
							Status:     operatorv1alpha1.DPFOperatorConfigStatus{Version: dpfVersion},
						},
						&corev1.Secret{
							ObjectMeta: metav1.ObjectMeta{Name: "pull-secret", Namespace: "test-ns"},
							Data:       map[string][]byte{".dockerconfigjson": []byte(`{"auths":{}}`)},
						}).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build(),
				Recorder:     record.NewFakeRecorder(10),
				ImageChecker: &fakeImageChecker{tags: []string{"4.22.17-doca3.4.2", "4.22.17-doca3.5.0"}},
				Repository:   testRepository,
			}
		}

		BeforeEach(func() {
			cr = &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{Name: "test", Namespace: "test-ns"},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					OCPReleaseImage: "quay.io/openshift-release-dev/ocp-release:4.22.17-multi",
					PullSecretRef:   corev1.LocalObjectReference{Name: "pull-secret"},
				},
			}
		})

		DescribeTable("should select the tag for the DOCA version of the installed DPF version",
			func(dpfVersion, expected string) {
				result, err := newLookup(&dpfVersion).LookupBlueFieldOCPLayerImage(context.Background(), cr)
				Expect(err).NotTo(HaveOccurred())
				Expect(result.RequeueAfter).To(BeZero())
				Expect(cr.Status.BlueFieldOCPLayerImage).To(Equal(testRepository + ":" + expected))

				cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
				Expect(cond).NotTo(BeNil())
				Expect(cond.Status).To(Equal(metav1.ConditionTrue))
				Expect(cond.Reason).To(Equal("ImageFound"))
			},
			Entry("DPF 26.4 uses DOCA 3.4", "v26.4.1", "4.22.17-doca3.4.2"),
			Entry("unknown DPF version uses the default DOCA version", "v26.12.0", "4.22.17-doca3.4.2"),
		)

		DescribeTable("should retry when the DPF version is not available",
			func(dpfVersion *string) {
				_, err := newLookup(dpfVersion).LookupBlueFieldOCPLayerImage(context.Background(), cr)
				Expect(err).To(HaveOccurred())

				cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
				Expect(cond).NotTo(BeNil())
				Expect(cond.Status).To(Equal(metav1.ConditionFalse))
				Expect(cond.Reason).To(Equal("DPFVersionNotFound"))
			},
			Entry("status.version not set", nil),
			Entry("status.version without a minor version", ptr.To("v26")),
		)
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
			It("should include version and repository in error message", func() {
				err := &VersionNotFoundError{
					Version:     "4.19.0-ec.5",
					DocaVersion: "3.4",
					Repository:  testRepository,
				}
				Expect(err.Error()).To(ContainSubstring("4.19.0-ec.5"))
				Expect(err.Error()).To(ContainSubstring("3.4"))
				Expect(err.Error()).To(ContainSubstring(testRepository))
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
