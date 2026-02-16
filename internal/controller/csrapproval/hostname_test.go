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

package csrapproval

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	certv1 "k8s.io/api/certificates/v1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

const (
	testHostname = "dpu-worker-01"
)

var _ = Describe("Hostname Extraction", func() {
	Describe("ExtractHostname from Bootstrap CSR", func() {
		Context("When CSR has valid CN with system:node: prefix", func() {
			It("should extract hostname correctly", func() {
				hostname := testHostname
				csrBytes := createTestCSR("system:node:" + hostname)

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-bootstrap-csr",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerBootstrap,
						Request:    csrBytes,
					},
				}

				extractedHostname, err := ExtractHostname(csr)
				Expect(err).NotTo(HaveOccurred())
				Expect(extractedHostname).To(Equal(hostname))
			})
		})

		Context("When CSR has invalid CN format", func() {
			It("should return error", func() {
				csrBytes := createTestCSR("invalid-format")

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-invalid-csr",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerBootstrap,
						Request:    csrBytes,
					},
				}

				_, err := ExtractHostname(csr)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("invalid CN format"))
			})
		})

		Context("When CSR has empty CN after prefix removal", func() {
			It("should return error", func() {
				csrBytes := createTestCSR("system:node:")

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-empty-hostname-csr",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerBootstrap,
						Request:    csrBytes,
					},
				}

				_, err := ExtractHostname(csr)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("invalid CN format"))
			})
		})
	})

	Describe("ExtractHostname from Serving CSR", func() {
		Context("When CSR has valid username with system:node: prefix", func() {
			It("should extract hostname correctly", func() {
				hostname := testHostname

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-serving-csr",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerServing,
						Username:   "system:node:" + hostname,
					},
				}

				extractedHostname, err := ExtractHostname(csr)
				Expect(err).NotTo(HaveOccurred())
				Expect(extractedHostname).To(Equal(hostname))
			})
		})

		Context("When CSR has invalid username format", func() {
			It("should return error", func() {
				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-invalid-serving-csr",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerServing,
						Username:   "invalid-format",
					},
				}

				_, err := ExtractHostname(csr)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("invalid username format"))
			})
		})

		Context("When CSR has empty username after prefix removal", func() {
			It("should return error", func() {
				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-empty-username-csr",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerServing,
						Username:   "system:node:",
					},
				}

				_, err := ExtractHostname(csr)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("invalid username format"))
			})
		})
	})

	Describe("ExtractHostname with unsupported signer", func() {
		Context("When CSR has unsupported signer name", func() {
			It("should return error", func() {
				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-unsupported-csr",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: "unsupported.signer.io/test",
						Username:   "system:node:test-host",
					},
				}

				_, err := ExtractHostname(csr)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("unsupported signer name"))
			})
		})

		Context("When CSR is nil", func() {
			It("should return error", func() {
				_, err := ExtractHostname(nil)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("CSR is nil"))
			})
		})
	})

	Describe("IsPending", func() {
		Context("When CSR has no conditions", func() {
			It("should return true", func() {
				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-pending-csr",
					},
					Status: certv1.CertificateSigningRequestStatus{
						Conditions: []certv1.CertificateSigningRequestCondition{},
					},
				}

				Expect(IsPending(csr)).To(BeTrue())
			})
		})

		Context("When CSR has Approved condition", func() {
			It("should return false", func() {
				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-approved-csr",
					},
					Status: certv1.CertificateSigningRequestStatus{
						Conditions: []certv1.CertificateSigningRequestCondition{
							{
								Type:   certv1.CertificateApproved,
								Status: corev1.ConditionTrue,
							},
						},
					},
				}

				Expect(IsPending(csr)).To(BeFalse())
			})
		})

		Context("When CSR has Denied condition", func() {
			It("should return false", func() {
				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-denied-csr",
					},
					Status: certv1.CertificateSigningRequestStatus{
						Conditions: []certv1.CertificateSigningRequestCondition{
							{
								Type:   certv1.CertificateDenied,
								Status: corev1.ConditionTrue,
							},
						},
					},
				}

				Expect(IsPending(csr)).To(BeFalse())
			})
		})

		Context("When CSR is nil", func() {
			It("should return false", func() {
				Expect(IsPending(nil)).To(BeFalse())
			})
		})
	})

	Describe("FilterBySignerName", func() {
		Context("When list contains mixed signer types", func() {
			It("should filter to only bootstrap and serving CSRs", func() {
				csrs := []certv1.CertificateSigningRequest{
					{
						ObjectMeta: metav1.ObjectMeta{Name: "bootstrap-csr"},
						Spec:       certv1.CertificateSigningRequestSpec{SignerName: SignerBootstrap},
					},
					{
						ObjectMeta: metav1.ObjectMeta{Name: "serving-csr"},
						Spec:       certv1.CertificateSigningRequestSpec{SignerName: SignerServing},
					},
					{
						ObjectMeta: metav1.ObjectMeta{Name: "other-csr"},
						Spec:       certv1.CertificateSigningRequestSpec{SignerName: "other.signer.io/test"},
					},
				}

				filtered := FilterBySignerName(csrs)
				Expect(filtered).To(HaveLen(2))
				Expect(filtered[0].Name).To(Equal("bootstrap-csr"))
				Expect(filtered[1].Name).To(Equal("serving-csr"))
			})
		})

		Context("When list contains no matching signers", func() {
			It("should return empty list", func() {
				csrs := []certv1.CertificateSigningRequest{
					{
						ObjectMeta: metav1.ObjectMeta{Name: "other-csr"},
						Spec:       certv1.CertificateSigningRequestSpec{SignerName: "other.signer.io/test"},
					},
				}

				filtered := FilterBySignerName(csrs)
				Expect(filtered).To(BeEmpty())
			})
		})

		Context("When list is empty", func() {
			It("should return empty list", func() {
				csrs := []certv1.CertificateSigningRequest{}

				filtered := FilterBySignerName(csrs)
				Expect(filtered).To(BeEmpty())
			})
		})
	})
})

