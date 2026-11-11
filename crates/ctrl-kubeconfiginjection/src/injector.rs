//! Port of `internal/controller/kubeconfiginjection/injector.go`.

use std::collections::BTreeMap;

use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::k8s::core::v1::LocalObjectReference;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::error::{find_in_chain, is_already_exists, is_conflict, is_not_found};
use ctrlrt::meta::{CONDITION_FALSE, CONDITION_TRUE};
use ctrlrt::recorder::{EVENT_TYPE_NORMAL, EVENT_TYPE_WARNING};
use ctrlrt::{
    Client, Ctx, Object, ObjectKey, ReconcileResult, Recorder, RecorderExt, Result, kv, wrapf,
};
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::namespaced_name;

/// KubeconfigSecretSuffix is the suffix for HC admin kubeconfig secrets
pub const KUBECONFIG_SECRET_SUFFIX: &str = "-admin-kubeconfig";

/// SourceKubeconfigSecretKey is the key name in the HostedCluster kubeconfig secret (created by Hypershift)
pub const SOURCE_KUBECONFIG_SECRET_KEY: &str = "kubeconfig";

/// DestinationKubeconfigSecretKey is the key name for kubeconfig data in the destination secret (expected by DPF operator)
pub const DESTINATION_KUBECONFIG_SECRET_KEY: &str = "super-admin.conf";

/// `errDPUClusterNotFound` signals the referenced DPUCluster CR does not exist (deleted or not created yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DpuClusterNotFoundError;

impl std::fmt::Display for DpuClusterNotFoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("dpu cluster not found")
    }
}

impl std::error::Error for DpuClusterNotFoundError {}

/// KubeconfigInjector handles kubeconfig injection from HostedCluster to DPUCluster
#[derive(Clone)]
pub struct KubeconfigInjector {
    pub client: Client,
    pub recorder: Recorder,
}

impl KubeconfigInjector {
    /// NewKubeconfigInjector creates a new KubeconfigInjector
    pub fn new(client: Client, recorder: Recorder) -> Self {
        Self { client, recorder }
    }

