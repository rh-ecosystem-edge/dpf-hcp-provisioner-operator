//! Port of `internal/controller/csrapproval/csrapproval.go`.

use std::sync::Arc;
use std::time::Duration;

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::error::{Result, is_conflict};
use ctrlrt::meta::{CONDITION_FALSE, CONDITION_TRUE};
use ctrlrt::recorder::{EVENT_TYPE_NORMAL, EVENT_TYPE_WARNING};
use ctrlrt::{Client, Ctx, Object, ObjectKey, ReconcileResult, Recorder, RecorderExt, kv, wrapf};
use hostedclient::{ClientManager, HostedClient};
use k8s_openapi::api::certificates::v1::{
    CertificateSigningRequest, CertificateSigningRequestCondition,
};

use crate::csr_content_validation::{validate_bootstrap_csr, validate_serving_csr};
use crate::csr_owner_validation::Validator;
use crate::hostname::{
    CERTIFICATE_APPROVED, SIGNER_BOOTSTRAP, SIGNER_SERVING, extract_hostname, is_pending,
};

/// CSRPollingInterval is how often to check for pending CSRs in the hosted cluster.
/// Polls continuously to handle bootstrap CSRs, serving CSRs, and edge cases like node
/// recreation.
pub const CSR_POLLING_INTERVAL: Duration = Duration::from_secs(30);

/// CSRApprover manages CSR auto-approval for a DPFHCPProvisioner.
pub struct CSRApprover {
    /// Client for management cluster (where operator runs)
    pub(crate) mgmt_client: Client,
    pub(crate) recorder: Recorder,
    /// Shared manager for cached hosted-cluster clients
    pub(crate) client_manager: Arc<ClientManager>,
}

impl CSRApprover {
    /// NewCSRApprover creates a new CSR approver. The hosted-cluster client manager is
    /// injected so it can be shared with other reconcilers (e.g. the hugepages reservation).
    pub fn new(
        mgmt_client: Client,
        recorder: Recorder,
        client_manager: Arc<ClientManager>,
    ) -> Self {
        Self {
            mgmt_client,
            recorder,
            client_manager,
        }
    }

