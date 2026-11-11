//! `ClientManager`: per-hosted-cluster client cache and kubeconfig handling.

use std::collections::{BTreeSet, HashMap};
use std::sync::RwLock;
use std::time::Duration;

use ctrlrt::error::Result;
use ctrlrt::{Client, Ctx, ObjectKey, wrapf};
use ctrlrt_runtime::KubeconfigOverrides;
use k8s_openapi::api::core::v1::Secret;
use kube::config::Kubeconfig;

use crate::hosted_client::HostedClient;

/// `config.Timeout` for hosted-cluster clients. Callers use request/response calls (not
/// watches), so a 30s timeout is appropriate.
const HOSTED_CLIENT_TIMEOUT: Duration = Duration::from_secs(30);
/// `config.QPS`
const HOSTED_CLIENT_QPS: f32 = 5.0;
/// `config.Burst`
const HOSTED_CLIENT_BURST: i32 = 10;

/// ClientManager manages hosted cluster client lifecycle.
///
/// Share it between reconcilers with `Arc<ClientManager>` (Go passes `*ClientManager`).
pub struct ClientManager {
    mgmt_client: Client,
    /// Caches clients for hosted clusters to avoid recreating them on every reconciliation.
    /// Each DPFHCPProvisioner creates a hosted cluster with its own API server. This map
    /// stores one client per hosted cluster (keyed by "namespace/name") to reuse connections
    /// and avoid expensive client creation (parsing kubeconfig, establishing TCP connections)
    /// every 30 seconds during CSR polling. The lock protects concurrent access, as multiple
    /// reconciliations can run concurrently.
    hc_clients: RwLock<HashMap<String, HostedClient>>,
}

impl std::fmt::Debug for ClientManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientManager").finish_non_exhaustive()
    }
}

fn cache_key(namespace: &str, name: &str) -> String {
    format!("{namespace}/{name}")
}

impl ClientManager {
    /// `NewClientManager(mgmtClient)`
    pub fn new(mgmt_client: Client) -> Self {
        Self {
            mgmt_client,
            hc_clients: RwLock::new(HashMap::new()),
        }
    }

    /// `GetHostedClusterClient` retrieves or creates a client for the hosted cluster.
    pub async fn get_hosted_cluster_client(
        &self,
        ctx: &Ctx,
        namespace: &str,
        name: &str,
    ) -> Result<HostedClient> {
        let key = cache_key(namespace, name);

        // Check cache with read lock
        if let Some(c) = self.read_cache().get(&key) {
            return Ok(c.clone());
        }

        // Create new client (outside lock to avoid holding lock during slow operation)
        let client = self.create_hosted_cluster_client(ctx, namespace, name).await?;

        // Cache the client with write lock
        self.write_cache().insert(key, client.clone());

        Ok(client)
    }

    /// `InvalidateClient` removes a cached client (useful when kubeconfig rotates).
    pub fn invalidate_client(&self, namespace: &str, name: &str) {
        self.write_cache().remove(&cache_key(namespace, name));
    }

    /// Inject a pre-built client for a hosted cluster into the cache (Go tests and
    /// in-package callers can write `cm.hcClients["ns/name"]` directly; this is the
    /// equivalent hook, e.g. to use a fake clientset).
    pub fn set_hosted_cluster_client(&self, namespace: &str, name: &str, client: HostedClient) {
        self.write_cache().insert(cache_key(namespace, name), client);
    }

    /// Whether a client is cached for the hosted cluster (`_, ok := cm.hcClients[key]`).
    pub fn has_cached_client(&self, namespace: &str, name: &str) -> bool {
        self.read_cache().contains_key(&cache_key(namespace, name))
    }

    /// Number of cached clients (`len(cm.hcClients)`).
    pub fn cached_client_count(&self) -> usize {
        self.read_cache().len()
    }

