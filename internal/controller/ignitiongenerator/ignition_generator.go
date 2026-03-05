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

package ignitiongenerator

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/content/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/content/live"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/content/target"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition"
	igncontent "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/content"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition/special"
)

const (
	retryInterval       = 30 * time.Second
	httpClientTimeout   = 30 * time.Second
	ignitionSecretName  = "ignition-server-ca-cert"
	ignitionTokenPrefix = "token-"
	ignitionTokenKey    = "token"
	configMapKeyName    = "BF_CFG_TEMPLATE"
	configMapNamePrefix = "bfcfg"
)

// IgnitionGenerator handles ignition configuration generation for DPF provisioning
type IgnitionGenerator struct {
	Client   client.Client
	Scheme   *runtime.Scheme
	Recorder record.EventRecorder
}

// NewIgnitionGenerator creates a new IgnitionGenerator
func NewIgnitionGenerator(c client.Client, s *runtime.Scheme, recorder record.EventRecorder) *IgnitionGenerator {
	return &IgnitionGenerator{
		Client:   c,
		Scheme:   s,
		Recorder: recorder,
	}
}

// GenerateIgnition performs the complete ignition generation workflow
func (ig *IgnitionGenerator) GenerateIgnition(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "ignition-generation")

	// Block if required ignition fields are missing — set condition so user knows what's missing
	var missing []string
	if cr.Spec.MachineOSURL == "" {
		missing = append(missing, "spec.machineOSURL")
	}
	if cr.Spec.MTU9000Enabled == nil {
		missing = append(missing, "spec.mtu9000Enabled")
	}
	if len(missing) > 0 {
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:               provisioningv1alpha1.IgnitionConfigured,
			Status:             metav1.ConditionFalse,
			Reason:             "IgnitionFieldsMissing",
			Message:            fmt.Sprintf("Required fields not set: %s", strings.Join(missing, ", ")),
			ObservedGeneration: cr.Generation,
		})
		return ctrl.Result{}, nil
	}

	// Execute ignition generation workflow
	if err := ig.generateIgnition(ctx, cr); err != nil {
		log.Error(err, "Ignition generation failed")

		// Set error condition and persist
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:               provisioningv1alpha1.IgnitionConfigured,
			Status:             metav1.ConditionFalse,
			Reason:             "IgnitionGenerationFailed",
			Message:            fmt.Sprintf("Failed to generate ignition: %v", err),
			ObservedGeneration: cr.Generation,
		})
		if updateErr := ig.Client.Status().Update(ctx, cr); updateErr != nil {
			return ctrl.Result{}, fmt.Errorf("failed to update status after ignition error: %w", updateErr)
		}

		ig.Recorder.Event(cr, corev1.EventTypeWarning, "IgnitionGenerationFailed", err.Error())
		return ctrl.Result{RequeueAfter: retryInterval}, nil
	}

	// Success
	meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.IgnitionConfigured,
		Status:             metav1.ConditionTrue,
		Reason:             "IgnitionGenerated",
		Message:            fmt.Sprintf("Ignition ConfigMap %s-%s.cfg created and DPFOperatorConfig updated successfully", configMapNamePrefix, cr.Spec.DPUClusterRef.Name),
		ObservedGeneration: cr.Generation,
	})

	// Persist status changes
	if err := ig.Client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status after successful ignition generation")
		return ctrl.Result{}, err
	}

	ig.Recorder.Event(cr, corev1.EventTypeNormal, "IgnitionConfigured",
		fmt.Sprintf("Ignition ConfigMap %s-%s.cfg configured successfully", configMapNamePrefix, cr.Spec.DPUClusterRef.Name))

	log.Info("Ignition generation completed successfully")
	return ctrl.Result{}, nil
}

