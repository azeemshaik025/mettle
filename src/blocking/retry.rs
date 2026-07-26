//! The blocking retry builder and its `call` driver.

use super::clock::{Clock, StdClock};
use crate::backoff::{Backoff, ExponentialBackoff};
use crate::shared::{should_retry_after, trace_retry};
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
/// # Ok::<(), std::io::Error>(())
/// ```
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
    pub fn call(mut self) -> Result<T, E> {
        let mut backoff = self.backoff;
        let mut attempt = 0u32;
        let start = self.clock.now();

        loop {
            let err = match (self.op)() {
                Ok(value) => return Ok(value),
                Err(err) => err,
            };

            match should_retry_after(&err, &self.when, &mut backoff, self.max_elapsed, || {
                self.clock.now().saturating_duration_since(start)
            }) {
                Some(delay) => {
                    attempt += 1;
                    trace_retry(attempt, &err, delay);
                    self.clock.sleep(delay);
                }
                None => return Err(err),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backoff::ExponentialBackoffConfig;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// A mock clock: records each sleep, advances a *virtual* now, returns instantly.
    #[derive(Clone)]
    struct MockClock {
        start: Instant,
        elapsed: Arc<Mutex<Duration>>,
        log: Arc<Mutex<Vec<Duration>>>,
    }
    impl MockClock {
        fn new() -> Self {
            Self {
                start: Instant::now(),
                elapsed: Arc::new(Mutex::new(Duration::ZERO)),
                log: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn slept(&self) -> Vec<Duration> {
            self.log.lock().unwrap().clone()
        }
    }
    impl Clock for MockClock {
        fn now(&self) -> Instant {
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
        let result: Result<i32, ()> = retry(|| Ok(42)).clock(clock.clone()).call();
        assert_eq!(result, Ok(42));
        assert!(clock.slept().is_empty());
    }

    #[test]
    fn retries_then_succeeds() {
        let clock = MockClock::new();
        let mut n = 0;
        let result: Result<i32, &str> = retry(|| {
            n += 1;
            if n < 3 { Err("boom") } else { Ok(42) }
        })
        .backoff(backoff(5))
        .clock(clock.clone())
        .call();

        assert_eq!(result, Ok(42));
        assert_eq!(n, 3); // 2 failures + 1 success
        assert_eq!(clock.slept(), vec![secs(1), secs(2)]);
    }

    #[test]
    fn stops_on_non_retryable() {
        let clock = MockClock::new();
        let mut calls = 0;
        let result: Result<i32, &str> = retry(|| {
            calls += 1;
            Err("nope")
        })
        .backoff(backoff(5))
        .clock(clock.clone())
        .when(|_e| false)
        .call();

        assert_eq!(result, Err("nope"));
        assert_eq!(calls, 1); // exactly one attempt
        assert!(clock.slept().is_empty());
    }

    #[test]
    fn exhausts_retries() {
        let clock = MockClock::new();
        let result: Result<i32, &str> = retry(|| Err("always"))
            .backoff(backoff(3))
            .clock(clock.clone())
            .call();

        assert_eq!(result, Err("always"));
        assert_eq!(clock.slept().len(), 3); // 3 retries → 3 sleeps, then give up
    }

    #[test]
    fn stops_on_time_budget() {
        let clock = MockClock::new();
        let result: Result<i32, &str> = retry(|| Err("slow"))
            .backoff(backoff(100)) // effectively unlimited retries
            .clock(clock.clone())
            .max_elapsed(secs(10))
            .call();

        assert_eq!(result, Err("slow"));
        // 1 (→1s), 2 (→3s), 4 (→7s); next would be 8 → 7+8=15 ≥ 10 → stop.
        assert_eq!(clock.slept(), vec![secs(1), secs(2), secs(4)]);
    }

    #[test]
    fn emits_a_tracing_event_per_retry() {
        // The op fails twice then succeeds → exactly two `mettle::retry` events.
        let events = crate::test_support::count_retry_events();
        let clock = MockClock::new();
        let mut n = 0;

        let out: Result<i32, &str> = retry(|| {
            n += 1;
            if n < 3 { Err("boom") } else { Ok(42) }
        })
        .backoff(backoff(5))
        .clock(clock)
        .call();

        assert_eq!(out, Ok(42));
        assert_eq!(events.get(), 2); // one event per retry
    }
}
