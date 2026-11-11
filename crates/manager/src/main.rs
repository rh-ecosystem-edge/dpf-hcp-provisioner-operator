//! `manager` — the DPF HCP provisioner operator binary. Line-by-line port of `cmd/main.go`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ctrlrt::k8s_openapi::api::core::v1::ConfigMap;
use ctrlrt::log::Logger;
use ctrlrt::{ManagerHandle, kv};
use ctrlrt_cmd::certwatcher::CertWatcher;
use ctrlrt_cmd::cmdline::ManagerFlags;
use ctrlrt_cmd::events::RecorderProvider;
use ctrlrt_cmd::metrics::{self, MetricsServer, MetricsServerOptions, TlsOpt};
use ctrlrt_runtime::manager::runnable_fn_with_leader_election as runnable_fn_le;
use ctrlrt_runtime::{
    ALL_NAMESPACES, ByObject, CacheConfig, CacheOptions, Manager, ManagerOptions,
};

/// `ctrl.Log.WithName("setup")`
fn setup_log() -> Logger {
    Logger::root().with_name("setup")
}

fn exit_with(err: &dyn std::fmt::Display, msg: &str, kvs: ctrlrt::log::KeyValues) -> ! {
    setup_log().error(err, msg, kvs);
    std::process::exit(1);
}

fn main() {
    // rustls needs a process-wide crypto provider (Go's crypto/tls is built in).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("failed to build tokio runtime: {e}");
            std::process::exit(1)
        });
    rt.block_on(run());
}

