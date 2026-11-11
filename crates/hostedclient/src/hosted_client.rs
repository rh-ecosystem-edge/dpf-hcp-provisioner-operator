//! [`HostedClient`]: the Rust stand-in for a hosted cluster's `*kubernetes.Clientset`.

use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use ctrlrt::error::{ApiError, Result};
use ctrlrt::{Client, Ctx, DeleteOptions, ListOptions, Object, ObjectKey};
use k8s_openapi::apimachinery::pkg::version::Info;

use crate::create_or_update::HostedResourceClient;

/// A fake `Discovery().ServerVersion()` implementation (client-go `FakeDiscovery`).
pub type ServerVersionFn = Arc<dyn Fn() -> Result<Info> + Send + Sync>;

#[derive(Clone)]
enum Discovery {
    /// Real API server: `GET /version`.
    Kube(kube::Client),
    /// Fake discovery (client-go `kubernetes/fake`): returns the configured result.
    Fake(ServerVersionFn),
}

/// Hosted cluster clientset (`*kubernetes.Clientset` / `kubernetes.Interface`).
///
/// Cheap to clone; clones share the underlying connection. Derefs to the typed
/// [`ctrlrt::Client`].
#[derive(Clone)]
pub struct HostedClient {
    client: Client,
    discovery: Discovery,
}

impl std::fmt::Debug for HostedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedClient").finish_non_exhaustive()
    }
}

impl HostedClient {
    /// Clientset backed by a real API server (`kubernetes.NewForConfig`).
    pub fn new(client: Client, kube: kube::Client) -> Self {
        Self {
            client,
            discovery: Discovery::Kube(kube),
        }
    }

    /// Clientset backed by a fake (e.g. `ctrlrt_fake::new_simple_clientset`), with client-go
    /// `FakeDiscovery` semantics: `ServerVersion()` succeeds (returns an empty version info).
    pub fn fake(client: Client) -> Self {
        Self {
            client,
            discovery: Discovery::Fake(Arc::new(|| Ok(Info::default()))),
        }
    }

    /// Replace the fake discovery `ServerVersion()` behaviour (e.g. to simulate an
    /// unreachable API server via a prepended `get version` reactor in Go tests).
    pub fn with_server_version(mut self, f: ServerVersionFn) -> Self {
        self.discovery = Discovery::Fake(f);
        self
    }

    /// The typed client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The underlying `kube::Client` when connected to a real API server.
    pub fn kube_client(&self) -> Option<&kube::Client> {
        match &self.discovery {
            Discovery::Kube(k) => Some(k),
            Discovery::Fake(_) => None,
        }
    }

    /// `clientset.Discovery().ServerVersion()`
    pub async fn server_version(&self) -> Result<Info> {
        match &self.discovery {
            Discovery::Kube(k) => ctrlrt_runtime::server_version(k).await,
            Discovery::Fake(f) => f(),
        }
    }

    /// A typed client scoped to a namespace (`CoreV1().Namespaces()`,
    /// `AppsV1().DaemonSets(ns)`, ...). The namespace is ignored for cluster-scoped kinds.
    pub fn resource<K: Object>(&self, namespace: &str) -> ResourceClient<K> {
        ResourceClient {
            client: self.client.clone(),
            namespace: if K::resource_info().namespaced {
                namespace.to_string()
            } else {
                String::new()
            },
            _marker: PhantomData,
        }
    }
}

impl std::ops::Deref for HostedClient {
    type Target = Client;
    fn deref(&self) -> &Client {
        &self.client
    }
}

/// A generated typed resource client (e.g. `typedcorev1.NamespaceInterface`), scoped to one
/// namespace like client-go's `DaemonSets(ns)`.
pub struct ResourceClient<K> {
    client: Client,
    namespace: String,
    _marker: PhantomData<fn() -> K>,
}

impl<K> Clone for ResourceClient<K> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            namespace: self.namespace.clone(),
            _marker: PhantomData,
        }
    }
}

impl<K: Object> ResourceClient<K> {
    /// The namespace this client is scoped to (empty for cluster-scoped kinds).
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The request body of a typed `Create`/`Update`: the object with the client's namespace
    /// (the API server fills an empty namespace from the request path and rejects a
    /// mismatching one).
    fn body(&self, obj: &K) -> Result<K> {
        let mut obj = obj.clone();
        if K::resource_info().namespaced {
            let ns = obj.namespace().to_string();
            if ns.is_empty() {
                obj.object_meta_mut().namespace = Some(self.namespace.clone());
            } else if ns != self.namespace {
                return Err(ApiError::bad_request(
                    "the namespace of the provided object does not match the namespace sent on the request",
                )
                .into());
            }
        }
        Ok(obj)
    }

    /// `Get(ctx, name, metav1.GetOptions{})`
    pub async fn get(&self, _ctx: &Ctx, name: &str) -> Result<K> {
        self.client
            .get::<K>(&ObjectKey::new(self.namespace.clone(), name))
            .await
    }

    /// `List(ctx, metav1.ListOptions{})`
    pub async fn list(&self, _ctx: &Ctx) -> Result<Vec<K>> {
        self.client
            .list::<K>(&ListOptions::new().in_namespace(self.namespace.clone()))
            .await
    }

    /// `Create(ctx, obj, metav1.CreateOptions{})`; returns the server's copy.
    pub async fn create(&self, _ctx: &Ctx, obj: &K) -> Result<K> {
        let mut body = self.body(obj)?;
        self.client.create(&mut body).await?;
        Ok(body)
    }

    /// `Update(ctx, obj, metav1.UpdateOptions{})`; returns the server's copy.
    pub async fn update(&self, _ctx: &Ctx, obj: &K) -> Result<K> {
        let mut body = self.body(obj)?;
        self.client.update(&mut body).await?;
        Ok(body)
    }

    /// `Delete(ctx, name, metav1.DeleteOptions{})`
    pub async fn delete(&self, _ctx: &Ctx, name: &str) -> Result<()> {
        self.client
            .delete_key::<K>(
                &ObjectKey::new(self.namespace.clone(), name),
                &DeleteOptions::default(),
            )
            .await
    }
}

#[async_trait]
impl<K: Object> HostedResourceClient<K> for ResourceClient<K> {
    async fn get(&self, ctx: &Ctx, name: &str) -> Result<K> {
        ResourceClient::get(self, ctx, name).await
    }
    async fn create(&self, ctx: &Ctx, obj: &K) -> Result<K> {
        ResourceClient::create(self, ctx, obj).await
    }
    async fn update(&self, ctx: &Ctx, obj: &K) -> Result<K> {
        ResourceClient::update(self, ctx, obj).await
    }
}
