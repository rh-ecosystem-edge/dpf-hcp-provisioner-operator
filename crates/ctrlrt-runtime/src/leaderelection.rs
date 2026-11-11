//! Leader election: port of client-go `tools/leaderelection` (LeaderElector, LeaseLock) and
//! controller-runtime `pkg/leaderelection` (resource lock construction, namespace detection).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use http::Method;
use k8s_openapi::api::core::v1::ObjectReference;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::Result;
use crate::client::{RequestInfo, RestClient};
use crate::config::RestConfig;
use crate::klog;
use crate::metrics;

/// `leaderelection.JitterFactor`
pub const JITTER_FACTOR: f64 = 1.2;

const IN_CLUSTER_NAMESPACE_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/namespace";

/// `resourcelock.LeaderElectionRecord`
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeaderElectionRecord {
    pub holder_identity: String,
    pub lease_duration_seconds: i64,
    pub acquire_time: Option<DateTime<Utc>>,
    pub renew_time: Option<DateTime<Utc>>,
    pub leader_transitions: i64,
    pub strategy: String,
    pub preferred_holder: String,
}

/// `metav1.MicroTime` JSON encoding (RFC3339 with microseconds, UTC).
fn micro_time(t: &Option<DateTime<Utc>>) -> Value {
    match t {
        Some(t) => Value::String(t.to_rfc3339_opts(SecondsFormat::Micros, true)),
        None => Value::Null,
    }
}

fn parse_time(v: Option<&Value>) -> Option<DateTime<Utc>> {
    v.and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
}

/// `LeaseSpecToLeaderElectionRecord`
fn spec_to_record(spec: &Value) -> LeaderElectionRecord {
    LeaderElectionRecord {
        holder_identity: spec
            .get("holderIdentity")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        lease_duration_seconds: spec
            .get("leaseDurationSeconds")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        leader_transitions: spec
            .get("leaseTransitions")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        acquire_time: parse_time(spec.get("acquireTime")),
        renew_time: parse_time(spec.get("renewTime")),
        preferred_holder: spec
            .get("preferredHolder")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        strategy: spec
            .get("strategy")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
    }
}

/// `LeaderElectionRecordToLeaseSpec`
fn record_to_spec(r: &LeaderElectionRecord) -> Value {
    let mut spec = json!({
        "holderIdentity": r.holder_identity,
        "leaseDurationSeconds": r.lease_duration_seconds,
        "acquireTime": micro_time(&r.acquire_time),
        "renewTime": micro_time(&r.renew_time),
        "leaseTransitions": r.leader_transitions,
    });
    if !r.preferred_holder.is_empty() {
        spec["preferredHolder"] = Value::String(r.preferred_holder.clone());
    }
    if !r.strategy.is_empty() {
        spec["strategy"] = Value::String(r.strategy.clone());
    }
    spec
}

/// Event recorder hook used by the lock (`ResourceLockConfig.EventRecorder`).
pub type LockRecorder = Arc<dyn Fn(&ObjectReference, &str, &str, &str) + Send + Sync>;

/// `resourcelock.LeaseLock`
pub struct LeaseLock {
    rest: RestClient,
    namespace: String,
    name: String,
    identity: String,
    labels: Option<std::collections::BTreeMap<String, String>>,
    recorder: Option<ctrlrt::Recorder>,
    lease: Mutex<Option<Value>>,
}

impl LeaseLock {
    pub fn new(
        rest: RestClient,
        namespace: &str,
        name: &str,
        identity: &str,
        recorder: Option<ctrlrt::Recorder>,
    ) -> Self {
        Self {
            rest,
            namespace: namespace.into(),
            name: name.into(),
            identity: identity.into(),
            labels: None,
            recorder,
            lease: Mutex::new(None),
        }
    }

    fn path(&self, with_name: bool) -> String {
        let mut p = format!(
            "/apis/coordination.k8s.io/v1/namespaces/{}/leases",
            self.namespace
        );
        if with_name {
            p.push('/');
            p.push_str(&self.name);
        }
        p
    }

