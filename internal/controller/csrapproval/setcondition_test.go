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

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
)

var _ = Describe("setCondition", func() {
	var (
		ctx        context.Context
		cr         *provisioningv1alpha1.DPFHCPProvisioner
		testScheme *runtime.Scheme
	)

	BeforeEach(func() {
		ctx = context.Background()

		testScheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(testScheme)).To(Succeed())

		cr = &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "doca",
				Namespace: "clusters",
			},
		}
	})

	newApprover := func(c client.Client) *CSRApprover {
		return &CSRApprover{
			mgmtClient: c,
			recorder:   record.NewFakeRecorder(10),
		}
	}

	It("succeeds when there is no conflict", func() {
		c := fake.NewClientBuilder().
			WithScheme(testScheme).
			WithObjects(cr).
			WithStatusSubresource(cr).
			Build()

		err := newApprover(c).setCondition(ctx, cr, metav1.ConditionTrue, provisioningv1alpha1.ReasonCSRApprovalActive, "active")
		Expect(err).NotTo(HaveOccurred())
	})

	It("retries and succeeds after a single conflict", func() {
		// The fake client enforces resourceVersion optimistic concurrency — if you hold a stale
		// copy of the object and another writer updates it first, the fake client's own tracker
		// returns the conflict error as a real API server.
		//
		// Race reproduced here:
		//   1. CSR controller fetches the CR (stale copy, resourceVersion N)
		//   2. Main controller updates the CR status (resourceVersion becomes N+1)
		//   3. CSR controller tries Status().Update with resourceVersion N → conflict
		//   4. RetryOnConflict re-fetches (gets N+1) and retries → success
		//
		// Before the RetryOnConflict fix, step 3 propagated the conflict error directly out and the
		// reconciler logged "Failed to process CSRs" on every 30-second poll.
		c := fake.NewClientBuilder().
			WithScheme(testScheme).
			WithObjects(cr).
			WithStatusSubresource(cr).
			Build()

		// Step 1: CSR controller fetches the CR — this is the stale copy it will use
		staleCR := cr.DeepCopy()
		Expect(c.Get(ctx, client.ObjectKeyFromObject(cr), staleCR)).To(Succeed())

		// Step 2: main controller updates the CR concurrently, incrementing the resourceVersion
		concurrentCR := staleCR.DeepCopy()
		concurrentCR.Status.Conditions = append(concurrentCR.Status.Conditions, metav1.Condition{
			Type:               "SomeOtherCondition",
			Status:             metav1.ConditionTrue,
			Reason:             "UpdatedByMainController",
			LastTransitionTime: metav1.Now(),
		})
		Expect(c.Status().Update(ctx, concurrentCR)).To(Succeed())

		// Step 3+4: setCondition is called with the stale copy — the fake client produces a real
		// conflict on the first attempt, then RetryOnConflict re-fetches and succeeds
		err := newApprover(c).setCondition(ctx, staleCR, metav1.ConditionTrue, provisioningv1alpha1.ReasonCSRApprovalActive, "active")
		Expect(err).NotTo(HaveOccurred())
	})

})
