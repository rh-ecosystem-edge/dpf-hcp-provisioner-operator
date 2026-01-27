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

package kubeconfiginjection

import (
	"bytes"
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

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/internal/common"
)

const (
	// KubeconfigSecretSuffix is the suffix for HC admin kubeconfig secrets
	KubeconfigSecretSuffix = "-admin-kubeconfig"

	// LabelOwnedBy is the label key for ownership tracking
	LabelOwnedBy = "dpf-hcp-bridge-operator/owned-by"

	// LabelNamespace is the label key for namespace tracking
	LabelNamespace = "dpf-hcp-bridge-operator/namespace"
)

// KubeconfigInjector handles kubeconfig injection from HostedCluster to DPUCluster
type KubeconfigInjector struct {
	Client   client.Client
	Recorder record.EventRecorder
}

// NewKubeconfigInjector creates a new KubeconfigInjector
func NewKubeconfigInjector(client client.Client, recorder record.EventRecorder) *KubeconfigInjector {
	return &KubeconfigInjector{
		Client:   client,
		Recorder: recorder,
	}
}

// InjectKubeconfig performs the kubeconfig injection workflow
//
// This function implements the complete kubeconfig injection flow:
// 1. Verify HC and NodePool created
// 2. Check injection state (secret + DPUCluster reference)
// 3. Detect HC kubeconfig secret availability
// 4. Create/update secret in DPUCluster namespace
// 5. Update DPUCluster CR spec.kubeconfig
// 6. Update DPFHCPBridge status (condition + kubeConfigSecretRef)
func (ki *KubeconfigInjector) InjectKubeconfig(ctx context.Context, bridge *provisioningv1alpha1.DPFHCPBridge) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues(
		"feature", "kubeconfig-injection",
		common.DPFHCPBridgeName, fmt.Sprintf("%s/%s", bridge.Namespace, bridge.Name),
	)

	log.Info("Starting kubeconfig injection",
		"bridge", bridge.Name,
		"namespace", bridge.Namespace,
		"targetNamespace", bridge.Spec.DPUClusterRef.Namespace)

	// Step 1: Verify HC and NodePool created
	if bridge.Status.HostedClusterRef == nil {
		log.V(1).Info("HostedCluster not created yet, skipping kubeconfig injection")
		return ctrl.Result{}, nil
	}

	// Step 2: Check injection state
	secretExists, dpuClusterUpdated, err := ki.checkInjectionState(ctx, bridge)
	if err != nil {
		log.Error(err, "Failed to check injection state")
		if condErr := ki.setCondition(ctx, bridge, metav1.ConditionFalse, provisioningv1alpha1.ReasonKubeConfigInjectionFailed,
			fmt.Sprintf("Failed to check injection state: %v", err)); condErr != nil {
			log.Error(condErr, "Failed to update condition")
		}
		return ctrl.Result{}, err
	}

	// Step 3: Detect HC kubeconfig secret availability
	available, secretName, err := ki.detectHCKubeconfigAvailability(ctx, bridge)
	if err != nil {
		log.Error(err, "Failed to detect HC kubeconfig availability")
		if condErr := ki.setCondition(ctx, bridge, metav1.ConditionFalse, provisioningv1alpha1.ReasonKubeConfigInjectionFailed,
			fmt.Sprintf("Failed to detect HC kubeconfig availability: %v", err)); condErr != nil {
			log.Error(condErr, "Failed to update condition")
		}
		return ctrl.Result{}, err
	}

	if !available {
		log.Info("HC kubeconfig secret not ready yet, waiting for watch to trigger",
			"hcName", bridge.Name,
			"hcNamespace", bridge.Namespace)
		if err := ki.setCondition(ctx, bridge, metav1.ConditionFalse, provisioningv1alpha1.ReasonKubeConfigPending,
			fmt.Sprintf("Waiting for Hypershift to create kubeconfig secret for HostedCluster %s", bridge.Name)); err != nil {
			log.Error(err, "Failed to update condition")
			return ctrl.Result{}, err
		}
		// Don't requeue - the watch on HC kubeconfig secrets will trigger reconciliation when the secret is created.
		return ctrl.Result{}, nil
	}

	log.Info("HC kubeconfig secret detected",
		"secretName", secretName,
		"hcNamespace", bridge.Namespace)

	// Step 4: Handle idempotency scenarios
	needsInjection, err := ki.handleIdempotencyScenarios(ctx, bridge, secretName, secretExists, dpuClusterUpdated)
	if err != nil {
		log.Error(err, "Failed to handle idempotency scenarios")
		if condErr := ki.setCondition(ctx, bridge, metav1.ConditionFalse, provisioningv1alpha1.ReasonKubeConfigInjectionFailed,
			fmt.Sprintf("Failed to handle idempotency scenarios: %v", err)); condErr != nil {
			log.Error(condErr, "Failed to update condition")
		}
		return ctrl.Result{}, err
	}

	// If idempotency scenario handled everything, we're done
	if !needsInjection {
		log.V(1).Info("Idempotency scenario handled, injection complete")
		if err := ki.setCondition(ctx, bridge, metav1.ConditionTrue, provisioningv1alpha1.ReasonKubeConfigInjected,
			fmt.Sprintf("Kubeconfig secret successfully created in namespace %s and DPUCluster CR updated", bridge.Spec.DPUClusterRef.Namespace)); err != nil {
			log.Error(err, "Failed to update condition")
			return ctrl.Result{}, err
		}
		return ctrl.Result{}, nil
	}

	// Step 5: Create/update secret in DPUCluster namespace
	if err := ki.createOrUpdateKubeconfigSecret(ctx, bridge, secretName); err != nil {
		log.Error(err, "Failed to create/update kubeconfig secret")
		if condErr := ki.setCondition(ctx, bridge, metav1.ConditionFalse, provisioningv1alpha1.ReasonKubeConfigInjectionFailed,
			fmt.Sprintf("Failed to create kubeconfig secret in namespace %s: %v", bridge.Spec.DPUClusterRef.Namespace, err)); condErr != nil {
			log.Error(condErr, "Failed to update condition")
		}
		// Event emitted by setCondition
		return ctrl.Result{}, err
	}

	log.Info("Kubeconfig secret created/updated",
		"secretName", secretName,
		"namespace", bridge.Spec.DPUClusterRef.Namespace)
	ki.Recorder.Event(bridge, corev1.EventTypeNormal, "KubeConfigInjected",
		fmt.Sprintf("Kubeconfig secret %s created in namespace %s", secretName, bridge.Spec.DPUClusterRef.Namespace))

	// Step 6: Update DPUCluster CR spec.kubeconfig (only if not already updated)
	if !dpuClusterUpdated {
		if err := ki.updateDPUClusterReference(ctx, bridge, secretName); err != nil {
			log.Error(err, "Failed to update DPUCluster reference")
			if condErr := ki.setCondition(ctx, bridge, metav1.ConditionFalse, provisioningv1alpha1.ReasonKubeConfigInjectionFailed,
				fmt.Sprintf("Failed to update DPUCluster spec.kubeconfig: %v", err)); condErr != nil {
				log.Error(condErr, "Failed to update condition")
			}
			// Event emitted by setCondition
			return ctrl.Result{}, err
		}

		log.Info("DPUCluster updated with kubeconfig reference",
			"dpuCluster", bridge.Spec.DPUClusterRef.Name,
			"namespace", bridge.Spec.DPUClusterRef.Namespace)
		ki.Recorder.Event(bridge, corev1.EventTypeNormal, "DPUClusterUpdated",
			fmt.Sprintf("DPUCluster %s/%s updated with kubeconfig reference", bridge.Spec.DPUClusterRef.Namespace, bridge.Spec.DPUClusterRef.Name))
	} else {
		log.V(1).Info("DPUCluster already updated, skipping update",
			"dpuCluster", bridge.Spec.DPUClusterRef.Name)
	}

	// Step 7: Update DPFHCPBridge status
	bridge.Status.KubeConfigSecretRef = &corev1.LocalObjectReference{
		Name: secretName,
	}
	if err := ki.setCondition(ctx, bridge, metav1.ConditionTrue, provisioningv1alpha1.ReasonKubeConfigInjected,
		fmt.Sprintf("Kubeconfig secret successfully created in namespace %s and DPUCluster CR updated", bridge.Spec.DPUClusterRef.Namespace)); err != nil {
		log.Error(err, "Failed to update condition")
		return ctrl.Result{}, err
	}

	log.Info("Kubeconfig injection completed successfully",
		"secretName", secretName,
		"dpuCluster", bridge.Spec.DPUClusterRef.Name)

	return ctrl.Result{}, nil
}

