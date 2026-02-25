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
	"testing"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/client-go/kubernetes"
	kubefake "k8s.io/client-go/kubernetes/fake"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
)

var (
	k8sClient     client.Client
	fakeClientset kubernetes.Interface
	scheme        *runtime.Scheme
)

func TestCSRApproval(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecs(t, "CSR Approval Suite")
}

var _ = BeforeSuite(func() {
	// Create scheme and register DPU types
	scheme = runtime.NewScheme()
	err := dpuprovisioningv1alpha1.AddToScheme(scheme)
	Expect(err).NotTo(HaveOccurred())

	// Create fake client for tests
	k8sClient = fake.NewClientBuilder().WithScheme(scheme).Build()

	// Create fake Kubernetes clientset for node operations
	fakeClientset = kubefake.NewSimpleClientset()
})

// createFakeNode creates a fake Kubernetes Node object for testing
func createFakeNode(name string) *corev1.Node {
	return &corev1.Node{
		ObjectMeta: metav1.ObjectMeta{
			Name: name,
		},
		Status: corev1.NodeStatus{
			Phase: corev1.NodeRunning,
		},
	}
}
