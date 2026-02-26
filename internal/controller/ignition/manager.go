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

package ignition

import (
	"context"
	"crypto/tls"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
)

const (
	// DPFHCPProvisionerNamespace is the namespace where the DPF HCP operator resources live.
	DPFHCPProvisionerNamespace = "dpf-hcp-provisioner-system"

	// DPFOperatorNamespace is the namespace where DPF operator resources (DPUFlavor, output CM) live.
	DPFOperatorNamespace = "dpf-operator-system"

	outputConfigMapName = "custom-bfb.cfg"
	commonCMName        = "dpf-ignition-content-common"
	liveCMName          = "dpf-ignition-content-live"
	targetCMName        = "dpf-ignition-content-target"
)

// IgnitionManager handles two-stage DPU ignition config generation for DPFHCPProvisioner.
type IgnitionManager struct {
	client   client.Client
	recorder record.EventRecorder
}

// NewIgnitionManager creates a new IgnitionManager.
func NewIgnitionManager(c client.Client, recorder record.EventRecorder) *IgnitionManager {
	return &IgnitionManager{client: c, recorder: recorder}
}

// GenerateAndApplyIgnition generates a two-stage DPU ignition config and writes it to the
// custom-bfb.cfg ConfigMap in dpf-operator-system. It is a no-op when IgnitionEndpointAvailable
// is not True on the CR.
func (m *IgnitionManager) GenerateAndApplyIgnition(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("provisioner", client.ObjectKeyFromObject(cr))

	// Check IgnitionEndpointAvailable — skip if not yet True.
	endpointCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionEndpointAvailable)
	if endpointCond == nil || endpointCond.Status != metav1.ConditionTrue {
		log.V(1).Info("IgnitionEndpointAvailable is not True, skipping ignition generation")
		return ctrl.Result{}, nil
	}

	log.Info("Generating ignition config")

	if cr.Status.HostedClusterRef == nil {
		log.V(1).Info("HostedClusterRef not set, skipping ignition generation")
		return ctrl.Result{}, nil
	}

	// Fetch HostedCluster to read the ignition endpoint URL.
	hc := &hyperv1.HostedCluster{}
	if err := m.client.Get(ctx, client.ObjectKey{
		Name:      cr.Status.HostedClusterRef.Name,
		Namespace: cr.Status.HostedClusterRef.Namespace,
	}, hc); err != nil {
		return ctrl.Result{}, fmt.Errorf("getting HostedCluster: %w", err)
	}

	endpoint := hc.Status.IgnitionEndpoint
	if endpoint == "" {
		log.V(1).Info("IgnitionEndpoint not yet set on HostedCluster")
		return ctrl.Result{}, nil
	}

	// Find the bearer token for the ignition endpoint.
	// HyperShift places it in the hosted control-plane namespace: {hcNamespace}-{hcName}.
	tokenNamespace := fmt.Sprintf("%s-%s", hc.Namespace, hc.Name)
	token, err := m.findToken(ctx, tokenNamespace, hc.Name)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("finding ignition token: %w", err)
	}

	// Fetch the base ignition config from the HyperShift ignition endpoint.
	baseIgn, err := m.fetchIgnition(ctx, endpoint, token)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("fetching ignition from endpoint: %w", err)
	}

	// Optionally fetch the OVS script from the specified DPUFlavor.
	var ovsScript string
	if cr.Spec.Flavor != "" {
		flavor := &dpuprovisioningv1alpha1.DPUFlavor{}
		if err := m.client.Get(ctx, client.ObjectKey{
			Name:      cr.Spec.Flavor,
			Namespace: DPFOperatorNamespace,
		}, flavor); err != nil {
			return ctrl.Result{}, fmt.Errorf("getting DPUFlavor %q: %w", cr.Spec.Flavor, err)
		}
		ovsScript = flavor.Spec.OVS.RawConfigScript
	}

	// Load the three content ConfigMaps.
	commonCM := &corev1.ConfigMap{}
	if err := m.client.Get(ctx, client.ObjectKey{Name: commonCMName, Namespace: DPFHCPProvisionerNamespace}, commonCM); err != nil {
		return ctrl.Result{}, fmt.Errorf("getting common content ConfigMap: %w", err)
	}
	liveCM := &corev1.ConfigMap{}
	if err := m.client.Get(ctx, client.ObjectKey{Name: liveCMName, Namespace: DPFHCPProvisionerNamespace}, liveCM); err != nil {
		return ctrl.Result{}, fmt.Errorf("getting live content ConfigMap: %w", err)
	}
	targetCM := &corev1.ConfigMap{}
	if err := m.client.Get(ctx, client.ObjectKey{Name: targetCMName, Namespace: DPFHCPProvisionerNamespace}, targetCM); err != nil {
		return ctrl.Result{}, fmt.Errorf("getting target content ConfigMap: %w", err)
	}

	commonContent, err := loadContentFromConfigMap(commonCM)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("loading common content: %w", err)
	}
	liveContent, err := loadContentFromConfigMap(liveCM)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("loading live content: %w", err)
	}
	targetContent, err := loadContentFromConfigMap(targetCM)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("loading target content: %w", err)
	}

	// Build the target ignition config (based on what HyperShift provided).
	targetIgn := baseIgn
	if cr.Spec.MachineOSURL != "" {
		if err := ReplaceMachineOSURL(targetIgn, cr.Spec.MachineOSURL); err != nil {
			log.Error(err, "Failed to replace MachineOSURL, continuing without replacement")
		}
	}
	addContentToIgnition(targetIgn, targetContent)
	addContentToIgnition(targetIgn, commonContent)
	AddFlavorOVSScript(targetIgn, ovsScript)
	if cr.Spec.MTU9000 {
		EnableMTU9000(targetIgn)
	}

	encodedTarget, err := EncodeIgnition(targetIgn)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("encoding target ignition: %w", err)
	}

	// Build the live (first-stage) ignition config.
	liveIgn := NewEmptyIgnition("3.4.0")
	liveIgn.Passwd = targetIgn.Passwd
	addContentToIgnition(liveIgn, liveContent)
	addContentToIgnition(liveIgn, commonContent)

	// Embed the encoded target ignition as /var/target.ign in the live image.
	mode644 := octalToDecimal(644)
	targetIgnFC := FileContents{Source: urlEncode(encodedTarget)}
	liveIgn.Storage.Files = append(liveIgn.Storage.Files, FileEntry{
		Path:     "/var/target.ign",
		Mode:     &mode644,
		Contents: &targetIgnFC,
	})

	liveJSON, err := json.Marshal(liveIgn)
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("marshaling live ignition: %w", err)
	}

	// Create or update the output ConfigMap in dpf-operator-system.
	outCM := &corev1.ConfigMap{
		ObjectMeta: metav1.ObjectMeta{
			Name:      outputConfigMapName,
			Namespace: DPFOperatorNamespace,
		},
	}
	op, err := controllerutil.CreateOrUpdate(ctx, m.client, outCM, func() error {
		if outCM.Data == nil {
			outCM.Data = make(map[string]string)
		}
		outCM.Data["BF_CFG_TEMPLATE"] = string(liveJSON)
		return nil
	})
	if err != nil {
		return ctrl.Result{}, fmt.Errorf("creating/updating output ConfigMap: %w", err)
	}
	log.Info("Output ConfigMap reconciled", "operation", op, "name", outputConfigMapName, "namespace", DPFOperatorNamespace)

	if err := m.setCondition(ctx, cr, metav1.ConditionTrue, "IgnitionGenerated",
		"Ignition config generated and applied successfully"); err != nil {
		return ctrl.Result{}, err
	}

	log.Info("Ignition generation complete")
	return ctrl.Result{}, nil
}

