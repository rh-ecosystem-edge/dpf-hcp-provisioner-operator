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
	"fmt"
	"sync"
	"time"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/tools/clientcmd"
	clientcmdapi "k8s.io/client-go/tools/clientcmd/api"
	"sigs.k8s.io/controller-runtime/pkg/client"
)

// ClientManager manages hosted cluster client lifecycle
type ClientManager struct {
	mgmtClient client.Client
	// mu protects concurrent access to hcClients map
	// Multiple reconciliations can run concurrently, so we need to protect map access
	mu sync.RWMutex
	// hcClients caches Kubernetes clientsets for hosted clusters to avoid recreating them on every reconciliation.
	// Each DPFHCPProvisioner creates a hosted cluster with its own API server. This map stores one clientset
	// per hosted cluster (keyed by "namespace/name") to reuse connections and avoid expensive client creation
	// (parsing kubeconfig, establishing TCP connections) every 30 seconds during CSR polling.
	// Without this cache, we would create 120+ clients per hour per hosted cluster.
	hcClients map[string]*kubernetes.Clientset
}

// NewClientManager creates a new client manager
func NewClientManager(mgmtClient client.Client) *ClientManager {
	return &ClientManager{
		mgmtClient: mgmtClient,
		hcClients:  make(map[string]*kubernetes.Clientset),
	}
}

// GetHostedClusterClient retrieves or creates a client for the hosted cluster
func (cm *ClientManager) GetHostedClusterClient(ctx context.Context, namespace, name string) (*kubernetes.Clientset, error) {
	key := namespace + "/" + name

	// Check cache with read lock
	cm.mu.RLock()
	if clientset, ok := cm.hcClients[key]; ok {
		cm.mu.RUnlock()
		return clientset, nil
	}
	cm.mu.RUnlock()

	// Create new client (outside lock to avoid holding lock during slow operation)
	clientset, err := cm.createHostedClusterClient(ctx, namespace, name)
	if err != nil {
		return nil, err
	}

	// Cache the client with write lock
	cm.mu.Lock()
	cm.hcClients[key] = clientset
	cm.mu.Unlock()

	return clientset, nil
}

// InvalidateClient removes a cached client (useful when kubeconfig rotates)
func (cm *ClientManager) InvalidateClient(namespace, name string) {
	key := namespace + "/" + name
	cm.mu.Lock()
	delete(cm.hcClients, key)
	cm.mu.Unlock()
}

// createHostedClusterClient creates a Kubernetes client for the hosted cluster
func (cm *ClientManager) createHostedClusterClient(ctx context.Context, namespace, name string) (*kubernetes.Clientset, error) {
	// Fetch kubeconfig secret
	kubeconfigData, err := cm.getKubeconfigData(ctx, namespace, name)
	if err != nil {
		return nil, fmt.Errorf("failed to get kubeconfig: %w", err)
	}

	// Parse kubeconfig
	kubeconfig, err := clientcmd.Load(kubeconfigData)
	if err != nil {
		return nil, fmt.Errorf("failed to parse kubeconfig: %w", err)
	}

	// Replace external endpoint with internal service DNS name
	// The HyperShift admin-kubeconfig uses external endpoints (LoadBalancer IP or NodePort)
	// which are not accessible from inside the operator pod's network.
	// We need to use the internal service endpoint for in-cluster access.
	if err := replaceServerWithInternalEndpoint(kubeconfig, namespace, name); err != nil {
		return nil, fmt.Errorf("failed to replace server endpoint: %w", err)
	}

	// Create REST config from modified kubeconfig
	config, err := clientcmd.NewDefaultClientConfig(*kubeconfig, &clientcmd.ConfigOverrides{}).ClientConfig()
	if err != nil {
		return nil, fmt.Errorf("failed to create rest config from kubeconfig: %w", err)
	}

	// Set reasonable timeouts for CSR API operations
	// We use List/Get/UpdateApproval operations (not watches), so a 30s timeout is appropriate
	config.Timeout = 30 * time.Second
	config.QPS = 5
	config.Burst = 10

	// Create clientset
	clientset, err := kubernetes.NewForConfig(config)
	if err != nil {
		return nil, fmt.Errorf("failed to create clientset: %w", err)
	}

	return clientset, nil
}