    fn read_cache(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, HostedClient>> {
        self.hc_clients.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write_cache(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, HostedClient>> {
        self.hc_clients.write().unwrap_or_else(|e| e.into_inner())
    }

    /// `createHostedClusterClient` creates a Kubernetes client for the hosted cluster.
    async fn create_hosted_cluster_client(
        &self,
        ctx: &Ctx,
        namespace: &str,
        name: &str,
    ) -> Result<HostedClient> {
        // Fetch kubeconfig secret
        let kubeconfig_data = self
            .get_kubeconfig_data(ctx, namespace, name)
            .await
            .map_err(|err| wrapf!(err, "failed to get kubeconfig: {}", err))?;

        // Parse kubeconfig
        let mut kubeconfig = load_kubeconfig(&kubeconfig_data)
            .map_err(|err| wrapf!(err, "failed to parse kubeconfig: {}", err))?;

        // Replace external endpoint with internal service DNS name.
        // The HyperShift admin-kubeconfig uses external endpoints (LoadBalancer IP or
        // NodePort) which are not accessible from inside the operator pod's network.
        // We need to use the internal service endpoint for in-cluster access.
        replace_server_with_internal_endpoint(Some(&mut kubeconfig), namespace, name)
            .map_err(|err| wrapf!(err, "failed to replace server endpoint: {}", err))?;

        // Create REST config from modified kubeconfig, with reasonable timeouts for
        // hosted-cluster API operations.
        let overrides = KubeconfigOverrides {
            server: None,
            timeout: HOSTED_CLIENT_TIMEOUT,
            qps: HOSTED_CLIENT_QPS,
            burst: HOSTED_CLIENT_BURST,
        };
        let config =
            ctrlrt_runtime::config::rest_config_from_kubeconfig_with(kubeconfig, &overrides)
                .await
                .map_err(|err| {
                    wrapf!(err, "failed to create rest config from kubeconfig: {}", err)
                })?;

        // Create clientset
        let (client, kube) = ctrlrt_runtime::client_from_rest_config(&config)
            .map_err(|err| wrapf!(err, "failed to create clientset: {}", err))?;

        Ok(HostedClient::new(client, kube))
    }

    /// `GetKubeconfigData` retrieves the kubeconfig data from the hosted cluster's admin
    /// secret.
    pub async fn get_kubeconfig_data(
        &self,
        _ctx: &Ctx,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<u8>> {
        // The kubeconfig secret name follows HyperShift convention:
        // <hostedcluster-name>-admin-kubeconfig
        let secret_key = ObjectKey::new(namespace, format!("{name}-admin-kubeconfig"));

        let secret: Secret = self
            .mgmt_client
            .get(&secret_key)
            .await
            .map_err(|err| wrapf!(err, "failed to get kubeconfig secret {}: {}", secret_key, err))?;

        let Some(kubeconfig_data) = secret.data.as_ref().and_then(|d| d.get("kubeconfig")) else {
            return Err(anyhow::anyhow!(
                "kubeconfig key not found in secret {}",
                secret_key
            ));
        };

        if kubeconfig_data.0.is_empty() {
            return Err(anyhow::anyhow!(
                "kubeconfig data is empty in secret {}",
                secret_key
            ));
        }

        Ok(kubeconfig_data.0.clone())
    }
}

/// `clientcmd.Load(data)`: parse kubeconfig bytes. Like client-go, empty input yields an
/// empty config and duplicate names in the named lists are rejected.
fn load_kubeconfig(data: &[u8]) -> Result<Kubeconfig> {
    if data.is_empty() {
        return Ok(Kubeconfig::default());
    }
    let kc = ctrlrt_runtime::parse_kubeconfig(data)?;
    check_duplicates("NamedCluster", "api.Cluster", kc.clusters.iter().map(|c| &c.name))?;
    check_duplicates("NamedAuthInfo", "api.AuthInfo", kc.auth_infos.iter().map(|c| &c.name))?;
    check_duplicates("NamedContext", "api.Context", kc.contexts.iter().map(|c| &c.name))?;
    Ok(kc)
}

fn check_duplicates<'a>(
    list: &str,
    target: &str,
    names: impl Iterator<Item = &'a String>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for n in names {
        if !seen.insert(n) {
            return Err(anyhow::anyhow!(
                "error converting *[]{} into *map[string]*{}: duplicate name \"{}\" in list",
                list,
                target,
                n
            ));
        }
    }
    Ok(())
}

/// `replaceServerWithInternalEndpoint` modifies the kubeconfig to use the internal service
/// DNS name instead of the external LoadBalancer IP or NodePort. This allows the operator pod
/// (running inside the cluster) to reach the hosted cluster API server via the internal
/// Kubernetes service.
///
/// HyperShift creates admin-kubeconfig with external endpoints:
/// - LoadBalancer: `https://10.6.135.42:6443` (example external IP, not accessible from
///   the operator pod)
/// - NodePort: `https://<node-ip>:31039` (example NodePort, dynamically allocated per cluster)
///
/// This function replaces it with the internal service DNS name:
/// `https://kube-apiserver.<namespace>-<name>.svc.cluster.local:6443`
///
/// Port 6443 is hardcoded to match HyperShift's implementation (`KASSVCPort = 6443`).
///
/// `None` mirrors a nil `*clientcmdapi.Config`.
pub fn replace_server_with_internal_endpoint(
    kubeconfig: Option<&mut Kubeconfig>,
    hosted_cluster_namespace: &str,
    hosted_cluster_name: &str,
) -> Result<()> {
    let Some(kubeconfig) = kubeconfig else {
        return Err(anyhow::anyhow!("kubeconfig is nil"));
    };

    // Find the current context
    let current_context = kubeconfig.current_context.clone().unwrap_or_default();
    if current_context.is_empty() {
        return Err(anyhow::anyhow!("kubeconfig has no current context"));
    }

    let Some(ctx_config) = kubeconfig
        .contexts
        .iter()
        .find(|c| c.name == current_context)
    else {
        return Err(anyhow::anyhow!(
            "context {} not found in kubeconfig",
            current_context
        ));
    };

    // Find the cluster referenced by the context
    let cluster_name = ctx_config
        .context
        .as_ref()
        .map(|c| c.cluster.clone())
        .unwrap_or_default();
    let Some(cluster) = kubeconfig
        .clusters
        .iter_mut()
        .find(|c| c.name == cluster_name)
    else {
        return Err(anyhow::anyhow!(
            "cluster {} not found in kubeconfig",
            cluster_name
        ));
    };

    // Construct the service namespace following HyperShift convention
    let service_namespace = format!("{hosted_cluster_namespace}-{hosted_cluster_name}");

    // Construct internal service DNS name with hardcoded port 6443 (matching HyperShift)
    let internal_server = format!("https://kube-apiserver.{service_namespace}.svc.cluster.local:6443");

    // Replace the server URL
    cluster.cluster.get_or_insert_with(Default::default).server = Some(internal_server);

    Ok(())
}

/// `TestConnection` verifies the hosted cluster client can connect to the API server.
pub async fn test_connection(_ctx: &Ctx, client: &HostedClient) -> Result<()> {
    client
        .server_version()
        .await
        .map_err(|err| wrapf!(err, "failed to connect to hosted cluster API server: {}", err))?;
    Ok(())
}