// generateIgnition executes the complete ignition generation workflow
func (ig *IgnitionGenerator) generateIgnition(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) error {
	log := logf.FromContext(ctx)

	// Step 1: Download HCP ignition
	log.V(1).Info("Downloading HCP ignition")
	hcpIgnitionBytes, err := ig.downloadHCPIgnition(ctx, cr)
	if err != nil {
		return fmt.Errorf("failed to download HCP ignition: %w", err)
	}

	// Step 2: Retrieve DPU Flavor configuration
	log.V(1).Info("Retrieving DPU Flavor configuration")
	dpuFlavor, err := ig.retrieveDPUFlavor(ctx, cr)
	if err != nil {
		return fmt.Errorf("failed to retrieve DPU Flavor: %w", err)
	}

	// Step 3: Build target ignition (HCP + DPF modifications)
	log.V(1).Info("Building target ignition")
	mtu9000Mode := cr.Spec.MTU9000Enabled != nil && *cr.Spec.MTU9000Enabled
	targetIgnition, err := ig.buildTargetIgnition(hcpIgnitionBytes, dpuFlavor, cr.Spec.MachineOSURL, mtu9000Mode)
	if err != nil {
		return fmt.Errorf("failed to build target ignition: %w", err)
	}

	// Step 4: Build live ignition (embed target)
	log.V(1).Info("Building live ignition")
	liveIgnition, err := ig.buildLiveIgnition(targetIgnition, hcpIgnitionBytes)
	if err != nil {
		return fmt.Errorf("failed to build live ignition: %w", err)
	}

	// Step 5: Create/Update ConfigMap
	log.V(1).Info("Creating/Updating ConfigMap")
	if err := ig.createOrUpdateConfigMap(ctx, cr, liveIgnition); err != nil {
		return fmt.Errorf("failed to create/update ConfigMap: %w", err)
	}

	// Step 6: Update DPFOperatorConfig
	log.V(1).Info("Updating DPFOperatorConfig")
	if err := ig.updateDPFOperatorConfig(ctx, cr); err != nil {
		return fmt.Errorf("failed to update DPFOperatorConfig: %w", err)
	}

	return nil
}

// controlPlaneNamespace returns the HyperShift control plane namespace for this CR.
// HyperShift creates it as "<hostedcluster-namespace>-<hostedcluster-name>".
func (ig *IgnitionGenerator) controlPlaneNamespace(cr *provisioningv1alpha1.DPFHCPProvisioner) string {
	return cr.Status.HostedClusterRef.Namespace + "-" + cr.Status.HostedClusterRef.Name
}

// getIgnitionCACert reads the ignition server's TLS CA certificate from the control plane namespace.
func (ig *IgnitionGenerator) getIgnitionCACert(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) ([]byte, error) {
	caSecret := &corev1.Secret{}
	key := types.NamespacedName{
		Name:      ignitionSecretName,
		Namespace: ig.controlPlaneNamespace(cr),
	}
	if err := ig.Client.Get(ctx, key, caSecret); err != nil {
		return nil, fmt.Errorf("failed to get ignition CA cert secret: %w", err)
	}

	caCert, ok := caSecret.Data["tls.crt"]
	if !ok {
		return nil, fmt.Errorf("tls.crt not found in secret %s", ignitionSecretName)
	}
	return caCert, nil
}

