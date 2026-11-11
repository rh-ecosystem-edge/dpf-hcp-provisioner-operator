//! `golang.org/x/time/rate.Limiter` (token bucket) and client-go's work queue rate limiters
//! (`k8s.io/client-go/util/workqueue/default_rate_limiters.go`).

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// `rate.Limiter`: a token bucket refilled at `limit` tokens per second up to `burst`.
#[derive(Debug)]
pub struct TokenBucket {
    limit: f64,
    burst: i64,
    state: Mutex<BucketState>,
}

#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last: Option<Instant>,
    last_event: Option<Instant>,
}

impl TokenBucket {
    /// `rate.NewLimiter(r, b)` (the bucket starts full).
    pub fn new(limit: f64, burst: i64) -> Self {
        Self {
            limit,
            burst,
            state: Mutex::new(BucketState {
                tokens: burst as f64,
                last: None,
                last_event: None,
            }),
        }
    }

    fn tokens_from_duration(&self, d: Duration) -> f64 {
        if self.limit <= 0.0 {
            return 0.0;
        }
        d.as_secs_f64() * self.limit
    }

    fn duration_from_tokens(&self, tokens: f64) -> Duration {
        if self.limit <= 0.0 {
            return Duration::MAX;
        }
        let seconds = tokens / self.limit;
        Duration::from_nanos((1e9 * seconds) as u64)
    }

    /// `lim.Reserve().Delay()` evaluated at `now`: always reserves one token and returns how
    /// long the caller must wait before acting.
    pub fn reserve_delay_at(&self, now: Instant) -> Duration {
        if self.limit.is_infinite() {
            return Duration::ZERO;
        }
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // advance
        let last = match st.last {
            Some(l) if now >= l => l,
            _ => now,
        };
        let elapsed = if st.last.is_none() {
            // Zero time in Go: elapsed is huge, bucket is refilled to burst.
            Duration::from_secs(u32::MAX as u64)
        } else {
            now.saturating_duration_since(last)
        };
        let mut tokens = st.tokens + self.tokens_from_duration(elapsed);
        if tokens > self.burst as f64 {
            tokens = self.burst as f64;
        }
        tokens -= 1.0;
        let wait = if tokens < 0.0 {
            self.duration_from_tokens(-tokens)
        } else {
            Duration::ZERO
        };
        let ok = 1 <= self.burst;
        if !ok {
            // Reservation not OK: Delay() returns InfDuration.
            return Duration::MAX;
        }
        st.last = Some(now);
        st.tokens = tokens;
        st.last_event = Some(now + wait);
        wait
    }

    /// `lim.Reserve().Delay()`.
    pub fn reserve_delay(&self) -> Duration {
        self.reserve_delay_at(Instant::now())
    }

    /// `lim.Wait(ctx)` (used by the client-side REST rate limiter).
    pub async fn wait(&self) {
        let d = self.reserve_delay();
        if !d.is_zero() && d != Duration::MAX {
            tokio::time::sleep(d).await;
        }
    }
}

/// `workqueue.TypedRateLimiter[T]`.
pub trait RateLimiter<T>: Send + Sync {
    /// `When(item)`: how long to wait before re-adding the item.
    fn when(&self, item: &T) -> Duration;
    /// `Forget(item)`: stop tracking the item.
    fn forget(&self, item: &T);
    /// `NumRequeues(item)`.
    fn num_requeues(&self, item: &T) -> i32;
}

/// `workqueue.TypedItemExponentialFailureRateLimiter`: `baseDelay * 2^failures`, capped.
pub struct ItemExponentialFailureRateLimiter<T> {
    failures: Mutex<HashMap<T, i32>>,
    base_delay: Duration,
    max_delay: Duration,
}

impl<T: Eq + Hash + Clone> ItemExponentialFailureRateLimiter<T> {
    pub fn new(base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            failures: Mutex::new(HashMap::new()),
            base_delay,
            max_delay,
        }
    }
}

impl<T: Eq + Hash + Clone + Send> RateLimiter<T> for ItemExponentialFailureRateLimiter<T> {
    fn when(&self, item: &T) -> Duration {
        let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let exp = *f.get(item).unwrap_or(&0);
        f.insert(item.clone(), exp + 1);
        // The backoff is capped such that 'calculated' value never overflows.
        let backoff = self.base_delay.as_nanos() as f64 * 2f64.powi(exp);
        if backoff > i64::MAX as f64 {
            return self.max_delay;
        }
        let calculated = Duration::from_nanos(backoff as u64);
        if calculated > self.max_delay {
            return self.max_delay;
        }
        calculated
    }
    fn forget(&self, item: &T) {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(item);
    }
    fn num_requeues(&self, item: &T) -> i32 {
        *self
            .failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(item)
            .unwrap_or(&0)
    }
}

