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
	"context"
	"crypto/rand"
	"fmt"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// SecretManager handles secret copying and ETCD key generation for HostedCluster
type SecretManager struct {
	client.Client
	Scheme *runtime.Scheme
}

// NewSecretManager creates a new SecretManager
func NewSecretManager(c client.Client, scheme *runtime.Scheme) *SecretManager {
	return &SecretManager{
		Client: c,
		Scheme: scheme,
	}
}

// CopySecrets copies pull-secret and ssh-key within the same namespace as DPFHCPProvisioner
// Returns ctrl.Result and error for reconciliation flow
func (sm *SecretManager) CopySecrets(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	// Copy pull-secret
	pullSecretName := fmt.Sprintf("%s-pull-secret", cr.Name)
	if err := sm.copyPullSecret(ctx, cr, pullSecretName); err != nil {
		log.Error(err, "Failed to copy pull-secret")
		return ctrl.Result{}, err
	}

	// Copy ssh-key
	sshKeyName := fmt.Sprintf("%s-ssh-key", cr.Name)
	if err := sm.copySSHKey(ctx, cr, sshKeyName); err != nil {
		log.Error(err, "Failed to copy ssh-key")
		return ctrl.Result{}, err
	}

	log.V(1).Info("Successfully copied secrets",
		"pullSecret", pullSecretName,
		"sshKey", sshKeyName,
		"namespace", cr.Namespace)

	return ctrl.Result{}, nil
}

// copyPullSecret copies the pull-secret within the same namespace with proper type and labels
func (sm *SecretManager) copyPullSecret(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, targetName string) error {
	log := logf.FromContext(ctx)

	// Get source pull-secret from DPFHCPProvisioner namespace
	sourceSecret := &corev1.Secret{}
	sourceKey := types.NamespacedName{
		Name:      cr.Spec.PullSecretRef.Name,
		Namespace: cr.Namespace,
	}

	if err := sm.Get(ctx, sourceKey, sourceSecret); err != nil {
		return fmt.Errorf("failed to get pull-secret %s/%s: %w", cr.Namespace, cr.Spec.PullSecretRef.Name, err)
	}

	// Check if target secret already exists with matching labels (idempotency)
	targetKey := types.NamespacedName{
		Name:      targetName,
		Namespace: cr.Namespace,
	}
	existingSecret := &corev1.Secret{}
	err := sm.Get(ctx, targetKey, existingSecret)
	if err == nil {
		// Secret exists, verify ownership via OwnerReference
		if metav1.IsControlledBy(existingSecret, cr) {
			log.V(1).Info("Pull-secret already exists and is owned by this DPFHCPProvisioner, reusing",
				"secret", targetName,
				"namespace", cr.Namespace)
			return nil
		}
		return fmt.Errorf("pull-secret %s exists in %s but is owned by different DPFHCPProvisioner", targetName, cr.Namespace)
	}

	if !apierrors.IsNotFound(err) {
		return fmt.Errorf("failed to check existing pull-secret: %w", err)
	}

	// Create target secret with correct type
	targetSecret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:      targetName,
			Namespace: cr.Namespace,
		},
		Type: corev1.SecretTypeDockerConfigJson,
		Data: sourceSecret.Data,
	}

	// Set owner reference for automatic garbage collection
	if err := controllerutil.SetControllerReference(cr, targetSecret, sm.Scheme); err != nil {
		return fmt.Errorf("failed to set owner reference on pull-secret: %w", err)
	}

	if err := sm.Create(ctx, targetSecret); err != nil {
		return fmt.Errorf("failed to create pull-secret: %w", err)
	}

	log.Info("Created pull-secret",
		"secret", targetName,
		"namespace", cr.Namespace)

	return nil
}