    /// ProcessCSRs processes and approves pending CSRs for a DPFHCPProvisioner.
    /// Called during each reconciliation loop to handle CSRs from worker nodes.
    pub async fn process_csrs(
        &self,
        ctx: &Ctx,
        dpfhcp: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<ReconcileResult> {
        let log = &ctx.log;
        let namespace = dpfhcp.namespace().to_string();
        let name = dpfhcp.name().to_string();

        // Step 1: Verify prerequisites (kubeconfig secret exists and is populated)
        if !self.is_kubeconfig_available(ctx, dpfhcp).await {
            log.v(1).info(
                "Kubeconfig secret not available, CSR auto-approval cannot start",
                kv![],
            );
            self.set_condition(
                ctx,
                dpfhcp,
                CONDITION_FALSE,
                provisioningv1alpha1::REASON_KUBECONFIG_NOT_AVAILABLE,
                "Kubeconfig secret not available for hosted cluster",
            )
            .await?;
            // Don't requeue - reconciliation will be triggered when main controller updates
            // DPFHCPProvisioner status (main controller watches kubeconfig secrets and updates
            // DPFHCPProvisioner.Status.KubeConfigSecretRef)
            return Ok(ReconcileResult::done());
        }

        // Step 2: Get or create hosted cluster client
        let hc_client = match self
            .client_manager
            .get_hosted_cluster_client(ctx, &namespace, &name)
            .await
        {
            Ok(c) => c,
            Err(err) => {
                log.error(&err, "Failed to create hosted cluster client", kv![]);
                let msg = format!("Cannot connect to hosted cluster: {err}");
                self.set_condition(
                    ctx,
                    dpfhcp,
                    CONDITION_FALSE,
                    provisioningv1alpha1::REASON_HOSTED_CLUSTER_NOT_REACHABLE,
                    &msg,
                )
                .await?;
                self.recorder
                    .event(dpfhcp, EVENT_TYPE_WARNING, "HostedClusterUnreachable", &msg);
                // Don't requeue - reconciliation will be triggered when main controller
                // updates DPFHCPProvisioner status (main controller watches HostedCluster and
                // updates DPFHCPProvisioner.Status.HostedClusterAvailable)
                return Ok(ReconcileResult::done());
            }
        };

        // Step 3: Test connection
        if let Err(err) = hostedclient::test_connection(ctx, &hc_client).await {
            log.error(&err, "Hosted cluster not reachable", kv![]);
            // Invalidate cached client so next reconciliation creates a fresh one.
            // This handles cases like expired credentials, kubeconfig rotation, etc.
            self.client_manager.invalidate_client(&namespace, &name);
            let msg = format!("Cannot connect to hosted cluster: {err}");
            self.set_condition(
                ctx,
                dpfhcp,
                CONDITION_FALSE,
                provisioningv1alpha1::REASON_HOSTED_CLUSTER_NOT_REACHABLE,
                &msg,
            )
            .await?;
            self.recorder
                .event(dpfhcp, EVENT_TYPE_WARNING, "HostedClusterUnreachable", &msg);
            return Ok(ReconcileResult::done());
        }

        // Step 4: Process any pending CSRs
        if let Err(err) = self.process_pending_csrs(ctx, dpfhcp, &hc_client).await {
            log.error(&err, "Failed to process pending CSRs", kv![]);
            // Transient error during normal operation - don't fail the reconciliation.
            // Continue polling to retry CSR processing.
            self.set_condition(
                ctx,
                dpfhcp,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_CSR_APPROVAL_ACTIVE,
                &format!("CSR processing encountered transient error: {err}"),
            )
            .await?;
            // Retry after polling interval (assume CSRs might be pending)
            return Ok(ReconcileResult::requeue_after(CSR_POLLING_INTERVAL));
        }

        // Step 5: Update condition and schedule next poll
        self.set_condition(
            ctx,
            dpfhcp,
            CONDITION_TRUE,
            provisioningv1alpha1::REASON_CSR_APPROVAL_ACTIVE,
            "CSR auto-approval active for worker nodes",
        )
        .await?;

        // Always poll every 30 seconds to check for new CSRs. This handles all cases:
        // initial bootstrap CSRs, serving CSRs after bootstrap, node recreation, etc.
        Ok(ReconcileResult::requeue_after(CSR_POLLING_INTERVAL))
    }

    /// StopCSRWatch cleans up resources for a DPFHCPProvisioner.
    /// Called during finalizer cleanup.
    pub fn stop_csr_watch(&self, ctx: &Ctx, dpfhcp: &provisioningv1alpha1::DPFHCPProvisioner) {
        let log = &ctx.log;
        log.info("Cleaning up CSR approver resources", kv![]);

        // Invalidate cached client
        self.client_manager
            .invalidate_client(dpfhcp.namespace(), dpfhcp.name());
    }

    /// isKubeconfigAvailable checks if the kubeconfig secret exists and is populated.
    async fn is_kubeconfig_available(
        &self,
        ctx: &Ctx,
        dpfhcp: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> bool {
        self.client_manager
            .get_kubeconfig_data(ctx, dpfhcp.namespace(), dpfhcp.name())
            .await
            .is_ok()
    }

    /// processPendingCSRs processes all pending CSRs in the hosted cluster.
    async fn process_pending_csrs(
        &self,
        ctx: &Ctx,
        dpfhcp: &provisioningv1alpha1::DPFHCPProvisioner,
        hc_client: &HostedClient,
    ) -> Result<()> {
        let log = &ctx.log;

        // List all CSRs
        let csr_list: Vec<CertificateSigningRequest> = hc_client
            .list_all()
            .await
            .map_err(|err| wrapf!(err, "failed to list CSRs: {}", err))?;

        // Filter to pending CSRs
        let pending_csrs: Vec<CertificateSigningRequest> = csr_list
            .into_iter()
            .filter(|csr| is_pending(Some(csr)))
            .collect();

        if pending_csrs.is_empty() {
            log.v(1).info("No pending CSRs found", kv![]);
            return Ok(());
        }

        log.info("Found pending CSRs", kv!["count" => pending_csrs.len()]);

        // Filter to bootstrap and serving CSRs only
        let mut bootstrap_csrs = Vec::new();
        let mut serving_csrs = Vec::new();
        let mut other_csrs = Vec::new();
        for csr in pending_csrs {
            match csr.spec.signer_name.as_str() {
                SIGNER_BOOTSTRAP => bootstrap_csrs.push(csr),
                SIGNER_SERVING => serving_csrs.push(csr),
                _ => other_csrs.push(csr),
            }
        }

        log.info(
            "Filtered CSRs by signer",
            kv![
                "bootstrapCount" => bootstrap_csrs.len(),
                "servingCount" => serving_csrs.len(),
                "otherCount" => other_csrs.len()
            ],
        );

        // Log other signers for debugging
        for csr in &other_csrs {
            log.info(
                "Skipping CSR with unsupported signer",
                kv!["csrName" => csr.name(), "signerName" => csr.spec.signer_name],
            );
        }

        // Process bootstrap CSRs
        for mut csr in bootstrap_csrs {
            if let Err(err) = self
                .process_csr(ctx, dpfhcp, hc_client, &mut csr, "bootstrap")
                .await
            {
                // Actual error - log and continue
                log.error(
                    &err,
                    "Failed to process bootstrap CSR",
                    kv!["csrName" => csr.name()],
                );
                continue;
            }
        }

        // Process serving CSRs
        for mut csr in serving_csrs {
            if let Err(err) = self
                .process_csr(ctx, dpfhcp, hc_client, &mut csr, "serving")
                .await
            {
                // Actual error - log and continue
                log.error(
                    &err,
                    "Failed to process serving CSR",
                    kv!["csrName" => csr.name()],
                );
                continue;
            }
        }

        Ok(())
    }

    /// processCSR processes a single CSR (validate and approve if valid).
    /// Returns:
    /// - Ok: CSR was successfully approved or skipped (validation failed but not an error
    ///   condition)
    /// - Err: Actual error occurred during processing
    async fn process_csr(
        &self,
        ctx: &Ctx,
        dpfhcp: &provisioningv1alpha1::DPFHCPProvisioner,
        hc_client: &HostedClient,
        csr: &mut CertificateSigningRequest,
        csr_type: &str,
    ) -> Result<()> {
        let log = &ctx.log;
        let csr_name = csr.name().to_string();

        // Extract hostname from CSR
        let hostname = match extract_hostname(Some(csr)) {
            Ok(h) => h,
            Err(err) => {
                log.info(
                    "Skipping CSR: cannot extract hostname",
                    kv!["csrName" => csr_name, "error" => err.to_string()],
                );
                return Ok(());
            }
        };

        log.info(
            "Processing CSR",
            kv!["csrName" => csr_name, "hostname" => hostname, "type" => csr_type],
        );

        // Perform comprehensive CSR validation based on type
        match csr.spec.signer_name.as_str() {
            SIGNER_BOOTSTRAP => {
                if let Err(err) = validate_bootstrap_csr(csr, &hostname) {
                    log.info(
                        "Skipping CSR: bootstrap validation failed",
                        kv!["csrName" => csr_name, "hostname" => hostname, "error" => err.to_string()],
                    );
                    return Ok(());
                }
            }
            SIGNER_SERVING => {
                if let Err(err) = validate_serving_csr(csr, &hostname) {
                    log.info(
                        "Skipping CSR: serving validation failed",
                        kv!["csrName" => csr_name, "hostname" => hostname, "error" => err.to_string()],
                    );
                    return Ok(());
                }
            }
            _ => {}
        }

        // Validate CSR ownership (check if DPU exists and node state matches CSR type)
        let validator = Validator::new(
            self.mgmt_client.clone(),
            Some(hc_client.clone()),
            &dpfhcp.spec.dpu_cluster_ref.namespace,
        );
        let is_bootstrap_csr = csr.spec.signer_name == SIGNER_BOOTSTRAP;
        let result = validator
            .validate_csr_owner(ctx, &hostname, is_bootstrap_csr)
            .await
            .map_err(|err| wrapf!(err, "failed to validate CSR ownership: {}", err))?;
        if !result.valid {
            log.info(
                "Skipping CSR: owner validation failed",
                kv!["csrName" => csr_name, "hostname" => hostname, "reason" => result.reason],
            );
            return Ok(());
        }

        // Approve the CSR
        self.approve_csr(ctx, hc_client, csr, &hostname, csr_type)
            .await
            .map_err(|err| wrapf!(err, "failed to approve CSR: {}", err))?;

        // Emit event for successful approval
        let event_msg = format!("Approved {csr_type} CSR for DPU worker node {hostname}");
        self.recorder
            .event(dpfhcp, EVENT_TYPE_NORMAL, "CSRApproved", &event_msg);

        log.info(
            "CSR approved",
            kv!["csrName" => csr_name, "hostname" => hostname, "type" => csr_type],
        );
        Ok(())
    }

    /// approveCSR approves a CSR by updating its status.
    async fn approve_csr(
        &self,
        _ctx: &Ctx,
        hc_client: &HostedClient,
        csr: &mut CertificateSigningRequest,
        hostname: &str,
        csr_type: &str,
    ) -> Result<()> {
        // Create approval condition
        csr.status
            .get_or_insert_with(Default::default)
            .conditions
            .get_or_insert_with(Vec::new)
            .push(CertificateSigningRequestCondition {
                type_: CERTIFICATE_APPROVED.to_string(),
                status: CONDITION_TRUE.to_string(),
                reason: Some("AutoApproved".to_string()),
                message: Some(format!(
                    "Auto-approved {csr_type} CSR for DPU worker node {hostname}"
                )),
                last_update_time: Some(ctrlrt::meta::now()),
                last_transition_time: None,
            });

        // Update CSR status (`CertificateSigningRequests().UpdateApproval`)
        hc_client.update_subresource(csr, "approval").await
    }

    /// setCondition updates the CSRAutoApprovalActive condition, retrying on conflict.
    /// It re-fetches the CR on each attempt so the update always uses the latest
    /// resourceVersion, avoiding conflicts with the main DPFHCPProvisioner controller
    /// running concurrently.
    pub(crate) async fn set_condition(
        &self,
        ctx: &Ctx,
        dpfhcp: &mut provisioningv1alpha1::DPFHCPProvisioner,
        status: &str,
        reason: &str,
        message: &str,
    ) -> Result<()> {
        let key = dpfhcp.key();
        // retry.RetryOnConflict(retry.DefaultRetry, fn): up to 5 attempts, 10ms apart
        // (factor 1, jitter 0.1); after the last conflicting attempt the conflict error is
        // returned.
        const STEPS: u32 = 5;
        let mut step = 0;
        loop {
            step += 1;
            match self
                .set_condition_once(ctx, &key, dpfhcp, status, reason, message)
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) if is_conflict(&err) => {
                    if step >= STEPS {
                        return Err(err);
                    }
                    tokio::time::sleep(retry_backoff()).await;
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn set_condition_once(
        &self,
        _ctx: &Ctx,
        key: &ObjectKey,
        dpfhcp: &mut provisioningv1alpha1::DPFHCPProvisioner,
        status: &str,
        reason: &str,
        message: &str,
    ) -> Result<()> {
        self.mgmt_client.get_into(key, dpfhcp).await?;

        let condition = ctrlrt::meta::condition(
            provisioningv1alpha1::CSR_AUTO_APPROVAL_ACTIVE,
            status,
            reason,
            message,
        );

        if !ctrlrt::meta::set_status_condition(&mut dpfhcp.status.conditions, condition) {
            return Ok(());
        }

        self.mgmt_client.update_status(dpfhcp).await?;

        let event_type = if status == CONDITION_FALSE {
            EVENT_TYPE_WARNING
        } else {
            EVENT_TYPE_NORMAL
        };
        self.recorder.event(dpfhcp, event_type, reason, message);
        Ok(())
    }
}

/// `retry.DefaultRetry` step: 10ms with 10% jitter.
fn retry_backoff() -> Duration {
    Duration::from_millis(10).mul_f64(1.0 + 0.1 * jitter_fraction())
}

/// A pseudo-random fraction in [0, 1) (`rand.Float64()` for backoff jitter).
fn jitter_fraction() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(0);
    (h.finish() >> 11) as f64 / (1u64 << 53) as f64
}