// getIgnitionToken finds the ignition bearer token from the control plane namespace.
// HyperShift names the secret "token-<cluster>-<hash>", so we search by prefix.
func (ig *IgnitionGenerator) getIgnitionToken(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (string, error) {
	ns := ig.controlPlaneNamespace(cr)
	prefix := ignitionTokenPrefix + cr.Status.HostedClusterRef.Name

	secretList := &corev1.SecretList{}
	if err := ig.Client.List(ctx, secretList, client.InNamespace(ns)); err != nil {
		return "", fmt.Errorf("failed to list secrets in %s: %w", ns, err)
	}

	for i := range secretList.Items {
		if strings.HasPrefix(secretList.Items[i].Name, prefix) {
			tokenBytes, ok := secretList.Items[i].Data[ignitionTokenKey]
			if !ok {
				return "", fmt.Errorf("token key not found in secret %s", secretList.Items[i].Name)
			}
			// Re-encode: Secret.Data is already decoded, but the ignition server expects the base64 form
			return base64.StdEncoding.EncodeToString(tokenBytes), nil
		}
	}
	return "", fmt.Errorf("no token secret with prefix %q found in namespace %s", prefix, ns)
}

// getIgnitionEndpoint reads the ignition endpoint URL from the HostedCluster status.
func (ig *IgnitionGenerator) getIgnitionEndpoint(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (string, error) {
	hc := &hyperv1.HostedCluster{}
	key := types.NamespacedName{
		Name:      cr.Status.HostedClusterRef.Name,
		Namespace: cr.Status.HostedClusterRef.Namespace,
	}
	if err := ig.Client.Get(ctx, key, hc); err != nil {
		return "", fmt.Errorf("failed to get HostedCluster: %w", err)
	}
	if hc.Status.IgnitionEndpoint == "" {
		return "", fmt.Errorf("ignition endpoint not available in HostedCluster status")
	}
	return hc.Status.IgnitionEndpoint, nil
}

// downloadHCPIgnition downloads the ignition configuration from the HostedCluster ignition endpoint.
func (ig *IgnitionGenerator) downloadHCPIgnition(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) ([]byte, error) {
	log := logf.FromContext(ctx)

	caCert, err := ig.getIgnitionCACert(ctx, cr)
	if err != nil {
		return nil, err
	}

	token, err := ig.getIgnitionToken(ctx, cr)
	if err != nil {
		return nil, err
	}

	endpoint, err := ig.getIgnitionEndpoint(ctx, cr)
	if err != nil {
		return nil, err
	}

	// Build HTTPS client with the ignition server's CA
	certPool := x509.NewCertPool()
	if !certPool.AppendCertsFromPEM(caCert) {
		return nil, fmt.Errorf("failed to parse CA certificate")
	}
	httpClient := &http.Client{
		Timeout: httpClientTimeout,
		Transport: &http.Transport{
			TLSClientConfig: &tls.Config{
				RootCAs:    certPool,
				MinVersion: tls.VersionTLS12,
			},
		},
	}

	// Download ignition
	ignitionURL := fmt.Sprintf("https://%s/ignition", endpoint)
	log.Info("Downloading ignition from HCP", "url", ignitionURL)

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, ignitionURL, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create HTTP request: %w", err)
	}
	req.Header.Set("Authorization", fmt.Sprintf("Bearer %s", token))

	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("HTTP request failed: %w", err)
	}

	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("HTTP request failed with status %d", resp.StatusCode)
	}

	ignitionData, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("failed to read response body: %w", err)
	}

	log.Info("Successfully downloaded HCP ignition", "size", len(ignitionData))
	return ignitionData, nil
}