// copySSHKey copies the ssh-key within the same namespace with proper type and labels
func (sm *SecretManager) copySSHKey(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, targetName string) error {
	log := logf.FromContext(ctx)

	// Get source ssh-key from DPFHCPProvisioner namespace
	sourceSecret := &corev1.Secret{}
	sourceKey := types.NamespacedName{
		Name:      cr.Spec.SSHKeySecretRef.Name,
		Namespace: cr.Namespace,
	}

	if err := sm.Get(ctx, sourceKey, sourceSecret); err != nil {
		return fmt.Errorf("failed to get ssh-key %s/%s: %w", cr.Namespace, cr.Spec.SSHKeySecretRef.Name, err)
	}

	// Check if target secret already exists with matching labels (idempotency)
	targetKey := types.NamespacedName{
		Name:      targetName,
		Namespace: cr.Namespace,
	}
	existingSecret := &corev1.Secret{}
	err := sm.Get(ctx, targetKey, existingSecret)
	if err == nil {
		// Secret exists, verify ownership via OwnerReference
		if metav1.IsControlledBy(existingSecret, cr) {
			log.V(1).Info("SSH key already exists and is owned by this DPFHCPProvisioner, reusing",
				"secret", targetName,
				"namespace", cr.Namespace)
			return nil
		}
		return fmt.Errorf("ssh-key %s exists in %s but is owned by different DPFHCPProvisioner", targetName, cr.Namespace)
	}

	if !apierrors.IsNotFound(err) {
		return fmt.Errorf("failed to check existing ssh-key: %w", err)
	}

	// Create target secret with correct type
	targetSecret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:      targetName,
			Namespace: cr.Namespace,
		},
		Type: corev1.SecretTypeOpaque,
		Data: sourceSecret.Data,
	}

	// Set owner reference for automatic garbage collection
	if err := controllerutil.SetControllerReference(cr, targetSecret, sm.Scheme); err != nil {
		return fmt.Errorf("failed to set owner reference on ssh-key: %w", err)
	}

	if err := sm.Create(ctx, targetSecret); err != nil {
		return fmt.Errorf("failed to create ssh-key: %w", err)
	}

	log.Info("Created ssh-key",
		"secret", targetName,
		"namespace", cr.Namespace)

	return nil
}

// GenerateETCDEncryptionKey generates a 32-byte random key for ETCD encryption
// Returns ctrl.Result and error for reconciliation flow
func (sm *SecretManager) GenerateETCDEncryptionKey(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	secretName := fmt.Sprintf("%s-etcd-encryption-key", cr.Name)
	targetKey := types.NamespacedName{
		Name:      secretName,
		Namespace: cr.Namespace,
	}

	// Check if secret already exists (idempotency)
	existingSecret := &corev1.Secret{}
	err := sm.Get(ctx, targetKey, existingSecret)
	if err == nil {
		// Secret exists, verify ownership via OwnerReference
		if metav1.IsControlledBy(existingSecret, cr) {
			log.V(1).Info("ETCD encryption key already exists and is owned by this DPFHCPProvisioner, reusing",
				"secret", secretName,
				"namespace", cr.Namespace)
			return ctrl.Result{}, nil
		}
		return ctrl.Result{}, fmt.Errorf("etcd encryption key %s exists in %s but is owned by different DPFHCPProvisioner", secretName, cr.Namespace)
	}

	if !apierrors.IsNotFound(err) {
		return ctrl.Result{}, fmt.Errorf("failed to check existing etcd encryption key: %w", err)
	}

	// Generate 32 random bytes for ETCD encryption
	keyBytes := make([]byte, 32)
	if _, err := rand.Read(keyBytes); err != nil {
		return ctrl.Result{}, fmt.Errorf("failed to generate random encryption key: %w", err)
	}

	// Create secret with proper type
	secret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:      secretName,
			Namespace: cr.Namespace,
		},
		Type: corev1.SecretTypeOpaque,
		Data: map[string][]byte{
			hyperv1.AESCBCKeySecretKey: keyBytes,
		},
	}

	// Set owner reference for automatic garbage collection
	if err := controllerutil.SetControllerReference(cr, secret, sm.Scheme); err != nil {
		return ctrl.Result{}, fmt.Errorf("failed to set owner reference on etcd encryption key: %w", err)
	}

	if err := sm.Create(ctx, secret); err != nil {
		return ctrl.Result{}, fmt.Errorf("failed to create etcd encryption key secret: %w", err)
	}

	log.Info("Generated ETCD encryption key",
		"secret", secretName,
		"namespace", cr.Namespace,
		"keyLength", len(keyBytes))

	return ctrl.Result{}, nil
}
