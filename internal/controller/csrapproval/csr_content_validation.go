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
	"net"

	certv1 "k8s.io/api/certificates/v1"
	"k8s.io/client-go/util/cert"
)

const (
	// BootstrapperUsername is the expected username for bootstrap CSRs
	BootstrapperUsername = "system:serviceaccount:openshift-machine-config-operator:node-bootstrapper"

	// Expected groups for bootstrap CSRs (from service account)
	groupServiceAccounts              = "system:serviceaccounts"
	groupServiceAccountsMachineConfig = "system:serviceaccounts:openshift-machine-config-operator"

	// Expected groups for serving CSRs (from authenticated nodes)
	groupNodes = "system:nodes"

	// Common group required for both bootstrap and serving CSRs
	groupAuthenticated = "system:authenticated"

	// Expected organization field in certificate request (both bootstrap and serving)
	expectedOrganization = "system:nodes"
)

// ValidateBootstrapCSR performs comprehensive validation of a bootstrap CSR
func ValidateBootstrapCSR(csr *certv1.CertificateSigningRequest, hostname string) error {
	// 1. Validate username and groups (must come from node-bootstrapper)
	if err := validateBootstrapperIdentity(csr); err != nil {
		return fmt.Errorf("bootstrapper identity validation failed: %w", err)
	}

	// 2. Validate key usages
	if err := validateBootstrapUsages(csr); err != nil {
		return fmt.Errorf("key usage validation failed: %w", err)
	}

	// 3. Parse and validate certificate request
	certReq, err := parseCSRRequest(csr)
	if err != nil {
		return fmt.Errorf("failed to parse CSR: %w", err)
	}

	// 4. Validate organization field
	if err := validateOrganization(certReq); err != nil {
		return fmt.Errorf("organization validation failed: %w", err)
	}

	// 5. Validate CN matches hostname
	if err := validateCN(certReq, hostname); err != nil {
		return fmt.Errorf("CN validation failed: %w", err)
	}

	// 6. Bootstrap CSRs must not contain any SANs (client-auth certificates)
	if len(certReq.DNSNames) > 0 || len(certReq.IPAddresses) > 0 ||
		len(certReq.EmailAddresses) > 0 || len(certReq.URIs) > 0 {
		return fmt.Errorf("bootstrap CSR must not contain SANs (DNS=%v, IP=%v, email=%v, URI=%v)",
			certReq.DNSNames, certReq.IPAddresses, certReq.EmailAddresses, certReq.URIs)
	}

	return nil
}

// ValidateServingCSR performs comprehensive validation of a serving CSR
func ValidateServingCSR(csr *certv1.CertificateSigningRequest, hostname string) error {
	// 1. Validate username and groups (must come from the node itself)
	if err := validateNodeIdentity(csr, hostname); err != nil {
		return fmt.Errorf("node identity validation failed: %w", err)
	}

	// 2. Validate key usages
	if err := validateServingUsages(csr); err != nil {
		return fmt.Errorf("key usage validation failed: %w", err)
	}

	// 3. Parse and validate certificate request
	certReq, err := parseCSRRequest(csr)
	if err != nil {
		return fmt.Errorf("failed to parse CSR: %w", err)
	}

	// 4. Validate organization field
	if err := validateOrganization(certReq); err != nil {
		return fmt.Errorf("organization validation failed: %w", err)
	}

	// 5. Validate CN matches hostname
	if err := validateCN(certReq, hostname); err != nil {
		return fmt.Errorf("CN validation failed: %w", err)
	}

	// 6. Validate DNS names and IP addresses
	if err := validateServingIdentities(certReq, hostname); err != nil {
		return fmt.Errorf("serving identities validation failed: %w", err)
	}

	return nil
}

// validateBootstrapperIdentity validates that bootstrap CSR comes from node-bootstrapper service account
func validateBootstrapperIdentity(csr *certv1.CertificateSigningRequest) error {
	// Check username
	if csr.Spec.Username != BootstrapperUsername {
		return fmt.Errorf("invalid username: expected %s, got %s", BootstrapperUsername, csr.Spec.Username)
	}

	// Check groups
	requiredGroups := map[string]bool{
		groupServiceAccounts:              false,
		groupServiceAccountsMachineConfig: false,
		groupAuthenticated:                false,
	}

	for _, group := range csr.Spec.Groups {
		if _, ok := requiredGroups[group]; ok {
			requiredGroups[group] = true
		}
	}

	// Verify all required groups are present
	for group, found := range requiredGroups {
		if !found {
			return fmt.Errorf("missing required group: %s", group)
		}
	}

	return nil
}

// validateNodeIdentity validates that serving CSR comes from the node itself
func validateNodeIdentity(csr *certv1.CertificateSigningRequest, hostname string) error {
	// Check username matches expected format
	expectedUsername := SystemNodePrefix + hostname
	if csr.Spec.Username != expectedUsername {
		return fmt.Errorf("invalid username: expected %s, got %s", expectedUsername, csr.Spec.Username)
	}

	// Check groups include system:nodes
	hasNodesGroup := false
	hasAuthenticatedGroup := false
	for _, group := range csr.Spec.Groups {
		if group == groupNodes {
			hasNodesGroup = true
		}
		if group == groupAuthenticated {
			hasAuthenticatedGroup = true
		}
	}

	if !hasNodesGroup {
		return fmt.Errorf("missing required group: %s", groupNodes)
	}
	if !hasAuthenticatedGroup {
		return fmt.Errorf("missing required group: %s", groupAuthenticated)
	}

	return nil
}

