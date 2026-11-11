//! Port of `controller-runtime/pkg/envtest/crd.go` (CRD reading, installation, waiting).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, PostParams};

use crate::{kube_error, log};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(10);

/// `envtest.CRDInstallOptions` (webhook conversion and scheme handling are not ported).
#[derive(Clone, Debug, Default)]
pub struct CrdInstallOptions {
    /// Paths to directories or files containing CRDs.
    pub paths: Vec<PathBuf>,
    /// CRDs to install in addition to the ones read from `paths`.
    pub crds: Vec<CustomResourceDefinition>,
    /// Fail if a path in `paths` does not exist.
    pub error_if_path_missing: bool,
    /// Max time to wait for the CRDs to appear (default 10s).
    pub max_time: Duration,
    /// Poll interval while waiting (default 100ms).
    pub poll_interval: Duration,
}

fn default_crd_options(o: &mut CrdInstallOptions) {
    if o.max_time.is_zero() {
        o.max_time = DEFAULT_MAX_WAIT;
    }
    if o.poll_interval.is_zero() {
        o.poll_interval = DEFAULT_POLL_INTERVAL;
    }
}

/// `envtest.InstallCRDs(config, options)`
pub async fn install_crds(
    client: &kube::Client,
    mut options: CrdInstallOptions,
) -> anyhow::Result<Vec<CustomResourceDefinition>> {
    default_crd_options(&mut options);
    read_crd_files(&mut options)
        .map_err(|err| ctrlrt::wrapf!(err, "unable to read CRD files: {}", err))?;
    create_crds(client, &mut options.crds)
        .await
        .map_err(|err| ctrlrt::wrapf!(err, "unable to create CRD instances: {}", err))?;
    wait_for_crds(client, &options.crds, &options)
        .await
        .map_err(|err| {
            ctrlrt::wrapf!(
                err,
                "something went wrong waiting for CRDs to appear as API resources: {}",
                err
            )
        })?;
    Ok(options.crds)
}

/// `envtest.ReadCRDFiles(options)` — appends the CRDs read from `options.paths`.
pub fn read_crd_files(options: &mut CrdInstallOptions) -> anyhow::Result<()> {
    if !options.paths.is_empty() {
        let list = render_crds(options)?;
        options.crds.extend(list);
    }
    Ok(())
}

fn go_io_error(op: &str, path: &Path, err: std::io::Error) -> anyhow::Error {
    let msg = match err.kind() {
        std::io::ErrorKind::NotFound => "no such file or directory".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        std::io::ErrorKind::NotADirectory => "not a directory".to_string(),
        _ => err.to_string(),
    };
    let e = anyhow::anyhow!("{} {}: {}", op, path.display(), msg);
    ctrlrt::wrapf!(err, "{}", e)
}

fn render_crds(options: &CrdInstallOptions) -> anyhow::Result<Vec<CustomResourceDefinition>> {
    // Keyed by name (the GVK of a CRD object is always the same); insertion order kept.
    let mut order: Vec<String> = Vec::new();
    let mut crds: BTreeMap<String, CustomResourceDefinition> = BTreeMap::new();

    for path in &options.paths {
        let info = match std::fs::metadata(path) {
            Ok(i) => i,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if options.error_if_path_missing {
                    return Err(go_io_error("stat", path, err));
                }
                continue;
            }
            Err(err) => return Err(go_io_error("stat", path, err)),
        };
        let (base, files): (PathBuf, Vec<String>) = if !info.is_dir() {
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (dir, vec![name])
        } else {
            let mut names = Vec::new();
            let entries = std::fs::read_dir(path).map_err(|err| go_io_error("open", path, err))?;
            for e in entries {
                let e = e.map_err(|err| go_io_error("readdirent", path, err))?;
                names.push(e.file_name().to_string_lossy().into_owned());
            }
            names.sort();
            (path.clone(), names)
        };

        log().v(1).info(
            "reading CRDs from path",
            ctrlrt::kv!["path" => path.display().to_string()],
        );
        for crd in read_crds(&base, &files)? {
            let name = crd.metadata.name.clone().unwrap_or_default();
            if crds.contains_key(&name) {
                log().info(
                    "there are more than one CRD definitions with the same <Group, Version, Kind, Name>",
                    ctrlrt::kv!["GVKN" => format!("apiextensions.k8s.io/v1, Kind=CustomResourceDefinition {name}")],
                );
            } else {
                order.push(name.clone());
            }
            crds.insert(name, crd);
        }
    }
    Ok(order.into_iter().filter_map(|n| crds.remove(&n)).collect())
}

