//! Port of internal/controller/ovs-hugepages/ovs-hugepages.go

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};

use apis::provisioning::v1alpha1::DPFHCPProvisioner;
use ctrl_dpuservicetemplate::ReleaseImageReader;
use ctrlrt::error::{go_quote, is_not_found};
use ctrlrt::k8s_openapi::api::apps::v1::{DaemonSet, DaemonSetSpec};
use ctrlrt::k8s_openapi::api::core::v1::{
    Container, Namespace, PodSpec, PodTemplateSpec, ResourceRequirements, Toleration,
};
use ctrlrt::k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use ctrlrt::{Client, Ctx, Object, Result, kv, wrapf};
use hostedclient::{ClientManager, HostedClient, OperationResult, create_or_update};
use registry::authn::Keychain;

use crate::quantity;

/// resolves to the "ose-pod" image in the release payload
const PAUSE_PAYLOAD_IMAGE: &str = "pod";
/// hugepage size selecting the "hugepages-2Mi" resource
pub const DEFAULT_HUGEPAGES_SIZE: &str = "2Mi";
/// pages per node; 250 x 2Mi = 500Mi reserved for OVS
pub const DEFAULT_HUGEPAGES_AMOUNT: i32 = 250;

/// OVSHugepagesNamespace holds the reservation DaemonSet in the hosted cluster.
pub const OVS_HUGEPAGES_NAMESPACE: &str = "openshift-doca-hugepages-holder";
/// DaemonSetName is the name of the reservation DaemonSet in the hosted cluster.
pub const DAEMON_SET_NAME: &str = "ovs-hugepages-reservation";
/// containerName is the name of the reservation container.
pub(crate) const CONTAINER_NAME: &str = "allocate";
/// reservationPriorityClassName ranks the reservation pods above ordinary workloads so they
/// reliably schedule (and, if needed, preempt) to hold the OVS-reserved hugepages on every
/// node. It is a built-in OpenShift priority class that always exists on the hosted cluster,
/// so referencing it never blocks pod admission.
const RESERVATION_PRIORITY_CLASS_NAME: &str = "system-node-critical";

/// Manager reconciles the hugepages reservation DaemonSet inside hosted clusters.
pub struct Manager {
    pub(crate) mgmt_client: Client,
    pub(crate) client_manager: Arc<ClientManager>,
    pub(crate) release_reader: Arc<dyn ReleaseImageReader>,

    /// pauseImages caches the resolved pause image per release image ref so we don't
    /// re-pull the release payload on every reconcile. (Go relies on reconciles running
    /// serially; the mutex only provides the interior mutability `&self` needs and is never
    /// held across an await.)
    pub(crate) pause_images: Mutex<HashMap<String, String>>,
}

impl std::fmt::Debug for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Manager").finish_non_exhaustive()
    }
}

impl Manager {
    /// `NewManager(mgmtClient, clientManager, releaseReader)`
    pub fn new(
        mgmt_client: Client,
        client_manager: Arc<ClientManager>,
        release_reader: Arc<dyn ReleaseImageReader>,
    ) -> Self {
        Self {
            mgmt_client,
            client_manager,
            release_reader,
            pause_images: Mutex::new(HashMap::new()),
        }
    }