/// Kept as a single function to mirror cmd/main.go.
async fn run() {
    // flag.Parse() (incl. --kubeconfig and the zap flags, as controller-runtime registers them)
    let flags = ManagerFlags::parse_command_line();

    // ctrl.SetLogger(zap.New(zap.UseFlagOptions(&opts)))
    ctrlrt_cmd::setup_log(&flags.zap);
    let setup = setup_log();

    // if the enable-http2 flag is false (the default), http/2 should be disabled
    // due to its vulnerabilities (HTTP/2 Stream Cancellation and Rapid Reset CVEs).
    let mut tls_opts: Vec<TlsOpt> = Vec::new();
    if !flags.enable_http2 {
        tls_opts.push(metrics::disable_http2_opt(setup.clone()));
    }

    // Create watchers for metrics and webhooks certificates
    let mut metrics_cert_watcher: Option<CertWatcher> = None;
    let mut webhook_cert_watcher: Option<CertWatcher> = None;

    if !flags.webhook_cert_path.is_empty() {
        setup.info(
            "Initializing webhook certificate watcher using provided certificates",
            kv![
                "webhook-cert-path" => flags.webhook_cert_path,
                "webhook-cert-name" => flags.webhook_cert_name,
                "webhook-cert-key" => flags.webhook_cert_key,
            ],
        );
        match CertWatcher::new(
            join(&flags.webhook_cert_path, &flags.webhook_cert_name),
            join(&flags.webhook_cert_path, &flags.webhook_cert_key),
        ) {
            Ok(w) => webhook_cert_watcher = Some(w),
            Err(e) => exit_with(
                &e,
                "Failed to initialize webhook certificate watcher",
                kv![],
            ),
        }
        // The webhook server is configured with these TLS options but, as in Go, it is never
        // started because the operator registers no webhooks.
    }

    // Metrics endpoint options (metricsserver.Options).
    let mut metrics_server_options = MetricsServerOptions {
        bind_address: flags.metrics_addr.clone(),
        secure_serving: flags.secure_metrics,
        tls_opts: tls_opts.clone(),
        ..Default::default()
    };

    if flags.secure_metrics {
        // FilterProvider protects the metrics endpoint with authn/authz.
        metrics_server_options.filter_provider = Some(Arc::new(|c: &ctrlrt::Client| {
            ctrlrt_cmd::filters::with_authentication_and_authorization(c)
        }));
    }

    if !flags.metrics_cert_path.is_empty() {
        setup.info(
            "Initializing metrics certificate watcher using provided certificates",
            kv![
                "metrics-cert-path" => flags.metrics_cert_path,
                "metrics-cert-name" => flags.metrics_cert_name,
                "metrics-cert-key" => flags.metrics_cert_key,
            ],
        );
        match CertWatcher::new(
            join(&flags.metrics_cert_path, &flags.metrics_cert_name),
            join(&flags.metrics_cert_path, &flags.metrics_cert_key),
        ) {
            Ok(w) => {
                metrics_server_options
                    .tls_opts
                    .push(metrics::cert_watcher_opt(w.clone()));
                metrics_cert_watcher = Some(w);
            }
            Err(e) => exit_with(
                &e,
                "to initialize metrics certificate watcher",
                kv!["error" => e.to_string()],
            ),
        }
    }

    // ctrl.NewManager(ctrl.GetConfigOrDie(), ctrl.Options{...})
    let config = ctrlrt_runtime::get_config_or_die(&flags.kubeconfig).await;

    // Event recorders (mgr.GetEventRecorderFor) are backed by client-go's broadcaster.
    let recorder_provider = match ctrlrt_runtime::client_from_rest_config(&config) {
        Ok((c, _)) => RecorderProvider::new(c, Logger::root().with_name("events")),
        Err(e) => exit_with(&e, "unable to start manager", kv![]),
    };
    let rp = recorder_provider.clone();

    let options = ManagerOptions {
        health_probe_bind_address: flags.probe_addr.clone(),
        leader_election: flags.enable_leader_election,
        leader_election_id: "4ebdb3db.dpu.hcp.io".into(),
        cache: CacheOptions {
            by_object: vec![
                ByObject::new::<ConfigMap>()
                    .namespace(
                        ALL_NAMESPACES,
                        CacheConfig::with_labels([(
                            ctrl_ignitiongenerator::BFCFG_TEMPLATE_LABEL,
                            "true",
                        )]),
                    )
                    .namespace(controller::OPERATOR_NAMESPACE, CacheConfig::default()),
            ],
            ..Default::default()
        },
        recorder_factory: Some(Arc::new(move |name: &str| rp.get_event_recorder_for(name))),
        // LeaderElectionReleaseOnCancel is left at its default (false), as in Go.
        ..Default::default()
    };
    let mgr = match Manager::new(config, options).await {
        Ok(m) => m,
        Err(e) => exit_with(&e, "unable to start manager", kv![]),
    };

    // The metrics server is created by NewManager in Go.
    match MetricsServer::new(metrics_server_options, &mgr.api_reader()) {
        Ok(Some(srv)) => {
            let runnable = runnable_fn_le(false, move |ctx: ctrlrt::Ctx| {
                let srv = srv.clone();
                async move { srv.start(ctx.cancel.clone()).await }
            });
            if let Err(e) = mgr.add_metrics_server(runnable) {
                exit_with(&e, "unable to start manager", kv![]);
            }
        }
        Ok(None) => {}
        Err(e) => exit_with(&e, "unable to start manager", kv![]),
    }

    let client = mgr.client();

    // Create event recorders for each controller/component
    let provisioner_recorder = mgr.event_recorder_for(common::PROVISIONER_CONTROLLER_NAME);
    let csr_approval_recorder = mgr.event_recorder_for(common::CSR_APPROVAL_CONTROLLER_NAME);

    // Initialize BlueField OCP Layer Image Lookup
    let image_lookup = Arc::new(ctrl_bfocplookup::ImageLookup::new(
        client.clone(),
        provisioner_recorder.clone(),
    ));
    // Initialize DPUCluster Validator
    let dpu_cluster_validator = Arc::new(ctrl_dpucluster::Validator::new(
        client.clone(),
        provisioner_recorder.clone(),
    ));
    // Initialize Secrets Validator
    let secrets_validator = Arc::new(ctrl_secrets::Validator::new(
        client.clone(),
        provisioner_recorder.clone(),
    ));
    // Initialize Secret Manager for HostedCluster lifecycle
    let secret_manager = Arc::new(ctrl_hostedcluster::SecretManager::new(client.clone()));
    // Initialize HostedCluster Manager
    let hosted_cluster_manager = Arc::new(ctrl_hostedcluster::HostedClusterManager::new(
        client.clone(),
    ));
    // Initialize NodePool Manager
    let node_pool_manager = Arc::new(ctrl_hostedcluster::NodePoolManager::new(client.clone()));
    // Initialize Kubeconfig Injector
    let kubeconfig_injector = Arc::new(ctrl_kubeconfiginjection::KubeconfigInjector::new(
        client.clone(),
        provisioner_recorder.clone(),
    ));
    // Initialize MetalLB Manager
    let metal_lb_manager = Arc::new(ctrl_metallb::MetalLBManager::new(
        client.clone(),
        provisioner_recorder.clone(),
    ));

    // A single shared hosted-cluster client manager, injected into every reconciler that
    // talks to hosted clusters (CSR approval and the hugepages reservation).
    let hosted_client_manager = Arc::new(hostedclient::ClientManager::new(client.clone()));

    // Initialize CSR Approver
    let csr_approver = Arc::new(ctrl_csrapproval::CSRApprover::new(
        client.clone(),
        csr_approval_recorder,
        hosted_client_manager.clone(),
    ));

    // Initialize Finalizer Manager with pluggable cleanup handlers.
    // Handlers are executed in registration order.
    let mut finalizer_manager =
        ctrl_finalizer::Manager::new(client.clone(), provisioner_recorder.clone());
    // 1. Kubeconfig injection cleanup (removes kubeconfig from DPUCluster namespace)
    finalizer_manager.register_handler(ctrl_kubeconfiginjection::CleanupHandler::new(
        client.clone(),
        provisioner_recorder.clone(),
    ));
    // 2. HostedCluster cleanup (HostedCluster, NodePool, services, secrets) — must run before
    //    MetalLB cleanup because LoadBalancer services depend on the IPAddressPool.
    finalizer_manager.register_handler(ctrl_hostedcluster::CleanupHandler::new(
        client.clone(),
        provisioner_recorder.clone(),
    ));
    // 3. MetalLB cleanup (IPAddressPool and L2Advertisement)
    finalizer_manager.register_handler(ctrl_metallb::CleanupHandler::new(
        client.clone(),
        provisioner_recorder.clone(),
    ));

    // Initialize Status Syncer for HostedCluster status mirroring
    let status_syncer = Arc::new(ctrl_hostedcluster::StatusSyncer::new(client.clone()));

    // Initialize Ignition Generator for DPF provisioning
    let ignition_generator = Arc::new(ctrl_ignitiongenerator::IgnitionGenerator::new(
        client.clone(),
        provisioner_recorder.clone(),
    ));

    // Initialize OVS Hugepages Manager for the hosted-cluster reservation DaemonSet
    let hugepages_manager = Arc::new(ctrl_ovshugepages::Manager::new(
        client.clone(),
        hosted_client_manager.clone(),
        Arc::new(ctrl_dpuservicetemplate::RemoteReleaseImageReader),
    ));

    // Setup main DPFHCPProvisioner controller
    let provisioner = Arc::new(controller::DPFHCPProvisionerReconciler {
        client: client.clone(),
        recorder: provisioner_recorder.clone(),
        image_lookup: Some(image_lookup),
        dpu_cluster_validator: Some(dpu_cluster_validator),
        secrets_validator: Some(secrets_validator),
        secret_manager: Some(secret_manager),
        metal_lb_manager: Some(metal_lb_manager),
        hosted_cluster_manager: Some(hosted_cluster_manager),
        node_pool_manager: Some(node_pool_manager),
        finalizer_manager: Some(Arc::new(finalizer_manager)),
        status_syncer: Some(status_syncer),
        kubeconfig_injector: Some(kubeconfig_injector),
        ignition_generator: Some(ignition_generator),
        hugepages_manager: Some(hugepages_manager),
        ..Default::default()
    });
    if let Err(e) = provisioner.setup_with_manager(&mgr) {
        exit_with(
            &e,
            "unable to create controller",
            kv!["controller" => "DPFHCPProvisioner"],
        );
    }

    // Setup CSR Approval controller (separate from main controller)
    let csr = Arc::new(ctrl_csrapproval::CSRApprovalReconciler {
        client: client.clone(),
        approver: Some(csr_approver),
    });
    if let Err(e) = csr.setup_with_manager(&mgr) {
        exit_with(
            &e,
            "unable to create controller",
            kv!["controller" => "CSRApproval"],
        );
    }

    // Setup DPUServiceTemplate controller (manages templates per DPUCluster namespace)
    let dst = Arc::new(ctrl_dpuservicetemplate::DPUServiceTemplateReconciler::new(
        client.clone(),
        Arc::new(ctrl_dpuservicetemplate::DPUServiceTemplateManager::new(
            client.clone(),
            mgr.api_reader(),
            Arc::new(ctrl_dpuservicetemplate::RemoteReleaseImageReader),
            controller::OPERATOR_NAMESPACE.to_string(),
        )),
    ));
    if let Err(e) = dst.setup_with_manager(&mgr) {
        exit_with(
            &e,
            "unable to create controller",
            kv!["controller" => "DPUServiceTemplate"],
        );
    }

    if let Some(w) = metrics_cert_watcher {
        setup.info("Adding metrics certificate watcher to manager", kv![]);
        if let Err(e) = mgr.add(cert_watcher_runnable(w)) {
            exit_with(
                &e,
                "unable to add metrics certificate watcher to manager",
                kv![],
            );
        }
    }

    if let Some(w) = webhook_cert_watcher {
        setup.info("Adding webhook certificate watcher to manager", kv![]);
        if let Err(e) = mgr.add(cert_watcher_runnable(w)) {
            exit_with(
                &e,
                "unable to add webhook certificate watcher to manager",
                kv![],
            );
        }
    }

    if let Err(e) = mgr.add_healthz_check("healthz", ctrlrt_runtime::ping()) {
        exit_with(&e, "unable to set up health check", kv![]);
    }
    if let Err(e) = mgr.add_readyz_check("readyz", ctrlrt_runtime::ping()) {
        exit_with(&e, "unable to set up ready check", kv![]);
    }

    setup.info("starting manager", kv![]);
    let result = mgr.start(ctrlrt_cmd::setup_signal_handler()).await;
    recorder_provider.stop(Duration::from_secs(30)).await;
    if let Err(e) = result {
        exit_with(&e, "problem running manager", kv![]);
    }
}

/// `certwatcher.CertWatcher` as a manager runnable (`NeedLeaderElection() == false`).
fn cert_watcher_runnable(w: CertWatcher) -> Arc<dyn ctrlrt_runtime::Runnable> {
    runnable_fn_le(false, move |ctx: ctrlrt::Ctx| {
        let w = w.clone();
        async move { w.start(ctx.cancel.clone()).await }
    })
}

/// `filepath.Join(dir, name)`
fn join(dir: &str, name: &str) -> String {
    Path::new(dir).join(name).to_string_lossy().into_owned()
}