fn read_crds(base: &Path, files: &[String]) -> anyhow::Result<Vec<CustomResourceDefinition>> {
    let mut out = Vec::new();
    for file in files {
        let ext = Path::new(file)
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !matches!(ext.as_str(), "json" | "yaml" | "yml") {
            continue;
        }
        let fp = base.join(file);
        for doc in read_documents(&fp)? {
            if let Some(crd) = parse_crd(doc)? {
                out.push(crd);
            }
        }
        log()
            .v(1)
            .info("read CRDs from file", ctrlrt::kv!["file" => file]);
    }
    Ok(out)
}

/// Decodes one document; returns `None` for documents that are not CRDs (Go skips them).
fn parse_crd(mut doc: serde_json::Value) -> anyhow::Result<Option<CustomResourceDefinition>> {
    let Some(obj) = doc.as_object_mut() else {
        return Ok(None);
    };
    let kind = obj.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let spec = obj.get("spec");
    let names_kind = spec
        .and_then(|s| s.get("names"))
        .and_then(|n| n.get("kind"))
        .and_then(|k| k.as_str())
        .unwrap_or("");
    let group = spec
        .and_then(|s| s.get("group"))
        .and_then(|g| g.as_str())
        .unwrap_or("");
    if kind != "CustomResourceDefinition" || names_kind.is_empty() || group.is_empty() {
        return Ok(None);
    }
    // The document is decoded into the apiextensions.k8s.io/v1 type, like Go does.
    obj.insert(
        "apiVersion".into(),
        serde_json::Value::String("apiextensions.k8s.io/v1".into()),
    );
    let crd: CustomResourceDefinition = serde_json::from_value(doc)
        .map_err(|err| anyhow::anyhow!("error unmarshaling JSON: {}", err))?;
    Ok(Some(crd))
}

/// Splits a (multi-document) YAML or JSON file into decoded documents.
fn read_documents(fp: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    let text = std::fs::read_to_string(fp).map_err(|err| go_io_error("open", fp, err))?;
    let docs: Vec<serde_json::Value> = serde_saphyr::from_multiple(&text)
        .map_err(|err| anyhow::anyhow!("error converting YAML to JSON: {}", err))?;
    Ok(docs.into_iter().filter(|d| !d.is_null()).collect())
}

/// `retry.DefaultBackoff` durations between attempts (Steps 4, 10ms, factor 5, jitter 0.1).
fn default_backoff_sleep(step: u32) -> Duration {
    let base = 10.0 * 5f64.powi(step as i32);
    let mut b = [0u8; 2];
    let jitter = if getrandom::fill(&mut b).is_ok() {
        f64::from(u16::from_le_bytes(b)) / f64::from(u16::MAX) * 0.1
    } else {
        0.0
    };
    Duration::from_micros((base * (1.0 + jitter) * 1000.0) as u64)
}

/// `envtest.CreateCRDs(config, crds)` — creates the CRDs, or updates them if they exist.
pub async fn create_crds(
    client: &kube::Client,
    crds: &mut [CustomResourceDefinition],
) -> anyhow::Result<()> {
    let api: Api<CustomResourceDefinition> = Api::all(client.clone());
    let pp = PostParams::default();
    for crd in crds.iter_mut() {
        let name = crd.metadata.name.clone().unwrap_or_default();
        let q = ctrlrt::error::go_quote(&name);
        log()
            .v(1)
            .info("installing CRD", ctrlrt::kv!["crd" => name]);
        match api.get(&name).await.map_err(kube_error) {
            Err(err) if ctrlrt::error::is_not_found(&err) => {
                let created = api
                    .create(&pp, crd)
                    .await
                    .map_err(kube_error)
                    .map_err(|err| ctrlrt::wrapf!(err, "unable to create CRD {}: {}", q, err))?;
                *crd = created;
            }
            Err(err) => {
                return Err(ctrlrt::wrapf!(
                    err,
                    "unable to get CRD {} to check if it exists: {}",
                    q,
                    err
                ));
            }
            Ok(_) => {
                log()
                    .v(1)
                    .info("CRD already exists, updating", ctrlrt::kv!["crd" => name]);
                // retry.RetryOnConflict(retry.DefaultBackoff, ...)
                let steps = 4;
                let mut step = 0;
                loop {
                    let attempt = async {
                        let existing = api.get(&name).await.map_err(kube_error)?;
                        crd.metadata.resource_version = existing.metadata.resource_version;
                        api.replace(&name, &pp, crd).await.map_err(kube_error)
                    }
                    .await;
                    match attempt {
                        Ok(updated) => {
                            *crd = updated;
                            break;
                        }
                        Err(err) if ctrlrt::error::is_conflict(&err) && step + 1 < steps => {
                            tokio::time::sleep(default_backoff_sleep(step)).await;
                            step += 1;
                        }
                        Err(err) => return Err(err),
                    }
                }
            }
        }
    }
    Ok(())
}

fn established(crd: &CustomResourceDefinition) -> bool {
    crd.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .is_some_and(|conds| {
            conds
                .iter()
                .any(|c| c.type_ == "Established" && c.status == "True")
        })
}