    fn pause_images(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.pause_images.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `ReconcileHugepagesDaemonSet` ensures the hugepages reservation namespace and
    /// DaemonSet exist in the hosted cluster for the given DPFHCPProvisioner. It is
    /// idempotent and safe to call on every reconcile.
    ///
    /// size is the hugepage size (selects the "hugepages-<size>" resource, e.g. "2Mi") and
    /// amount is the number of pages reserved per node (e.g. 250). The reserved quantity is
    /// amount x size (e.g. 250 x 2Mi = 500Mi). An empty size falls back to
    /// [`DEFAULT_HUGEPAGES_SIZE`]; an amount of zero disables the reservation. An omitted
    /// amount is defaulted to [`DEFAULT_HUGEPAGES_AMOUNT`] by the DPFHCPProvisionerConfig CRD.
    pub async fn reconcile_hugepages_daemon_set(
        &self,
        ctx: &Ctx,
        cr: &DPFHCPProvisioner,
        size: &str,
        amount: i32,
    ) -> Result<()> {
        let size = if size.is_empty() {
            DEFAULT_HUGEPAGES_SIZE
        } else {
            size
        };
        let amount = if amount < 0 {
            DEFAULT_HUGEPAGES_AMOUNT
        } else {
            amount
        };

        let hc_client = self
            .client_manager
            .get_hosted_cluster_client(ctx, cr.namespace(), cr.name())
            .await
            .map_err(|err| wrapf!(err, "failed to get hosted cluster client: {}", err))?;

        // When disabled (amount == 0) no image is needed: ensure_daemon_set just removes
        // any existing reservation.
        if amount == 0 {
            if let Err(err) = ensure_daemon_set(ctx, &hc_client, "", size, amount).await {
                self.invalidate_stale_client(cr);
                return Err(err);
            }
            return Ok(());
        }

        if let Err(err) = ensure_namespace(ctx, &hc_client).await {
            self.invalidate_stale_client(cr);
            return Err(err);
        }

        // Resolve the pause image from the hosted cluster's own release payload so the
        // reservation pods run the exact image their nodes already have. On failure we
        // return an error so the reconcile requeues rather than deploying a guessed image.
        let pause_image = self.resolve_pause_image(ctx, cr).await?;

        // Go's `resource.MustParse(size)` in buildDaemonSet panics on an invalid size (the
        // CRD enum prevents that); the panic unwinds past the client invalidation below, so
        // the equivalent error is returned here, before it.
        hugepages_quantity(size, amount)?;

        if let Err(err) = ensure_daemon_set(ctx, &hc_client, &pause_image, size, amount).await {
            self.invalidate_stale_client(cr);
            return Err(err);
        }
        Ok(())
    }

    /// `invalidateStaleClient` drops the cached hosted-cluster clientset after a hosted
    /// cluster API call fails, so the next reconcile rebuilds it from the current kubeconfig
    /// secret. This self-heals stale cached clients, e.g. when the hosted cluster was deleted
    /// and recreated with the same name or its kubeconfig was rotated. Same approach as the
    /// CSR approver.
    fn invalidate_stale_client(&self, cr: &DPFHCPProvisioner) {
        self.client_manager
            .invalidate_client(cr.namespace(), cr.name());
    }

    /// `resolvePauseImage` returns the pause/sandbox image for the reservation pods,
    /// resolved from the aarch64-specific release payload and cached per release image. DPU
    /// nodes are aarch64, so resolution fails rather than risking an image for another
    /// architecture.
    pub(crate) async fn resolve_pause_image(
        &self,
        ctx: &Ctx,
        cr: &DPFHCPProvisioner,
    ) -> Result<String> {
        let log = &ctx.log;
        let release_image = cr.spec.ocp_release_image.as_str();

        if let Some(cached) = self.pause_images().get(release_image) {
            return Ok(cached.clone());
        }

        let keychain = common::keychain_from_pull_secret(
            &self.mgmt_client,
            &cr.spec.pull_secret_ref.name,
            cr.namespace(),
        )
        .await
        .map_err(|err| wrapf!(err, "getting pull secret keychain: {}", err))?;

        let resolved =
            if common::is_multi_arch_release_image(ctx, release_image, keychain.clone()).await {
                // Multi-arch release: component images listed in the payload are multi-arch
                // manifest lists, so the kubelet on aarch64 DPU nodes pulls the correct
                // architecture automatically.
                let resolved = self
                    .release_reader
                    .get_component_image(ctx, release_image, PAUSE_PAYLOAD_IMAGE, keychain)
                    .await
                    .map_err(|err| {
                        wrapf!(
                            err,
                            "resolving pause image from release {}: {}",
                            go_quote(release_image),
                            err
                        )
                    })?;
                if resolved.is_empty() {
                    return Err(anyhow::anyhow!(
                        "component {} resolved to an empty image from release {}",
                        go_quote(PAUSE_PAYLOAD_IMAGE),
                        go_quote(release_image)
                    ));
                }
                resolved
            } else {
                self.try_aarch64_release(ctx, release_image, keychain)
                    .await
                    .map_err(|err| {
                        wrapf!(
                            err,
                            "resolving aarch64 pause image from release {}: {}",
                            go_quote(release_image),
                            err
                        )
                    })?
            };

        self.pause_images()
            .insert(release_image.to_string(), resolved.clone());
        log.v(1).info(
            "Resolved hugepages reservation pause image",
            kv!["releaseImage" => release_image, "pauseImage" => resolved],
        );
        Ok(resolved)
    }

    /// `tryAarch64Release` resolves the pause image from the aarch64-specific release
    /// payload.
    async fn try_aarch64_release(
        &self,
        ctx: &Ctx,
        release_image: &str,
        keychain: Arc<dyn Keychain>,
    ) -> Result<String> {
        let version = ctrl_bfocplookup::extract_ocp_version(ctx, release_image, keychain.clone())
            .await
            .map_err(|err| wrapf!(err, "extracting OCP version: {}", err))?;
        let aarch64_release = common::aarch64_release_image(release_image, &version);
        let resolved = self
            .release_reader
            .get_component_image(ctx, &aarch64_release, PAUSE_PAYLOAD_IMAGE, keychain)
            .await
            .map_err(|err| {
                wrapf!(
                    err,
                    "reading release {}: {}",
                    go_quote(&aarch64_release),
                    err
                )
            })?;
        if resolved.is_empty() {
            return Err(anyhow::anyhow!(
                "component {} resolved to an empty image from release {}",
                go_quote(PAUSE_PAYLOAD_IMAGE),
                go_quote(&aarch64_release)
            ));
        }
        Ok(resolved)
    }
}

/// `ensureNamespace` creates the reservation namespace in the hosted cluster if missing.
pub(crate) async fn ensure_namespace(ctx: &Ctx, hc_client: &HostedClient) -> Result<()> {
    let mut ns = Namespace {
        metadata: ObjectMeta {
            name: Some(OVS_HUGEPAGES_NAMESPACE.to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let namespaces = hc_client.resource::<Namespace>("");
    create_or_update(
        ctx,
        &namespaces,
        &mut ns,
        Some(&mut |ns: &mut Namespace| {
            let ns_labels = ns.metadata.labels.get_or_insert_with(BTreeMap::new);
            for (k, v) in labels() {
                ns_labels.insert(k, v);
            }
            Ok(())
        }),
    )
    .await
    .map_err(|err| {
        wrapf!(
            err,
            "failed to ensure namespace {}: {}",
            OVS_HUGEPAGES_NAMESPACE,
            err
        )
    })?;
    Ok(())
}

/// `ensureDaemonSet` reconciles the reservation DaemonSet in the hosted cluster and logs
/// what it did. An amount of zero removes any existing DaemonSet so its pods stop reserving
/// pages.
pub(crate) async fn ensure_daemon_set(
    ctx: &Ctx,
    hc_client: &HostedClient,
    image: &str,
    size: &str,
    amount: i32,
) -> Result<()> {
    let log = &ctx.log;
    let ds_client = hc_client.resource::<DaemonSet>(OVS_HUGEPAGES_NAMESPACE);

    if amount == 0 {
        if let Err(err) = ds_client.delete(ctx, DAEMON_SET_NAME).await
            && !is_not_found(&err)
        {
            return Err(wrapf!(
                err,
                "failed to delete DaemonSet {}/{}: {}",
                OVS_HUGEPAGES_NAMESPACE,
                DAEMON_SET_NAME,
                err
            ));
        }
        log.v(1).info(
            "Hugepages reservation disabled in hosted cluster",
            kv!["namespace" => OVS_HUGEPAGES_NAMESPACE, "name" => DAEMON_SET_NAME],
        );
        return Ok(());
    }

    let mut desired = build_daemon_set(image, size, amount)?;
    let desired_labels = desired.metadata.labels.clone();
    let desired_template = desired
        .spec
        .as_ref()
        .map(|s| s.template.clone())
        .unwrap_or_default();
    let result = create_or_update(
        ctx,
        &ds_client,
        &mut desired,
        Some(&mut |ds: &mut DaemonSet| {
            ds.metadata.labels = desired_labels.clone();
            ds.spec.get_or_insert_with(Default::default).template = desired_template.clone();
            Ok(())
        }),
    )
    .await
    .map_err(|err| {
        wrapf!(
            err,
            "failed to ensure DaemonSet {}/{}: {}",
            OVS_HUGEPAGES_NAMESPACE,
            DAEMON_SET_NAME,
            err
        )
    })?;

    match result {
        OperationResult::Created => log.v(1).info(
            "Created hugepages reservation DaemonSet in hosted cluster",
            kv![
                "namespace" => OVS_HUGEPAGES_NAMESPACE,
                "name" => DAEMON_SET_NAME,
                "hugepagesSize" => size,
                "hugepagesCount" => amount,
            ],
        ),
        OperationResult::Updated => log.v(1).info(
            "Updated hugepages reservation DaemonSet in hosted cluster (drift corrected)",
            kv![
                "namespace" => OVS_HUGEPAGES_NAMESPACE,
                "name" => DAEMON_SET_NAME,
                "hugepagesSize" => size,
                "hugepagesCount" => amount,
            ],
        ),
        OperationResult::None => log.v(1).info(
            "Hugepages reservation DaemonSet up to date in hosted cluster",
            kv!["namespace" => OVS_HUGEPAGES_NAMESPACE, "name" => DAEMON_SET_NAME],
        ),
    }
    Ok(())
}

/// `buildDaemonSet` builds the reservation DaemonSet. The pod does nothing but idle; its
/// resource requests/limits reserve the hugepages on every node it lands on. image is the
/// pause image the reservation pods run; size selects the "hugepages-<size>" resource and
/// amount is the number of pages; the reserved quantity is amount x size.
///
/// Fails (where Go's `resource.MustParse` panics) when size is not a valid quantity.
pub(crate) fn build_daemon_set(image: &str, size: &str, amount: i32) -> Result<DaemonSet> {
    let hugepages_resource = format!("hugepages-{size}");
    let hugepages_qty = hugepages_quantity(size, amount)?;

    let requests = BTreeMap::from([
        ("cpu".to_string(), Quantity("1m".into())),
        ("memory".to_string(), Quantity("32Mi".into())),
        (hugepages_resource.clone(), hugepages_qty.clone()),
    ]);
    let limits = BTreeMap::from([
        ("cpu".to_string(), Quantity("1m".into())),
        ("memory".to_string(), Quantity("64Mi".into())),
        (hugepages_resource, hugepages_qty),
    ]);

    Ok(DaemonSet {
        metadata: ObjectMeta {
            name: Some(DAEMON_SET_NAME.to_string()),
            namespace: Some(OVS_HUGEPAGES_NAMESPACE.to_string()),
            labels: Some(labels()),
            ..Default::default()
        },
        spec: Some(DaemonSetSpec {
            selector: LabelSelector {
                match_labels: Some(labels()),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels()),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    // Rank above ordinary workloads so the reservation reliably holds the
                    // hugepages that must not be consumed by other pods.
                    priority_class_name: Some(RESERVATION_PRIORITY_CLASS_NAME.to_string()),
                    // Every node of the hosted cluster is a DPU, so there is no nodeSelector
                    // — we want one pod on every node. Tolerate all taints so the
                    // reservation lands even on tainted/not-ready nodes.
                    tolerations: Some(vec![Toleration {
                        operator: Some("Exists".to_string()),
                        ..Default::default()
                    }]),
                    containers: vec![Container {
                        name: CONTAINER_NAME.to_string(),
                        image: Some(image.to_string()),
                        resources: Some(ResourceRequirements {
                            requests: Some(requests),
                            limits: Some(limits),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// `hugepagesQuantity` returns the total hugepages quantity to reserve: amount pages each of
/// the given page size (e.g. size "2Mi", amount 250 -> 500Mi). The result is always a
/// multiple of the page size, as Kubernetes requires for hugepage resources.
pub(crate) fn hugepages_quantity(size: &str, amount: i32) -> Result<Quantity> {
    let page_size = quantity::must_parse_value(size).map_err(|e| anyhow::anyhow!(e))?;
    Ok(Quantity(quantity::new_binary_si_string(
        page_size.wrapping_mul(i64::from(amount)),
    )))
}

/// `labels` returns the common labels applied to the reservation resources.
pub(crate) fn labels() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "app.kubernetes.io/name".to_string(),
            DAEMON_SET_NAME.to_string(),
        ),
        (
            "app.kubernetes.io/managed-by".to_string(),
            "dpf-hcp-provisioner-operator".to_string(),
        ),
    ])
}
