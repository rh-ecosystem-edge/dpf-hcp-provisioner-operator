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

package e2e

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
	"testing"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/test/utils"
)

// imageRepository extracts the repository from a full image reference.
// e.g. "quay.io/org/image:tag" -> "quay.io/org/image"
func imageRepository(image string) string {
	if i := strings.LastIndex(image, ":"); i > 0 {
		afterColon := image[i+1:]
		if !strings.Contains(afterColon, "/") {
			return image[:i]
		}
	}
	return image
}

// imageTag extracts the tag from a full image reference.
// e.g. "quay.io/org/image:tag" -> "tag"
// Returns "latest" if no tag is specified.
func imageTag(image string) string {
	if i := strings.LastIndex(image, ":"); i > 0 {
		afterColon := image[i+1:]
		if !strings.Contains(afterColon, "/") {
			return afterColon
		}
	}
	return "latest"
}

func TestE2E(t *testing.T) {
	RegisterFailHandler(Fail)
	_, _ = fmt.Fprintf(GinkgoWriter, "Starting dpf-hcp-provisioner-operator e2e test suite\n")
	RunSpecs(t, "e2e suite")
}

var _ = BeforeSuite(func() {
	By("deploying HyperShift operator on management cluster")
	cmd := exec.Command("make", "e2e-deploy-hypershift")
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to deploy HyperShift operator")

	By("installing external DPF CRDs")
	cmd = exec.Command("make", "e2e-install-dpf-crds")
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to install DPF CRDs")

	// Determine operator image and chart.
	// In CI, IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI is injected automatically via the
	// workflow dependencies block. For local testing, set it manually:
	//   export IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI=quay.io/your-org/your-image:tag
	operatorImage := os.Getenv("IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI")
	ExpectWithOffset(1, operatorImage).NotTo(BeEmpty(),
		"IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI must be set. "+
			"In CI this is injected automatically. "+
			"For local testing, export it with your operator image.")

	operatorChart := os.Getenv("OPERATOR_HELM_CHART")
	if operatorChart == "" {
		operatorChart = "helm/dpf-hcp-provisioner-operator"
	}
	_, _ = fmt.Fprintf(GinkgoWriter, "Using operator image: %s\n", operatorImage)
	_, _ = fmt.Fprintf(GinkgoWriter, "Using helm chart: %s\n", operatorChart)

	By("deploying the operator via helm chart")
	cmd = exec.Command("helm", "upgrade", "--install",
		"dpf-hcp-provisioner-operator", operatorChart,
		"--create-namespace",
		"--namespace", "dpf-hcp-provisioner-system",
		"--set", fmt.Sprintf("image.repository=%s", imageRepository(operatorImage)),
		"--set", fmt.Sprintf("image.tag=%s", imageTag(operatorImage)),
		"--set", "logLevel=debug",
		"--wait", "--timeout", "5m",
	)
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to deploy operator via helm")
})

var _ = AfterSuite(func() {
	By("undeploying the operator via helm")
	cmd := exec.Command("helm", "uninstall", "dpf-hcp-provisioner-operator",
		"--namespace", "dpf-hcp-provisioner-system")
	_, _ = utils.Run(cmd)
})