// checkInjectionState checks the current state of the kubeconfig injection work
// to determine if it has already been completed (fully or partially) for idempotency.
// Returns (secretExists, dpuClusterUpdated, error)
func (ki *KubeconfigInjector) checkInjectionState(ctx context.Context, bridge *provisioningv1alpha1.DPFHCPBridge) (bool, bool, error) {
	log := logf.FromContext(ctx)

	secretName := bridge.Name + KubeconfigSecretSuffix
	dpuClusterNamespace := bridge.Spec.DPUClusterRef.Namespace

	// Check if secret exists in DPUCluster namespace
	secret := &corev1.Secret{}
	secretKey := types.NamespacedName{
		Name:      secretName,
		Namespace: dpuClusterNamespace,
	}
	secretErr := ki.Client.Get(ctx, secretKey, secret)
	secretExists := secretErr == nil

	if secretErr != nil && !apierrors.IsNotFound(secretErr) {
		return false, false, fmt.Errorf("failed to check secret existence: %w", secretErr)
	}

	// Check if DPUCluster spec.kubeconfig is populated
	dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{}
	dpuClusterKey := types.NamespacedName{
		Name:      bridge.Spec.DPUClusterRef.Name,
		Namespace: dpuClusterNamespace,
	}
	dpuClusterErr := ki.Client.Get(ctx, dpuClusterKey, dpuCluster)
	if dpuClusterErr != nil {
		if apierrors.IsNotFound(dpuClusterErr) {
			return secretExists, false, fmt.Errorf("DPUCluster %s/%s not found", dpuClusterNamespace, bridge.Spec.DPUClusterRef.Name)
		}
		return secretExists, false, fmt.Errorf("failed to get DPUCluster: %w", dpuClusterErr)
	}

	dpuClusterUpdated := dpuCluster.Spec.Kubeconfig == secretName

	log.V(1).Info("Injection state check",
		"secretExists", secretExists,
		"dpuClusterUpdated", dpuClusterUpdated,
		"dpuClusterKubeconfig", dpuCluster.Spec.Kubeconfig,
		"expectedSecretName", secretName)

	return secretExists, dpuClusterUpdated, nil
}