    fn info(&self) -> RequestInfo {
        RequestInfo {
            group: "coordination.k8s.io".into(),
            resource: "leases".into(),
            name: self.name.clone(),
        }
    }

    /// `Get`
    pub async fn get(&self) -> Result<LeaderElectionRecord> {
        let v = self
            .rest
            .request(
                Method::GET,
                &self.path(true),
                None,
                None,
                None,
                Some(&self.info()),
            )
            .await?;
        let rec = spec_to_record(v.get("spec").unwrap_or(&Value::Null));
        *self.lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        Ok(rec)
    }

    /// `Create`
    pub async fn create(&self, ler: &LeaderElectionRecord) -> Result<()> {
        let mut md = json!({"name": self.name, "namespace": self.namespace});
        if let Some(l) = &self.labels {
            md["labels"] = json!(l);
        }
        let body = json!({
            "apiVersion": "coordination.k8s.io/v1",
            "kind": "Lease",
            "metadata": md,
            "spec": record_to_spec(ler),
        });
        let v = self
            .rest
            .request(
                Method::POST,
                &self.path(false),
                Some(serde_json::to_vec(&body)?),
                Some("application/json"),
                None,
                Some(&RequestInfo {
                    name: String::new(),
                    ..self.info()
                }),
            )
            .await?;
        *self.lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        Ok(())
    }

    /// `Update`
    pub async fn update(&self, ler: &LeaderElectionRecord) -> Result<()> {
        let mut lease = match self.lease.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(l) => l,
            None => {
                return Err(anyhow::anyhow!(
                    "lease not initialized, call get or create first"
                ));
            }
        };
        lease["spec"] = record_to_spec(ler);
        if let Some(labels) = &self.labels {
            let md = &mut lease["metadata"];
            if md.get("labels").is_none_or(|l| l.is_null()) {
                md["labels"] = json!({});
            }
            for (k, v) in labels {
                md["labels"][k] = Value::String(v.clone());
            }
        }
        if let Value::Object(m) = &mut lease {
            m.insert("apiVersion".into(), "coordination.k8s.io/v1".into());
            m.insert("kind".into(), "Lease".into());
        }
        let v = self
            .rest
            .request(
                Method::PUT,
                &self.path(true),
                Some(serde_json::to_vec(&lease)?),
                Some("application/json"),
                None,
                Some(&self.info()),
            )
            .await?;
        *self.lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        Ok(())
    }

    /// `RecordEvent`
    pub fn record_event(&self, s: &str) {
        let Some(rec) = &self.recorder else { return };
        let lease = self.lease.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let md = lease
            .as_ref()
            .and_then(|l| l.get("metadata").cloned())
            .unwrap_or(Value::Null);
        let field = |k: &str| {
            md.get(k)
                .and_then(Value::as_str)
                .map(String::from)
                .filter(|s| !s.is_empty())
        };
        let reference = ObjectReference {
            api_version: Some("coordination.k8s.io/v1".into()),
            kind: Some("Lease".into()),
            name: field("name"),
            namespace: field("namespace"),
            uid: field("uid"),
            resource_version: field("resourceVersion"),
            field_path: None,
        };
        rec.event_ref(
            &reference,
            ctrlrt::recorder::EVENT_TYPE_NORMAL,
            "LeaderElection",
            &format!("{} {}", self.identity, s),
        );
    }

    /// `Describe`
    pub fn describe(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    /// `Identity`
    pub fn identity(&self) -> &str {
        &self.identity
    }
}

/// `leaderelection.LeaderElectionConfig` (subset).
#[derive(Clone, Debug)]
pub struct LeaderElectionConfig {
    pub lease_duration: Duration,
    pub renew_deadline: Duration,
    pub retry_period: Duration,
    pub release_on_cancel: bool,
    /// `Name` (used for metrics).
    pub name: String,
}

struct Observed {
    record: LeaderElectionRecord,
    time: Option<Instant>,
    raw: Option<LeaderElectionRecord>,
}

/// `leaderelection.LeaderElector`
pub struct LeaderElector {
    cfg: LeaderElectionConfig,
    lock: Arc<LeaseLock>,
    observed: Mutex<Observed>,
}

