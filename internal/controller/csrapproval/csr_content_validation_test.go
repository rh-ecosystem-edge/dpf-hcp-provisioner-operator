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
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	certv1 "k8s.io/api/certificates/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

var _ = Describe("CSR Validation", func() {
	Describe("ValidateBootstrapCSR", func() {
		Context("When CSR has valid bootstrap configuration", func() {
			It("should pass validation", func() {
				hostname := testHostname
				csrBytes := createTestCSRWithOrganization("system:node:"+hostname, "system:nodes")

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-bootstrap-csr",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerBootstrap,
						Username:   BootstrapperUsername,
						Groups: []string{
							groupServiceAccounts,
							groupServiceAccountsMachineConfig,
							groupAuthenticated,
						},
						Usages: []certv1.KeyUsage{
							certv1.UsageDigitalSignature,
							certv1.UsageClientAuth,
						},
						Request: csrBytes,
					},
				}

				err := ValidateBootstrapCSR(csr, hostname)
				Expect(err).NotTo(HaveOccurred())
			})
		})

		Context("When CSR has invalid username", func() {
			It("should fail validation", func() {
				hostname := testHostname
				csrBytes := createTestCSRWithOrganization("system:node:"+hostname, "system:nodes")

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-invalid-username",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerBootstrap,
						Username:   "wrong-username",
						Groups: []string{
							groupServiceAccounts,
							groupServiceAccountsMachineConfig,
							groupAuthenticated,
						},
						Usages: []certv1.KeyUsage{
							certv1.UsageDigitalSignature,
							certv1.UsageClientAuth,
						},
						Request: csrBytes,
					},
				}

				err := ValidateBootstrapCSR(csr, hostname)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("invalid username"))
			})
		})

		Context("When CSR is missing required groups", func() {
			It("should fail validation", func() {
				hostname := testHostname
				csrBytes := createTestCSRWithOrganization("system:node:"+hostname, "system:nodes")

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-missing-groups",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerBootstrap,
						Username:   BootstrapperUsername,
						Groups: []string{
							groupAuthenticated, // Missing other groups
						},
						Usages: []certv1.KeyUsage{
							certv1.UsageDigitalSignature,
							certv1.UsageClientAuth,
						},
						Request: csrBytes,
					},
				}

				err := ValidateBootstrapCSR(csr, hostname)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("missing required group"))
			})
		})

		Context("When CSR is missing required usages", func() {
			It("should fail validation", func() {
				hostname := testHostname
				csrBytes := createTestCSRWithOrganization("system:node:"+hostname, "system:nodes")

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-missing-usages",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerBootstrap,
						Username:   BootstrapperUsername,
						Groups: []string{
							groupServiceAccounts,
							groupServiceAccountsMachineConfig,
							groupAuthenticated,
						},
						Usages: []certv1.KeyUsage{
							certv1.UsageDigitalSignature, // Missing client auth
						},
						Request: csrBytes,
					},
				}

				err := ValidateBootstrapCSR(csr, hostname)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("missing required usage"))
			})
		})

		Context("When CSR has invalid organization", func() {
			It("should fail validation", func() {
				hostname := testHostname
				csrBytes := createTestCSRWithOrganization("system:node:"+hostname, "wrong-org")

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-invalid-org",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerBootstrap,
						Username:   BootstrapperUsername,
						Groups: []string{
							groupServiceAccounts,
							groupServiceAccountsMachineConfig,
							groupAuthenticated,
						},
						Usages: []certv1.KeyUsage{
							certv1.UsageDigitalSignature,
							certv1.UsageClientAuth,
						},
						Request: csrBytes,
					},
				}

				err := ValidateBootstrapCSR(csr, hostname)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("invalid organization"))
			})
		})
	})

	Describe("ValidateServingCSR", func() {
		Context("When CSR has valid serving configuration", func() {
			It("should pass validation", func() {
				hostname := testHostname
				csrBytes := createServingCSRWithDNS("system:node:"+hostname, hostname)

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-serving-csr",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerServing,
						Username:   "system:node:" + hostname,
						Groups: []string{
							groupNodes,
							groupAuthenticated,
						},
						Usages: []certv1.KeyUsage{
							certv1.UsageDigitalSignature,
							certv1.UsageServerAuth,
						},
						Request: csrBytes,
					},
				}

				err := ValidateServingCSR(csr, hostname)
				Expect(err).NotTo(HaveOccurred())
			})
		})

		Context("When CSR has invalid username", func() {
			It("should fail validation", func() {
				hostname := testHostname
				csrBytes := createServingCSRWithDNS("system:node:"+hostname, hostname)

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-invalid-username",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerServing,
						Username:   "wrong-username",
						Groups: []string{
							groupNodes,
							groupAuthenticated,
						},
						Usages: []certv1.KeyUsage{
							certv1.UsageDigitalSignature,
							certv1.UsageServerAuth,
						},
						Request: csrBytes,
					},
				}

				err := ValidateServingCSR(csr, hostname)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("invalid username"))
			})
		})

		Context("When CSR is missing system:nodes group", func() {
			It("should fail validation", func() {
				hostname := testHostname
				csrBytes := createServingCSRWithDNS("system:node:"+hostname, hostname)

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-missing-nodes-group",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerServing,
						Username:   "system:node:" + hostname,
						Groups: []string{
							groupAuthenticated, // Missing system:nodes
						},
						Usages: []certv1.KeyUsage{
							certv1.UsageDigitalSignature,
							certv1.UsageServerAuth,
						},
						Request: csrBytes,
					},
				}

				err := ValidateServingCSR(csr, hostname)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("missing required group: system:nodes"))
			})
		})

		Context("When CSR is missing required usages", func() {
			It("should fail validation", func() {
				hostname := testHostname
				csrBytes := createServingCSRWithDNS("system:node:"+hostname, hostname)

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-missing-usages",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerServing,
						Username:   "system:node:" + hostname,
						Groups: []string{
							groupNodes,
							groupAuthenticated,
						},
						Usages: []certv1.KeyUsage{
							certv1.UsageDigitalSignature, // Missing server auth
						},
						Request: csrBytes,
					},
				}

				err := ValidateServingCSR(csr, hostname)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("missing required usage"))
			})
		})

		Context("When CSR has wrong hostname in DNS SAN", func() {
			It("should fail validation", func() {
				hostname := testHostname
				csrBytes := createServingCSRWithDNS("system:node:"+hostname, "wrong-hostname")

				csr := &certv1.CertificateSigningRequest{
					ObjectMeta: metav1.ObjectMeta{
						Name: "test-wrong-dns-name",
					},
					Spec: certv1.CertificateSigningRequestSpec{
						SignerName: SignerServing,
						Username:   "system:node:" + hostname,
						Groups: []string{
							groupNodes,
							groupAuthenticated,
						},
						Usages: []certv1.KeyUsage{
							certv1.UsageDigitalSignature,
							certv1.UsageServerAuth,
						},
						Request: csrBytes,
					},
				}

				err := ValidateServingCSR(csr, hostname)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("does not match hostname"))
			})
		})
	})
})
