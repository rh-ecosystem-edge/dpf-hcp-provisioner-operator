//! Port of `test/e2e/e2e_fake_ocp_on_kind_test.go`: stubs for the OpenShift resources the
//! operator reads, so the DPUServiceTemplate controller can run on Kind.

use std::collections::BTreeMap;

use apis::openshift::config::v1 as configv1;
use ctrlrt::ObjectKey;
use ctrlrt::error::is_already_exists;
use e2e::ginkgo::{NodeResult, by};
use e2e::gomega as g;
use k8s_openapi::ByteString;
use k8s_openapi::api::apps::v1::{DaemonSet, DaemonSetSpec};
use k8s_openapi::api::core::v1::{Container, PodSpec, PodTemplateSpec, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};

use crate::Env;
use crate::ci_helpers::{create_namespace, meta};

/// `createClusterVersionStub` creates a ClusterVersion CR with the given version and release image.
pub async fn create_cluster_version_stub(
    env: &Env,
    version: &str,
    release_image: &str,
) -> NodeResult {
    let mut cv = configv1::ClusterVersion {
        metadata: meta("", "version"),
        spec: configv1::ClusterVersionSpec {
            cluster_id: "e2e-test-cluster".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    if let Err(e) = env.client.create(&mut cv).await
        && !is_already_exists(&e)
    {
        g::ok(Err::<(), _>(e), "Failed to create ClusterVersion stub")?;
    }

    // Patch status subresource
    g::ok(
        env.client
            .get_into(&ObjectKey::cluster("version"), &mut cv)
            .await,
        "",
    )?;
    cv.status.desired = configv1::Release {
        version: version.into(),
        image: release_image.into(),
        ..Default::default()
    };
    g::ok(
        env.client.update_status(&mut cv).await,
        "Failed to update ClusterVersion status",
    )?;
    Ok(())
}

/// The `ovnkube-node-dpu-host` DaemonSet stub.
pub fn ovn_daemon_set_stub(image: &str) -> DaemonSet {
    let labels = BTreeMap::from([("app".to_string(), "ovnkube-node-dpu-host".to_string())]);
    DaemonSet {
        metadata: meta("openshift-ovn-kubernetes", "ovnkube-node-dpu-host"),
        spec: Some(DaemonSetSpec {
            selector: LabelSelector {
                match_labels: Some(labels.clone()),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    node_selector: Some(BTreeMap::from([(
                        "non-existent-label".to_string(),
                        "true".to_string(),
                    )])),
                    containers: vec![Container {
                        name: "ovnkube-controller".into(),
                        image: Some(image.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// `createOVNDaemonSetStub` creates the ovnkube-node-dpu-host DaemonSet with a
/// rollout-complete status.
pub async fn create_ovn_daemon_set_stub(env: &Env, image: &str) -> NodeResult {
    by("creating openshift-ovn-kubernetes namespace");
    create_namespace(env, "openshift-ovn-kubernetes").await?;

    // Use a nodeSelector that matches nothing so the real DaemonSet controller
    // sets DesiredNumberScheduled=0, making 0==0==0 pass the rollout check.
    let mut ds = ovn_daemon_set_stub(image);
    if let Err(e) = env.client.create(&mut ds).await
        && !is_already_exists(&e)
    {
        g::ok(Err::<(), _>(e), "Failed to create OVN DaemonSet stub")?;
    }
    Ok(())
}

/// The `.dockerconfigjson` of the pull secret stub (`json.Marshal` of the Go map: sorted keys).
pub fn stub_docker_config_json() -> Vec<u8> {
    let docker_config = serde_json::json!({
        "auths": {
            "quay.io": {
                "auth": "ZTJlLXRlc3Q6cGFzcw==",
            },
        },
    });
    gocompat::json::marshal(&docker_config)
}

/// `createPullSecretStub` creates a dummy pull secret in openshift-config.
pub async fn create_pull_secret_stub(env: &Env) -> NodeResult {
    by("creating openshift-config namespace");
    create_namespace(env, "openshift-config").await?;

    let docker_config_json = stub_docker_config_json();

    let mut secret = Secret {
        metadata: meta("openshift-config", "pull-secret"),
        type_: Some("kubernetes.io/dockerconfigjson".into()),
        data: Some(BTreeMap::from([(
            ".dockerconfigjson".to_string(),
            ByteString(docker_config_json),
        )])),
        ..Default::default()
    };
    if let Err(e) = env.client.create(&mut secret).await
        && !is_already_exists(&e)
    {
        g::ok(Err::<(), _>(e), "Failed to create pull secret stub")?;
    }
    Ok(())
}

/// `createDummySecrets` creates placeholder secrets for use on Kind where real secrets don't exist.
pub async fn create_dummy_secrets(env: &Env, ns: &str, names: &[&str]) -> NodeResult {
    for name in names {
        let mut secret = Secret {
            metadata: meta(ns, name),
            data: Some(BTreeMap::from([(
                "dummy".to_string(),
                ByteString(b"data".to_vec()),
            )])),
            ..Default::default()
        };
        if let Err(e) = env.client.create(&mut secret).await
            && !is_already_exists(&e)
        {
            g::ok(Err::<(), _>(e), &format!("Failed to create secret {name}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_secret_stub_bytes() {
        assert_eq!(
            String::from_utf8(stub_docker_config_json()).unwrap(),
            r#"{"auths":{"quay.io":{"auth":"ZTJlLXRlc3Q6cGFzcw=="}}}"#
        );
    }

    #[test]
    fn ovn_daemon_set_stub_shape() {
        let v = ctrlrt::client::to_value(&ovn_daemon_set_stub("img")).unwrap();
        assert_eq!(
            v["spec"]["selector"]["matchLabels"]["app"],
            "ovnkube-node-dpu-host"
        );
        assert_eq!(
            v["spec"]["template"]["spec"]["nodeSelector"]["non-existent-label"],
            "true"
        );
        assert_eq!(
            v["spec"]["template"]["spec"]["containers"][0]["image"],
            "img"
        );
    }
}
