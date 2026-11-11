//! Port of `remote/transport/retry.go` and `internal/retry` (+ `wait.ExponentialBackoff`).

use std::io;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::error::make_error;
use super::http::NetError;
use super::{Request, Response, RoundTripper};

/// `retry.Backoff` (`wait.Backoff`).
#[derive(Debug, Clone, PartialEq)]
pub struct Backoff {
    pub duration: Duration,
    pub factor: f64,
    pub jitter: f64,
    pub steps: u32,
    pub cap: Duration,
}

impl Backoff {
    /// `remote.defaultRetryBackoff`: 1s, factor 3, jitter 0.1, 3 steps.
    pub fn remote_default() -> Self {
        Backoff {
            duration: Duration::from_secs(1),
            factor: 3.0,
            jitter: 0.1,
            steps: 3,
            cap: Duration::ZERO,
        }
    }

    /// `transport.defaultBackoff`: 100ms, factor 3, jitter 0.1, 3 steps.
    pub fn transport_default() -> Self {
        Backoff {
            duration: Duration::from_millis(100),
            ..Self::remote_default()
        }
    }

    /// `remote.fastBackoff`
    pub fn fast() -> Self {
        Backoff {
            duration: Duration::from_millis(1),
            ..Self::remote_default()
        }
    }

    fn jitter(d: Duration, max_factor: f64) -> Duration {
        let max_factor = if max_factor <= 0.0 { 1.0 } else { max_factor };
        d + Duration::from_secs_f64(rand::random::<f64>() * max_factor * d.as_secs_f64())
    }

    /// `Backoff.Step()`
    fn step(&mut self) -> Duration {
        if self.steps < 1 {
            if self.jitter > 0.0 {
                return Self::jitter(self.duration, self.jitter);
            }
            return self.duration;
        }
        self.steps -= 1;
        let duration = self.duration;
        if self.factor != 0.0 {
            self.duration = Duration::from_secs_f64(self.duration.as_secs_f64() * self.factor);
            if !self.cap.is_zero() && self.duration > self.cap {
                self.duration = self.cap;
                self.steps = 0;
            }
        }
        if self.jitter > 0.0 {
            return Self::jitter(duration, self.jitter);
        }
        duration
    }
}

/// `retry.Predicate`
pub type Predicate = Arc<dyn Fn(&anyhow::Error) -> bool + Send + Sync>;

/// `remote.defaultRetryPredicate`: temporary errors, EOF / unexpected EOF, EPIPE,
/// ECONNRESET, closed connections.
pub fn default_retry_predicate(err: &anyhow::Error) -> bool {
    for e in err.chain() {
        if let Some(te) = e.downcast_ref::<super::Error>()
            && te.temporary() {
                return true;
            }
        if let Some(ne) = e.downcast_ref::<NetError>()
            && ne.retryable {
                return true;
            }
        if let Some(ioe) = e.downcast_ref::<io::Error>()
            && matches!(
                ioe.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::TimedOut
            ) {
                return true;
            }
    }
    false
}

/// `retryTransport`
pub struct RetryTransport {
    inner: Arc<dyn RoundTripper>,
    backoff: Backoff,
    predicate: Predicate,
    codes: Vec<u16>,
}

impl RetryTransport {
    /// `transport.NewRetry(inner, WithRetryBackoff, WithRetryPredicate, WithRetryStatusCodes)`
    pub fn new(
        inner: Arc<dyn RoundTripper>,
        backoff: Backoff,
        predicate: Predicate,
        codes: Vec<u16>,
    ) -> Self {
        RetryTransport {
            inner,
            backoff,
            predicate,
            codes,
        }
    }
}

#[async_trait]
impl RoundTripper for RetryTransport {
    async fn round_trip(&self, req: Request) -> anyhow::Result<Response> {
        let mut backoff = self.backoff.clone();
        // Go keeps the last (out, err) pair produced by the closure.
        let mut last: Option<anyhow::Result<Response>> = None;
        while backoff.steps > 0 {
            let (out, retry) = match self.inner.round_trip(req.clone()).await {
                Ok(mut resp) if self.codes.contains(&resp.status) => {
                    // retryError: drains the body (which stays consumed) into a
                    // temporary transport.Error that the predicate then judges.
                    let rerr = match resp.body.read_all().await {
                        Ok(body) => {
                            let mut e = make_error(&resp, &body);
                            e.set_temporary();
                            anyhow::Error::new(e)
                        }
                        Err(ioe) => NetError::from_io(ioe),
                    };
                    let retry = (self.predicate)(&rerr);
                    (Ok(resp), retry)
                }
                Ok(resp) => (Ok(resp), false),
                Err(e) => {
                    let retry = (self.predicate)(&e);
                    (Err(e), retry)
                }
            };
            last = Some(out);
            if !retry || backoff.steps == 1 {
                break;
            }
            tokio::time::sleep(backoff.step()).await;
        }
        last.unwrap_or_else(|| Err(anyhow::anyhow!("retry: no attempts made")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_steps() {
        let mut b = Backoff {
            jitter: 0.0,
            ..Backoff::remote_default()
        };
        assert_eq!(b.step(), Duration::from_secs(1));
        assert_eq!(b.step(), Duration::from_secs(3));
        assert_eq!(b.steps, 1);
    }
}
