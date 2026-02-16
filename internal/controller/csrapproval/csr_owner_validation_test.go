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
	"context"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
)

var _ = Describe("CSR Owner Validation - DPU Existence Check", func() {
	var (
		ctx          context.Context
		mgmtClient   client.Client
		dpuNamespace string
		testHostname string
	)

	BeforeEach(func() {
		ctx = context.Background()
		mgmtClient = k8sClient
		dpuNamespace = "test-dpu-namespace"
		testHostname = "test-dpu-node"
	})

	Describe("DPU Existence Check", func() {
		Context("when DPU exists with matching name", func() {
			It("should find DPU with exact name match", func() {
				// Create DPU with name matching hostname
				// Note: Implementation matches by DPU object name, not Spec fields
				dpu := &dpuprovisioningv1alpha1.DPU{
					ObjectMeta: metav1.ObjectMeta{
						Name:      testHostname,
						Namespace: dpuNamespace,
					},
					Spec: dpuprovisioningv1alpha1.DPUSpec{
						DPUNodeName:   "node-1",
						DPUDeviceName: "device-1",
						BFB:           "bfb-1",
						SerialNumber:  "SN123456",
					},
				}
				Expect(mgmtClient.Create(ctx, dpu)).To(Succeed())
				DeferCleanup(mgmtClient.Delete, ctx, dpu)

				// Create validator (using nil for hcClient since we're only testing DPU lookup)
				validator := NewValidator(mgmtClient, nil, dpuNamespace)

				// Check if DPU exists
				exists, err := validator.dpuExists(ctx, testHostname)
				Expect(err).NotTo(HaveOccurred())
				Expect(exists).To(BeTrue())
			})
		})

		Context("when DPU does not exist", func() {
			It("should not find DPU when none exists", func() {
				// Don't create any DPU

				validator := NewValidator(mgmtClient, nil, dpuNamespace)

				// Check if DPU exists
				exists, err := validator.dpuExists(ctx, testHostname)
				Expect(err).NotTo(HaveOccurred())
				Expect(exists).To(BeFalse())
			})

			It("should not find DPU in different namespace", func() {
				// Create DPU in different namespace
				dpu := &dpuprovisioningv1alpha1.DPU{
					ObjectMeta: metav1.ObjectMeta{
						Name:      testHostname,
						Namespace: "wrong-namespace",
					},
					Spec: dpuprovisioningv1alpha1.DPUSpec{
						DPUNodeName:   "node-1",
						DPUDeviceName: "device-1",
						BFB:           "bfb-1",
						SerialNumber:  "SN123456",
					},
				}
				Expect(mgmtClient.Create(ctx, dpu)).To(Succeed())
				DeferCleanup(mgmtClient.Delete, ctx, dpu)

				validator := NewValidator(mgmtClient, nil, dpuNamespace)

				// Check if DPU exists in test namespace (should not find it)
				exists, err := validator.dpuExists(ctx, testHostname)
				Expect(err).NotTo(HaveOccurred())
				Expect(exists).To(BeFalse())
			})

			It("should not find DPU with different name", func() {
				// Create DPU with different object name
				dpu := &dpuprovisioningv1alpha1.DPU{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "dpu-123",
						Namespace: dpuNamespace,
					},
					Spec: dpuprovisioningv1alpha1.DPUSpec{
						DPUNodeName:   "node-1",
						DPUDeviceName: "device-1",
						BFB:           "bfb-1",
						SerialNumber:  "SN123456",
					},
				}
				Expect(mgmtClient.Create(ctx, dpu)).To(Succeed())
				DeferCleanup(mgmtClient.Delete, ctx, dpu)

				validator := NewValidator(mgmtClient, nil, dpuNamespace)

				// Check if DPU exists with test hostname (should not match)
				exists, err := validator.dpuExists(ctx, testHostname)
				Expect(err).NotTo(HaveOccurred())
				Expect(exists).To(BeFalse())
			})
		})

		Context("when multiple DPUs exist", func() {
			It("should find the correct DPU among multiple", func() {
				// Create multiple DPUs
				dpu1 := &dpuprovisioningv1alpha1.DPU{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "dpu-1",
						Namespace: dpuNamespace,
					},
					Spec: dpuprovisioningv1alpha1.DPUSpec{
						DPUNodeName:   "node-1",
						DPUDeviceName: "device-1",
						BFB:           "bfb-1",
						SerialNumber:  "SN111111",
					},
				}
				dpu2 := &dpuprovisioningv1alpha1.DPU{
					ObjectMeta: metav1.ObjectMeta{
						Name:      testHostname, // This one matches our search
						Namespace: dpuNamespace,
					},
					Spec: dpuprovisioningv1alpha1.DPUSpec{
						DPUNodeName:   "node-2",
						DPUDeviceName: "device-2",
						BFB:           "bfb-2",
						SerialNumber:  "SN222222",
					},
				}
				dpu3 := &dpuprovisioningv1alpha1.DPU{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "dpu-3",
						Namespace: dpuNamespace,
					},
					Spec: dpuprovisioningv1alpha1.DPUSpec{
						DPUNodeName:   "node-3",
						DPUDeviceName: "device-3",
						BFB:           "bfb-3",
						SerialNumber:  "SN333333",
					},
				}
				Expect(mgmtClient.Create(ctx, dpu1)).To(Succeed())
				DeferCleanup(mgmtClient.Delete, ctx, dpu1)
				Expect(mgmtClient.Create(ctx, dpu2)).To(Succeed())
				DeferCleanup(mgmtClient.Delete, ctx, dpu2)
				Expect(mgmtClient.Create(ctx, dpu3)).To(Succeed())
				DeferCleanup(mgmtClient.Delete, ctx, dpu3)

				validator := NewValidator(mgmtClient, nil, dpuNamespace)

				// Check if DPU exists (should find dpu-2 by matching object name)
				exists, err := validator.dpuExists(ctx, testHostname)
				Expect(err).NotTo(HaveOccurred())
				Expect(exists).To(BeTrue())
			})
		})
	})
})