/// `wait.Jitter(d, maxFactor)`
fn jitter(d: Duration, max_factor: f64) -> Duration {
    let f = if max_factor <= 0.0 { 1.0 } else { max_factor };
    d + Duration::from_secs_f64(fastrand::f64() * f * d.as_secs_f64())
}

impl LeaderElector {
    /// `NewLeaderElector`
    pub fn new(cfg: LeaderElectionConfig, lock: Arc<LeaseLock>) -> Result<Self> {
        if cfg.lease_duration <= cfg.renew_deadline {
            return Err(anyhow::anyhow!(
                "leaseDuration must be greater than renewDeadline"
            ));
        }
        if cfg.renew_deadline <= cfg.retry_period.mul_f64(JITTER_FACTOR) {
            return Err(anyhow::anyhow!(
                "renewDeadline must be greater than retryPeriod*JitterFactor"
            ));
        }
        if cfg.lease_duration.is_zero() {
            return Err(anyhow::anyhow!("leaseDuration must be greater than zero"));
        }
        if cfg.renew_deadline.is_zero() {
            return Err(anyhow::anyhow!("renewDeadline must be greater than zero"));
        }
        if cfg.retry_period.is_zero() {
            return Err(anyhow::anyhow!("retryPeriod must be greater than zero"));
        }
        if lock.identity().is_empty() {
            return Err(anyhow::anyhow!("Lock identity is empty"));
        }
        metrics::LEADER_GAUGE
            .with_label_values(&[&cfg.name])
            .set(0.0);
        Ok(Self {
            cfg,
            lock,
            observed: Mutex::new(Observed {
                record: LeaderElectionRecord::default(),
                time: None,
                raw: None,
            }),
        })
    }

    fn obs(&self) -> std::sync::MutexGuard<'_, Observed> {
        self.observed.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `IsLeader()`
    pub fn is_leader(&self) -> bool {
        self.obs().record.holder_identity == self.lock.identity()
    }

    /// `GetLeader()`
    pub fn get_leader(&self) -> String {
        self.obs().record.holder_identity.clone()
    }

    fn is_lease_valid(&self, now: Instant) -> bool {
        let o = self.obs();
        match o.time {
            None => false,
            Some(t) => t + Duration::from_secs(o.record.lease_duration_seconds.max(0) as u64) > now,
        }
    }

    fn set_observed_record(&self, r: &LeaderElectionRecord) {
        let mut o = self.obs();
        o.record = r.clone();
        o.time = Some(Instant::now());
    }

    /// `Run(ctx)`: blocks until leadership is lost or `ctx` is cancelled. `on_started` is invoked
    /// (in a new task) once leadership is acquired, `on_stopped` always when `run` returns.
    pub async fn run<S, T>(self: Arc<Self>, ctx: CancellationToken, on_started: S, on_stopped: T)
    where
        S: FnOnce() + Send + 'static,
        T: FnOnce(),
    {
        struct Guard<T: FnOnce()>(Option<T>);
        impl<T: FnOnce()> Drop for Guard<T> {
            fn drop(&mut self) {
                if let Some(f) = self.0.take() {
                    f()
                }
            }
        }
        let _stopped = Guard(Some(on_stopped));
        if !self.acquire(&ctx).await {
            return; // ctx signalled done
        }
        let lead_ctx = ctx.child_token();
        tokio::spawn(async move { on_started() });
        self.renew(&lead_ctx).await;
        lead_ctx.cancel();
    }

    async fn acquire(&self, ctx: &CancellationToken) -> bool {
        let desc = self.lock.describe();
        klog::info(
            "leaderelection.go:258",
            "Attempting to acquire leader lease...",
            &[("lock", Value::String(desc.clone()))],
        );
        loop {
            if ctx.is_cancelled() {
                return false;
            }
            let succeeded = tokio::select! {
                _ = ctx.cancelled() => return false,
                s = self.try_acquire_or_renew() => s,
            };
            if succeeded {
                self.lock.record_event("became leader");
                metrics::LEADER_GAUGE
                    .with_label_values(&[&self.cfg.name])
                    .set(1.0);
                klog::info(
                    "leaderelection.go:272",
                    "Successfully acquired lease",
                    &[("lock", Value::String(desc.clone()))],
                );
                return true;
            }
            klog::info_v(
                4,
                "leaderelection.go:267",
                "Failed to acquire lease",
                &[("lock", Value::String(desc.clone()))],
            );
            tokio::select! {
                _ = ctx.cancelled() => return false,
                _ = tokio::time::sleep(jitter(self.cfg.retry_period, JITTER_FACTOR)) => {}
            }
        }
    }