    /// InjectKubeconfig performs the kubeconfig injection workflow
    ///
    /// This function implements the complete kubeconfig injection flow:
    /// 1. Verify HC and NodePool created
    /// 2. Check injection state (secret + DPUCluster reference)
    /// 3. Detect HC kubeconfig secret availability
    /// 4. Create/update secret in DPUCluster namespace
    /// 5. Update DPUCluster CR spec.kubeconfig
    /// 6. Update DPFHCPProvisioner status (condition + kubeConfigSecretRef)
    pub async fn inject_kubeconfig(
        &self,
        ctx: &Ctx,
        provisioner: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<ReconcileResult> {
        let log = ctx.log.with_values(kv![
            "feature" => "kubeconfig-injection",
            common::DPFHCP_PROVISIONER_NAME => format!("{}/{}", provisioner.namespace(), provisioner.name())
        ]);

        log.info(
            "Starting kubeconfig injection",
            kv![
                "provisioner" => provisioner.name(),
                "namespace" => provisioner.namespace(),
                "targetNamespace" => provisioner.spec.dpu_cluster_ref.namespace
            ],
        );

        // Step 1: Verify HC and NodePool created
        if provisioner.status.hosted_cluster_ref.is_none() {
            log.v(1).info(
                "HostedCluster not created yet, skipping kubeconfig injection",
                kv![],
            );
            return Ok(ReconcileResult::default());
        }

        // Step 2: Check injection state
        let (secret_exists, dpu_cluster_updated) =
            match self.check_injection_state(ctx, provisioner).await {
                Ok(v) => v,
                Err(err) => {
                    if find_in_chain::<DpuClusterNotFoundError>(&err).is_some() {
                        let ref_ = provisioner.spec.dpu_cluster_ref.clone();
                        log.info(
                            "DPUCluster not found, skipping kubeconfig injection until recreated",
                            kv![
                                "dpuClusterNamespace" => ref_.namespace,
                                "dpuClusterName" => ref_.name
                            ],
                        );
                        let msg =
                            format!("Waiting for DPUCluster {}/{}", ref_.namespace, ref_.name);
                        if let Err(cond_err) = self
                            .set_condition(
                                ctx,
                                provisioner,
                                CONDITION_FALSE,
                                provisioningv1alpha1::REASON_KUBE_CONFIG_PENDING,
                                &msg,
                            )
                            .await
                        {
                            log.error(&cond_err, "Failed to update condition", kv![]);
                            return Err(cond_err);
                        }
                        return Ok(ReconcileResult::default());
                    }
                    log.error(&err, "Failed to check injection state", kv![]);
                    if let Err(cond_err) = self
                        .set_condition(
                            ctx,
                            provisioner,
                            CONDITION_FALSE,
                            provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTION_FAILED,
                            &format!("Failed to check injection state: {err}"),
                        )
                        .await
                    {
                        log.error(&cond_err, "Failed to update condition", kv![]);
                    }
                    return Err(err);
                }
            };

        // Step 3: Detect HC kubeconfig secret availability
        let (available, secret_name) = match self
            .detect_hc_kubeconfig_availability(ctx, provisioner)
            .await
        {
            Ok(v) => v,
            Err(err) => {
                log.error(&err, "Failed to detect HC kubeconfig availability", kv![]);
                if let Err(cond_err) = self
                    .set_condition(
                        ctx,
                        provisioner,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTION_FAILED,
                        &format!("Failed to detect HC kubeconfig availability: {err}"),
                    )
                    .await
                {
                    log.error(&cond_err, "Failed to update condition", kv![]);
                }
                return Err(err);
            }
        };

        if !available {
            log.info(
                "HC kubeconfig secret not ready yet, waiting for watch to trigger",
                kv!["hcName" => provisioner.name(), "hcNamespace" => provisioner.namespace()],
            );
            let msg = format!(
                "Waiting for Hypershift to create kubeconfig secret for HostedCluster {}",
                provisioner.name()
            );
            if let Err(err) = self
                .set_condition(
                    ctx,
                    provisioner,
                    CONDITION_FALSE,
                    provisioningv1alpha1::REASON_KUBE_CONFIG_PENDING,
                    &msg,
                )
                .await
            {
                log.error(&err, "Failed to update condition", kv![]);
                return Err(err);
            }
            // Don't requeue - the watch on HC kubeconfig secrets will trigger reconciliation when the secret is created.
            return Ok(ReconcileResult::default());
        }

        log.info(
            "HC kubeconfig secret detected",
            kv!["secretName" => secret_name, "hcNamespace" => provisioner.namespace()],
        );

        // Step 4: Handle idempotency scenarios
        let needs_injection = match self
            .handle_idempotency_scenarios(
                ctx,
                provisioner,
                &secret_name,
                secret_exists,
                dpu_cluster_updated,
            )
            .await
        {
            Ok(v) => v,
            Err(err) => {
                log.error(&err, "Failed to handle idempotency scenarios", kv![]);
                if let Err(cond_err) = self
                    .set_condition(
                        ctx,
                        provisioner,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTION_FAILED,
                        &format!("Failed to handle idempotency scenarios: {err}"),
                    )
                    .await
                {
                    log.error(&cond_err, "Failed to update condition", kv![]);
                }
                return Err(err);
            }
        };

        // If idempotency scenario handled everything, we're done
        if !needs_injection {
            log.v(1)
                .info("Idempotency scenario handled, injection complete", kv![]);
            let msg = format!(
                "Kubeconfig secret successfully created in namespace {} and DPUCluster CR updated",
                provisioner.spec.dpu_cluster_ref.namespace
            );
            if let Err(err) = self
                .set_condition(
                    ctx,
                    provisioner,
                    CONDITION_TRUE,
                    provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED,
                    &msg,
                )
                .await
            {
                log.error(&err, "Failed to update condition", kv![]);
                return Err(err);
            }
            return Ok(ReconcileResult::default());
        }

        // Step 5: Create/update secret in DPUCluster namespace
        if let Err(err) = self
            .create_or_update_kubeconfig_secret(ctx, provisioner, &secret_name)
            .await
        {
            log.error(&err, "Failed to create/update kubeconfig secret", kv![]);
            let msg = format!(
                "Failed to create kubeconfig secret in namespace {}: {}",
                provisioner.spec.dpu_cluster_ref.namespace, err
            );
            if let Err(cond_err) = self
                .set_condition(
                    ctx,
                    provisioner,
                    CONDITION_FALSE,
                    provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTION_FAILED,
                    &msg,
                )
                .await
            {
                log.error(&cond_err, "Failed to update condition", kv![]);
            }
            // Event emitted by setCondition
            return Err(err);
        }

        log.info(
            "Kubeconfig secret created/updated",
            kv![
                "secretName" => secret_name,
                "namespace" => provisioner.spec.dpu_cluster_ref.namespace
            ],
        );
        self.recorder.event(
            provisioner,
            EVENT_TYPE_NORMAL,
            "KubeConfigInjected",
            &format!(
                "Kubeconfig secret {} created in namespace {}",
                secret_name, provisioner.spec.dpu_cluster_ref.namespace
            ),
        );

        // Step 6: Update DPUCluster CR spec.kubeconfig (only if not already updated)
        if !dpu_cluster_updated {
            if let Err(err) = self
                .update_dpu_cluster_reference(ctx, provisioner, &secret_name)
                .await
            {
                log.error(&err, "Failed to update DPUCluster reference", kv![]);
                if let Err(cond_err) = self
                    .set_condition(
                        ctx,
                        provisioner,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTION_FAILED,
                        &format!("Failed to update DPUCluster spec.kubeconfig: {err}"),
                    )
                    .await
                {
                    log.error(&cond_err, "Failed to update condition", kv![]);
                }
                // Event emitted by setCondition
                return Err(err);
            }

            log.info(
                "DPUCluster updated with kubeconfig reference",
                kv![
                    "dpuCluster" => provisioner.spec.dpu_cluster_ref.name,
                    "namespace" => provisioner.spec.dpu_cluster_ref.namespace
                ],
            );
            self.recorder.event(
                provisioner,
                EVENT_TYPE_NORMAL,
                "DPUClusterUpdated",
                &format!(
                    "DPUCluster {}/{} updated with kubeconfig reference",
                    provisioner.spec.dpu_cluster_ref.namespace,
                    provisioner.spec.dpu_cluster_ref.name
                ),
            );
        } else {
            log.v(1).info(
                "DPUCluster already updated, skipping update",
                kv!["dpuCluster" => provisioner.spec.dpu_cluster_ref.name],
            );
        }

        // Step 7: Update DPFHCPProvisioner status
        provisioner.status.kube_config_secret_ref = Some(LocalObjectReference {
            name: secret_name.clone(),
            ..Default::default()
        });
        let msg = format!(
            "Kubeconfig secret successfully created in namespace {} and DPUCluster CR updated",
            provisioner.spec.dpu_cluster_ref.namespace
        );
        if let Err(err) = self
            .set_condition(
                ctx,
                provisioner,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED,
                &msg,
            )
            .await
        {
            log.error(&err, "Failed to update condition", kv![]);
            return Err(err);
        }

        log.info(
            "Kubeconfig injection completed successfully",
            kv![
                "secretName" => secret_name,
                "dpuCluster" => provisioner.spec.dpu_cluster_ref.name
            ],
        );

        Ok(ReconcileResult::default())
    }

    /// checkInjectionState checks the current state of the kubeconfig injection work
    /// to determine if it has already been completed (fully or partially) for idempotency.
    /// Returns (secretExists, dpuClusterUpdated)
    async fn check_injection_state(
        &self,
        ctx: &Ctx,
        provisioner: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<(bool, bool)> {
        let log = &ctx.log;

        let secret_name = format!("{}{}", provisioner.name(), KUBECONFIG_SECRET_SUFFIX);
        let dpu_cluster_namespace = &provisioner.spec.dpu_cluster_ref.namespace;

        // Check if secret exists in DPUCluster namespace
        let secret_key = ObjectKey::new(dpu_cluster_namespace, &secret_name);
        let secret_res = self.client.get::<Secret>(&secret_key).await;
        let secret_exists = secret_res.is_ok();

        if let Err(secret_err) = secret_res
            && !is_not_found(&secret_err)
        {
            return Err(wrapf!(
                secret_err,
                "failed to check secret existence: {}",
                secret_err
            ));
        }

        // Check if DPUCluster spec.kubeconfig is populated
        let dpu_cluster_key = ObjectKey::new(
            dpu_cluster_namespace,
            &provisioner.spec.dpu_cluster_ref.name,
        );
        let dpu_cluster = match self
            .client
            .get::<dpuprovisioningv1alpha1::DPUCluster>(&dpu_cluster_key)
            .await
        {
            Ok(v) => v,
            Err(dpu_cluster_err) => {
                if is_not_found(&dpu_cluster_err) {
                    return Err(DpuClusterNotFoundError.into());
                }
                return Err(wrapf!(
                    dpu_cluster_err,
                    "failed to get DPUCluster: {}",
                    dpu_cluster_err
                ));
            }
        };

        let dpu_cluster_updated = dpu_cluster.spec.kubeconfig == secret_name;

        log.v(1).info(
            "Injection state check",
            kv![
                "secretExists" => secret_exists,
                "dpuClusterUpdated" => dpu_cluster_updated,
                "dpuClusterKubeconfig" => dpu_cluster.spec.kubeconfig,
                "expectedSecretName" => secret_name
            ],
        );

        Ok((secret_exists, dpu_cluster_updated))
    }

    /// detectHCKubeconfigAvailability checks if the HC admin kubeconfig secret exists
    /// Returns (available, secretName)
    async fn detect_hc_kubeconfig_availability(
        &self,
        ctx: &Ctx,
        provisioner: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> Result<(bool, String)> {
        let log = &ctx.log;

        let secret_name = format!("{}{}", provisioner.name(), KUBECONFIG_SECRET_SUFFIX);
        let secret_key = ObjectKey::new(provisioner.namespace(), &secret_name);

        let err = match self.client.get::<Secret>(&secret_key).await {
            Ok(_) => {
                log.v(1).info(
                    "HC kubeconfig secret found",
                    kv!["secretName" => secret_name],
                );
                return Ok((true, secret_name));
            }
            Err(err) => err,
        };

        if is_not_found(&err) {
            log.v(1).info(
                "HC kubeconfig secret not ready yet",
                kv!["secretName" => secret_name, "namespace" => provisioner.namespace()],
            );
            return Ok((false, String::new()));
        }

        Err(wrapf!(err, "failed to check HC kubeconfig secret: {}", err))
    }

    /// handleIdempotencyScenarios handles different idempotency scenarios
    /// Scenario A: Secret exists AND DPUCluster updated -> check drift
    /// Scenario B: Secret exists BUT DPUCluster NOT updated -> update DPUCluster only
    /// Scenario C: Secret does NOT exist BUT DPUCluster updated -> recreate secret
    /// Scenario D: Neither exists -> proceed with normal injection
    /// Returns needsInjection
    async fn handle_idempotency_scenarios(
        &self,
        ctx: &Ctx,
        provisioner: &mut provisioningv1alpha1::DPFHCPProvisioner,
        secret_name: &str,
        secret_exists: bool,
        dpu_cluster_updated: bool,
    ) -> Result<bool> {
        let log = &ctx.log;

        if secret_exists && dpu_cluster_updated {
            // Scenario A: Check for drift
            log.v(1).info(
                "Scenario A: Secret exists and DPUCluster updated, checking for drift",
                kv![],
            );
            let has_drift = match self.check_drift(ctx, provisioner, secret_name).await {
                Ok(v) => v,
                Err(err) => return Err(wrapf!(err, "failed to check drift: {}", err)),
            };
            if has_drift {
                log.info(
                    "Kubeconfig drift detected, will update destination secret",
                    kv![
                        "secretName" => secret_name,
                        "namespace" => provisioner.spec.dpu_cluster_ref.namespace
                    ],
                );
                self.recorder.event(
                    provisioner,
                    EVENT_TYPE_NORMAL,
                    "DriftCorrected",
                    "Kubeconfig secret content drift detected and corrected",
                );
                // Return true to trigger secret update
                return Ok(true);
            }
            // No drift, no injection needed
            return Ok(false);
        }

        if secret_exists && !dpu_cluster_updated {
            // Scenario B: Update DPUCluster only
            log.info(
                "Scenario B: Secret exists but DPUCluster not updated, completing injection",
                kv![
                    "secretName" => secret_name,
                    "dpuCluster" => provisioner.spec.dpu_cluster_ref.name
                ],
            );
            if let Err(err) = self
                .update_dpu_cluster_reference(ctx, provisioner, secret_name)
                .await
            {
                return Err(wrapf!(
                    err,
                    "failed to update DPUCluster reference: {}",
                    err
                ));
            }
            // Update status
            provisioner.status.kube_config_secret_ref = Some(LocalObjectReference {
                name: secret_name.to_string(),
                ..Default::default()
            });
            return Ok(false);
        }

        if !secret_exists && dpu_cluster_updated {
            // Scenario C: Recreate secret
            log.info(
                "Scenario C: Secret missing but DPUCluster updated, recreating secret",
                kv![
                    "secretName" => secret_name,
                    "namespace" => provisioner.spec.dpu_cluster_ref.namespace
                ],
            );
            // Return true to trigger secret creation
            return Ok(true);
        }

        // Scenario D: Neither exists, proceed with normal injection
        log.v(1).info(
            "Scenario D: Normal injection flow, neither secret nor DPUCluster reference exists",
            kv![],
        );
        Ok(true)
    }

    /// checkDrift compares source and destination secret content
    /// Returns hasDrift
    async fn check_drift(
        &self,
        ctx: &Ctx,
        provisioner: &provisioningv1alpha1::DPFHCPProvisioner,
        secret_name: &str,
    ) -> Result<bool> {
        let log = &ctx.log;

        // Get source secret from HC namespace
        let source_key = ObjectKey::new(provisioner.namespace(), secret_name);
        let source_secret: Secret = match self.client.get(&source_key).await {
            Ok(v) => v,
            Err(err) => return Err(wrapf!(err, "failed to get source secret: {}", err)),
        };

        // Get destination secret from DPUCluster namespace
        let dest_key = ObjectKey::new(&provisioner.spec.dpu_cluster_ref.namespace, secret_name);
        let dest_secret: Secret = match self.client.get(&dest_key).await {
            Ok(v) => v,
            Err(err) => return Err(wrapf!(err, "failed to get destination secret: {}", err)),
        };

        // Compare kubeconfig data
        // Source secret (from HC) uses "kubeconfig" key, destination uses "super-admin.conf"
        let source_data = source_secret
            .data
            .as_ref()
            .and_then(|d| d.get(SOURCE_KUBECONFIG_SECRET_KEY));
        let dest_data = dest_secret
            .data
            .as_ref()
            .and_then(|d| d.get(DESTINATION_KUBECONFIG_SECRET_KEY));

        let Some(source_data) = source_data else {
            return Err(anyhow::anyhow!(
                "source secret missing '{}' key",
                SOURCE_KUBECONFIG_SECRET_KEY
            ));
        };
        let Some(dest_data) = dest_data else {
            return Err(anyhow::anyhow!(
                "destination secret missing '{}' key",
                DESTINATION_KUBECONFIG_SECRET_KEY
            ));
        };

        let has_drift = source_data.0 != dest_data.0;
        if has_drift {
            log.info(
                "Kubeconfig content drift detected",
                kv![
                    "source" => namespaced_name(&source_key),
                    "destination" => namespaced_name(&dest_key)
                ],
            );
        }

        Ok(has_drift)
    }

    /// createOrUpdateKubeconfigSecret creates or updates the secret in DPUCluster namespace
    async fn create_or_update_kubeconfig_secret(
        &self,
        ctx: &Ctx,
        provisioner: &provisioningv1alpha1::DPFHCPProvisioner,
        source_secret_name: &str,
    ) -> Result<()> {
        let log = &ctx.log;

        // Get source secret from HC namespace
        let source_key = ObjectKey::new(provisioner.namespace(), source_secret_name);
        let source_secret: Secret = match self.client.get(&source_key).await {
            Ok(v) => v,
            Err(err) => {
                return Err(wrapf!(
                    err,
                    "failed to read source kubeconfig secret: {}",
                    err
                ));
            }
        };

        // Extract kubeconfig data from source secret (HC namespace)
        // Source secret uses "kubeconfig" key (created by Hypershift)
        let Some(kubeconfig_data) = source_secret
            .data
            .as_ref()
            .and_then(|d| d.get(SOURCE_KUBECONFIG_SECRET_KEY))
            .cloned()
        else {
            return Err(anyhow::anyhow!(
                "source secret missing '{}' key",
                SOURCE_KUBECONFIG_SECRET_KEY
            ));
        };

        // Create destination secret with "super-admin.conf" key (expected by DPF operator)
        let dest_labels = BTreeMap::from([
            (
                common::LABEL_DPFHCP_PROVISIONER_NAME.to_string(),
                provisioner.name().to_string(),
            ),
            (
                common::LABEL_DPFHCP_PROVISIONER_NAMESPACE.to_string(),
                provisioner.namespace().to_string(),
            ),
        ]);
        let dest_data: BTreeMap<String, ByteString> = BTreeMap::from([(
            DESTINATION_KUBECONFIG_SECRET_KEY.to_string(),
            kubeconfig_data,
        )]);
        let mut dest_secret = Secret {
            metadata: ObjectMeta {
                name: Some(source_secret_name.to_string()),
                namespace: Some(provisioner.spec.dpu_cluster_ref.namespace.clone()),
                labels: Some(dest_labels.clone()),
                ..Default::default()
            },
            type_: Some("Opaque".to_string()),
            data: Some(dest_data.clone()),
            ..Default::default()
        };

        // Try to create
        let err = match self.client.create(&mut dest_secret).await {
            Ok(()) => {
                log.info(
                    "Created kubeconfig secret",
                    kv![
                        "secretName" => source_secret_name,
                        "namespace" => provisioner.spec.dpu_cluster_ref.namespace
                    ],
                );
                return Ok(());
            }
            Err(err) => err,
        };

        // If already exists, update it
        if is_already_exists(&err) {
            let existing_key = ObjectKey::new(
                &provisioner.spec.dpu_cluster_ref.namespace,
                source_secret_name,
            );
            let mut existing: Secret = match self.client.get(&existing_key).await {
                Ok(v) => v,
                Err(err) => {
                    return Err(wrapf!(
                        err,
                        "failed to get existing secret for update: {}",
                        err
                    ));
                }
            };

            existing.data = Some(dest_data);
            existing.metadata.labels = Some(dest_labels);
            if let Err(err) = self.client.update(&mut existing).await {
                return Err(wrapf!(err, "failed to update kubeconfig secret: {}", err));
            }

            log.info(
                "Updated existing kubeconfig secret",
                kv![
                    "secretName" => source_secret_name,
                    "namespace" => provisioner.spec.dpu_cluster_ref.namespace
                ],
            );
            return Ok(());
        }

        Err(wrapf!(err, "failed to create kubeconfig secret: {}", err))
    }

    /// updateDPUClusterReference updates DPUCluster spec.kubeconfig field
    async fn update_dpu_cluster_reference(
        &self,
        ctx: &Ctx,
        provisioner: &provisioningv1alpha1::DPFHCPProvisioner,
        secret_name: &str,
    ) -> Result<()> {
        let log = &ctx.log;

        // Get DPUCluster CR
        let dpu_cluster_key = ObjectKey::new(
            &provisioner.spec.dpu_cluster_ref.namespace,
            &provisioner.spec.dpu_cluster_ref.name,
        );
        let mut dpu_cluster: dpuprovisioningv1alpha1::DPUCluster =
            match self.client.get(&dpu_cluster_key).await {
                Ok(v) => v,
                Err(err) => return Err(wrapf!(err, "failed to get DPUCluster: {}", err)),
            };

        // Update spec.kubeconfig field
        dpu_cluster.spec.kubeconfig = secret_name.to_string();

        // Persist update
        if let Err(err) = self.client.update(&mut dpu_cluster).await {
            if is_conflict(&err) {
                return Err(wrapf!(
                    err,
                    "update conflict on DPUCluster, will retry: {}",
                    err
                ));
            }
            return Err(wrapf!(
                err,
                "failed to update DPUCluster spec.kubeconfig: {}",
                err
            ));
        }

        log.v(1).info(
            "Updated DPUCluster spec.kubeconfig",
            kv![
                "dpuCluster" => dpu_cluster.name(),
                "namespace" => dpu_cluster.namespace(),
                "kubeconfig" => secret_name
            ],
        );

        Ok(())
    }

    /// setCondition updates the KubeConfigInjected condition and persists it immediately.
    /// This ensures users can see the condition status even if the injection fails.
    /// Emits Kubernetes events only when the condition status or reason changes to avoid spam.
    /// Returns error if status update fails (controller-runtime will requeue automatically).
    async fn set_condition(
        &self,
        ctx: &Ctx,
        provisioner: &mut provisioningv1alpha1::DPFHCPProvisioner,
        status: &str,
        reason: &str,
        message: &str,
    ) -> Result<()> {
        let log = &ctx.log;

        // Capture previous condition state to detect changes

        // Update condition in memory
        let condition = ctrlrt::meta::condition_gen(
            provisioningv1alpha1::KUBE_CONFIG_INJECTED,
            status,
            reason,
            message,
            provisioner.generation(),
        );

        // Emit event only if condition status/reason changed (avoid spam)
        if ctrlrt::meta::set_status_condition(&mut provisioner.status.conditions, condition) {
            let mut event_type = EVENT_TYPE_NORMAL;
            if status == CONDITION_FALSE {
                event_type = EVENT_TYPE_WARNING;
            }
            self.recorder
                .event(provisioner, event_type, reason, message);
        }

        // Persist status immediately so users can see the condition
        if let Err(err) = self.client.update_status(provisioner).await {
            if is_conflict(&err) {
                // ResourceVersion conflict - controller-runtime will requeue automatically
                log.v(1).info(
                    "Status update conflict, will retry",
                    kv![
                        "condition" => provisioningv1alpha1::KUBE_CONFIG_INJECTED,
                        "reason" => reason
                    ],
                );
                return Err(err);
            }
            log.error(
                &err,
                "Failed to update KubeConfigInjected condition",
                kv!["reason" => reason],
            );
            return Err(wrapf!(
                err,
                "failed to update KubeConfigInjected condition: {}",
                err
            ));
        }

        Ok(())
    }
}