// findToken searches the given namespace for a Secret whose name contains "token-{clusterName}"
// and returns the value of its "token" data key.
func (m *IgnitionManager) findToken(ctx context.Context, namespace, clusterName string) (string, error) {
	var secrets corev1.SecretList
	if err := m.client.List(ctx, &secrets, client.InNamespace(namespace)); err != nil {
		return "", fmt.Errorf("listing secrets in %s: %w", namespace, err)
	}
	log := logf.FromContext(ctx)
	searchStr := "token-" + clusterName
	for _, s := range secrets.Items {
		if strings.Contains(s.Name, searchStr) {
			log.Info("Found token secret candidate", "name", s.Name, "namespace", namespace, "type", s.Type)
			if raw, ok := s.Data["token"]; ok {
				// The Go client base64-decodes Secret.data automatically.
				// Re-encode to match what oc/kubectl jsonpath={.data.token} returns,
				// which is what the ignition server expects as the Bearer token.
				token := base64.StdEncoding.EncodeToString(raw)
				log.Info("Using ignition token", "secretName", s.Name, "tokenPrefix", token[:min(16, len(token))])
				return token, nil
			}
		}
	}
	return "", fmt.Errorf("no token secret containing %q found in namespace %s", searchStr, namespace)
}

// fetchIgnition fetches the ignition JSON from the HyperShift ignition endpoint.
func (m *IgnitionManager) fetchIgnition(ctx context.Context, endpoint, token string) (ign *Ignition, err error) {
	httpClient := &http.Client{
		Transport: &http.Transport{
			TLSClientConfig: &tls.Config{InsecureSkipVerify: true}, //nolint:gosec // ignition endpoint uses self-signed cert
		},
	}

	reqURL := "https://" + endpoint + "/ignition"
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, reqURL, nil)
	if err != nil {
		return nil, fmt.Errorf("creating request: %w", err)
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/json")

	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("GET %s: %w", reqURL, err)
	}

	defer func() {
		if cerr := resp.Body.Close(); cerr != nil && err == nil {
			err = fmt.Errorf("response body close: %w", cerr)
		}
	}()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("reading response body: %w", err)
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("unexpected status %d from %s: %s", resp.StatusCode, reqURL, string(body))
	}

	if err := json.Unmarshal(body, &ign); err != nil {
		return nil, fmt.Errorf("unmarshaling ignition response: %w", err)
	}
	return ign, nil
}

// setCondition updates the IgnitionConfigured condition on the DPFHCPProvisioner CR.
// It only writes to the API server when the condition actually changes.
func (m *IgnitionManager) setCondition(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, status metav1.ConditionStatus, reason, message string) error {
	log := logf.FromContext(ctx)

	condition := metav1.Condition{
		Type:               provisioningv1alpha1.IgnitionConfigured,
		Status:             status,
		Reason:             reason,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		log.V(1).Info("Updating IgnitionConfigured condition", "status", status, "reason", reason)
		eventType := "Normal"
		eventReason := "IgnitionConfigured"
		if status == metav1.ConditionFalse {
			eventType = "Warning"
			eventReason = "IgnitionConfigurationFailed"
		}
		m.recorder.Event(cr, eventType, eventReason, message)
		if err := m.client.Status().Update(ctx, cr); err != nil {
			return fmt.Errorf("updating IgnitionConfigured condition: %w", err)
		}
	}

	return nil
}
