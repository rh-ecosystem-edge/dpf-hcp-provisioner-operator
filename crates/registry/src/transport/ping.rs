//! Port of `remote/transport/ping.go`.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use ::http::Method;

use super::challenge::response_challenges;
use super::{Client, Request, RoundTripper, check_error};
use crate::name::Registry;

/// 300ms is the default fallback period for go's DNS dialer.
const FALLBACK_DELAY: Duration = Duration::from_millis(300);

/// `transport.Challenge`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Challenge {
    pub scheme: String,
    /// Following the challenge there are often key/value pairs
    /// e.g. `Bearer service="gcr.io",realm="https://auth.gcr.io/v36/tokenz"`
    pub parameters: BTreeMap<String, String>,
    /// Whether we had to use http to complete the Ping.
    pub insecure: bool,
}

/// `multierrs`
#[derive(Debug)]
pub struct MultiError(pub Vec<anyhow::Error>);

impl fmt::Display for MultiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, e) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for MultiError {}

/// `transport.Ping`: GET /v2/ against the registry and return the auth challenge.
pub async fn ping(reg: &Registry, t: Arc<dyn RoundTripper>) -> anyhow::Result<Challenge> {
    if reg.scheme() == "http" {
        return ping_parallel(reg, t).await;
    }
    ping_single(reg, t, "https").await
}

async fn ping_single(
    reg: &Registry,
    t: Arc<dyn RoundTripper>,
    scheme: &str,
) -> anyhow::Result<Challenge> {
    let client = Client::new(t);
    let url = format!("{scheme}://{}/v2/", reg.registry_str());
    let req = Request::new(Method::GET, &url)?;
    let mut resp = client.do_request(req).await?;
    let insecure = resp.request.url.scheme == "http";
    match resp.status {
        200 => {
            let _ = resp.body.read_all().await;
            Ok(Challenge {
                insecure,
                ..Default::default()
            })
        }
        401 => {
            let challenges = response_challenges(&resp);
            let _ = resp.body.read_all().await;
            if !challenges.is_empty() {
                let wac = pick_from_multiple_challenges(challenges);
                return Ok(Challenge {
                    scheme: wac.scheme,
                    parameters: wac.parameters,
                    insecure,
                });
            }
            Ok(Challenge {
                scheme: resp.header("WWW-Authenticate"),
                insecure,
                ..Default::default()
            })
        }
        status => match check_error(resp, &[200, 401]).await {
            Ok(_) => Err(anyhow::anyhow!("unexpected status code {status}")),
            Err(e) => Err(e),
        },
    }
}

fn pick_from_multiple_challenges(
    challenges: Vec<super::challenge::Challenge>,
) -> super::challenge::Challenge {
    for wac in &challenges {
        let s = wac.scheme.to_ascii_lowercase();
        if s == "basic" || s == "bearer" {
            return wac.clone();
        }
    }
    challenges.into_iter().next().unwrap_or_default()
}

/// Happy-eyeballs style race between https and http (Go `pingParallel`).
async fn ping_parallel(reg: &Registry, t: Arc<dyn RoundTripper>) -> anyhow::Result<Challenge> {
    type PingFut<'a> = Pin<Box<dyn Future<Output = anyhow::Result<Challenge>> + Send + 'a>>;
    let mut primary: PingFut<'_> = Box::pin(ping_single(reg, t.clone(), "https"));
    let mut fallback: PingFut<'_> = Box::pin(std::future::pending());
    let mut fallback_started = false;
    let timer = tokio::time::sleep(FALLBACK_DELAY);
    tokio::pin!(timer);
    let mut primary_err: Option<anyhow::Error> = None;
    let mut fallback_err: Option<anyhow::Error> = None;

    loop {
        tokio::select! {
            _ = &mut timer, if !fallback_started => {
                fallback_started = true;
                fallback = Box::pin(ping_single(reg, t.clone(), "http"));
            }
            res = &mut primary, if primary_err.is_none() => {
                match res {
                    Ok(c) => return Ok(c),
                    Err(e) => {
                        primary_err = Some(e);
                        if !fallback_started {
                            // Primary failed before the fallback started: start it now.
                            fallback_started = true;
                            fallback = Box::pin(ping_single(reg, t.clone(), "http"));
                        }
                    }
                }
            }
            res = &mut fallback, if fallback_started && fallback_err.is_none() => {
                match res {
                    Ok(c) => return Ok(c),
                    Err(e) => fallback_err = Some(e),
                }
            }
        }
        if let (Some(_), Some(_)) = (&primary_err, &fallback_err) {
            let errs: Vec<anyhow::Error> = primary_err.take().into_iter().chain(fallback_err.take()).collect();
            return Err(MultiError(errs).into());
        }
    }
}