/// `envtest.WaitForCRDs(config, crds, options)` — polls discovery until every served
/// version of every CRD lists the CRD's plural; additionally waits for the CRDs'
/// `Established` condition.
pub async fn wait_for_crds(
    client: &kube::Client,
    crds: &[CustomResourceDefinition],
    options: &CrdInstallOptions,
) -> anyhow::Result<()> {
    let mut waiting_for: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for crd in crds {
        for version in &crd.spec.versions {
            if version.served {
                let gv = format!("{}/{}", crd.spec.group, version.name);
                log()
                    .v(1)
                    .info("adding API in waitlist", ctrlrt::kv!["GV" => gv]);
                waiting_for
                    .entry(gv)
                    .or_default()
                    .insert(crd.spec.names.plural.clone());
            }
        }
    }
    let mut pending_established: BTreeSet<String> = crds
        .iter()
        .filter_map(|c| c.metadata.name.clone())
        .collect();
    let api: Api<CustomResourceDefinition> = Api::all(client.clone());
    let poll_interval = if options.poll_interval.is_zero() {
        DEFAULT_POLL_INTERVAL
    } else {
        options.poll_interval
    };
    let max_time = if options.max_time.is_zero() {
        DEFAULT_MAX_WAIT
    } else {
        options.max_time
    };
    let deadline = Instant::now() + max_time;

    loop {
        // poller.poll
        let mut all_found = true;
        let gvs: Vec<String> = waiting_for.keys().cloned().collect();
        for gv in gvs {
            let Some(resources) = waiting_for.get_mut(&gv) else {
                continue;
            };
            if resources.is_empty() {
                waiting_for.remove(&gv);
                continue;
            }
            match client.list_api_group_resources(&gv).await {
                Ok(list) => {
                    for r in &list.resources {
                        resources.remove(&r.name);
                    }
                    if !resources.is_empty() {
                        all_found = false;
                    }
                }
                Err(_) => {
                    all_found = false;
                    break;
                }
            }
        }
        if all_found {
            let names: Vec<String> = pending_established.iter().cloned().collect();
            for name in names {
                match api.get(&name).await {
                    Ok(c) if established(&c) => {
                        pending_established.remove(&name);
                    }
                    _ => {
                        all_found = false;
                    }
                }
            }
        }
        if all_found {
            return Ok(());
        }
        if Instant::now() + poll_interval > deadline {
            anyhow::bail!("context deadline exceeded");
        }
        tokio::time::sleep(poll_interval).await;
    }
}

/// `envtest.UninstallCRDs(config, options)`
pub async fn uninstall_crds(
    client: &kube::Client,
    mut options: CrdInstallOptions,
) -> anyhow::Result<()> {
    read_crd_files(&mut options)?;
    let api: Api<CustomResourceDefinition> = Api::all(client.clone());
    for crd in &options.crds {
        let name = crd.metadata.name.clone().unwrap_or_default();
        log()
            .v(1)
            .info("uninstalling CRD", ctrlrt::kv!["crd" => name]);
        if let Err(err) = api
            .delete(&name, &Default::default())
            .await
            .map_err(kube_error)
            && !ctrlrt::error::is_not_found(&err)
        {
            return Err(err);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_crd_skips_non_crds() {
        let cm = serde_json::json!({"apiVersion": "v1", "kind": "ConfigMap"});
        assert!(parse_crd(cm).unwrap().is_none());
        let no_group = serde_json::json!({
            "apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
            "spec": {"names": {"kind": "Foo"}}
        });
        assert!(parse_crd(no_group).unwrap().is_none());
    }

    #[test]
    fn missing_path_errors_like_go() {
        let mut o = CrdInstallOptions {
            paths: vec![PathBuf::from("/nonexistent/crds")],
            error_if_path_missing: true,
            ..Default::default()
        };
        assert_eq!(
            read_crd_files(&mut o).unwrap_err().to_string(),
            "stat /nonexistent/crds: no such file or directory"
        );
        o.error_if_path_missing = false;
        read_crd_files(&mut o).unwrap();
        assert!(o.crds.is_empty());
    }

    #[test]
    fn reads_repo_crds() {
        let root = crate::workspace_root();
        let mut o = CrdInstallOptions {
            paths: vec![
                root.join("config/crd/bases"),
                root.join("config/crd/external"),
            ],
            error_if_path_missing: true,
            ..Default::default()
        };
        read_crd_files(&mut o).unwrap();
        assert!(o.crds.iter().any(|c| c.metadata.name.as_deref()
            == Some("dpfhcpprovisionerconfigs.provisioning.dpu.hcp.io")));
        assert!(
            o.crds
                .iter()
                .any(|c| c.metadata.name.as_deref()
                    == Some("hostedclusters.hypershift.openshift.io"))
        );
    }
}
