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
)

// fakeImageChecker implements ImageChecker for testing
type fakeImageChecker struct {
	err error
}

func (f *fakeImageChecker) CheckTag(ctx context.Context, ref name.Reference, keychain authn.Keychain) error {
	return f.err
}

const testRepository = "quay.io/test-org/bluefield-os"

var _ = Describe("BlueField OCP Layer Image Lookup", func() {
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

	Describe("Registry Tag Lookup", func() {
		var lookup *ImageLookup

		BeforeEach(func() {
			lookup = &ImageLookup{}
		})

		Context("When version tag exists in registry", func() {
			It("should return the full image reference", func() {
				lookup.ImageChecker = &fakeImageChecker{err: nil}
				image, err := lookup.validateTagExists(context.Background(), testRepository, "4.19.0-ec.5", authn.DefaultKeychain)
				Expect(err).NotTo(HaveOccurred())
				Expect(image).To(Equal(testRepository + ":4.19.0-ec.5"))
			})
		})

		Context("When version tag does not exist in registry", func() {
			It("should return VersionNotFoundError", func() {
				lookup.ImageChecker = &fakeImageChecker{
					err: fmt.Errorf("MANIFEST_UNKNOWN: manifest unknown"),
				}
				_, err := lookup.validateTagExists(context.Background(), testRepository, "4.20.0-ec.1", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var versionNotFoundErr *VersionNotFoundError
				Expect(err).To(BeAssignableToTypeOf(versionNotFoundErr))
			})
		})

		Context("When registry returns a 404 not found error", func() {
			It("should return VersionNotFoundError", func() {
				lookup.ImageChecker = &fakeImageChecker{
					err: fmt.Errorf("unexpected status code 404 not found"),
				}
				_, err := lookup.validateTagExists(context.Background(), testRepository, "4.19.0-ec.5", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var versionNotFoundErr *VersionNotFoundError
				Expect(err).To(BeAssignableToTypeOf(versionNotFoundErr))
			})
		})

		Context("When registry returns an auth error", func() {
			It("should return RegistryAuthError", func() {
				lookup.ImageChecker = &fakeImageChecker{
					err: fmt.Errorf("UNAUTHORIZED: authentication required"),
				}
				_, err := lookup.validateTagExists(context.Background(), testRepository, "4.19.0-ec.5", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var authErr *RegistryAuthError
				Expect(err).To(BeAssignableToTypeOf(authErr))
			})
		})

		Context("When registry returns a transient error", func() {
			It("should return RegistryAccessError", func() {
				lookup.ImageChecker = &fakeImageChecker{
					err: fmt.Errorf("connection refused"),
				}
				_, err := lookup.validateTagExists(context.Background(), testRepository, "4.19.0-ec.5", authn.DefaultKeychain)
				Expect(err).To(HaveOccurred())
				var accessErr *RegistryAccessError
				Expect(err).To(BeAssignableToTypeOf(accessErr))
			})
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
			It("should include version and repository in error message", func() {
				err := &VersionNotFoundError{
					Version:    "4.19.0-ec.5",
					Repository: testRepository,
				}
				Expect(err.Error()).To(ContainSubstring("4.19.0-ec.5"))
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

	Describe("isNotFoundError", func() {
		It("should detect MANIFEST_UNKNOWN errors", func() {
			Expect(isNotFoundError(fmt.Errorf("MANIFEST_UNKNOWN: manifest unknown"))).To(BeTrue())
		})

		It("should detect NOT_FOUND errors", func() {
			Expect(isNotFoundError(fmt.Errorf("NOT_FOUND: resource not found"))).To(BeTrue())
		})

		It("should detect 404 errors", func() {
			Expect(isNotFoundError(fmt.Errorf("unexpected status code 404"))).To(BeTrue())
		})

		It("should not match non-not-found errors", func() {
			Expect(isNotFoundError(fmt.Errorf("connection refused"))).To(BeFalse())
		})
	})
})
