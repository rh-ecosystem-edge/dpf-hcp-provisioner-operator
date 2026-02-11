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
	"fmt"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

const (
	// Event reasons
	ReasonSecretsValid        = "SecretsValid"
	ReasonSSHKeySecretMissing = "SSHKeySecretMissing"
	ReasonSSHKeySecretInvalid = "SSHKeySecretInvalid"
	ReasonPullSecretMissing   = "PullSecretMissing"
	ReasonPullSecretInvalid   = "PullSecretInvalid"
	ReasonSecretsAccessDenied = "SecretsAccessDenied"
	ReasonSecretsRecovered    = "SecretsRecovered"

	// Secret keys
	SSHPublicKeySecretKey = "id_rsa.pub"
	PullSecretKey         = ".dockerconfigjson"
)

// Validator validates secret references and updates status accordingly
type Validator struct {
	client   client.Client
	recorder record.EventRecorder
}

// NewValidator creates a new secrets validator
func NewValidator(client client.Client, recorder record.EventRecorder) *Validator {
	return &Validator{
		client:   client,
		recorder: recorder,
	}
}

// ValidateSecrets validates that both SSH key and pull secret exist and have required keys
func (v *Validator) ValidateSecrets(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "secrets-validation")

	log.V(1).Info("Validating secrets",
		"sshKeySecretRef", cr.Spec.SSHKeySecretRef.Name,
		"pullSecretRef", cr.Spec.PullSecretRef.Name,
		"namespace", cr.Namespace)

	// Validate SSH key secret
	sshSecret := &corev1.Secret{}
	if err := v.client.Get(ctx, types.NamespacedName{
		Name:      cr.Spec.SSHKeySecretRef.Name,
		Namespace: cr.Namespace,
	}, sshSecret); err != nil {
		if apierrors.IsNotFound(err) {
			return v.handleSSHKeySecretMissing(ctx, cr)
		}
		if apierrors.IsForbidden(err) {
			return v.handleSecretsAccessDenied(ctx, cr, "SSH key secret", err)
		}
		// Transient error - retry
		log.V(1).Info("Transient error fetching SSH key secret, will retry",
			"error", err.Error())
		return ctrl.Result{Requeue: true}, err
	}

	// Validate SSH secret has required key
	if _, ok := sshSecret.Data[SSHPublicKeySecretKey]; !ok {
		return v.handleSSHKeySecretInvalid(ctx, cr)
	}

	// Validate pull secret
	pullSecret := &corev1.Secret{}
	if err := v.client.Get(ctx, types.NamespacedName{
		Name:      cr.Spec.PullSecretRef.Name,
		Namespace: cr.Namespace,
	}, pullSecret); err != nil {
		if apierrors.IsNotFound(err) {
			return v.handlePullSecretMissing(ctx, cr)
		}
		if apierrors.IsForbidden(err) {
			return v.handleSecretsAccessDenied(ctx, cr, "pull secret", err)
		}
		// Transient error - retry
		log.V(1).Info("Transient error fetching pull secret, will retry",
			"error", err.Error())
		return ctrl.Result{Requeue: true}, err
	}

	// Validate pull secret has required key
	if _, ok := pullSecret.Data[PullSecretKey]; !ok {
		return v.handlePullSecretInvalid(ctx, cr)
	}

	// Both secrets exist and are valid
	return v.handleSecretsValid(ctx, cr)
}

// handleSSHKeySecretMissing handles the case when SSH key secret is not found
func (v *Validator) handleSSHKeySecretMissing(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "secrets-validation")

	message := fmt.Sprintf("SSH key secret '%s' not found in namespace '%s'",
		cr.Spec.SSHKeySecretRef.Name, cr.Namespace)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.SecretsValid,
		Status:             metav1.ConditionFalse,
		Reason:             ReasonSSHKeySecretMissing,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeWarning, ReasonSSHKeySecretMissing, message)
		log.Info("SSH key secret not found",
			"secretName", cr.Spec.SSHKeySecretRef.Name,
			"namespace", cr.Namespace)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Do NOT requeue - permanent error requiring user to create secret
	return ctrl.Result{}, nil
}

