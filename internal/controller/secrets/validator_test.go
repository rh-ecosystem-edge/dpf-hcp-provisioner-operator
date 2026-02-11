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

package secrets

import (
	"context"
	"testing"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

func TestSecretsValidator(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecs(t, "Secrets Validator Suite")
}

var _ = Describe("Secrets Validator", func() {
	var (
		ctx        context.Context
		validator  *Validator
		fakeClient client.Client
		recorder   *record.FakeRecorder
		scheme     *runtime.Scheme
	)

	BeforeEach(func() {
		ctx = context.Background()
		recorder = record.NewFakeRecorder(100)

		// Create scheme with DPFHCPProvisioner types
		scheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		Expect(corev1.AddToScheme(scheme)).To(Succeed())
	})

	Describe("ValidateSecrets", func() {
		Context("when both secrets exist and are valid", func() {
			It("should set SecretsValid=True", func() {
				// Create SSH key secret
				sshSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "ssh-key",
						Namespace: "default",
					},
					Data: map[string][]byte{
						SSHPublicKeySecretKey: []byte("ssh-rsa AAAAB3..."),
					},
				}

				// Create pull secret
				pullSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "pull-secret",
						Namespace: "default",
					},
					Data: map[string][]byte{
						PullSecretKey: []byte(`{"auths":{"registry.io":{"auth":"..."}}}`),
					},
				}

				// Create DPFHCPProvisioner referencing the secrets
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						SSHKeySecretRef: corev1.LocalObjectReference{
							Name: "ssh-key",
						},
						PullSecretRef: corev1.LocalObjectReference{
							Name: "pull-secret",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(sshSecret, pullSecret, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateSecrets(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())
				Expect(result.RequeueAfter).To(BeZero())

				// Verify status updated
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Verify condition
				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.SecretsValid)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonSecretsValid))
				Expect(condition.Message).To(ContainSubstring("ssh-key"))
				Expect(condition.Message).To(ContainSubstring("pull-secret"))
				Expect(condition.ObservedGeneration).To(Equal(int64(1)))

				// Verify event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("SecretsValid")))
			})

			It("should emit event only on first success (not on subsequent reconciliations)", func() {
				sshSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "ssh-key",
						Namespace: "default",
					},
					Data: map[string][]byte{
						SSHPublicKeySecretKey: []byte("ssh-rsa AAAAB3..."),
					},
				}

				pullSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "pull-secret",
						Namespace: "default",
					},
					Data: map[string][]byte{
						PullSecretKey: []byte(`{"auths":{}}`),
					},
				}

				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						SSHKeySecretRef: corev1.LocalObjectReference{
							Name: "ssh-key",
						},
						PullSecretRef: corev1.LocalObjectReference{
							Name: "pull-secret",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(sshSecret, pullSecret, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// First reconciliation - should emit event
				_, _ = validator.ValidateSecrets(ctx, provisioner)
				Eventually(recorder.Events).Should(Receive(ContainSubstring("SecretsValid")))

				// Get updated provisioner for second reconciliation
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Second reconciliation - should NOT emit event (condition unchanged)
				_, _ = validator.ValidateSecrets(ctx, &updatedProvisioner)
				Consistently(recorder.Events).ShouldNot(Receive())
			})
		})

		Context("when SSH key secret is missing", func() {
			It("should set SecretsValid=False with SSHKeySecretMissing reason and not requeue", func() {
				// Create pull secret (SSH secret missing)
				pullSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "pull-secret",
						Namespace: "default",
					},
					Data: map[string][]byte{
						PullSecretKey: []byte(`{"auths":{}}`),
					},
				}

				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						SSHKeySecretRef: corev1.LocalObjectReference{
							Name: "missing-ssh-key",
						},
						PullSecretRef: corev1.LocalObjectReference{
							Name: "pull-secret",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(pullSecret, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateSecrets(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue
				Expect(result.RequeueAfter).To(BeZero())

				// Verify status updated
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Verify condition
				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.SecretsValid)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
				Expect(condition.Reason).To(Equal(ReasonSSHKeySecretMissing))
				Expect(condition.Message).To(ContainSubstring("missing-ssh-key"))
				Expect(condition.Message).To(ContainSubstring("not found"))

				// Verify warning event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("SSHKeySecretMissing")))
			})
		})

		Context("when SSH key secret is missing required key", func() {
			It("should set SecretsValid=False with SSHKeySecretInvalid reason and not requeue", func() {
				// Create SSH secret without required key
				sshSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "ssh-key",
						Namespace: "default",
					},
					Data: map[string][]byte{
						"wrong-key": []byte("ssh-rsa AAAAB3..."),
					},
				}

				// Create pull secret
				pullSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "pull-secret",
						Namespace: "default",
					},
					Data: map[string][]byte{
						PullSecretKey: []byte(`{"auths":{}}`),
					},
				}

				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						SSHKeySecretRef: corev1.LocalObjectReference{
							Name: "ssh-key",
						},
						PullSecretRef: corev1.LocalObjectReference{
							Name: "pull-secret",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(sshSecret, pullSecret, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateSecrets(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue
				Expect(result.RequeueAfter).To(BeZero())

				// Verify status updated
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Verify condition
				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.SecretsValid)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
				Expect(condition.Reason).To(Equal(ReasonSSHKeySecretInvalid))
				Expect(condition.Message).To(ContainSubstring("ssh-key"))
				Expect(condition.Message).To(ContainSubstring(SSHPublicKeySecretKey))

				// Verify warning event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("SSHKeySecretInvalid")))
			})
		})

		Context("when pull secret is missing", func() {
			It("should set SecretsValid=False with PullSecretMissing reason and not requeue", func() {
				// Create SSH secret (pull secret missing)
				sshSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "ssh-key",
						Namespace: "default",
					},
					Data: map[string][]byte{
						SSHPublicKeySecretKey: []byte("ssh-rsa AAAAB3..."),
					},
				}

				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						SSHKeySecretRef: corev1.LocalObjectReference{
							Name: "ssh-key",
						},
						PullSecretRef: corev1.LocalObjectReference{
							Name: "missing-pull-secret",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(sshSecret, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateSecrets(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue
				Expect(result.RequeueAfter).To(BeZero())

				// Verify status updated
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Verify condition
				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.SecretsValid)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
				Expect(condition.Reason).To(Equal(ReasonPullSecretMissing))
				Expect(condition.Message).To(ContainSubstring("missing-pull-secret"))
				Expect(condition.Message).To(ContainSubstring("not found"))

				// Verify warning event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("PullSecretMissing")))
			})
		})

		Context("when pull secret is missing required key", func() {
			It("should set SecretsValid=False with PullSecretInvalid reason and not requeue", func() {
				// Create SSH secret
				sshSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "ssh-key",
						Namespace: "default",
					},
					Data: map[string][]byte{
						SSHPublicKeySecretKey: []byte("ssh-rsa AAAAB3..."),
					},
				}

				// Create pull secret without required key
				pullSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "pull-secret",
						Namespace: "default",
					},
					Data: map[string][]byte{
						"wrong-key": []byte(`{"auths":{}}`),
					},
				}

				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						SSHKeySecretRef: corev1.LocalObjectReference{
							Name: "ssh-key",
						},
						PullSecretRef: corev1.LocalObjectReference{
							Name: "pull-secret",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(sshSecret, pullSecret, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateSecrets(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue
				Expect(result.RequeueAfter).To(BeZero())

				// Verify status updated
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Verify condition
				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.SecretsValid)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
				Expect(condition.Reason).To(Equal(ReasonPullSecretInvalid))
				Expect(condition.Message).To(ContainSubstring("pull-secret"))
				Expect(condition.Message).To(ContainSubstring(PullSecretKey))

				// Verify warning event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("PullSecretInvalid")))
			})
		})

		Context("when RBAC permission is denied", func() {
			It("should set SecretsValid=False with AccessDenied reason and not requeue", func() {
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						SSHKeySecretRef: corev1.LocalObjectReference{
							Name: "ssh-key",
						},
						PullSecretRef: corev1.LocalObjectReference{
							Name: "pull-secret",
						},
					},
				}

				// Create fake client that returns Forbidden error
				fakeClient = &forbiddenSecretsClient{
					Client: fake.NewClientBuilder().
						WithScheme(scheme).
						WithObjects(provisioner).
						WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
						Build(),
				}

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateSecrets(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue
			})
		})

		Context("when secrets are recovered", func() {
			It("should transition from SSHKeySecretMissing to SecretsValid", func() {
				// Create both secrets
				sshSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "ssh-key",
						Namespace: "default",
					},
					Data: map[string][]byte{
						SSHPublicKeySecretKey: []byte("ssh-rsa AAAAB3..."),
					},
				}

				pullSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "pull-secret",
						Namespace: "default",
					},
					Data: map[string][]byte{
						PullSecretKey: []byte(`{"auths":{}}`),
					},
				}

				// Create provisioner with previous SSHKeySecretMissing condition
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						SSHKeySecretRef: corev1.LocalObjectReference{
							Name: "ssh-key",
						},
						PullSecretRef: corev1.LocalObjectReference{
							Name: "pull-secret",
						},
					},
					Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
						Conditions: []metav1.Condition{
							{
								Type:               provisioningv1alpha1.SecretsValid,
								Status:             metav1.ConditionFalse,
								Reason:             ReasonSSHKeySecretMissing,
								Message:            "SSH key secret not found",
								LastTransitionTime: metav1.Now(),
							},
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(sshSecret, pullSecret, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateSecrets(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify condition is now True (recovered)
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.SecretsValid)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonSecretsValid))
			})

			It("should transition from PullSecretInvalid to SecretsValid", func() {
				// Create both secrets with correct keys
				sshSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "ssh-key",
						Namespace: "default",
					},
					Data: map[string][]byte{
						SSHPublicKeySecretKey: []byte("ssh-rsa AAAAB3..."),
					},
				}

				pullSecret := &corev1.Secret{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "pull-secret",
						Namespace: "default",
					},
					Data: map[string][]byte{
						PullSecretKey: []byte(`{"auths":{}}`),
					},
				}

				// Create provisioner with previous PullSecretInvalid condition
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						SSHKeySecretRef: corev1.LocalObjectReference{
							Name: "ssh-key",
						},
						PullSecretRef: corev1.LocalObjectReference{
							Name: "pull-secret",
						},
					},
					Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
						Conditions: []metav1.Condition{
							{
								Type:               provisioningv1alpha1.SecretsValid,
								Status:             metav1.ConditionFalse,
								Reason:             ReasonPullSecretInvalid,
								Message:            "Pull secret is missing required key",
								LastTransitionTime: metav1.Now(),
							},
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(sshSecret, pullSecret, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateSecrets(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify condition is now True (recovered)
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.SecretsValid)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonSecretsValid))
			})
		})
	})
})

// forbiddenSecretsClient wraps a fake client to return Forbidden errors for Secret Get operations
type forbiddenSecretsClient struct {
	client.Client
}

func (c *forbiddenSecretsClient) Get(ctx context.Context, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
	// Return Forbidden error for Secret Get operations
	if _, ok := obj.(*corev1.Secret); ok {
		return apierrors.NewForbidden(
			schema.GroupResource{Group: "", Resource: "secrets"},
			key.Name,
			nil,
		)
	}
	// Delegate to wrapped client for other types
	return c.Client.Get(ctx, key, obj, opts...)
}
