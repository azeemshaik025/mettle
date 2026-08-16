//! The blocking retry builder and its `call` driver.

use super::clock::{Clock, StdClock};
use crate::backoff::{Backoff, ExponentialBackoff};
use crate::error::RetryError;
use crate::shared::{Decision, give_up, should_retry_after, trace_retry};
use std::time::Duration;

/// A configurable blocking retry operation.
///
/// Create it with [`retry`], tune it with the builder methods
/// ([`backoff`](Retry::backoff), [`clock`](Retry::clock), [`when`](Retry::when),
/// [`max_elapsed`](Retry::max_elapsed)), then run it with [`call`](Retry::call).
#[must_use = "a `Retry` does nothing until you `.call()` it"]
pub struct Retry<F, B, C, P> {
    op: F,
    backoff: B,
    clock: C,
    when: P,
    max_elapsed: Option<Duration>,
}

/// Start retrying `op`, with sensible defaults for everything else: exponential backoff,
/// the std clock, retry-on-any-error, and no time budget.
///
/// Override any default with the builder methods, then [`call`](Retry::call). The simplest
/// use is just the operation:
///
/// ```no_run
/// # use mettle::blocking::retry;
/// # fn fetch() -> Result<u32, std::io::Error> { Ok(1) }
/// let value = retry(fetch).call()?;
/// # let _ = value;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// On failure the error is a [`RetryError`]; for the bare error, `.map_err(RetryError::into_error)`.
pub fn retry<F, T, E>(op: F) -> Retry<F, ExponentialBackoff, StdClock, fn(&E) -> bool>
where
    F: FnMut() -> Result<T, E>,
{
    Retry {
        op,
        backoff: ExponentialBackoff::default(),
        clock: StdClock,
        when: (|_| true) as fn(&E) -> bool,
        max_elapsed: None,
    }
}

impl<F, B, C, P> Retry<F, B, C, P> {
    /// Override the backoff strategy (any [`Backoff`]).
    pub fn backoff<B2>(self, backoff: B2) -> Retry<F, B2, C, P> {
        Retry {
            backoff,
            op: self.op,
            when: self.when,
            clock: self.clock,
            max_elapsed: self.max_elapsed,
        }
    }

    /// Override the clock (any [`Clock`]), e.g. a mock clock in tests.
    pub fn clock<C2>(self, clock: C2) -> Retry<F, B, C2, P> {
        Retry {
            clock,
            op: self.op,
            backoff: self.backoff,
            when: self.when,
            max_elapsed: self.max_elapsed,
        }
    }

    /// Give up once this much total time has elapsed (default: no limit).
    ///
    /// Checked *between* attempts, when one returns. It cannot interrupt an attempt that is still
    /// running, and there is no blocking equivalent of the async `attempt_timeout`: interrupting a
    /// blocking closure would mean running it on another thread, which would force
    /// `Send + 'static` onto your operation, and two tests here exist to forbid exactly that. Bound the call itself instead, with whatever it offers, such as
    /// `TcpStream::set_read_timeout` or your client's own timeout setting.
    pub fn max_elapsed(mut self, budget: Duration) -> Self {
        self.max_elapsed = Some(budget);
        self
    }
}

impl<F, T, E, B, C, P> Retry<F, B, C, P>
where
    F: FnMut() -> Result<T, E>,
{
    /// Only retry errors for which `predicate` returns `true` (default: retry all).
    ///
    /// The predicate sees each `&E`, so `e`'s type is inferred; no annotation needed.
    pub fn when<P2>(self, predicate: P2) -> Retry<F, B, C, P2>
    where
        P2: Fn(&E) -> bool,
    {
        Retry {
            when: predicate,
            op: self.op,
            backoff: self.backoff,
            clock: self.clock,
            max_elapsed: self.max_elapsed,
        }
    }
}

