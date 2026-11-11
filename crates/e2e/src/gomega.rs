//! The Gomega matchers and async assertions used by the e2e suite.
//!
//! * Matchers return `Result<(), AssertionFailure>`. With `?` they become a hard failure in a
//!   node (`Expect(...)` → [`Interrupt::Fail`]) or a retryable failure inside a polled function
//!   (`g.Expect(...)` → [`Poll::Retry`]).
//! * Inside a polled function, helpers that use the *global* `Expect`/`Fail` (Go helpers called
//!   from `Eventually` bodies) return [`Interrupt`], which converts to [`Poll::Abort`]: as in
//!   Gomega, such a failure ends the spec immediately instead of being retried.
//! * [`eventually`] / [`consistently`] reproduce Gomega's polling: the function is called
//!   immediately, then every `interval`; `Eventually` fails with the last failure when the
//!   timeout fires before the next poll, `Consistently` fails at the first failure and succeeds
//!   when the duration elapses.
//!
//! Failure messages follow Gomega's layout (optional description, then the matcher message).

use std::fmt::{Debug, Display};
use std::time::Duration;

use tokio::time::Instant;

use crate::ginkgo::{Interrupt, NodeResult};

/// A failed matcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssertionFailure(pub String);

impl From<AssertionFailure> for Interrupt {
    fn from(a: AssertionFailure) -> Self {
        Interrupt::Fail(a.0)
    }
}

/// Outcome of one call of a polled function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poll {
    /// `g.Expect(...)` failed: try again.
    Retry(String),
    /// A global `Expect`/`Fail`/`Skip` fired: stop polling and propagate.
    Abort(Interrupt),
}

impl From<AssertionFailure> for Poll {
    fn from(a: AssertionFailure) -> Self {
        Poll::Retry(a.0)
    }
}

impl From<Interrupt> for Poll {
    fn from(i: Interrupt) -> Self {
        Poll::Abort(i)
    }
}

/// Result of a polled function (`func(g Gomega)`).
pub type PollResult<T = ()> = Result<T, Poll>;

/// Result of a matcher.
pub type Check = Result<(), AssertionFailure>;

/// Gomega's `format.MaxLength`.
const MAX_LENGTH: usize = 4000;

fn short_type_name<T: ?Sized>() -> String {
    let full = std::any::type_name::<T>();
    // strip module paths inside generics too: a::b::C<d::E> -> C<E>
    let mut out = String::new();
    let mut token = String::new();
    for ch in full.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == ':' {
            token.push(ch);
        } else {
            out.push_str(token.rsplit("::").next().unwrap_or(&token));
            token.clear();
            out.push(ch);
        }
    }
    out.push_str(token.rsplit("::").next().unwrap_or(&token));
    out
}

/// `format.Object(value, 1)`: `<type>: value`, truncated like Gomega.
pub fn format_object<T: Debug + ?Sized>(v: &T) -> String {
    let mut s = format!("{v:?}");
    if s.len() > MAX_LENGTH {
        let mut cut = MAX_LENGTH;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("...\nGomega truncated this representation as it exceeds 'format.MaxLength'.");
    }
    format!("    <{}>: {}", short_type_name::<T>(), s)
}

fn failure(desc: &str, message: String) -> AssertionFailure {
    if desc.is_empty() {
        AssertionFailure(message)
    } else {
        AssertionFailure(format!("{desc}\n{message}"))
    }
}

/// `Expect(err).NotTo(HaveOccurred(), desc)` / `Expect(f()).To(Succeed())`
pub fn succeed<T, E: Display>(r: &Result<T, E>, desc: &str) -> Check {
    match r {
        Ok(_) => Ok(()),
        Err(e) => Err(failure(
            desc,
            format!("Unexpected error:\n    {e}\noccurred"),
        )),
    }
}

/// `x, err := f(); Expect(err).NotTo(HaveOccurred(), desc)`: the value on success.
pub fn ok<T, E: Display>(r: Result<T, E>, desc: &str) -> Result<T, AssertionFailure> {
    match r {
        Ok(v) => Ok(v),
        Err(e) => Err(failure(
            desc,
            format!("Unexpected error:\n    {e}\noccurred"),
        )),
    }
}