// retrieveDPUFlavor retrieves the DPU Flavor configuration from DPUDeployment
func (ig *IgnitionGenerator) retrieveDPUFlavor(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (*dpuprovisioningv1alpha1.DPUFlavor, error) {
	// Get DPUDeployment
	dpuDeployment := &dpuservicev1alpha1.DPUDeployment{}
	deploymentKey := types.NamespacedName{
		Name:      cr.Spec.DPUDeploymentRef.Name,
		Namespace: cr.Spec.DPUDeploymentRef.Namespace,
	}
	if err := ig.Client.Get(ctx, deploymentKey, dpuDeployment); err != nil {
		return nil, fmt.Errorf("failed to get DPUDeployment: %w", err)
	}

	// Extract flavor name
	flavorName := dpuDeployment.Spec.DPUs.Flavor
	if flavorName == "" {
		return nil, fmt.Errorf("DPUDeployment.Spec.DPUs.Flavor is empty")
	}

	// Get DPUFlavor from DPUDeployment's namespace
	dpuFlavor := &dpuprovisioningv1alpha1.DPUFlavor{}
	flavorKey := types.NamespacedName{
		Name:      flavorName,
		Namespace: cr.Spec.DPUDeploymentRef.Namespace,
	}
	if err := ig.Client.Get(ctx, flavorKey, dpuFlavor); err != nil {
		return nil, fmt.Errorf("failed to get DPUFlavor %s: %w", flavorName, err)
	}

	return dpuFlavor, nil
}

// buildTargetIgnition builds the target ignition with HCP ignition + DPF modifications
func (ig *IgnitionGenerator) buildTargetIgnition(hcpIgnitionBytes []byte, dpuFlavor *dpuprovisioningv1alpha1.DPUFlavor, machineOSURL string, mtu9000Mode bool) (*ignition.Ignition, error) {
	// Parse HCP ignition
	targetIgnition := &ignition.Ignition{}
	if err := json.Unmarshal(hcpIgnitionBytes, targetIgnition); err != nil {
		return nil, fmt.Errorf("failed to parse HCP ignition: %w", err)
	}

	// Replace machine OS URL
	if err := special.ReplaceMachineOSURL(targetIgnition, machineOSURL); err != nil {
		return nil, fmt.Errorf("failed to replace machine OS URL: %w", err)
	}

	// Add target content files and systemd units
	targetProvider := target.NewProvider()
	if err := igncontent.AddContent(targetIgnition, targetProvider); err != nil {
		return nil, fmt.Errorf("failed to add target content: %w", err)
	}

	// Add common content files and systemd units
	commonProvider := common.NewProvider()
	if err := igncontent.AddContent(targetIgnition, commonProvider); err != nil {
		return nil, fmt.Errorf("failed to add common content: %w", err)
	}

	// Convert DPUFlavor to ignition.Flavor format
	ignFlavor := &ignition.Flavor{
		OVS: ignition.OVS{
			RawConfigScript: dpuFlavor.Spec.OVS.RawConfigScript,
		},
	}

	// Add flavor OVS script
	special.AddFlavorOVSScript(targetIgnition, ignFlavor)

	// Enable MTU 9000 mode if requested
	if mtu9000Mode {
		special.EnableMTU9000(targetIgnition)
	}

	return targetIgnition, nil
}

// buildLiveIgnition builds the live ignition with embedded target ignition
func (ig *IgnitionGenerator) buildLiveIgnition(targetIgnition *ignition.Ignition, hcpIgnitionBytes []byte) (*ignition.Ignition, error) {
	// Parse HCP to extract passwd
	hcpIgnition := &ignition.Ignition{}
	if err := json.Unmarshal(hcpIgnitionBytes, hcpIgnition); err != nil {
		return nil, fmt.Errorf("failed to parse HCP ignition for passwd: %w", err)
	}

	// Create empty live ignition
	liveIgnition := ignition.NewEmptyIgnition("3.2.0")

	// Copy passwd from HCP ignition
	if hcpIgnition.Passwd != nil {
		liveIgnition.Passwd = hcpIgnition.Passwd
	}

	// Add live content files and systemd units
	liveProvider := live.NewProvider()
	if err := igncontent.AddContent(liveIgnition, liveProvider); err != nil {
		return nil, fmt.Errorf("failed to add live content: %w", err)
	}

	// Add common content files and systemd units
	commonProvider := common.NewProvider()
	if err := igncontent.AddContent(liveIgnition, commonProvider); err != nil {
		return nil, fmt.Errorf("failed to add common content: %w", err)
	}

	// Encode target ignition (gzip + base64)
	encodedTarget, err := ignition.EncodeIgnition(targetIgnition)
	if err != nil {
		return nil, fmt.Errorf("failed to encode target ignition: %w", err)
	}

	// Embed encoded target as file at /var/target.ign
	targetFile := ignition.FileEntry{
		Path:      "/var/target.ign",
		Mode:      0644,
		Overwrite: true,
		Contents: ignition.FileContents{
			Source: fmt.Sprintf("data:;base64,%s", encodedTarget),
		},
	}
	liveIgnition.Storage.Files = append(liveIgnition.Storage.Files, targetFile)

	return liveIgnition, nil
}

// createOrUpdateConfigMap creates or updates the ignition ConfigMap in DPUCluster namespace
func (ig *IgnitionGenerator) createOrUpdateConfigMap(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, liveIgnition *ignition.Ignition) error {
	log := logf.FromContext(ctx)

	// Marshal live ignition to JSON
	ignitionJSON, err := json.Marshal(liveIgnition)
	if err != nil {
		return fmt.Errorf("failed to marshal live ignition: %w", err)
	}

	cmName := fmt.Sprintf("%s-%s.cfg", configMapNamePrefix, cr.Spec.DPUClusterRef.Name)
	cmNamespace := cr.Spec.DPUClusterRef.Namespace

	cm := &corev1.ConfigMap{
		ObjectMeta: metav1.ObjectMeta{
			Name:      cmName,
			Namespace: cmNamespace,
		},
		Data: map[string]string{
			configMapKeyName: string(ignitionJSON) + "\n",
		},
	}

	// Set owner reference if same namespace (cross-namespace owner refs not supported)
	if cr.Namespace == cmNamespace {
		if err := controllerutil.SetControllerReference(cr, cm, ig.Scheme); err != nil {
			log.V(1).Info("Cannot set owner reference", "error", err)
		}
	}

	// Create or update
	existingCM := &corev1.ConfigMap{}
	cmKey := types.NamespacedName{Name: cmName, Namespace: cmNamespace}
	err = ig.Client.Get(ctx, cmKey, existingCM)

	if err != nil {
		if apierrors.IsNotFound(err) {
			// Create new ConfigMap
			if err := ig.Client.Create(ctx, cm); err != nil {
				return fmt.Errorf("failed to create ConfigMap: %w", err)
			}
			log.Info("Created ignition ConfigMap", "name", cmName, "namespace", cmNamespace)
			return nil
		}
		return fmt.Errorf("failed to get ConfigMap: %w", err)
	}

	// Update existing ConfigMap
	existingCM.Data = cm.Data
	if err := ig.Client.Update(ctx, existingCM); err != nil {
		return fmt.Errorf("failed to update ConfigMap: %w", err)
	}

	log.Info("Updated ignition ConfigMap", "name", cmName, "namespace", cmNamespace)
	return nil
}

// updateDPFOperatorConfig updates the DPFOperatorConfig to reference the ignition ConfigMap
func (ig *IgnitionGenerator) updateDPFOperatorConfig(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) error {
	log := logf.FromContext(ctx)

	configMapName := fmt.Sprintf("%s-%s.cfg", configMapNamePrefix, cr.Spec.DPUClusterRef.Name)
	dpuClusterNamespace := cr.Spec.DPUClusterRef.Namespace

	// List DPFOperatorConfig in DPUCluster namespace (expect one instance)
	configList := &operatorv1alpha1.DPFOperatorConfigList{}
	if err := ig.Client.List(ctx, configList, client.InNamespace(dpuClusterNamespace)); err != nil {
		return fmt.Errorf("failed to list DPFOperatorConfig: %w", err)
	}

	if len(configList.Items) == 0 {
		return fmt.Errorf("no DPFOperatorConfig found in namespace %s", dpuClusterNamespace)
	}

	if len(configList.Items) > 1 {
		log.Info("WARNING: Multiple DPFOperatorConfig instances found, using first",
			"count", len(configList.Items), "namespace", dpuClusterNamespace)
	}

	dpfConfig := &configList.Items[0]

	// Check if already set to avoid unnecessary patch
	if dpfConfig.Spec.ProvisioningController.BFCFGTemplateConfigMap != nil &&
		*dpfConfig.Spec.ProvisioningController.BFCFGTemplateConfigMap == configMapName {
		log.V(1).Info("DPFOperatorConfig already points to correct ConfigMap, skipping patch")
		return nil
	}

	// Patch the field
	patch := client.MergeFrom(dpfConfig.DeepCopy())
	dpfConfig.Spec.ProvisioningController.BFCFGTemplateConfigMap = &configMapName

	if err := ig.Client.Patch(ctx, dpfConfig, patch); err != nil {
		return fmt.Errorf("failed to patch DPFOperatorConfig: %w", err)
	}

	log.Info("Updated DPFOperatorConfig", "configMap", configMapName, "namespace", dpuClusterNamespace)
	return nil
}