// validateUsages validates key usages against an allowed set
// Requires exact match - no extra usages allowed
// The allowedUsages map should have all required usages with value false
func validateUsages(csr *certv1.CertificateSigningRequest, allowedUsages map[certv1.KeyUsage]bool) error {
	// Check each usage in CSR
	for _, usage := range csr.Spec.Usages {
		if _, ok := allowedUsages[usage]; !ok {
			// Disallowed usage present
			return fmt.Errorf("disallowed usage present: %s", usage)
		}
		allowedUsages[usage] = true
	}

	// Verify all required usages are present
	for usage, found := range allowedUsages {
		if !found {
			return fmt.Errorf("missing required usage: %s", usage)
		}
	}

	return nil
}

// validateBootstrapUsages validates key usages for bootstrap CSRs
// Based on actual environment: digital signature + client auth
// Requires exact match - no extra usages allowed
func validateBootstrapUsages(csr *certv1.CertificateSigningRequest) error {
	allowedUsages := map[certv1.KeyUsage]bool{
		certv1.UsageDigitalSignature: false,
		certv1.UsageClientAuth:       false,
	}
	return validateUsages(csr, allowedUsages)
}

// validateServingUsages validates key usages for serving CSRs
// Based on actual environment: digital signature + server auth
// Requires exact match - no extra usages allowed
func validateServingUsages(csr *certv1.CertificateSigningRequest) error {
	allowedUsages := map[certv1.KeyUsage]bool{
		certv1.UsageDigitalSignature: false,
		certv1.UsageServerAuth:       false,
	}
	return validateUsages(csr, allowedUsages)
}

// parseCSRRequest parses the X509 certificate request from a CSR
func parseCSRRequest(csr *certv1.CertificateSigningRequest) (*x509.CertificateRequest, error) {
	// csr.Spec.Request is already a []byte containing PEM-encoded certificate request
	// The Kubernetes Go client handles base64 decoding from JSON/YAML for us
	// So we can use it directly without additional base64 decoding

	// Parse PEM block
	pemBlock, _ := pem.Decode(csr.Spec.Request)
	if pemBlock == nil {
		return nil, fmt.Errorf("failed to decode PEM block from CSR request")
	}
	if pemBlock.Type != cert.CertificateRequestBlockType {
		return nil, fmt.Errorf("unexpected PEM block type %q, expected %s", pemBlock.Type, cert.CertificateRequestBlockType)
	}

	// Parse X509 certificate request
	certReq, err := x509.ParseCertificateRequest(pemBlock.Bytes)
	if err != nil {
		return nil, fmt.Errorf("failed to parse X509 certificate request: %w", err)
	}

	return certReq, nil
}

// validateOrganization validates that the CSR has the correct organization field
func validateOrganization(certReq *x509.CertificateRequest) error {
	if len(certReq.Subject.Organization) != 1 {
		return fmt.Errorf("expected exactly one organization, got %d", len(certReq.Subject.Organization))
	}

	if certReq.Subject.Organization[0] != expectedOrganization {
		return fmt.Errorf("invalid organization: expected %s, got %s", expectedOrganization, certReq.Subject.Organization[0])
	}

	return nil
}

// validateCN validates that the CN matches the expected hostname format
func validateCN(certReq *x509.CertificateRequest, hostname string) error {
	expectedCN := SystemNodePrefix + hostname
	if certReq.Subject.CommonName != expectedCN {
		return fmt.Errorf("invalid CN: expected %s, got %s", expectedCN, certReq.Subject.CommonName)
	}

	return nil
}

// validateServingIdentities validates DNS names and IP addresses in serving CSR
// Serving CSRs must carry exactly one DNS SAN equal to the node hostname
func validateServingIdentities(certReq *x509.CertificateRequest, hostname string) error {
	// Serving CSRs must contain exactly one DNS SAN
	if len(certReq.DNSNames) != 1 {
		return fmt.Errorf("CSR must contain exactly one DNS SAN, got %d: %v", len(certReq.DNSNames), certReq.DNSNames)
	}

	// The DNS SAN must match the hostname
	if certReq.DNSNames[0] != hostname {
		return fmt.Errorf("CSR DNS SAN %q does not match hostname %q", certReq.DNSNames[0], hostname)
	}

	// Validate that IP addresses are valid (if any)
	for _, ip := range certReq.IPAddresses {
		if !hasValidIPAddress(ip) {
			return fmt.Errorf("CSR contains invalid IP address: %v", ip)
		}
	}

	return nil
}

// hasValidIPAddress checks if an IP address is valid for a node
func hasValidIPAddress(ip net.IP) bool {
	if ip == nil {
		return false
	}
	// Reject unspecified, loopback, and multicast addresses
	return !ip.IsUnspecified() && !ip.IsLoopback() && !ip.IsMulticast()
}