/// `Expect(err).To(HaveOccurred(), desc)`
pub fn have_occurred<T: Debug, E>(r: &Result<T, E>, desc: &str) -> Check {
    match r {
        Err(_) => Ok(()),
        Ok(_) => Err(failure(
            desc,
            "Expected an error to have occurred.  Got:\n    <nil>: nil".to_string(),
        )),
    }
}

/// `Expect(actual).To(Equal(expected), desc)`
pub fn equal<A, B>(actual: &A, expected: &B, desc: &str) -> Check
where
    A: PartialEq<B> + Debug + ?Sized,
    B: Debug + ?Sized,
{
    if actual == expected {
        Ok(())
    } else {
        Err(failure(
            desc,
            format!(
                "Expected\n{}\nto equal\n{}",
                format_object(actual),
                format_object(expected)
            ),
        ))
    }
}

/// `Expect(actual).NotTo(Equal(expected), desc)`
pub fn not_equal<A, B>(actual: &A, expected: &B, desc: &str) -> Check
where
    A: PartialEq<B> + Debug + ?Sized,
    B: Debug + ?Sized,
{
    if actual != expected {
        Ok(())
    } else {
        Err(failure(
            desc,
            format!(
                "Expected\n{}\nnot to equal\n{}",
                format_object(actual),
                format_object(expected)
            ),
        ))
    }
}

/// `Expect(actual).To(SatisfyAny(Equal(a), Equal(b), ...), desc)`
pub fn equal_any<A, B>(actual: &A, options: &[B], desc: &str) -> Check
where
    A: PartialEq<B> + Debug + ?Sized,
    B: Debug,
{
    if options.iter().any(|o| actual == o) {
        Ok(())
    } else {
        let matchers: Vec<String> = options.iter().map(|o| format!("{o:?}")).collect();
        Err(failure(
            desc,
            format!(
                "Expected\n{}\nto match one of\n    {}",
                format_object(actual),
                matchers.join(", ")
            ),
        ))
    }
}

/// `Expect(b).To(BeTrue(), desc)`
pub fn be_true(b: bool, desc: &str) -> Check {
    if b {
        Ok(())
    } else {
        Err(failure(
            desc,
            "Expected\n    <bool>: false\nto be true".to_string(),
        ))
    }
}

/// Length of a Go value as `len(x)` (strings, slices, maps).
pub trait GoLen {
    fn go_len(&self) -> usize;
}
impl GoLen for str {
    fn go_len(&self) -> usize {
        self.len()
    }
}
impl GoLen for String {
    fn go_len(&self) -> usize {
        self.len()
    }
}
impl<T> GoLen for [T] {
    fn go_len(&self) -> usize {
        self.len()
    }
}
impl<T> GoLen for Vec<T> {
    fn go_len(&self) -> usize {
        self.len()
    }
}
impl<K, V> GoLen for std::collections::BTreeMap<K, V> {
    fn go_len(&self) -> usize {
        self.len()
    }
}
impl GoLen for serde_json::Map<String, serde_json::Value> {
    fn go_len(&self) -> usize {
        self.len()
    }
}
/// A nil slice/map (`None`) has length 0.
impl<T: GoLen> GoLen for Option<T> {
    fn go_len(&self) -> usize {
        self.as_ref().map_or(0, GoLen::go_len)
    }
}

/// `Expect(v).To(BeEmpty(), desc)`
pub fn be_empty<T: GoLen + Debug + ?Sized>(v: &T, desc: &str) -> Check {
    if v.go_len() == 0 {
        Ok(())
    } else {
        Err(failure(
            desc,
            format!("Expected\n{}\nto be empty", format_object(v)),
        ))
    }
}