    async fn renew(&self, ctx: &CancellationToken) {
        loop {
            if ctx.is_cancelled() {
                break;
            }
            // wait.PollUntilContextTimeout(ctx, RetryPeriod, RenewDeadline, true, tryAcquireOrRenew)
            let deadline = tokio::time::Instant::now() + self.cfg.renew_deadline;
            let err: Option<&str> = loop {
                let r = tokio::select! {
                    _ = ctx.cancelled() => None,
                    _ = tokio::time::sleep_until(deadline) => Some(false),
                    ok = self.try_acquire_or_renew() => Some(ok),
                };
                match r {
                    None => break Some("context canceled"),
                    Some(true) => break None,
                    Some(false) => {}
                }
                if tokio::time::Instant::now() >= deadline {
                    break Some("context deadline exceeded");
                }
                tokio::select! {
                    _ = ctx.cancelled() => break Some("context canceled"),
                    _ = tokio::time::sleep_until(deadline) => break Some("context deadline exceeded"),
                    _ = tokio::time::sleep(self.cfg.retry_period) => {}
                }
            };
            let desc = self.lock.describe();
            match err {
                None => {
                    klog::info_v(
                        5,
                        "leaderelection.go:295",
                        "Successfully renewed lease",
                        &[("lock", Value::String(desc))],
                    );
                }
                Some(e) => {
                    metrics::LEADER_GAUGE
                        .with_label_values(&[&self.cfg.name])
                        .set(0.0);
                    klog::info(
                        "leaderelection.go:299",
                        "Failed to renew lease",
                        &[
                            ("lock", Value::String(desc)),
                            ("err", Value::String(e.to_string())),
                        ],
                    );
                    break;
                }
            }
            tokio::select! {
                _ = ctx.cancelled() => break,
                _ = tokio::time::sleep(self.cfg.retry_period) => {}
            }
        }
        if self.cfg.release_on_cancel {
            self.release().await;
        }
        self.lock.record_event("stopped leading");
    }

    async fn release(&self) -> bool {
        let fut = async {
            if let Err(e) = self.lock.get().await {
                if !ctrlrt::error::is_not_found(&e) {
                    klog::error(
                        "leaderelection.go:318",
                        &e,
                        "error retrieving resource lock",
                        &[("lock", Value::String(self.lock.describe()))],
                    );
                    return false;
                }
                klog::info(
                    "leaderelection.go:321",
                    "lease lock not found",
                    &[("lock", Value::String(self.lock.describe()))],
                );
                return false;
            }
            if !self.is_leader() {
                return true;
            }
            let old = self.obs().record.clone();
            let now = Utc::now();
            let rec = LeaderElectionRecord {
                leader_transitions: old.leader_transitions,
                lease_duration_seconds: 1,
                renew_time: Some(now),
                acquire_time: Some(now),
                ..Default::default()
            };
            if let Err(e) = self.lock.update(&rec).await {
                klog::error(
                    "leaderelection.go:336",
                    &e,
                    "Failed to release lease",
                    &[("lock", Value::String(self.lock.describe()))],
                );
                return false;
            }
            self.set_observed_record(&rec);
            true
        };
        tokio::time::timeout(self.cfg.renew_deadline, fut)
            .await
            .unwrap_or(false)
    }