/// `workqueue.TypedBucketRateLimiter`: overall rate limiting (not per item).
pub struct BucketRateLimiter {
    pub limiter: TokenBucket,
}

impl BucketRateLimiter {
    pub fn new(qps: f64, burst: i64) -> Self {
        Self {
            limiter: TokenBucket::new(qps, burst),
        }
    }
}

impl<T> RateLimiter<T> for BucketRateLimiter {
    fn when(&self, _item: &T) -> Duration {
        self.limiter.reserve_delay()
    }
    fn forget(&self, _item: &T) {}
    fn num_requeues(&self, _item: &T) -> i32 {
        0
    }
}

/// `workqueue.TypedItemFastSlowRateLimiter`.
pub struct ItemFastSlowRateLimiter<T> {
    failures: Mutex<HashMap<T, i32>>,
    max_fast_attempts: i32,
    fast_delay: Duration,
    slow_delay: Duration,
}

impl<T: Eq + Hash + Clone> ItemFastSlowRateLimiter<T> {
    pub fn new(fast_delay: Duration, slow_delay: Duration, max_fast_attempts: i32) -> Self {
        Self {
            failures: Mutex::new(HashMap::new()),
            max_fast_attempts,
            fast_delay,
            slow_delay,
        }
    }
}

impl<T: Eq + Hash + Clone + Send> RateLimiter<T> for ItemFastSlowRateLimiter<T> {
    fn when(&self, item: &T) -> Duration {
        let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let n = f.get(item).copied().unwrap_or(0) + 1;
        f.insert(item.clone(), n);
        if n <= self.max_fast_attempts {
            self.fast_delay
        } else {
            self.slow_delay
        }
    }
    fn forget(&self, item: &T) {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(item);
    }
    fn num_requeues(&self, item: &T) -> i32 {
        *self
            .failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(item)
            .unwrap_or(&0)
    }
}

/// `workqueue.TypedMaxOfRateLimiter`: the worst case of all limiters.
pub struct MaxOfRateLimiter<T> {
    limiters: Vec<Box<dyn RateLimiter<T>>>,
}

impl<T> MaxOfRateLimiter<T> {
    pub fn new(limiters: Vec<Box<dyn RateLimiter<T>>>) -> Self {
        Self { limiters }
    }
}

impl<T: Send + Sync> RateLimiter<T> for MaxOfRateLimiter<T> {
    fn when(&self, item: &T) -> Duration {
        let mut ret = Duration::ZERO;
        for l in &self.limiters {
            let curr = l.when(item);
            if curr > ret {
                ret = curr;
            }
        }
        ret
    }
    fn forget(&self, item: &T) {
        for l in &self.limiters {
            l.forget(item);
        }
    }
    fn num_requeues(&self, item: &T) -> i32 {
        let mut ret = 0;
        for l in &self.limiters {
            let curr = l.num_requeues(item);
            if curr > ret {
                ret = curr;
            }
        }
        ret
    }
}

