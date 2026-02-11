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

package bluefield

import (
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

var _ = Describe("BlueField Image Resolver", func() {
	Describe("OCP Version Extraction", func() {
		Context("When extracting version with -multi suffix", func() {
			It("should strip the -multi suffix", func() {
				ocpReleaseImage := "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi"
				version, err := extractOCPVersion(ocpReleaseImage)
				Expect(err).NotTo(HaveOccurred())
				Expect(version).To(Equal("4.19.0-ec.5"))
			})
		})

		Context("When extracting version with -x86_64 suffix", func() {
			It("should strip the -x86_64 suffix", func() {
				ocpReleaseImage := "quay.io/openshift-release-dev/ocp-release:4.18.2-rc.1-x86_64"
				version, err := extractOCPVersion(ocpReleaseImage)
				Expect(err).NotTo(HaveOccurred())
				Expect(version).To(Equal("4.18.2-rc.1"))
			})
		})

		Context("When extracting version with -amd64 suffix", func() {
			It("should strip the -amd64 suffix", func() {
				ocpReleaseImage := "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-amd64"
				version, err := extractOCPVersion(ocpReleaseImage)
				Expect(err).NotTo(HaveOccurred())
				Expect(version).To(Equal("4.19.0-ec.5"))
			})
		})

		Context("When extracting version with -arm64 suffix", func() {
			It("should strip the -arm64 suffix", func() {
				ocpReleaseImage := "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-arm64"
				version, err := extractOCPVersion(ocpReleaseImage)
				Expect(err).NotTo(HaveOccurred())
				Expect(version).To(Equal("4.19.0-ec.5"))
			})
		})

		Context("When extracting version with -ppc64le suffix", func() {
			It("should strip the -ppc64le suffix", func() {
				ocpReleaseImage := "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-ppc64le"
				version, err := extractOCPVersion(ocpReleaseImage)
				Expect(err).NotTo(HaveOccurred())
				Expect(version).To(Equal("4.19.0-ec.5"))
			})
		})

		Context("When extracting version with -s390x suffix", func() {
			It("should strip the -s390x suffix", func() {
				ocpReleaseImage := "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-s390x"
				version, err := extractOCPVersion(ocpReleaseImage)
				Expect(err).NotTo(HaveOccurred())
				Expect(version).To(Equal("4.19.0-ec.5"))
			})
		})

		Context("When extracting version without any suffix", func() {
			It("should return the version as-is", func() {
				ocpReleaseImage := "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5"
				version, err := extractOCPVersion(ocpReleaseImage)
				Expect(err).NotTo(HaveOccurred())
				Expect(version).To(Equal("4.19.0-ec.5"))
			})
		})

		Context("When image URL has no tag separator", func() {
			It("should return an error", func() {
				ocpReleaseImage := "quay.io/openshift-release-dev/ocp-release"
				_, err := extractOCPVersion(ocpReleaseImage)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("missing tag separator"))
			})
		})

		Context("When image URL has empty tag", func() {
			It("should return an error", func() {
				ocpReleaseImage := "quay.io/openshift-release-dev/ocp-release:"
				_, err := extractOCPVersion(ocpReleaseImage)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("empty tag"))
			})
		})
	})

	Describe("ConfigMap Lookup", func() {
		var configMap *corev1.ConfigMap

		BeforeEach(func() {
			configMap = &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "ocp-bluefield-images",
					Namespace: "dpf-hcp-provisioner-system",
				},
				Data: map[string]string{
					"4.19.0-ec.5": "quay.io/edge-infrastructure/bluefield-rhcos:4.19.0-ec.5",
					"4.18.0":      "quay.io/edge-infrastructure/bluefield-rhcos:4.18.0",
				},
			}
		})

		Context("When version exists in ConfigMap", func() {
			It("should return the BlueField image URL", func() {
				blueFieldImage, err := lookupBlueFieldImage(configMap, "4.19.0-ec.5")
				Expect(err).NotTo(HaveOccurred())
				Expect(blueFieldImage).To(Equal("quay.io/edge-infrastructure/bluefield-rhcos:4.19.0-ec.5"))
			})
		})

		Context("When version does not exist in ConfigMap", func() {
			It("should return VersionNotFoundError", func() {
				_, err := lookupBlueFieldImage(configMap, "4.20.0-ec.1")
				Expect(err).To(HaveOccurred())
				var versionNotFoundErr *VersionNotFoundError
				Expect(err).To(BeAssignableToTypeOf(versionNotFoundErr))
			})
		})

		Context("When ConfigMap has nil Data", func() {
			It("should return VersionNotFoundError", func() {
				configMap.Data = nil
				_, err := lookupBlueFieldImage(configMap, "4.19.0-ec.5")
				Expect(err).To(HaveOccurred())
				var versionNotFoundErr *VersionNotFoundError
				Expect(err).To(BeAssignableToTypeOf(versionNotFoundErr))
			})
		})

		Context("When ConfigMap has empty Data map", func() {
			It("should return VersionNotFoundError", func() {
				configMap.Data = map[string]string{}
				_, err := lookupBlueFieldImage(configMap, "4.19.0-ec.5")
				Expect(err).To(HaveOccurred())
				var versionNotFoundErr *VersionNotFoundError
				Expect(err).To(BeAssignableToTypeOf(versionNotFoundErr))
			})
		})
	})

	Describe("BlueField Image URL Validation", func() {
		Context("When URL is valid", func() {
			It("should not return an error", func() {
				url := "quay.io/edge-infrastructure/bluefield-rhcos:4.19.0-ec.5"
				err := validateBlueFieldImageURL(url, "4.19.0-ec.5")
				Expect(err).NotTo(HaveOccurred())
			})
		})

		Context("When URL is valid with port", func() {
			It("should not return an error", func() {
				url := "registry.example.com:5000/bluefield-rhcos:4.19.0-ec.5"
				err := validateBlueFieldImageURL(url, "4.19.0-ec.5")
				Expect(err).NotTo(HaveOccurred())
			})
		})

		Context("When URL is empty", func() {
			It("should return InvalidBlueFieldImageURLError", func() {
				err := validateBlueFieldImageURL("", "4.19.0-ec.5")
				Expect(err).To(HaveOccurred())
				var invalidURLErr *InvalidBlueFieldImageURLError
				Expect(err).To(BeAssignableToTypeOf(invalidURLErr))
			})
		})

		Context("When URL is missing colon separator", func() {
			It("should return InvalidBlueFieldImageURLError", func() {
				url := "quay.io/edge-infrastructure/bluefield-rhcos"
				err := validateBlueFieldImageURL(url, "4.19.0-ec.5")
				Expect(err).To(HaveOccurred())
				var invalidURLErr *InvalidBlueFieldImageURLError
				Expect(err).To(BeAssignableToTypeOf(invalidURLErr))
			})
		})
	})

	Describe("Error Types", func() {
		Context("ConfigMapNotFoundError", func() {
			It("should have a descriptive error message", func() {
				err := &ConfigMapNotFoundError{Err: nil}
				Expect(err.Error()).To(ContainSubstring("ConfigMap ocp-bluefield-images not found"))
			})
		})

		Context("ConfigMapAccessDeniedError", func() {
			It("should have a descriptive error message", func() {
				err := &ConfigMapAccessDeniedError{Err: nil}
				Expect(err.Error()).To(ContainSubstring("RBAC permissions"))
			})
		})

		Context("VersionNotFoundError", func() {
			It("should include version and available versions in error message", func() {
				err := &VersionNotFoundError{
					Version:           "4.19.0-ec.5",
					AvailableVersions: []string{"4.18.0", "4.17.0"},
				}
				Expect(err.Error()).To(ContainSubstring("4.19.0-ec.5"))
				Expect(err.Error()).To(ContainSubstring("available versions"))
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

		Context("InvalidBlueFieldImageURLError", func() {
			It("should include version and message in error message", func() {
				err := &InvalidBlueFieldImageURLError{
					Version: "4.19.0-ec.5",
					URL:     "",
					Message: "BlueField image URL is empty",
				}
				Expect(err.Error()).To(ContainSubstring("4.19.0-ec.5"))
				Expect(err.Error()).To(ContainSubstring("empty"))
			})
		})
	})
})