impl<F, T, E, B, C, P> Retry<F, B, C, P>
where
    F: FnMut() -> Result<T, E>,
    B: Backoff,
    C: Clock,
    P: Fn(&E) -> bool,
    E: std::fmt::Debug,
{
    /// Run the operation, blocking between attempts, until it succeeds or gives up.
    ///
    /// # Errors
    /// Returns a [`RetryError`] carrying the last error, the attempt count, the elapsed time, and
    /// why it stopped. For the bare error, `.map_err(RetryError::into_error)`.
    pub fn call(mut self) -> Result<T, RetryError<E>> {
        let mut backoff = self.backoff;
        let mut retries = 0u32;
        let start = self.clock.now();
        let elapsed = || self.clock.now().saturating_duration_since(start);

        loop {
            let err = match (self.op)() {
                Ok(value) => return Ok(value),
                Err(err) => err,
            };

            match should_retry_after(&err, &self.when, &mut backoff, self.max_elapsed, elapsed) {
                Decision::Retry(delay) => {
                    retries += 1;
                    trace_retry(retries, &err, delay);
                    self.clock.sleep(delay);
                }
                Decision::Stop { reason, elapsed: m } => {
                    return Err(give_up(err, retries, reason, m, elapsed));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backoff::ExponentialBackoffConfig;
    use crate::error::StopReason;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// A mock clock: records each sleep, advances a *virtual* now, returns instantly.
    #[derive(Clone)]
    struct MockClock {
        start: Instant,
        elapsed: Arc<Mutex<Duration>>,
        log: Arc<Mutex<Vec<Duration>>>,
        now_calls: Arc<AtomicUsize>,
    }
    impl MockClock {
        fn new() -> Self {
            Self {
                start: Instant::now(),
                elapsed: Arc::new(Mutex::new(Duration::ZERO)),
                log: Arc::new(Mutex::new(Vec::new())),
                now_calls: Arc::new(AtomicUsize::new(0)),
            }
        }
        fn slept(&self) -> Vec<Duration> {
            self.log.lock().unwrap().clone()
        }
        fn now_calls(&self) -> usize {
            self.now_calls.load(SeqCst)
        }
    }
    impl Clock for MockClock {
        fn now(&self) -> Instant {
            self.now_calls.fetch_add(1, SeqCst);
            self.start + *self.elapsed.lock().unwrap()
        }
        fn sleep(&self, dur: Duration) {
            self.log.lock().unwrap().push(dur);
            *self.elapsed.lock().unwrap() += dur;
        }
    }

    fn backoff(max_retries: u32) -> ExponentialBackoff {
        ExponentialBackoff::new(ExponentialBackoffConfig {
            factor: 2,
            base: Duration::from_secs(1),
            max_retries,
            max_delay: Duration::from_secs(100),
        })
        .unwrap()
    }
    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn succeeds_first_try() {
        let clock = MockClock::new();
        let result: Result<i32, RetryError<()>> = retry(|| Ok(42)).clock(clock.clone()).call();
        assert_eq!(result.unwrap(), 42);
        assert!(clock.slept().is_empty());
    }

    #[test]
    fn retries_then_succeeds() {
        let clock = MockClock::new();
        let mut n = 0;
        let result: Result<i32, RetryError<&str>> = retry(|| {
            n += 1;
            if n < 3 { Err("boom") } else { Ok(42) }
        })
        .backoff(backoff(5))
        .clock(clock.clone())
        .call();

        assert_eq!(result.unwrap(), 42);
        assert_eq!(n, 3); // 2 failures + 1 success
        assert_eq!(clock.slept(), vec![secs(1), secs(2)]);
    }

    #[test]
    fn stops_on_non_retryable() {
        let clock = MockClock::new();
        let mut calls = 0;
        let result: Result<i32, RetryError<&str>> = retry(|| {
            calls += 1;
            Err("nope")
        })
        .backoff(backoff(5))
        .clock(clock.clone())
        .when(|_e| false)
        .call();

        let err = result.unwrap_err();
        assert_eq!(*err.error(), "nope");
        assert_eq!(err.stop_reason(), StopReason::NotRetryable);
        assert_eq!(err.attempts(), 1); // the rejected attempt still ran
        assert_eq!(err.elapsed(), Duration::ZERO);
        assert_eq!(calls, 1); // exactly one attempt
        assert!(clock.slept().is_empty());
    }

    #[test]
    fn exhausts_retries() {
        let clock = MockClock::new();
        let result: Result<i32, RetryError<&str>> = retry(|| Err("always"))
            .backoff(backoff(3))
            .clock(clock.clone())
            .call();

        let err = result.unwrap_err();
        assert_eq!(*err.error(), "always");
        assert_eq!(err.stop_reason(), StopReason::RetriesExhausted);
        assert_eq!(err.attempts(), 4); // 3 retries → 4 attempts
        assert_eq!(err.elapsed(), secs(7)); // 1 + 2 + 4 on the mock clock
        assert_eq!(clock.slept().len(), 3); // 3 retries → 3 sleeps, then give up
    }

    #[test]
    fn stops_on_time_budget() {
        let clock = MockClock::new();
        let result: Result<i32, RetryError<&str>> = retry(|| Err("slow"))
            .backoff(backoff(100)) // effectively unlimited retries
            .clock(clock.clone())
            .max_elapsed(secs(10))
            .call();

        let err = result.unwrap_err();
        assert_eq!(*err.error(), "slow");
        assert_eq!(err.stop_reason(), StopReason::MaxElapsed);
        assert_eq!(err.attempts(), 4);
        assert!(
            err.elapsed() < secs(10),
            "elapsed must stay under the budget"
        );
        // 1 (→1s), 2 (→3s), 4 (→7s); next would be 8 → 7+8=15 ≥ 10 → stop.
        assert_eq!(clock.slept(), vec![secs(1), secs(2), secs(4)]);
    }

    #[test]
    fn drives_a_jittered_backoff() {
        // The async twin of this lives in src/retry.rs. Both drivers take any `Backoff`, so both
        // have to be shown driving a randomized one (ADR001 decision 3).
        let clock = MockClock::new();
        let out: Result<i32, RetryError<&str>> = retry(|| Err("boom"))
            .backoff(backoff(3).jittered_with_seed(42))
            .clock(&clock)
            .call();

        assert_eq!(*out.unwrap_err().error(), "boom");
        let slept = clock.slept();
        assert_eq!(slept.len(), 3);
        // Underlying exponential is 1s, 2s, 4s; full jitter can only shrink each one.
        for (d, cap) in slept.iter().zip([secs(1), secs(2), secs(4)]) {
            assert!(*d <= cap, "jittered delay {d:?} exceeded {cap:?}");
        }
    }

    #[test]
    fn drives_a_decorrelated_backoff() {
        use crate::backoff::{DecorrelatedBackoff, DecorrelatedBackoffConfig};

        let clock = MockClock::new();
        let out: Result<i32, RetryError<&str>> = retry(|| Err("boom"))
            .backoff(
                DecorrelatedBackoff::with_seed(
                    DecorrelatedBackoffConfig {
                        base: secs(1),
                        max_retries: 4,
                        max_delay: secs(20),
                    },
                    7,
                )
                .unwrap(),
            )
            .clock(&clock)
            .call();

        assert_eq!(*out.unwrap_err().error(), "boom");
        let slept = clock.slept();
        assert_eq!(slept.len(), 4);
        assert!(
            slept.iter().all(|d| *d >= secs(1) && *d <= secs(20)),
            "delays escaped [base, max_delay]: {slept:?}"
        );
    }

    #[test]
    fn accepts_a_borrowed_or_shared_clock() {
        // `.clock(c)` takes the clock by value, so without the reference impls a test could hand
        // over its mock and never read it back. Both forms must reach the same mock.
        let clock = MockClock::new();
        let _: Result<i32, RetryError<&str>> =
            retry(|| Err("x")).backoff(backoff(2)).clock(&clock).call();
        assert_eq!(clock.slept(), vec![secs(1), secs(2)]);

        let shared = std::sync::Arc::new(MockClock::new());
        let _: Result<i32, RetryError<&str>> = retry(|| Err("x"))
            .backoff(backoff(2))
            .clock(Arc::clone(&shared))
            .call();
        assert_eq!(shared.slept(), vec![secs(1), secs(2)]);
    }

    #[test]
    fn clock_reads_do_not_scale_with_attempts() {
        // The async twin of this lives in src/retry.rs. `elapsed()` is public now, so both
        // drivers have to agree on what it costs.
        for retries in [3, 30] {
            let clock = MockClock::new();
            let _: Result<i32, RetryError<&str>> = retry(|| Err("x"))
                .backoff(backoff(retries))
                .clock(clock.clone())
                .call();
            assert_eq!(clock.now_calls(), 2, "with {retries} retries and no budget");
        }

        let clock = MockClock::new();
        let _: Result<i32, RetryError<&str>> = retry(|| Err("x"))
            .backoff(backoff(3))
            .clock(clock.clone())
            .max_elapsed(secs(1000))
            .call();
        assert_eq!(clock.now_calls(), 5); // start + 3 decisions + terminal
    }

    #[test]
    fn emits_a_tracing_event_per_retry() {
        // The op fails twice then succeeds → exactly two `mettle::retry` events.
        let events = crate::test_support::count_retry_events();
        let clock = MockClock::new();
        let mut n = 0;

        let out: Result<i32, RetryError<&str>> = retry(|| {
            n += 1;
            if n < 3 { Err("boom") } else { Ok(42) }
        })
        .backoff(backoff(5))
        .clock(clock)
        .call();

        assert_eq!(out.unwrap(), 42);
        assert_eq!(events.get(), 2); // one event per retry
    }
}