// getKubeconfigData retrieves the kubeconfig data from the admin secret
func (cm *ClientManager) getKubeconfigData(ctx context.Context, namespace, name string) ([]byte, error) {
	// The kubeconfig secret name follows HyperShift convention: <hostedcluster-name>-admin-kubeconfig
	secretName := name + "-admin-kubeconfig"

	secret := &corev1.Secret{}
	secretKey := types.NamespacedName{
		Namespace: namespace,
		Name:      secretName,
	}

	if err := cm.mgmtClient.Get(ctx, secretKey, secret); err != nil {
		return nil, fmt.Errorf("failed to get kubeconfig secret %s: %w", secretKey, err)
	}

	kubeconfigData, ok := secret.Data["kubeconfig"]
	if !ok {
		return nil, fmt.Errorf("kubeconfig key not found in secret %s", secretKey)
	}

	if len(kubeconfigData) == 0 {
		return nil, fmt.Errorf("kubeconfig data is empty in secret %s", secretKey)
	}

	return kubeconfigData, nil
}

// replaceServerWithInternalEndpoint modifies the kubeconfig to use internal service DNS name
// instead of the external LoadBalancer IP or NodePort. This allows the operator pod (running inside the cluster)
// to reach the hosted cluster API server via the internal Kubernetes service.
//
// HyperShift creates admin-kubeconfig with external endpoints:
// - LoadBalancer: https://10.6.135.42:6443 (example external IP, not accessible from operator pod)
// - NodePort: https://<node-ip>:31039 (example NodePort, dynamically allocated per cluster)
//
// This function replaces it with the internal service DNS name:
// https://kube-apiserver.<namespace>-<name>.svc.cluster.local:6443
//
// Port 6443 is hardcoded to match HyperShift's implementation. HyperShift itself hardcodes
// the kube-apiserver port as a constant (KASSVCPort = 6443) in their codebase.
func replaceServerWithInternalEndpoint(kubeconfig *clientcmdapi.Config, hostedClusterNamespace, hostedClusterName string) error {
	if kubeconfig == nil {
		return fmt.Errorf("kubeconfig is nil")
	}

	// Find the current context
	currentContext := kubeconfig.CurrentContext
	if currentContext == "" {
		return fmt.Errorf("kubeconfig has no current context")
	}

	ctxConfig, ok := kubeconfig.Contexts[currentContext]
	if !ok {
		return fmt.Errorf("context %s not found in kubeconfig", currentContext)
	}

	// Find the cluster referenced by the context
	clusterName := ctxConfig.Cluster
	cluster, ok := kubeconfig.Clusters[clusterName]
	if !ok {
		return fmt.Errorf("cluster %s not found in kubeconfig", clusterName)
	}

	// Construct the service namespace following HyperShift convention
	serviceNamespace := fmt.Sprintf("%s-%s", hostedClusterNamespace, hostedClusterName)

	// Construct internal service DNS name with hardcoded port 6443 (matching HyperShift's approach)
	internalServer := fmt.Sprintf("https://kube-apiserver.%s.svc.cluster.local:6443", serviceNamespace)

	// Replace the server URL
	cluster.Server = internalServer

	return nil
}

// TestConnection verifies the hosted cluster client can connect to the API server
func TestConnection(ctx context.Context, clientset *kubernetes.Clientset) error {
	_, err := clientset.Discovery().ServerVersion()
	if err != nil {
		return fmt.Errorf("failed to connect to hosted cluster API server: %w", err)
	}
	return nil
}