/// `workqueue.DefaultTypedControllerRateLimiter()`: max of per-item exponential backoff
/// (5ms .. 1000s) and an overall 10 qps / 100 burst token bucket.
pub fn default_controller_rate_limiter<T>() -> MaxOfRateLimiter<T>
where
    T: Eq + Hash + Clone + Send + Sync + 'static,
{
    MaxOfRateLimiter::new(vec![
        Box::new(ItemExponentialFailureRateLimiter::<T>::new(
            Duration::from_millis(5),
            Duration::from_secs(1000),
        )),
        Box::new(BucketRateLimiter::new(10.0, 100)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    // client-go: TestItemExponentialFailureRateLimiter
    #[test]
    fn test_item_exponential_failure_rate_limiter() {
        let limiter = ItemExponentialFailureRateLimiter::new(
            Duration::from_millis(1),
            Duration::from_secs(1),
        );
        let one = "one".to_string();
        let two = "two".to_string();
        assert_eq!(limiter.when(&one), Duration::from_millis(1));
        assert_eq!(limiter.when(&one), Duration::from_millis(2));
        assert_eq!(limiter.when(&one), Duration::from_millis(4));
        assert_eq!(limiter.when(&one), Duration::from_millis(8));
        assert_eq!(limiter.when(&one), Duration::from_millis(16));
        assert_eq!(limiter.num_requeues(&one), 5);

        assert_eq!(limiter.when(&two), Duration::from_millis(1));
        assert_eq!(limiter.when(&two), Duration::from_millis(2));
        assert_eq!(limiter.num_requeues(&two), 2);

        limiter.forget(&one);
        assert_eq!(limiter.num_requeues(&one), 0);
        assert_eq!(limiter.when(&one), Duration::from_millis(1));
    }

    // client-go: TestItemExponentialFailureRateLimiterOverFlow
    #[test]
    fn test_item_exponential_failure_rate_limiter_overflow() {
        let limiter = ItemExponentialFailureRateLimiter::new(
            Duration::from_millis(1),
            Duration::from_secs(1000),
        );
        let one = "one".to_string();
        for _ in 0..5 {
            limiter.when(&one);
        }
        assert_eq!(limiter.when(&one), Duration::from_millis(32));
        for _ in 0..1000 {
            limiter.when(&one);
        }
        assert_eq!(limiter.when(&one), Duration::from_secs(1000));

        let limiter = ItemExponentialFailureRateLimiter::new(
            Duration::from_secs(60),
            Duration::from_secs(1000 * 3600),
        );
        let two = "two".to_string();
        for _ in 0..2 {
            limiter.when(&two);
        }
        assert_eq!(limiter.when(&two), Duration::from_secs(4 * 60));
        for _ in 0..1000 {
            limiter.when(&two);
        }
        assert_eq!(limiter.when(&two), Duration::from_secs(1000 * 3600));
    }

    // client-go: TestItemFastSlowRateLimiter
    #[test]
    fn test_item_fast_slow_rate_limiter() {
        let limiter =
            ItemFastSlowRateLimiter::new(Duration::from_millis(5), Duration::from_secs(10), 3);
        let one = "one".to_string();
        let two = "two".to_string();
        assert_eq!(limiter.when(&one), Duration::from_millis(5));
        assert_eq!(limiter.when(&one), Duration::from_millis(5));
        assert_eq!(limiter.when(&one), Duration::from_millis(5));
        assert_eq!(limiter.when(&one), Duration::from_secs(10));
        assert_eq!(limiter.when(&one), Duration::from_secs(10));
        assert_eq!(limiter.num_requeues(&one), 5);
        assert_eq!(limiter.when(&two), Duration::from_millis(5));
        assert_eq!(limiter.when(&two), Duration::from_millis(5));
        assert_eq!(limiter.num_requeues(&two), 2);
        limiter.forget(&one);
        assert_eq!(limiter.num_requeues(&one), 0);
        assert_eq!(limiter.when(&one), Duration::from_millis(5));
    }

    // client-go: TestMaxOfRateLimiter
    #[test]
    fn test_max_of_rate_limiter() {
        let limiter: MaxOfRateLimiter<String> = MaxOfRateLimiter::new(vec![
            Box::new(ItemFastSlowRateLimiter::new(
                Duration::from_millis(5),
                Duration::from_secs(3),
                3,
            )),
            Box::new(ItemExponentialFailureRateLimiter::new(
                Duration::from_millis(1),
                Duration::from_secs(1),
            )),
        ]);
        let one = "one".to_string();
        let two = "two".to_string();
        assert_eq!(limiter.when(&one), Duration::from_millis(5));
        assert_eq!(limiter.when(&one), Duration::from_millis(5));
        assert_eq!(limiter.when(&one), Duration::from_millis(5));
        assert_eq!(limiter.when(&one), Duration::from_secs(3));
        assert_eq!(limiter.when(&one), Duration::from_secs(3));
        assert_eq!(limiter.num_requeues(&one), 5);
        assert_eq!(limiter.when(&two), Duration::from_millis(5));
        assert_eq!(limiter.when(&two), Duration::from_millis(5));
        assert_eq!(limiter.num_requeues(&two), 2);
        limiter.forget(&one);
        assert_eq!(limiter.num_requeues(&one), 0);
        assert_eq!(limiter.when(&one), Duration::from_millis(5));
    }

    #[test]
    fn token_bucket_burst_then_rate() {
        let tb = TokenBucket::new(10.0, 100);
        let now = Instant::now();
        for _ in 0..100 {
            assert_eq!(tb.reserve_delay_at(now), Duration::ZERO);
        }
        // the 101st token is 100ms away, the 102nd 200ms away (reservations accumulate)
        assert_eq!(tb.reserve_delay_at(now), Duration::from_millis(100));
        assert_eq!(tb.reserve_delay_at(now), Duration::from_millis(200));
        // after one second, 10 tokens were refilled: the debt of 2 is paid, 8 left
        let later = now + Duration::from_secs(1);
        for _ in 0..8 {
            assert_eq!(tb.reserve_delay_at(later), Duration::ZERO);
        }
        assert_eq!(tb.reserve_delay_at(later), Duration::from_millis(100));
    }

    #[test]
    fn default_controller_rate_limiter_is_max_of_both() {
        let rl = default_controller_rate_limiter::<String>();
        let a = "a".to_string();
        assert_eq!(rl.when(&a), Duration::from_millis(5));
        assert_eq!(rl.when(&a), Duration::from_millis(10));
        assert_eq!(rl.num_requeues(&a), 2);
        rl.forget(&a);
        assert_eq!(rl.num_requeues(&a), 0);
        // exponential: 5ms * 2^n caps at 1000s
        for _ in 0..40 {
            rl.when(&a);
        }
        assert_eq!(rl.when(&a), Duration::from_secs(1000));
    }
}