/// `Expect(v).NotTo(BeEmpty(), desc)`
pub fn not_empty<T: GoLen + Debug + ?Sized>(v: &T, desc: &str) -> Check {
    if v.go_len() != 0 {
        Ok(())
    } else {
        Err(failure(
            desc,
            format!("Expected\n{}\nnot to be empty", format_object(v)),
        ))
    }
}

/// `Expect(ptr).To(BeNil(), desc)`
pub fn be_nil<T: Debug>(v: &Option<T>, desc: &str) -> Check {
    match v {
        None => Ok(()),
        Some(_) => Err(failure(
            desc,
            format!("Expected\n{}\nto be nil", format_object(v)),
        )),
    }
}

/// `Expect(ptr).NotTo(BeNil(), desc)`
pub fn not_nil<T: Debug>(v: &Option<T>, desc: &str) -> Check {
    match v {
        Some(_) => Ok(()),
        None => Err(failure(
            desc,
            "Expected\n    <nil>: nil\nnot to be nil".to_string(),
        )),
    }
}

/// `Expect(s).To(ContainSubstring(sub), desc)`
pub fn contain_substring(actual: &str, sub: &str, desc: &str) -> Check {
    if actual.contains(sub) {
        Ok(())
    } else {
        Err(failure(
            desc,
            format!(
                "Expected\n{}\nto contain substring\n{}",
                format_object(actual),
                format_object(sub)
            ),
        ))
    }
}

/// `Expect(m).To(HaveKey(key), desc)` for a `map[string]interface{}` (nil map: `None`).
pub fn have_key(
    m: &Option<serde_json::Map<String, serde_json::Value>>,
    key: &str,
    desc: &str,
) -> Check {
    if m.as_ref().is_some_and(|m| m.contains_key(key)) {
        Ok(())
    } else {
        Err(failure(
            desc,
            format!(
                "Expected\n{}\nto have key\n{}",
                format_object(m),
                format_object(key)
            ),
        ))
    }
}

/// `Eventually(f, timeout, interval).Should(Succeed(), desc)`; returns `f`'s value.
pub async fn eventually<T, F>(
    timeout: Duration,
    interval: Duration,
    desc: &str,
    mut f: F,
) -> NodeResult<T>
where
    F: AsyncFnMut() -> PollResult<T>,
{
    let start = Instant::now();
    let deadline = start + timeout;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(Poll::Abort(i)) => return Err(i),
            Err(Poll::Retry(msg)) => {
                let now = Instant::now();
                if now >= deadline || now + interval > deadline {
                    tokio::time::sleep_until(deadline).await;
                    let elapsed = start.elapsed().as_secs_f64();
                    let mut m = format!("Timed out after {elapsed:.3}s.\n");
                    if !desc.is_empty() {
                        m.push_str(desc);
                        m.push('\n');
                    }
                    m.push_str("The function passed to Eventually failed with:\n");
                    m.push_str(&msg);
                    return Err(Interrupt::Fail(m));
                }
                tokio::time::sleep(interval).await;
            }
        }
    }
}