// detectHCKubeconfigAvailability checks if the HC admin kubeconfig secret exists
// Returns (available, secretName, error)
func (ki *KubeconfigInjector) detectHCKubeconfigAvailability(ctx context.Context, bridge *provisioningv1alpha1.DPFHCPBridge) (bool, string, error) {
	log := logf.FromContext(ctx)

	secretName := bridge.Name + KubeconfigSecretSuffix
	secret := &corev1.Secret{}
	secretKey := types.NamespacedName{
		Name:      secretName,
		Namespace: bridge.Namespace,
	}

	err := ki.Client.Get(ctx, secretKey, secret)
	if err == nil {
		log.V(1).Info("HC kubeconfig secret found",
			"secretName", secretName)
		return true, secretName, nil
	}

	if apierrors.IsNotFound(err) {
		log.V(1).Info("HC kubeconfig secret not ready yet",
			"secretName", secretName,
			"namespace", bridge.Namespace)
		return false, "", nil
	}

	return false, "", fmt.Errorf("failed to check HC kubeconfig secret: %w", err)
}

// handleIdempotencyScenarios handles different idempotency scenarios
// Scenario A: Secret exists AND DPUCluster updated -> check drift
// Scenario B: Secret exists BUT DPUCluster NOT updated -> update DPUCluster only
// Scenario C: Secret does NOT exist BUT DPUCluster updated -> recreate secret
// Scenario D: Neither exists -> proceed with normal injection
// Returns (needsInjection, error)
func (ki *KubeconfigInjector) handleIdempotencyScenarios(ctx context.Context, bridge *provisioningv1alpha1.DPFHCPBridge, secretName string, secretExists, dpuClusterUpdated bool) (bool, error) {
	log := logf.FromContext(ctx)

	if secretExists && dpuClusterUpdated {
		// Scenario A: Check for drift
		log.V(1).Info("Scenario A: Secret exists and DPUCluster updated, checking for drift")
		hasDrift, err := ki.checkDrift(ctx, bridge, secretName)
		if err != nil {
			return false, fmt.Errorf("failed to check drift: %w", err)
		}
		if hasDrift {
			log.Info("Kubeconfig drift detected, will update destination secret",
				"secretName", secretName,
				"namespace", bridge.Spec.DPUClusterRef.Namespace)
			ki.Recorder.Event(bridge, corev1.EventTypeNormal, "DriftCorrected",
				"Kubeconfig secret content drift detected and corrected")
			// Return true to trigger secret update
			return true, nil
		}
		// No drift, no injection needed
		return false, nil
	}

	if secretExists && !dpuClusterUpdated {
		// Scenario B: Update DPUCluster only
		log.Info("Scenario B: Secret exists but DPUCluster not updated, completing injection",
			"secretName", secretName,
			"dpuCluster", bridge.Spec.DPUClusterRef.Name)
		if err := ki.updateDPUClusterReference(ctx, bridge, secretName); err != nil {
			return false, fmt.Errorf("failed to update DPUCluster reference: %w", err)
		}
		// Update status
		bridge.Status.KubeConfigSecretRef = &corev1.LocalObjectReference{
			Name: secretName,
		}
		return false, nil
	}

	if !secretExists && dpuClusterUpdated {
		// Scenario C: Recreate secret
		log.Info("Scenario C: Secret missing but DPUCluster updated, recreating secret",
			"secretName", secretName,
			"namespace", bridge.Spec.DPUClusterRef.Namespace)
		// Return true to trigger secret creation
		return true, nil
	}

	// Scenario D: Neither exists, proceed with normal injection
	log.V(1).Info("Scenario D: Normal injection flow, neither secret nor DPUCluster reference exists")
	return true, nil
}

