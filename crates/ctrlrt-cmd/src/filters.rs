//! controller-runtime `pkg/metrics/filters.WithAuthenticationAndAuthorization`: protects the
//! metrics endpoint with delegated authentication (TokenReview) and authorization
//! (SubjectAccessReview, non-resource URL + lower-cased HTTP verb) against the API server.
//!
//! Behaviour mirrors k8s.io/apiserver's `DelegatingAuthenticatorConfig` (bearer token +
//! websocket protocol authenticators, token cache with 1m TTL for successes and failures,
//! `system:authenticated` group adder, 10s TokenReview timeout) and
//! `DelegatingAuthorizerConfig` (SubjectAccessReview webhook, 5m allow / 30s deny cache), both
//! with the same retry backoff (500ms, factor 1.5, jitter 0.2, 5 steps).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use ctrlrt::kv;
use ctrlrt::log::Logger;
use hyper::http::{HeaderMap, StatusCode, request::Parts};
use k8s_openapi::api::authentication::v1::{TokenReview, TokenReviewSpec};
use k8s_openapi::api::authorization::v1::{
    NonResourceAttributes, SubjectAccessReview, SubjectAccessReviewSpec, SubjectAccessReviewStatus,
};
use tokio::time::Instant;

use crate::klog;
use crate::metrics::{Filter, Handler, HttpResponse, http_error};

/// `user.AllAuthenticated`
pub const ALL_AUTHENTICATED: &str = "system:authenticated";
/// `user.AllUnauthenticated`
pub const ALL_UNAUTHENTICATED: &str = "system:unauthenticated";
/// `user.Anonymous`
pub const ANONYMOUS: &str = "system:anonymous";

const BEARER_PROTOCOL_PREFIX: &str = "base64url.bearer.authorization.k8s.io.";
const MAX_CONTROLLED_ATTR_CACHE_SIZE: usize = 10000;
const AUTHZ_CACHE_SIZE: usize = 8192;
const TOKEN_CACHE_SIZE: usize = 4096;

/// `user.DefaultInfo`
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserInfo {
    pub name: String,
    pub uid: String,
    pub groups: Vec<String>,
    pub extra: Option<BTreeMap<String, Vec<String>>>,
}

/// `wait.Backoff`
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    pub duration: Duration,
    pub factor: f64,
    pub jitter: f64,
    pub steps: u32,
}

/// The backoff used by `WithAuthenticationAndAuthorization`.
pub const WEBHOOK_RETRY_BACKOFF: Backoff = Backoff {
    duration: Duration::from_millis(500),
    factor: 1.5,
    jitter: 0.2,
    steps: 5,
};

fn rand_f64() -> f64 {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 8];
    if ring::rand::SystemRandom::new().fill(&mut b).is_err() {
        return 0.5;
    }
    (u64::from_le_bytes(b) >> 11) as f64 / (1u64 << 53) as f64
}

/// `webhook.DefaultShouldRetry`
pub fn default_should_retry(err: &ctrlrt::Error) -> bool {
    if let Some(api) = ctrlrt::error::as_api_error(err) {
        let retry_after = api
            .details
            .as_ref()
            .map(|d| d.retry_after_seconds)
            .unwrap_or(0);
        return api.reason == ctrlrt::error::REASON_INTERNAL_ERROR
            || api.reason == ctrlrt::error::REASON_TIMEOUT
            || api.reason == ctrlrt::error::REASON_TOO_MANY_REQUESTS
            || api.code == 429
            || (api.details.is_some() && api.reason == ctrlrt::error::REASON_SERVER_TIMEOUT)
            || retry_after > 0;
    }
    let s = err.to_string();
    s.contains("connection reset by peer") || s.contains("http2: client connection lost")
}