// createTestCSR creates a test CSR with the given CN
func createTestCSR(cn string) []byte {
	// Generate a private key
	privateKey, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		panic(err)
	}

	// Create CSR template
	template := x509.CertificateRequest{
		Subject: pkix.Name{
			CommonName: cn,
		},
	}

	// Create CSR
	csrBytes, err := x509.CreateCertificateRequest(rand.Reader, &template, privateKey)
	if err != nil {
		panic(err)
	}

	// Encode to PEM
	pemBlock := pem.EncodeToMemory(&pem.Block{
		Type:  "CERTIFICATE REQUEST",
		Bytes: csrBytes,
	})

	// Return PEM-encoded bytes directly (Kubernetes Go client handles base64 decoding from JSON/YAML)
	return pemBlock
}

// createTestCSRWithOrganization creates a test CSR with the given CN and organization
func createTestCSRWithOrganization(cn, org string) []byte {
	// Generate a private key
	privateKey, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		panic(err)
	}

	// Create CSR template with organization
	template := x509.CertificateRequest{
		Subject: pkix.Name{
			CommonName:   cn,
			Organization: []string{org},
		},
	}

	// Create CSR
	csrBytes, err := x509.CreateCertificateRequest(rand.Reader, &template, privateKey)
	if err != nil {
		panic(err)
	}

	// Encode to PEM
	pemBlock := pem.EncodeToMemory(&pem.Block{
		Type:  "CERTIFICATE REQUEST",
		Bytes: csrBytes,
	})

	// Return PEM-encoded bytes directly (Kubernetes Go client handles base64 decoding from JSON/YAML)
	return pemBlock
}

// createServingCSRWithDNS creates a test CSR with the given CN and DNS names
func createServingCSRWithDNS(cn, dnsName string) []byte {
	// Generate a private key
	privateKey, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		panic(err)
	}

	// Create CSR template with organization and DNS names
	template := x509.CertificateRequest{
		Subject: pkix.Name{
			CommonName:   cn,
			Organization: []string{"system:nodes"},
		},
		DNSNames: []string{dnsName},
	}

	// Create CSR
	csrBytes, err := x509.CreateCertificateRequest(rand.Reader, &template, privateKey)
	if err != nil {
		panic(err)
	}

	// Encode to PEM
	pemBlock := pem.EncodeToMemory(&pem.Block{
		Type:  "CERTIFICATE REQUEST",
		Bytes: csrBytes,
	})

	// Return PEM-encoded bytes directly (Kubernetes Go client handles base64 decoding from JSON/YAML)
	return pemBlock
}
