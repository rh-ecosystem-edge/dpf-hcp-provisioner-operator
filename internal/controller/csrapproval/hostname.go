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
	"crypto/x509"
	"encoding/pem"
	"fmt"
	"strings"

	certv1 "k8s.io/api/certificates/v1"
)

const (
	// SignerBootstrap is the signer name for bootstrap CSRs
	SignerBootstrap = "kubernetes.io/kube-apiserver-client-kubelet"

	// SignerServing is the signer name for serving CSRs
	SignerServing = "kubernetes.io/kubelet-serving"

	// SystemNodePrefix is the prefix used in CSR CN and username
	SystemNodePrefix = "system:node:"
)

// ExtractHostname extracts the hostname from a CSR based on its signer type.
// For bootstrap CSRs: extracts from CN in the certificate request
// For serving CSRs: extracts from username field
func ExtractHostname(csr *certv1.CertificateSigningRequest) (string, error) {
	if csr == nil {
		return "", fmt.Errorf("CSR is nil")
	}

	switch csr.Spec.SignerName {
	case SignerBootstrap:
		return extractHostnameFromBootstrapCSR(csr)
	case SignerServing:
		return extractHostnameFromServingCSR(csr)
	default:
		return "", fmt.Errorf("unsupported signer name: %s", csr.Spec.SignerName)
	}
}

// extractHostnameFromBootstrapCSR extracts hostname from bootstrap CSR's certificate request CN
func extractHostnameFromBootstrapCSR(csr *certv1.CertificateSigningRequest) (string, error) {
	// csr.Spec.Request is already a []byte containing PEM-encoded certificate request

	// Parse PEM block
	pemBlock, _ := pem.Decode(csr.Spec.Request)
	if pemBlock == nil {
		return "", fmt.Errorf("failed to decode PEM block from CSR request")
	}

	// Parse X509 certificate request
	certReq, err := x509.ParseCertificateRequest(pemBlock.Bytes)
	if err != nil {
		return "", fmt.Errorf("failed to parse X509 certificate request: %w", err)
	}

	// Extract hostname from CN (format: "system:node:<hostname>")
	cn := certReq.Subject.CommonName
	hostname := strings.TrimPrefix(cn, SystemNodePrefix)

	if hostname == "" || hostname == cn {
		return "", fmt.Errorf("invalid CN format, expected 'system:node:<hostname>', got: %s", cn)
	}

	return hostname, nil
}

// extractHostnameFromServingCSR extracts hostname from serving CSR's username field
func extractHostnameFromServingCSR(csr *certv1.CertificateSigningRequest) (string, error) {
	// Username format: "system:node:<hostname>"
	username := csr.Spec.Username
	hostname := strings.TrimPrefix(username, SystemNodePrefix)

	if hostname == "" || hostname == username {
		return "", fmt.Errorf("invalid username format, expected 'system:node:<hostname>', got: %s", username)
	}

	return hostname, nil
}

// IsPending checks if a CSR is in pending state (no Approved or Denied condition)
func IsPending(csr *certv1.CertificateSigningRequest) bool {
	if csr == nil {
		return false
	}

	for _, condition := range csr.Status.Conditions {
		if condition.Type == certv1.CertificateApproved || condition.Type == certv1.CertificateDenied {
			return false
		}
	}
	return true
}

// FilterBySignerName filters CSRs to only include bootstrap and serving CSR types
func FilterBySignerName(csrs []certv1.CertificateSigningRequest) []certv1.CertificateSigningRequest {
	filtered := make([]certv1.CertificateSigningRequest, 0, len(csrs))

	for _, csr := range csrs {
		if csr.Spec.SignerName == SignerBootstrap || csr.Spec.SignerName == SignerServing {
			filtered = append(filtered, csr)
		}
	}

	return filtered
}