/// `Consistently(f, duration, interval).Should(Succeed(), desc)`
pub async fn consistently<F>(
    duration: Duration,
    interval: Duration,
    desc: &str,
    mut f: F,
) -> NodeResult
where
    F: AsyncFnMut() -> PollResult<()>,
{
    let start = Instant::now();
    let deadline = start + duration;
    loop {
        match f().await {
            Ok(()) => {}
            Err(Poll::Abort(i)) => return Err(i),
            Err(Poll::Retry(msg)) => {
                let elapsed = start.elapsed().as_secs_f64();
                let mut m = format!("Failed after {elapsed:.3}s.\n");
                if !desc.is_empty() {
                    m.push_str(desc);
                    m.push('\n');
                }
                m.push_str("The function passed to Consistently failed with:\n");
                m.push_str(&msg);
                return Err(Interrupt::Fail(m));
            }
        }
        let now = Instant::now();
        if now >= deadline || now + interval > deadline {
            tokio::time::sleep_until(deadline).await;
            return Ok(());
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matcher_messages() {
        assert!(equal("a", "a", "").is_ok());
        let e = equal(
            &"Pending".to_string(),
            &"Ready",
            "CR phase is Pending, expected Ready",
        )
        .unwrap_err();
        assert_eq!(
            e.0,
            "CR phase is Pending, expected Ready\nExpected\n    <String>: \"Pending\"\nto equal\n    <&str>: \"Ready\""
        );
        assert_eq!(
            be_true(false, "HostedCluster not yet available")
                .unwrap_err()
                .0,
            "HostedCluster not yet available\nExpected\n    <bool>: false\nto be true"
        );
        assert!(not_empty("", "x").is_err());
        assert!(be_empty(&Vec::<u8>::new(), "").is_ok());
        assert!(not_empty(&None::<Vec<u8>>, "").is_err());
        assert!(
            equal_any(
                &"GeneratingIgnition",
                &["WaitingForControlPlane", "GeneratingIgnition"],
                ""
            )
            .is_ok()
        );
        assert!(have_key(&None, "ignition", "").is_err());
        let r: Result<(), String> = Err("boom".into());
        assert_eq!(
            succeed(&r, "ctx").unwrap_err().0,
            "ctx\nUnexpected error:\n    boom\noccurred"
        );
        assert!(have_occurred(&r, "").is_ok());
    }

    #[test]
    fn conversions() {
        let p: Poll = AssertionFailure("x".into()).into();
        assert_eq!(p, Poll::Retry("x".into()));
        let p: Poll = Interrupt::Fail("y".into()).into();
        assert_eq!(p, Poll::Abort(Interrupt::Fail("y".into())));
    }

    #[tokio::test(start_paused = true)]
    async fn eventually_polls_until_success() {
        let mut calls = 0;
        let start = Instant::now();
        let v = eventually(
            Duration::from_secs(30),
            Duration::from_secs(10),
            "",
            async || {
                calls += 1;
                if calls < 3 {
                    Err(Poll::Retry("not yet".into()))
                } else {
                    Ok(calls)
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(v, 3);
        assert_eq!(start.elapsed(), Duration::from_secs(20));
    }

    #[tokio::test(start_paused = true)]
    async fn eventually_times_out_with_last_message() {
        let mut calls = 0;
        let start = Instant::now();
        let err = eventually::<(), _>(
            Duration::from_secs(25),
            Duration::from_secs(10),
            "Timed out waiting for phase Ready",
            async || {
                calls += 1;
                Err(Poll::Retry(format!("attempt {calls}")))
            },
        )
        .await
        .unwrap_err();
        assert_eq!(calls, 3);
        assert_eq!(start.elapsed(), Duration::from_secs(25));
        let Interrupt::Fail(m) = err else { panic!() };
        assert!(
            m.starts_with("Timed out after 25.000s.\nTimed out waiting for phase Ready\n"),
            "{m}"
        );
        assert!(m.ends_with("attempt 3"), "{m}");
    }

    #[tokio::test(start_paused = true)]
    async fn eventually_aborts_on_global_failure() {
        let mut calls = 0;
        let err = eventually::<(), _>(
            Duration::from_secs(25),
            Duration::from_secs(1),
            "",
            async || {
                calls += 1;
                Err(Interrupt::Fail("hard".into()).into())
            },
        )
        .await
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(err, Interrupt::Fail("hard".into()));
    }

    #[tokio::test(start_paused = true)]
    async fn consistently_semantics() {
        let mut calls = 0;
        let start = Instant::now();
        consistently(
            Duration::from_secs(20),
            Duration::from_secs(5),
            "",
            async || {
                calls += 1;
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(calls, 5);
        assert_eq!(start.elapsed(), Duration::from_secs(20));

        let mut calls = 0;
        let err = consistently(
            Duration::from_secs(20),
            Duration::from_secs(5),
            "",
            async || {
                calls += 1;
                if calls == 2 {
                    Err(Poll::Retry("changed".into()))
                } else {
                    Ok(())
                }
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, Interrupt::Fail(m) if m.starts_with("Failed after 5.000s.") && m.ends_with("changed"))
        );
    }
}
