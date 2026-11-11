//! Port of `internal/controller/metallb/metallb.go`.

use std::collections::BTreeMap;

use apis::metallb::v1beta1 as metallbv1beta1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use common::OperatorConfig;
use ctrlrt::meta::{CONDITION_FALSE, CONDITION_TRUE};
use ctrlrt::{
    Client, Ctx, Object, OperationResult, ReconcileResult, Recorder, RecorderExt, Result, kv, wrapf,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::namespaced_name;

/// MetalLBManager handles MetalLB resource management for DPFHCPProvisioner
#[derive(Clone)]
pub struct MetalLBManager {
    client: Client,
    recorder: Recorder,
}

impl MetalLBManager {
    /// NewMetalLBManager creates a new MetalLB manager
    pub fn new(client: Client, recorder: Recorder) -> Self {
        Self { client, recorder }
    }

    /// ConfigureMetalLB orchestrates MetalLB resource configuration for a DPFHCPProvisioner
    /// It creates and maintains IPAddressPool and L2Advertisement resources when LoadBalancer exposure is needed.
    pub async fn configure_metal_lb(
        &self,
        ctx: &Ctx,
        provisioner: &mut provisioningv1alpha1::DPFHCPProvisioner,
        operator_config: &OperatorConfig,
    ) -> Result<ReconcileResult> {
        let log = ctx
            .log
            .with_values(kv!["provisioner" => namespaced_name(&provisioner.key())]);

        // Check if MetalLB configuration is needed
        if !provisioner.should_expose_through_load_balancer() {
            log.v(1).info(
                "ShouldExposeThroughLoadBalancer is false, skipping MetalLB configuration",
                kv![],
            );
            return Ok(ReconcileResult::default());
        }

        // Check if MetalLB is disabled in operator config
        if operator_config.disable_metal_lb {
            log.info(
                "MetalLB disabled in operator config - skipping MetalLB configuration",
                kv![],
            );
            if let Err(err) = self
                .set_condition(
                    ctx,
                    provisioner,
                    CONDITION_TRUE,
                    "MetalLBDisabled",
                    "MetalLB disabled in operator configuration",
                )
                .await
            {
                log.error(&err, "Failed to update MetalLBConfigured condition", kv![]);
                return Err(err);
            }
            return Ok(ReconcileResult::default());
        }

        log.info("Starting MetalLB configuration", kv![]);

        // Configure IPAddressPool
        log.v(1).info("Configuring IPAddressPool", kv![]);
        if let Err(err) = self.ensure_ip_address_pool(ctx, provisioner).await {
            if ctrlrt::error::is_no_match(&err) {
                return self
                    .handle_missing_metal_lb_crds(ctx, provisioner, err)
                    .await;
            }

            log.error(&err, "Failed to configure IPAddressPool", kv![]);

            if let Err(cond_err) = self
                .set_condition(
                    ctx,
                    provisioner,
                    CONDITION_FALSE,
                    "CreatingIPAddressPool",
                    &format!("Failed to create/update IPAddressPool: {err}"),
                )
                .await
            {
                log.error(
                    &cond_err,
                    "Failed to update MetalLBConfigured condition",
                    kv![],
                );
            }

            return Err(err);
        }
        log.info(
            "IPAddressPool configured successfully",
            kv!["name" => provisioner.name(), "namespace" => common::OPENSHIFT_OPERATORS_NAMESPACE],
        );

        // Configure L2Advertisement
        log.v(1).info("Configuring L2Advertisement", kv![]);
        if let Err(err) = self.ensure_l2_advertisement(ctx, provisioner).await {
            if ctrlrt::error::is_no_match(&err) {
                return self
                    .handle_missing_metal_lb_crds(ctx, provisioner, err)
                    .await;
            }

            log.error(&err, "Failed to configure L2Advertisement", kv![]);

            if let Err(cond_err) = self
                .set_condition(
                    ctx,
                    provisioner,
                    CONDITION_FALSE,
                    "L2AdvertisementFailed",
                    &format!("Failed to create/update L2Advertisement: {err}"),
                )
                .await
            {
                log.error(
                    &cond_err,
                    "Failed to update MetalLBConfigured condition",
                    kv![],
                );
            }

            return Err(err);
        }
        log.info(
            "L2Advertisement configured successfully",
            kv![
                "name" => format!("advertise-{}", provisioner.name()),
                "namespace" => common::OPENSHIFT_OPERATORS_NAMESPACE
            ],
        );

        // Update condition to True - both resources successfully configured
        if let Err(err) = self
            .set_condition(
                ctx,
                provisioner,
                CONDITION_TRUE,
                "MetalLBReady",
                "MetalLB configured successfully",
            )
            .await
        {
            log.error(&err, "Failed to update MetalLBConfigured condition", kv![]);
            return Err(err);
        }

        log.info("MetalLB configuration complete", kv![]);
        Ok(ReconcileResult::default())
    }

    /// setCondition updates the MetalLBConfigured condition on the DPFHCPProvisioner CR
    /// and emits events when the condition changes to avoid event spam
    async fn set_condition(
        &self,
        ctx: &Ctx,
        provisioner: &mut provisioningv1alpha1::DPFHCPProvisioner,
        status: &str,
        reason: &str,
        message: &str,
    ) -> Result<()> {
        let log = &ctx.log;

        let mut condition = ctrlrt::meta::condition_gen(
            provisioningv1alpha1::METAL_LB_CONFIGURED,
            status,
            reason,
            message,
            provisioner.generation(),
        );
        condition.last_transition_time = ctrlrt::meta::now();

        // Only update if condition actually changed
        if ctrlrt::meta::set_status_condition(&mut provisioner.status.conditions, condition) {
            log.v(1).info(
                "Updating MetalLBConfigured condition",
                kv!["status" => status, "reason" => reason, "message" => message],
            );

            // Emit event only when condition status/reason changed (avoid spam)
            let mut event_type = "Normal";
            let mut event_reason = "MetalLBConfigured";
            if status == CONDITION_FALSE {
                event_type = "Warning";
                event_reason = "MetalLBConfigurationFailed";
            }
            self.recorder
                .event(provisioner, event_type, event_reason, message);

            if let Err(err) = self.client.update_status(provisioner).await {
                return Err(wrapf!(err, "updating MetalLBConfigured condition: {}", err));
            }
        }

        Ok(())
    }

    /// ensureIPAddressPool creates or updates the IPAddressPool resource
    async fn ensure_ip_address_pool(
        &self,
        ctx: &Ctx,
        provisioner: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<()> {
        let log = &ctx.log;

        let mut pool = metallbv1beta1::IPAddressPool {
            metadata: ObjectMeta {
                name: Some(provisioner.name().to_string()),
                namespace: Some(common::OPENSHIFT_OPERATORS_NAMESPACE.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let op = ctrlrt::create_or_update(&self.client, &mut pool, |pool| {
            // Verify ownership if resource already exists (ResourceVersion is set by API server on existing resources)
            if !pool.resource_version().is_empty()
                && !common::is_owned_by_provisioner(
                    pool.metadata.labels.as_ref(),
                    provisioner.name(),
                    provisioner.namespace(),
                )
            {
                return Err(anyhow::anyhow!(
                    "IPAddressPool {}/{} exists but is not owned by DPFHCPProvisioner {}/{} (missing ownership labels)",
                    pool.namespace(),
                    pool.name(),
                    provisioner.namespace(),
                    provisioner.name()
                ));
            }

            // Set ownership labels
            let labels = pool.metadata.labels.get_or_insert_with(BTreeMap::new);
            labels.insert(
                common::LABEL_DPFHCP_PROVISIONER_NAME.to_string(),
                provisioner.name().to_string(),
            );
            labels.insert(
                common::LABEL_DPFHCP_PROVISIONER_NAMESPACE.to_string(),
                provisioner.namespace().to_string(),
            );

            // Set desired spec
            pool.spec = metallbv1beta1::IPAddressPoolSpec {
                addresses: Some(vec![format!("{}/32", provisioner.spec.virtual_ip)]),
                service_allocation: Some(metallbv1beta1::ServiceAllocation {
                    namespaces: vec![format!(
                        "{}-{}",
                        provisioner.namespace(),
                        provisioner.name()
                    )],
                    ..Default::default()
                }),
                auto_assign: ctrlrt::ptr::to(true),
                ..Default::default()
            };

            Ok(())
        })
        .await;

        let op = match op {
            Ok(op) => op,
            Err(err) => return Err(wrapf!(err, "creating or updating IPAddressPool: {}", err)),
        };

        // Log the operation result
        match op {
            OperationResult::Created => {
                log.info(
                    "Created IPAddressPool",
                    kv!["name" => pool.name(), "namespace" => pool.namespace()],
                );
            }
            OperationResult::Updated => {
                log.info(
                    "Updated IPAddressPool (drift corrected)",
                    kv!["name" => pool.name()],
                );
                self.recorder.event(
                    provisioner,
                    "Normal",
                    "MetalLBDriftCorrected",
                    &format!("Corrected spec drift in IPAddressPool {}", pool.name()),
                );
            }
            OperationResult::None => {
                log.v(1).info(
                    "IPAddressPool already matches desired state",
                    kv!["name" => pool.name()],
                );
            }
            _ => {}
        }

        Ok(())
    }

    /// ensureL2Advertisement creates or updates the L2Advertisement resource
    async fn ensure_l2_advertisement(
        &self,
        ctx: &Ctx,
        provisioner: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<()> {
        let log = &ctx.log;

        let mut advert = metallbv1beta1::L2Advertisement {
            metadata: ObjectMeta {
                name: Some(format!("advertise-{}", provisioner.name())),
                namespace: Some(common::OPENSHIFT_OPERATORS_NAMESPACE.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let op = ctrlrt::create_or_update(&self.client, &mut advert, |advert| {
            // Verify ownership if resource already exists (ResourceVersion is set by API server on existing resources)
            if !advert.resource_version().is_empty()
                && !common::is_owned_by_provisioner(
                    advert.metadata.labels.as_ref(),
                    provisioner.name(),
                    provisioner.namespace(),
                )
            {
                return Err(anyhow::anyhow!(
                    "L2Advertisement {}/{} exists but is not owned by DPFHCPProvisioner {}/{} (missing ownership labels)",
                    advert.namespace(),
                    advert.name(),
                    provisioner.namespace(),
                    provisioner.name()
                ));
            }

            // Set ownership labels
            let labels = advert.metadata.labels.get_or_insert_with(BTreeMap::new);
            labels.insert(
                common::LABEL_DPFHCP_PROVISIONER_NAME.to_string(),
                provisioner.name().to_string(),
            );
            labels.insert(
                common::LABEL_DPFHCP_PROVISIONER_NAMESPACE.to_string(),
                provisioner.namespace().to_string(),
            );

            // Set desired spec
            advert.spec = metallbv1beta1::L2AdvertisementSpec {
                ip_address_pools: vec![provisioner.name().to_string()],
                ..Default::default()
            };

            Ok(())
        })
        .await;

        let op = match op {
            Ok(op) => op,
            Err(err) => return Err(wrapf!(err, "creating or updating L2Advertisement: {}", err)),
        };

        // Log the operation result
        match op {
            OperationResult::Created => {
                log.info(
                    "Created L2Advertisement",
                    kv!["name" => advert.name(), "namespace" => advert.namespace()],
                );
            }
            OperationResult::Updated => {
                log.info(
                    "Updated L2Advertisement (drift corrected)",
                    kv!["name" => advert.name()],
                );
                self.recorder.event(
                    provisioner,
                    "Normal",
                    "MetalLBDriftCorrected",
                    &format!("Corrected spec drift in L2Advertisement {}", advert.name()),
                );
            }
            OperationResult::None => {
                log.v(1).info(
                    "L2Advertisement already matches desired state",
                    kv!["name" => advert.name()],
                );
            }
            _ => {}
        }

        Ok(())
    }

    /// handleMissingMetalLBCRDs handles the case when MetalLB CRDs are not installed on bare-metal platforms.
    /// This method is called after the platform check, so we know we're on bare-metal and MetalLB is required.
    async fn handle_missing_metal_lb_crds(
        &self,
        ctx: &Ctx,
        provisioner: &mut provisioningv1alpha1::DPFHCPProvisioner,
        original_err: ctrlrt::Error,
    ) -> Result<ReconcileResult> {
        let log = &ctx.log;

        if let Err(cond_err) = self
            .set_condition(
                ctx,
                provisioner,
                CONDITION_FALSE,
                "MetalLBNotInstalled",
                "LoadBalancer mode requires MetalLB on non-cloud platforms. Install MetalLB operator or remove VirtualIP to use NodePort mode.",
            )
            .await
        {
            log.error(&cond_err, "Failed to update MetalLBConfigured condition", kv![]);
        }

        Err(wrapf!(
            original_err,
            "MetalLB CRDs not found: {}. Install MetalLB operator for LoadBalancer support on bare-metal",
            original_err
        ))
    }
}