// checkDrift compares source and destination secret content
// Returns (hasDrift, error)
func (ki *KubeconfigInjector) checkDrift(ctx context.Context, bridge *provisioningv1alpha1.DPFHCPBridge, secretName string) (bool, error) {
	log := logf.FromContext(ctx)

	// Get source secret from HC namespace
	sourceSecret := &corev1.Secret{}
	sourceKey := types.NamespacedName{
		Name:      secretName,
		Namespace: bridge.Namespace,
	}
	if err := ki.Client.Get(ctx, sourceKey, sourceSecret); err != nil {
		return false, fmt.Errorf("failed to get source secret: %w", err)
	}

	// Get destination secret from DPUCluster namespace
	destSecret := &corev1.Secret{}
	destKey := types.NamespacedName{
		Name:      secretName,
		Namespace: bridge.Spec.DPUClusterRef.Namespace,
	}
	if err := ki.Client.Get(ctx, destKey, destSecret); err != nil {
		return false, fmt.Errorf("failed to get destination secret: %w", err)
	}

	// Compare kubeconfig data
	sourceData, sourceOk := sourceSecret.Data["kubeconfig"]
	destData, destOk := destSecret.Data["kubeconfig"]

	if !sourceOk {
		return false, fmt.Errorf("source secret missing 'kubeconfig' key")
	}
	if !destOk {
		return false, fmt.Errorf("destination secret missing 'kubeconfig' key")
	}

	hasDrift := !bytes.Equal(sourceData, destData)
	if hasDrift {
		log.Info("Kubeconfig content drift detected",
			"source", sourceKey,
			"destination", destKey)
	}

	return hasDrift, nil
}

