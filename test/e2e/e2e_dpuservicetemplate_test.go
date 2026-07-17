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
	"context"
	"encoding/json"
	"time"

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
)

const (
	managedByLabel = "dpfhcpprovisioner.dpu.hcp.io/managed"

	generalTemplateReconcileTimeout  = 2 * time.Minute
	generalTemplateReconcileInterval = 5 * time.Second
)

var templateNames = []string{"ovn", "doca-telemetry-service", "hbn"}

var _ = Describe("DPUServiceTemplate E2E", Ordered, func() {

	BeforeAll(func() {
		By("creating stub resources for DPUServiceTemplate controller")
		if !testingOnOCP {
			// On Kind, OpenShift resources don't exist, so we create stubs.
			// On real OCP they already exist and must not be overwritten.
			createPullSecretStub()
			createClusterVersionStub(stubOCPVersion, stubReleaseImage)
			createOVNDaemonSetStub(stubOVNImage)
			createDummySecrets(ciNamespace, sshKeySecretName, pullSecretName)
		}

		By("creating DPUCluster namespace and stub")
		createNamespace(dpuClusterNS)
		createDPUClusterStub(dpuClusterNS, dpuClusterName)

		By("creating DPUFlavor stub")
		createDPUFlavorStub(dpuClusterNS, dpuFlavorName)

		By("creating DPUDeployment stub")
		createDPUDeploymentStub(dpuClusterNS, dpuDeploymentName, dpuFlavorName)

		By("creating DPFOperatorConfig with version")
		setDPFOperatorConfigVersion(stubDPFVersion)

		By("enabling DPUServiceTemplate management")
		enableManageDPUServiceTemplates()

		By("creating DPFHCPProvisioner to trigger template creation")
		createDPFHCPProvisioner(ciNamespace, provisionerName)
	})

	AfterAll(func() {
		By("cleaning up DPUServiceTemplate test resources")
		cleanupTemplateTestResources()
	})

	Context("Template Lifecycle", func() {
		It("should create all three DPUServiceTemplates when provisioner exists", func() {
			for _, name := range templateNames {
				By("waiting for template " + name)
				Eventually(func(g Gomega) {
					template := &dpuservicev1alpha1.DPUServiceTemplate{}
					err := k8sClient.Get(context.Background(), types.NamespacedName{
						Name: name, Namespace: dpuClusterNS,
					}, template)
					g.Expect(err).NotTo(HaveOccurred(), "Template %s not found", name)
					g.Expect(template.Labels[managedByLabel]).To(Equal("true"),
						"Template %s should have managed-by label", name)
				}, generalTemplateReconcileTimeout, generalTemplateReconcileInterval).Should(Succeed())
			}
		})

		It("should have correct template structure", func() {
			for _, name := range templateNames {
				template := &dpuservicev1alpha1.DPUServiceTemplate{}
				err := k8sClient.Get(context.Background(), types.NamespacedName{
					Name: name, Namespace: dpuClusterNS,
				}, template)
				Expect(err).NotTo(HaveOccurred())

				By("verifying " + name + " has chart source")
				Expect(template.Spec.HelmChart.Source.RepoURL).NotTo(BeEmpty(),
					"Template %s should have repoURL", name)
				Expect(template.Spec.HelmChart.Source.Chart).NotTo(BeEmpty(),
					"Template %s should have chart name", name)
				Expect(template.Spec.HelmChart.Source.Version).NotTo(BeEmpty(),
					"Template %s should have chart version", name)

				By("verifying " + name + " has values")
				Expect(template.Spec.HelmChart.Values).NotTo(BeNil(),
					"Template %s should have values", name)
				var values map[string]any
				Expect(json.Unmarshal(template.Spec.HelmChart.Values.Raw, &values)).To(Succeed(),
					"Template %s values should be valid JSON", name)
			}
		})

		It("should self-heal when a template is deleted", func() {
			ctx := context.Background()
			templateToDelete := "hbn"

			By("deleting the " + templateToDelete + " template")
			template := &dpuservicev1alpha1.DPUServiceTemplate{}
			err := k8sClient.Get(ctx, types.NamespacedName{
				Name: templateToDelete, Namespace: dpuClusterNS,
			}, template)
			Expect(err).NotTo(HaveOccurred())
			originalUID := template.UID
			Expect(k8sClient.Delete(ctx, template)).To(Succeed())

			By("waiting for template to be recreated")
			Eventually(func(g Gomega) {
				err := k8sClient.Get(ctx, types.NamespacedName{
					Name: templateToDelete, Namespace: dpuClusterNS,
				}, template)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(template.UID).NotTo(Equal(originalUID), "Template should be a newly recreated object")
				g.Expect(template.Labels[managedByLabel]).To(Equal("true"))
			}, generalTemplateReconcileTimeout, generalTemplateReconcileInterval).Should(Succeed())
		})

		It("should self-heal when a template is modified", func() {
			ctx := context.Background()
			templateToModify := "doca-telemetry-service"

			By("reading current template")
			template := &dpuservicev1alpha1.DPUServiceTemplate{}
			err := k8sClient.Get(ctx, types.NamespacedName{
				Name: templateToModify, Namespace: dpuClusterNS,
			}, template)
			Expect(err).NotTo(HaveOccurred())
			originalVersion := template.Spec.HelmChart.Source.Version

			By("modifying the template chart version")
			template.Spec.HelmChart.Source.Version = "0.0.0-tampered"
			Expect(k8sClient.Update(ctx, template)).To(Succeed())

			By("waiting for template to be corrected")
			Eventually(func(g Gomega) {
				err := k8sClient.Get(ctx, types.NamespacedName{
					Name: templateToModify, Namespace: dpuClusterNS,
				}, template)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(template.Spec.HelmChart.Source.Version).To(Equal(originalVersion),
					"Chart version should be restored")
			}, generalTemplateReconcileTimeout, generalTemplateReconcileInterval).Should(Succeed())
		})

		It("should delete templates when provisioner is deleted", func() {
			ctx := context.Background()

			By("verifying templates exist before deletion")
			for _, name := range templateNames {
				template := &dpuservicev1alpha1.DPUServiceTemplate{}
				err := k8sClient.Get(ctx, types.NamespacedName{
					Name: name, Namespace: dpuClusterNS,
				}, template)
				Expect(err).NotTo(HaveOccurred(), "Template %s should exist before provisioner deletion", name)
			}

			By("deleting the DPFHCPProvisioner")
			forceDeleteProvisioner(ciNamespace, provisionerName)

			By("verifying all templates are deleted")
			Eventually(func(g Gomega) {
				var list dpuservicev1alpha1.DPUServiceTemplateList
				err := k8sClient.List(ctx, &list,
					client.InNamespace(dpuClusterNS),
					client.MatchingLabels{managedByLabel: "true"},
				)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(list.Items).To(BeEmpty(), "All managed templates should be deleted")
			}, generalTemplateReconcileTimeout, generalTemplateReconcileInterval).Should(Succeed())
		})
	})
})