    /// `tryAcquireOrRenew`
    async fn try_acquire_or_renew(&self) -> bool {
        let now = Utc::now();
        let mut rec = LeaderElectionRecord {
            holder_identity: self.lock.identity().to_string(),
            lease_duration_seconds: self.cfg.lease_duration.as_secs() as i64,
            renew_time: Some(now),
            acquire_time: Some(now),
            ..Default::default()
        };
        let desc = self.lock.describe();

        // 1. fast path for the leader to update optimistically assuming that the record observed
        // last time is the current version.
        if self.is_leader() && self.is_lease_valid(Instant::now()) {
            let old = self.obs().record.clone();
            rec.acquire_time = old.acquire_time;
            rec.leader_transitions = old.leader_transitions;
            match self.lock.update(&rec).await {
                Ok(()) => {
                    self.set_observed_record(&rec);
                    return true;
                }
                Err(e) => klog::error(
                    "leaderelection.go:445",
                    &e,
                    "Failed to update lease optimistically, falling back to slow path",
                    &[("lock", Value::String(desc.clone()))],
                ),
            }
        }

        // 2. obtain the lease
        let old = match self.lock.get().await {
            Ok(r) => r,
            Err(e) => {
                if !ctrlrt::error::is_not_found(&e) {
                    klog::error(
                        "leaderelection.go:452",
                        &e,
                        "Error retrieving lease lock",
                        &[("lock", Value::String(desc))],
                    );
                    return false;
                }
                if let Err(e) = self.lock.create(&rec).await {
                    klog::error(
                        "leaderelection.go:456",
                        &e,
                        "Error initially creating lease lock",
                        &[("lock", Value::String(desc))],
                    );
                    return false;
                }
                self.set_observed_record(&rec);
                return true;
            }
        };

        // 3. Record obtained, check the Identity & Time
        let changed = self.obs().raw.as_ref() != Some(&old);
        if changed {
            self.set_observed_record(&old);
            self.obs().raw = Some(old.clone());
        }
        if !old.holder_identity.is_empty()
            && self.is_lease_valid(Instant::now())
            && !self.is_leader()
        {
            klog::info_v(
                4,
                "leaderelection.go:472",
                "Lease is held by and has not yet expired",
                &[
                    ("lock", Value::String(desc)),
                    ("holder", Value::String(old.holder_identity.clone())),
                ],
            );
            return false;
        }

        // 4. We're going to try to update. The leaderElectionRecord is set to it's default
        // here. Let's correct it before updating.
        if self.is_leader() {
            rec.acquire_time = old.acquire_time;
            rec.leader_transitions = old.leader_transitions;
            metrics::LEADER_SLOWPATH
                .with_label_values(&[&self.cfg.name])
                .inc();
        } else {
            rec.leader_transitions = old.leader_transitions + 1;
        }

        // update the lock itself
        if let Err(e) = self.lock.update(&rec).await {
            klog::error(
                "leaderelection.go:488",
                &e,
                "Failed to update lease",
                &[("lock", Value::String(desc))],
            );
            return false;
        }
        self.set_observed_record(&rec);
        true
    }
}

/// controller-runtime `getInClusterNamespace`.
pub fn get_in_cluster_namespace() -> Result<String> {
    match std::fs::metadata(IN_CLUSTER_NAMESPACE_PATH) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(anyhow::anyhow!(
            "not running in-cluster, please specify LeaderElectionNamespace"
        )),
        Err(e) => Err(anyhow::anyhow!("error checking namespace file: {e}")),
        Ok(_) => std::fs::read_to_string(IN_CLUSTER_NAMESPACE_PATH)
            .map_err(|e| anyhow::anyhow!("error reading namespace file: {e}")),
    }
}

/// Options for [`new_resource_lock`] (`leaderelection.Options`).
#[derive(Clone, Debug, Default)]
pub struct ResourceLockOptions {
    pub leader_election: bool,
    pub leader_election_resource_lock: String,
    pub leader_election_namespace: String,
    pub leader_election_id: String,
    pub renew_deadline: Duration,
}