// createOrUpdateKubeconfigSecret creates or updates the secret in DPUCluster namespace
func (ki *KubeconfigInjector) createOrUpdateKubeconfigSecret(ctx context.Context, bridge *provisioningv1alpha1.DPFHCPBridge, sourceSecretName string) error {
	log := logf.FromContext(ctx)

	// Get source secret from HC namespace
	sourceSecret := &corev1.Secret{}
	sourceKey := types.NamespacedName{
		Name:      sourceSecretName,
		Namespace: bridge.Namespace,
	}
	if err := ki.Client.Get(ctx, sourceKey, sourceSecret); err != nil {
		return fmt.Errorf("failed to read source kubeconfig secret: %w", err)
	}

	// Extract kubeconfig data
	kubeconfigData, ok := sourceSecret.Data["kubeconfig"]
	if !ok {
		return fmt.Errorf("source secret missing 'kubeconfig' key")
	}

	// Create destination secret
	destSecret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:      sourceSecretName,
			Namespace: bridge.Spec.DPUClusterRef.Namespace,
			Labels: map[string]string{
				LabelOwnedBy:   bridge.Name,
				LabelNamespace: bridge.Namespace,
			},
		},
		Type: corev1.SecretTypeOpaque,
		Data: map[string][]byte{
			"kubeconfig": kubeconfigData,
		},
	}

	// Try to create
	err := ki.Client.Create(ctx, destSecret)
	if err == nil {
		log.Info("Created kubeconfig secret",
			"secretName", sourceSecretName,
			"namespace", bridge.Spec.DPUClusterRef.Namespace)
		return nil
	}

	// If already exists, update it
	if apierrors.IsAlreadyExists(err) {
		existing := &corev1.Secret{}
		existingKey := types.NamespacedName{
			Name:      sourceSecretName,
			Namespace: bridge.Spec.DPUClusterRef.Namespace,
		}
		if err := ki.Client.Get(ctx, existingKey, existing); err != nil {
			return fmt.Errorf("failed to get existing secret for update: %w", err)
		}

		existing.Data = destSecret.Data
		existing.Labels = destSecret.Labels
		if err := ki.Client.Update(ctx, existing); err != nil {
			return fmt.Errorf("failed to update kubeconfig secret: %w", err)
		}

		log.Info("Updated existing kubeconfig secret",
			"secretName", sourceSecretName,
			"namespace", bridge.Spec.DPUClusterRef.Namespace)
		return nil
	}

	return fmt.Errorf("failed to create kubeconfig secret: %w", err)
}

// updateDPUClusterReference updates DPUCluster spec.kubeconfig field
func (ki *KubeconfigInjector) updateDPUClusterReference(ctx context.Context, bridge *provisioningv1alpha1.DPFHCPBridge, secretName string) error {
	log := logf.FromContext(ctx)

	// Get DPUCluster CR
	dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{}
	dpuClusterKey := types.NamespacedName{
		Name:      bridge.Spec.DPUClusterRef.Name,
		Namespace: bridge.Spec.DPUClusterRef.Namespace,
	}
	if err := ki.Client.Get(ctx, dpuClusterKey, dpuCluster); err != nil {
		return fmt.Errorf("failed to get DPUCluster: %w", err)
	}

	// Update spec.kubeconfig field
	dpuCluster.Spec.Kubeconfig = secretName

	// Persist update
	if err := ki.Client.Update(ctx, dpuCluster); err != nil {
		if apierrors.IsConflict(err) {
			return fmt.Errorf("update conflict on DPUCluster, will retry: %w", err)
		}
		return fmt.Errorf("failed to update DPUCluster spec.kubeconfig: %w", err)
	}

	log.V(1).Info("Updated DPUCluster spec.kubeconfig",
		"dpuCluster", dpuCluster.Name,
		"namespace", dpuCluster.Namespace,
		"kubeconfig", secretName)

	return nil
}

// setCondition updates the KubeConfigInjected condition and persists it immediately.
// This ensures users can see the condition status even if the injection fails.
// Emits Kubernetes events only when the condition status or reason changes to avoid spam.
// Returns error if status update fails (controller-runtime will requeue automatically).
func (ki *KubeconfigInjector) setCondition(ctx context.Context, bridge *provisioningv1alpha1.DPFHCPBridge, status metav1.ConditionStatus, reason, message string) error {
	log := logf.FromContext(ctx)

	// Capture previous condition state to detect changes

	// Update condition in memory
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.KubeConfigInjected,
		Status:             status,
		Reason:             reason,
		Message:            message,
		ObservedGeneration: bridge.Generation,
	}

	// Emit event only if condition status/reason changed (avoid spam)
	if changed := meta.SetStatusCondition(&bridge.Status.Conditions, condition); changed {
		eventType := corev1.EventTypeNormal
		if status == metav1.ConditionFalse {
			eventType = corev1.EventTypeWarning
		}
		ki.Recorder.Event(bridge, eventType, reason, message)
	}

	// Persist status immediately so users can see the condition
	if err := ki.Client.Status().Update(ctx, bridge); err != nil {
		if apierrors.IsConflict(err) {
			// ResourceVersion conflict - controller-runtime will requeue automatically
			log.V(1).Info("Status update conflict, will retry",
				"condition", provisioningv1alpha1.KubeConfigInjected,
				"reason", reason)
			return err
		}
		log.Error(err, "Failed to update KubeConfigInjected condition",
			"reason", reason)
		return fmt.Errorf("failed to update KubeConfigInjected condition: %w", err)
	}

	return nil
}
