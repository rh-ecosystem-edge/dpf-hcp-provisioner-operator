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

package hostedcluster

import (
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

var _ = Describe("HostedCluster Builder", func() {
	var (
		hm *HostedClusterManager
		cr *provisioningv1alpha1.DPFHCPProvisioner
	)

	BeforeEach(func() {
		hm = &HostedClusterManager{}
		cr = &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-provisioner",
				Namespace: "default",
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
				OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
				BaseDomain:                     "example.com",
				EtcdStorageClass:               "ceph-rbd",
				ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
				VirtualIP:                      "192.168.1.100", // Default to LoadBalancer mode for tests
			},
		}
	})

	Context("Basic HostedCluster Fields", func() {
		It("should set correct metadata", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Name).To(Equal("test-provisioner"))
			Expect(hc.Namespace).To(Equal("default"))
		})

		It("should set release image from DPFHCPProvisioner spec", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.Release.Image).To(Equal(cr.Spec.OCPReleaseImage))
		})

		It("should reference correct secret names", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.PullSecret.Name).To(Equal("test-provisioner-pull-secret"))
			Expect(hc.Spec.SSHKey.Name).To(Equal("test-provisioner-ssh-key"))
		})

		It("should set DNS base domain", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.DNS.BaseDomain).To(Equal("example.com"))
		})

		It("should set platform to None", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.Platform.Type).To(Equal(hyperv1.NonePlatform))
		})
	})

	Context("ETCD Configuration", func() {
		It("should configure managed ETCD with persistent volume", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.Etcd.ManagementType).To(Equal(hyperv1.Managed))
			Expect(hc.Spec.Etcd.Managed).ToNot(BeNil())
			Expect(hc.Spec.Etcd.Managed.Storage.Type).To(Equal(hyperv1.PersistentVolumeEtcdStorage))
			Expect(hc.Spec.Etcd.Managed.Storage.PersistentVolume).ToNot(BeNil())
		})

		It("should use storage class from DPFHCPProvisioner spec", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(*hc.Spec.Etcd.Managed.Storage.PersistentVolume.StorageClassName).To(Equal("ceph-rbd"))
		})

		It("should set ETCD volume size to 8Gi", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.Etcd.Managed.Storage.PersistentVolume.Size.String()).To(Equal("8Gi"))
		})
	})

	Context("Network Configuration", func() {
		It("should set network type to Other", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.Networking.NetworkType).To(Equal(hyperv1.Other))
		})

		It("should set default service network CIDR", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.Networking.ServiceNetwork).To(HaveLen(1))
			Expect(hc.Spec.Networking.ServiceNetwork[0].CIDR.String()).To(Equal("172.31.0.0/16"))
		})

		It("should set default cluster network CIDR", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.Networking.ClusterNetwork).To(HaveLen(1))
			Expect(hc.Spec.Networking.ClusterNetwork[0].CIDR.String()).To(Equal("10.132.0.0/14"))
		})

		It("should have empty machine network", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.Networking.MachineNetwork).To(BeEmpty())
		})
	})

	Context("Availability Policies", func() {
		It("should set SingleReplica when specified", func() {
			cr.Spec.ControlPlaneAvailabilityPolicy = hyperv1.SingleReplica

			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.ControllerAvailabilityPolicy).To(Equal(hyperv1.SingleReplica))
		})

		It("should set HighlyAvailable when specified", func() {
			cr.Spec.ControlPlaneAvailabilityPolicy = hyperv1.HighlyAvailable

			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.ControllerAvailabilityPolicy).To(Equal(hyperv1.HighlyAvailable))
		})
	})

	Context("Secret Encryption", func() {
		It("should configure AESCBC encryption", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.SecretEncryption).ToNot(BeNil())
			Expect(hc.Spec.SecretEncryption.Type).To(Equal(hyperv1.AESCBC))
			Expect(hc.Spec.SecretEncryption.AESCBC).ToNot(BeNil())
		})

		It("should reference ETCD encryption key secret", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.SecretEncryption.AESCBC.ActiveKey.Name).To(Equal("test-provisioner-etcd-encryption-key"))
		})
	})

	Context("Service Publishing Strategy", func() {
		It("should configure 4 services in LoadBalancer mode", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.Services).To(HaveLen(4))
		})

		It("should use LoadBalancer for APIServer", func() {
			hc := hm.buildHostedCluster(cr, "")

			apiServerStrategy := findServiceStrategy(hc.Spec.Services, hyperv1.APIServer)
			Expect(apiServerStrategy).ToNot(BeNil())
			Expect(apiServerStrategy.Type).To(Equal(hyperv1.LoadBalancer))
		})

		It("should use Route for OAuthServer", func() {
			hc := hm.buildHostedCluster(cr, "")

			oauthStrategy := findServiceStrategy(hc.Spec.Services, hyperv1.OAuthServer)
			Expect(oauthStrategy).ToNot(BeNil())
			Expect(oauthStrategy.Type).To(Equal(hyperv1.Route))
		})

		It("should use Route for Konnectivity", func() {
			hc := hm.buildHostedCluster(cr, "")

			konnectivityStrategy := findServiceStrategy(hc.Spec.Services, hyperv1.Konnectivity)
			Expect(konnectivityStrategy).ToNot(BeNil())
			Expect(konnectivityStrategy.Type).To(Equal(hyperv1.Route))
		})

		It("should use Route for Ignition", func() {
			hc := hm.buildHostedCluster(cr, "")

			ignitionStrategy := findServiceStrategy(hc.Spec.Services, hyperv1.Ignition)
			Expect(ignitionStrategy).ToNot(BeNil())
			Expect(ignitionStrategy.Type).To(Equal(hyperv1.Route))
		})
	})

	Context("InfraID Generation", func() {
		It("should generate non-empty infraID", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.InfraID).ToNot(BeEmpty())
		})

		It("should generate infraID with cluster name prefix", func() {
			hc := hm.buildHostedCluster(cr, "")

			Expect(hc.Spec.InfraID).To(HavePrefix("test-provisioner-"))
		})

		It("should generate infraID with random suffix", func() {
			hc1 := hm.buildHostedCluster(cr, "")
			hc2 := hm.buildHostedCluster(cr, "")

			// InfraID includes random suffix, so they should be different
			Expect(hc1.Spec.InfraID).ToNot(Equal(hc2.Spec.InfraID))
			// But both should start with the cluster name
			Expect(hc1.Spec.InfraID).To(HavePrefix("test-provisioner-"))
			Expect(hc2.Spec.InfraID).To(HavePrefix("test-provisioner-"))
		})
	})
})

// Helper function to find strategy for a specific service
func findServiceStrategy(strategies []hyperv1.ServicePublishingStrategyMapping, service hyperv1.ServiceType) *hyperv1.ServicePublishingStrategy {
	for _, s := range strategies {
		if s.Service == service {
			return &s.ServicePublishingStrategy
		}
	}
	return nil
}