/// `webhook.WithExponentialBackoff`: run `f` until it succeeds, fails with a non-retriable
/// error or the steps are exhausted (the last error is returned). `deadline` emulates the
/// request context timeout.
pub async fn with_exponential_backoff<T, F, Fut>(
    backoff: Backoff,
    deadline: Option<Instant>,
    mut f: F,
) -> ctrlrt::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ctrlrt::Result<T>>,
{
    let mut b = backoff;
    let mut last_err: Option<ctrlrt::Error> = None;
    while b.steps > 0 {
        let res = match deadline {
            Some(d) => match tokio::time::timeout_at(d, f()).await {
                Ok(r) => r,
                Err(_) => Err(anyhow::anyhow!("context deadline exceeded")),
            },
            None => f().await,
        };
        match res {
            Ok(v) => return Ok(v),
            Err(e) => {
                if !default_should_retry(&e) {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
        if b.steps == 1 {
            break;
        }
        // Backoff.Step()
        let mut wait = b.duration;
        b.steps -= 1;
        b.duration = b.duration.mul_f64(b.factor);
        if b.jitter > 0.0 {
            wait += wait.mul_f64(rand_f64() * b.jitter);
        }
        let sleep = tokio::time::sleep(wait);
        match deadline {
            Some(d) => {
                tokio::select! {
                    _ = sleep => {}
                    _ = tokio::time::sleep_until(d) => break,
                }
            }
            None => sleep.await,
        }
    }
    Err(last_err.unwrap_or_else(|| {
        anyhow::anyhow!("webhook call failed: timed out waiting for the condition")
    }))
}

/// Result of an authenticator: Ok(Some(user)) authenticated, Ok(None) not authenticated
/// (no credentials), Err(msg) error.
pub type AuthResult = Result<Option<UserInfo>, String>;

/// sha256(token) -> (expiry, authenticated user or None).
type TokenCache = HashMap<Vec<u8>, (Instant, Option<UserInfo>)>;

/// `WebhookTokenAuthenticator` wrapped in the token cache.
pub struct TokenAuthenticator {
    client: ctrlrt::Client,
    backoff: Backoff,
    timeout: Duration,
    ttl: Duration,
    cache: Mutex<TokenCache>,
}

impl TokenAuthenticator {
    pub fn new(client: ctrlrt::Client) -> Self {
        Self {
            client,
            backoff: WEBHOOK_RETRY_BACKOFF,
            timeout: Duration::from_secs(10),
            ttl: Duration::from_secs(60),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// `AuthenticateToken` (cached).
    pub async fn authenticate_token(&self, token: &str) -> AuthResult {
        let key = ring::digest::digest(&ring::digest::SHA256, token.as_bytes())
            .as_ref()
            .to_vec();
        let now = Instant::now();
        if let Ok(c) = self.cache.lock()
            && let Some((exp, user)) = c.get(&key)
            && *exp > now
        {
            return Ok(user.clone());
        }
        let res = self.authenticate_token_uncached(token).await;
        if let Ok(user) = &res
            && let Ok(mut c) = self.cache.lock()
        {
            if c.len() >= TOKEN_CACHE_SIZE {
                c.retain(|_, (exp, _)| *exp > now);
                if c.len() >= TOKEN_CACHE_SIZE {
                    c.clear();
                }
            }
            c.insert(key, (now + self.ttl, user.clone()));
        }
        res
    }

    async fn authenticate_token_uncached(&self, token: &str) -> AuthResult {
        let deadline = Instant::now() + self.timeout;
        let client = self.client.clone();
        let result = with_exponential_backoff(self.backoff, Some(deadline), || {
            let client = client.clone();
            let mut tr = TokenReview {
                spec: TokenReviewSpec {
                    token: Some(token.to_string()),
                    audiences: None,
                },
                ..Default::default()
            };
            async move {
                client.create(&mut tr).await?;
                Ok(tr)
            }
        })
        .await;
        let tr = match result {
            Ok(tr) => tr,
            Err(err) => {
                klog::errorf(
                    "webhook.go:154",
                    &format!("Failed to make webhook authenticator request: {err}"),
                );
                return Err(err.to_string());
            }
        };
        let status = tr.status.unwrap_or_default();
        if !status.authenticated.unwrap_or(false) {
            return match status.error.filter(|e| !e.is_empty()) {
                Some(e) => Err(e),
                None => Ok(None),
            };
        }
        let u = status.user.unwrap_or_default();
        Ok(Some(UserInfo {
            name: u.username.unwrap_or_default(),
            uid: u.uid.unwrap_or_default(),
            groups: u.groups.unwrap_or_default(),
            extra: u.extra,
        }))
    }
}

fn header_values<'a>(h: &'a HeaderMap, name: &str) -> Vec<&'a str> {
    h.get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect()
}

/// `bearertoken.Authenticator.AuthenticateRequest`
pub async fn authenticate_bearer(tokens: &TokenAuthenticator, headers: &HeaderMap) -> AuthResult {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    if auth.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = auth.splitn(3, ' ').collect();
    if parts.len() < 2 || parts[0].to_lowercase() != "bearer" {
        return Ok(None);
    }
    let token = parts[1];
    if token.is_empty() {
        return Ok(None);
    }
    match tokens.authenticate_token(token).await {
        Ok(Some(u)) => Ok(Some(u)),
        Ok(None) => Err("invalid bearer token".into()),
        Err(e) => Err(e),
    }
}

/// `websocket.ProtocolAuthenticator.AuthenticateRequest`
pub async fn authenticate_websocket(
    tokens: &TokenAuthenticator,
    headers: &HeaderMap,
) -> AuthResult {
    let upgrade = headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let is_upgrade = header_values(headers, "connection")
        .iter()
        .any(|h| h.to_lowercase().contains("upgrade"));
    if !upgrade.eq_ignore_ascii_case("websocket") || !is_upgrade {
        return Ok(None);
    }
    let mut token = String::new();
    let mut saw = false;
    let mut filtered = 0usize;
    for h in header_values(headers, "sec-websocket-protocol") {
        for p in h.split(',') {
            let p = p.trim();
            let Some(enc) = p.strip_prefix(BEARER_PROTOCOL_PREFIX) else {
                filtered += 1;
                continue;
            };
            if saw {
                return Err("multiple base64.bearer.authorization tokens specified".into());
            }
            saw = true;
            let dec = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(enc)
                .map_err(|_| "invalid base64.bearer.authorization token encoding".to_string())?;
            token = String::from_utf8(dec)
                .map_err(|_| "invalid base64.bearer.authorization token".to_string())?;
        }
    }
    if !token.is_empty() && filtered == 0 {
        return Err("missing additional subprotocol".into());
    }
    if token.is_empty() {
        return Ok(None);
    }
    match tokens.authenticate_token(&token).await {
        Ok(Some(u)) => Ok(Some(u)),
        Ok(None) => Err("invalid bearer token".into()),
        Err(e) => Err(e),
    }
}

/// The delegating request authenticator: union(bearer, websocket) + authenticated group
/// adder.
pub async fn authenticate_request(tokens: &TokenAuthenticator, headers: &HeaderMap) -> AuthResult {
    // union.New(bearer, websocket): first success wins, errors are aggregated.
    let mut errs: Vec<String> = Vec::new();
    let mut user = None;
    match authenticate_bearer(tokens, headers).await {
        Ok(Some(u)) => user = Some(u),
        Ok(None) => {}
        Err(e) => errs.push(e),
    }
    if user.is_none() {
        match authenticate_websocket(tokens, headers).await {
            Ok(Some(u)) => user = Some(u),
            Ok(None) => {}
            Err(e) => errs.push(e),
        }
    }
    let Some(mut u) = user else {
        if errs.is_empty() {
            return Ok(None);
        }
        return Err(crate::certwatcher::aggregate(&errs));
    };
    // group.AuthenticatedGroupAdder
    if u.name != ANONYMOUS
        && !u
            .groups
            .iter()
            .any(|g| g == ALL_AUTHENTICATED || g == ALL_UNAUTHENTICATED)
    {
        u.groups.push(ALL_AUTHENTICATED.to_string());
    }
    Ok(Some(u))
}

/// `authorizer.Decision`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Deny,
    Allow,
    NoOpinion,
}

/// `WebhookAuthorizer` for non-resource requests.
pub struct SarAuthorizer {
    client: ctrlrt::Client,
    backoff: Backoff,
    allow_ttl: Duration,
    deny_ttl: Duration,
    cache: Mutex<HashMap<String, (Instant, SubjectAccessReviewStatus)>>,
}

impl SarAuthorizer {
    pub fn new(client: ctrlrt::Client) -> Self {
        Self {
            client,
            backoff: WEBHOOK_RETRY_BACKOFF,
            allow_ttl: Duration::from_secs(5 * 60),
            deny_ttl: Duration::from_secs(30),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// `Authorize(ctx, attributes)` for a non-resource request: (decision, reason, error).
    pub async fn authorize(
        &self,
        user: &UserInfo,
        verb: &str,
        path: &str,
    ) -> (Decision, String, Option<String>) {
        let spec = SubjectAccessReviewSpec {
            user: Some(user.name.clone()),
            uid: Some(user.uid.clone()).filter(|s| !s.is_empty()),
            groups: Some(user.groups.clone()).filter(|g| !g.is_empty()),
            extra: user.extra.clone(),
            non_resource_attributes: Some(NonResourceAttributes {
                path: Some(path.to_string()),
                verb: Some(verb.to_string()),
            }),
            resource_attributes: None,
        };
        let key = serde_json::to_string(&spec).unwrap_or_default();
        let now = Instant::now();
        let cached = self.cache.lock().ok().and_then(|c| {
            c.get(&key)
                .filter(|(exp, _)| *exp > now)
                .map(|(_, s)| s.clone())
        });
        let status = match cached {
            Some(s) => s,
            None => {
                let client = self.client.clone();
                let res = with_exponential_backoff(self.backoff, None, || {
                    let client = client.clone();
                    let mut sar = SubjectAccessReview {
                        spec: spec.clone(),
                        ..Default::default()
                    };
                    async move {
                        client.create(&mut sar).await?;
                        Ok(sar)
                    }
                })
                .await;
                let sar = match res {
                    Ok(s) => s,
                    Err(err) => {
                        klog::errorf(
                            "webhook.go:269",
                            &format!("Failed to make webhook authorizer request: {err}"),
                        );
                        return (Decision::NoOpinion, String::new(), Some(err.to_string()));
                    }
                };
                let status = sar.status.unwrap_or_default();
                let size = verb.len() + path.len();
                if size < MAX_CONTROLLED_ATTR_CACHE_SIZE
                    && let Ok(mut c) = self.cache.lock()
                {
                    if c.len() >= AUTHZ_CACHE_SIZE {
                        c.retain(|_, (exp, _)| *exp > now);
                        if c.len() >= AUTHZ_CACHE_SIZE {
                            c.clear();
                        }
                    }
                    let ttl = if status.allowed {
                        self.allow_ttl
                    } else {
                        self.deny_ttl
                    };
                    c.insert(key, (now + ttl, status.clone()));
                }
                status
            }
        };
        let denied = status.denied.unwrap_or(false);
        let reason = status.reason.clone().unwrap_or_default();
        if denied && status.allowed {
            return (
                Decision::Deny,
                reason,
                Some("webhook subject access review returned both allow and deny response".into()),
            );
        }
        if denied {
            return (Decision::Deny, reason, None);
        }
        if status.allowed {
            return (Decision::Allow, reason, None);
        }
        (Decision::NoOpinion, reason, None)
    }
}

/// `filters.WithAuthenticationAndAuthorization(config, httpClient)`: a filter provider.
pub fn with_authentication_and_authorization(client: &ctrlrt::Client) -> ctrlrt::Result<Filter> {
    let tokens = Arc::new(TokenAuthenticator::new(client.clone()));
    let authz = Arc::new(SarAuthorizer::new(client.clone()));
    Ok(Arc::new(move |log: Logger, handler: Handler| {
        let tokens = tokens.clone();
        let authz = authz.clone();
        let h: Handler = Arc::new(move |parts: Parts| {
            let tokens = tokens.clone();
            let authz = authz.clone();
            let log = log.clone();
            let handler = handler.clone();
            Box::pin(async move { filter_request(&tokens, &authz, &log, &handler, parts).await })
        });
        Ok(h)
    }))
}

async fn filter_request(
    tokens: &TokenAuthenticator,
    authz: &SarAuthorizer,
    log: &Logger,
    handler: &Handler,
    parts: Parts,
) -> HttpResponse {
    let user = match authenticate_request(tokens, &parts.headers).await {
        Err(e) => {
            log.error(&e, "Authentication failed", kv![]);
            return http_error("Authentication failed", StatusCode::INTERNAL_SERVER_ERROR);
        }
        Ok(None) => {
            log.v(4).info("Authentication failed", kv![]);
            return http_error("Unauthorized", StatusCode::UNAUTHORIZED);
        }
        Ok(Some(u)) => u,
    };
    let verb = parts.method.as_str().to_lowercase();
    let path = parts.uri.path().to_string();
    let (decision, reason, err) = authz.authorize(&user, &verb, &path).await;
    if let Some(err) = err {
        let msg = format!("Authorization for user {} failed", user.name);
        log.error(&err, &msg, kv![]);
        return http_error(&msg, StatusCode::INTERNAL_SERVER_ERROR);
    }
    if decision != Decision::Allow {
        let msg = format!("Authorization denied for user {}", user.name);
        log.v(4).info(&format!("{msg}: {reason}"), kv![]);
        return http_error(&msg, StatusCode::FORBIDDEN);
    }
    handler(parts).await
}