/// controller-runtime `leaderelection.NewResourceLock`: `None` when leader election is off.
pub fn new_resource_lock(
    config: &RestConfig,
    recorder_for: &dyn Fn(&str) -> ctrlrt::Recorder,
    mut options: ResourceLockOptions,
) -> Result<Option<Arc<LeaseLock>>> {
    if !options.leader_election {
        return Ok(None);
    }
    if options.leader_election_resource_lock.is_empty() {
        options.leader_election_resource_lock = "leases".into();
    }
    match options.leader_election_resource_lock.as_str() {
        "leases" => {}
        "endpoints" => {
            return Err(anyhow::anyhow!(
                "endpoints lock is removed, migrate to leases"
            ));
        }
        "configmaps" => {
            return Err(anyhow::anyhow!(
                "configmaps lock is removed, migrate to leases"
            ));
        }
        "endpointsleases" => {
            return Err(anyhow::anyhow!(
                "endpointsleases lock is removed, migrate to leases"
            ));
        }
        "configmapsleases" => {
            return Err(anyhow::anyhow!(
                "configmapsleases lock is removed, migrated to leases"
            ));
        }
        other => return Err(anyhow::anyhow!("Invalid lock-type {other}")),
    }
    if options.leader_election_id.is_empty() {
        return Err(anyhow::anyhow!("LeaderElectionID must be configured"));
    }
    if options.leader_election_namespace.is_empty() {
        options.leader_election_namespace = match get_in_cluster_namespace() {
            Ok(ns) => ns,
            Err(e) => {
                let msg = format!("unable to find leader election namespace: {e}");
                return Err(ctrlrt::error::wrap(e, msg));
            }
        };
    }
    let id = format!(
        "{}_{}",
        crate::util::os_hostname().map_err(|e| anyhow::anyhow!("{e}"))?,
        crate::util::new_uuid()
    );
    let mut cfg = config.clone().add_user_agent("leader-election");
    if !options.renew_deadline.is_zero() {
        let timeout = (options.renew_deadline / 2).max(Duration::from_secs(1));
        cfg.timeout = Some(timeout);
    }
    let client = cfg.build_client()?;
    let rest = RestClient::new(client).with_base(cfg.host());
    Ok(Some(Arc::new(LeaseLock::new(
        rest,
        &options.leader_election_namespace,
        &options.leader_election_id,
        &id,
        Some(recorder_for(&id)),
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trip() {
        let t = DateTime::parse_from_rfc3339("2024-05-06T07:08:09.123456Z")
            .unwrap()
            .with_timezone(&Utc);
        let r = LeaderElectionRecord {
            holder_identity: "h_1".into(),
            lease_duration_seconds: 15,
            acquire_time: Some(t),
            renew_time: Some(t),
            leader_transitions: 2,
            ..Default::default()
        };
        let spec = record_to_spec(&r);
        assert_eq!(spec["acquireTime"], "2024-05-06T07:08:09.123456Z");
        assert_eq!(spec["leaseDurationSeconds"], 15);
        assert!(spec.get("preferredHolder").is_none());
        assert_eq!(spec_to_record(&spec), r);
    }

    #[tokio::test]
    async fn elector_validation() {
        let client = kube::Client::new(
            tower::service_fn(|_req: http::Request<kube::client::Body>| async {
                Err::<http::Response<kube::client::Body>, std::io::Error>(std::io::Error::other(
                    "x",
                ))
            }),
            "default",
        );
        let lock = Arc::new(LeaseLock::new(
            RestClient::new(client),
            "ns",
            "id",
            "me",
            None,
        ));
        let mk = |l, r, p| LeaderElectionConfig {
            lease_duration: Duration::from_secs(l),
            renew_deadline: Duration::from_secs(r),
            retry_period: Duration::from_secs(p),
            release_on_cancel: false,
            name: "id".into(),
        };
        assert_eq!(
            LeaderElector::new(mk(10, 10, 2), lock.clone())
                .err()
                .unwrap()
                .to_string(),
            "leaseDuration must be greater than renewDeadline"
        );
        assert_eq!(
            LeaderElector::new(mk(15, 2, 2), lock.clone())
                .err()
                .unwrap()
                .to_string(),
            "renewDeadline must be greater than retryPeriod*JitterFactor"
        );
        assert!(LeaderElector::new(mk(15, 10, 2), lock).is_ok());
    }
}