// handleSSHKeySecretInvalid handles the case when SSH key secret is missing required key
func (v *Validator) handleSSHKeySecretInvalid(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "secrets-validation")

	message := fmt.Sprintf("SSH key secret '%s' is missing required key '%s'",
		cr.Spec.SSHKeySecretRef.Name, SSHPublicKeySecretKey)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.SecretsValid,
		Status:             metav1.ConditionFalse,
		Reason:             ReasonSSHKeySecretInvalid,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeWarning, ReasonSSHKeySecretInvalid, message)
		log.Info("SSH key secret is invalid",
			"secretName", cr.Spec.SSHKeySecretRef.Name,
			"missingKey", SSHPublicKeySecretKey)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Do NOT requeue - permanent error requiring user to fix secret
	return ctrl.Result{}, nil
}

// handlePullSecretMissing handles the case when pull secret is not found
func (v *Validator) handlePullSecretMissing(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "secrets-validation")

	message := fmt.Sprintf("Pull secret '%s' not found in namespace '%s'",
		cr.Spec.PullSecretRef.Name, cr.Namespace)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.SecretsValid,
		Status:             metav1.ConditionFalse,
		Reason:             ReasonPullSecretMissing,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeWarning, ReasonPullSecretMissing, message)
		log.Info("Pull secret not found",
			"secretName", cr.Spec.PullSecretRef.Name,
			"namespace", cr.Namespace)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Do NOT requeue - permanent error requiring user to create secret
	return ctrl.Result{}, nil
}

// handlePullSecretInvalid handles the case when pull secret is missing required key
func (v *Validator) handlePullSecretInvalid(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "secrets-validation")

	message := fmt.Sprintf("Pull secret '%s' is missing required key '%s'",
		cr.Spec.PullSecretRef.Name, PullSecretKey)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.SecretsValid,
		Status:             metav1.ConditionFalse,
		Reason:             ReasonPullSecretInvalid,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeWarning, ReasonPullSecretInvalid, message)
		log.Info("Pull secret is invalid",
			"secretName", cr.Spec.PullSecretRef.Name,
			"missingKey", PullSecretKey)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Do NOT requeue - permanent error requiring user to fix secret
	return ctrl.Result{}, nil
}

// handleSecretsAccessDenied handles RBAC permission errors
func (v *Validator) handleSecretsAccessDenied(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, secretType string, err error) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "secrets-validation")

	message := fmt.Sprintf("Operator lacks RBAC permissions to access %s in namespace '%s': %v",
		secretType, cr.Namespace, err)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.SecretsValid,
		Status:             metav1.ConditionFalse,
		Reason:             ReasonSecretsAccessDenied,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeWarning, ReasonSecretsAccessDenied, message)
		log.Error(err, "RBAC permission denied for secret",
			"secretType", secretType,
			"namespace", cr.Namespace)
	}

	// Update status
	if updateErr := v.client.Status().Update(ctx, cr); updateErr != nil {
		log.Error(updateErr, "Failed to update status")
		return ctrl.Result{}, updateErr
	}

	// Do NOT requeue - permanent error requiring admin to fix RBAC
	return ctrl.Result{}, nil
}

// handleSecretsValid handles the case when both secrets exist and are valid
func (v *Validator) handleSecretsValid(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "secrets-validation")

	// Get previous condition to check for recovery

	// Check for recovery

	message := fmt.Sprintf("Both SSH key secret '%s' and pull secret '%s' are valid",
		cr.Spec.SSHKeySecretRef.Name, cr.Spec.PullSecretRef.Name)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.SecretsValid,
		Status:             metav1.ConditionTrue,
		Reason:             ReasonSecretsValid,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed (e.g., recovered from invalid state)
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeNormal, ReasonSecretsValid, message)
		log.Info("Secrets validated",
			"sshKeySecret", cr.Spec.SSHKeySecretRef.Name,
			"pullSecret", cr.Spec.PullSecretRef.Name)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Success - continue with reconciliation
	return ctrl.Result{}, nil
}
