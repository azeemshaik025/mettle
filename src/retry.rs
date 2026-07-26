//! Retry a fallible async operation, backing off between attempts.
//!
//! Wrap your operation with [`retry()`] and await it; override the defaults only if you need to.

use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use pin_project_lite::pin_project;

use crate::backoff::{Backoff, ExponentialBackoff};
use crate::clock::{Clock, TokioClock};
use crate::shared::{should_retry_after, trace_retry};

/// A configurable retry operation.
///
/// Create it with [`retry`], tune it with the builder methods
/// ([`backoff`](Retry::backoff), [`clock`](Retry::clock), [`when`](Retry::when),
/// [`max_elapsed`](Retry::max_elapsed)), then `.await` it (it implements [`IntoFuture`]).
#[must_use = "a `Retry` does nothing until you `.await` it"]
pub struct Retry<F, B, C, P> {
    op: F,
    backoff: B,
    clock: C,
    when: P,
    max_elapsed: Option<Duration>,
}

/// Start retrying `op`, with sensible defaults for everything else: exponential backoff,
/// the Tokio clock, retry-on-any-error, and no time budget.
///
/// Override any default with the builder methods, then `.await`. The simplest use is just
/// the operation:
///
/// ```no_run
/// # use mettle::retry;
/// # async fn demo() -> Result<(), std::io::Error> {
/// let value = retry(|| async { Ok::<_, std::io::Error>(1) }).await?;
/// # let _ = value;
/// # Ok(())
/// # }
/// ```
pub fn retry<F, Fut, T, E>(op: F) -> Retry<F, ExponentialBackoff, TokioClock, fn(&E) -> bool>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    Retry {
        op,
        backoff: ExponentialBackoff::default(),
        clock: TokioClock,
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

impl<F, Fut, T, E, B, C, P> Retry<F, B, C, P>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
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

impl<F, Fut, T, E, B, C, P> IntoFuture for Retry<F, B, C, P>
where
    C: Clock,
    P: Fn(&E) -> bool,
    E: std::fmt::Debug,
    F: FnMut() -> Fut,
    B: Backoff,
    Fut: Future<Output = Result<T, E>>,
{
    type Output = Result<T, E>;
    type IntoFuture = RetryFuture<F, Fut, B, C, P, C::Sleep>;

    fn into_future(self) -> Self::IntoFuture {
        let start = self.clock.now();
        RetryFuture {
            start,
            state: RetryState::Idle,
            attempt: 0,
            op: self.op,
            when: self.when,
            clock: self.clock,
            backoff: self.backoff,
            max_elapsed: self.max_elapsed,
        }
    }
}

pin_project! {
    /// The future produced by awaiting a [`Retry`]. You rarely name it directly.
    ///
    /// It borrows only what the operation borrows, and is `Send` only when its parts are, so
    /// the operation may borrow local state and need not be `Send`.
    pub struct RetryFuture<F, Fut, B, C, P, S> {
        op: F,
        when: P,
        clock: C,
        backoff: B,
        start: Instant,
        attempt: u32,

        #[pin]
        state: RetryState<Fut, S>,

        max_elapsed: Option<Duration>,
    }
}

pin_project! {
    // Which phase the retry is in. Exactly one variant is live at a time, so the
    // illegal combinations (both futures in flight, or neither) are unrepresentable.
    #[project = RetryStateProj]
    enum RetryState<Fut, S> {
        Idle,
        Sleeping { #[pin] delay: S },
        Attempting { #[pin] fut: Fut },
    }
}

impl<F, Fut, T, E, B, C, P, S> Future for RetryFuture<F, Fut, B, C, P, S>
where
    B: Backoff,
    P: Fn(&E) -> bool,
    E: std::fmt::Debug,
    F: FnMut() -> Fut,
    C: Clock<Sleep = S>,
    S: Future<Output = ()>,
    Fut: Future<Output = Result<T, E>>,
{
    type Output = Result<T, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            let next = match this.state.as_mut().project() {
                // Nothing in flight — start an attempt.
                RetryStateProj::Idle => RetryState::Attempting { fut: (this.op)() },

                // An attempt is in flight — drive it.
                RetryStateProj::Attempting { fut } => match fut.poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(value)) => return Poll::Ready(Ok(value)),
                    Poll::Ready(Err(err)) => {
                        let step = should_retry_after(
                            &err,
                            &*this.when,
                            &mut *this.backoff,
                            *this.max_elapsed,
                            || this.clock.now().saturating_duration_since(*this.start),
                        );
                        match step {
                            Some(delay) => {
                                *this.attempt += 1;
                                trace_retry(*this.attempt, &err, delay);
                                RetryState::Sleeping {
                                    delay: this.clock.sleep(delay),
                                }
                            }
                            None => return Poll::Ready(Err(err)),
                        }
                    }
                },

                // Backing off — wait, then start the next attempt.
                RetryStateProj::Sleeping { delay } => match delay.poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => RetryState::Attempting { fut: (this.op)() },
                },
            };
            this.state.set(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backoff::{ExponentialBackoff, ExponentialBackoffConfig};
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// A mock clock: records each sleep, advances a *virtual* now, completes instantly.
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
        type Sleep = std::future::Ready<()>;

        fn now(&self) -> Instant {
            self.now_calls.fetch_add(1, SeqCst);
            self.start + *self.elapsed.lock().unwrap()
        }
        fn sleep(&self, dur: Duration) -> std::future::Ready<()> {
            self.log.lock().unwrap().push(dur);
            *self.elapsed.lock().unwrap() += dur;
            std::future::ready(())
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

    #[tokio::test]
    async fn succeeds_first_try() {
        let clock = MockClock::new();
        let result: Result<i32, ()> = retry(|| async { Ok(42) }).clock(clock.clone()).await;
        assert_eq!(result, Ok(42));
        assert!(clock.slept().is_empty()); // no retries → no sleeps
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let clock = MockClock::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = attempts.clone();
        let result: Result<i32, &str> = retry(move || {
            let a = a.clone();
            async move {
                let n = a.fetch_add(1, SeqCst);
                if n < 2 { Err("boom") } else { Ok(42) }
            }
        })
        .backoff(backoff(5))
        .clock(clock.clone())
        .await;

        assert_eq!(result, Ok(42));
        assert_eq!(attempts.load(SeqCst), 3); // 2 failures + 1 success
        assert_eq!(clock.slept(), vec![secs(1), secs(2)]); // slept after each failure
    }

    #[tokio::test]
    async fn stops_on_non_retryable() {
        let clock = MockClock::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = attempts.clone();
        let result: Result<i32, &str> = retry(move || {
            let a = a.clone();
            async move {
                a.fetch_add(1, SeqCst);
                Err("nope")
            }
        })
        .backoff(backoff(5))
        .clock(clock.clone())
        .when(|_e| false) // nothing is retryable
        .await;

        assert_eq!(result, Err("nope"));
        assert_eq!(attempts.load(SeqCst), 1); // exactly one attempt
        assert!(clock.slept().is_empty());
    }

    #[tokio::test]
    async fn exhausts_retries() {
        let clock = MockClock::new();
        let result: Result<i32, &str> = retry(|| async { Err("always") })
            .backoff(backoff(3))
            .clock(clock.clone())
            .await;

        assert_eq!(result, Err("always"));
        assert_eq!(clock.slept().len(), 3); // 3 retries → 3 sleeps, then give up
    }

    #[tokio::test]
    async fn stops_on_time_budget() {
        let clock = MockClock::new();
        let result: Result<i32, &str> = retry(|| async { Err("slow") })
            .backoff(backoff(100)) // effectively unlimited retries
            .clock(clock.clone())
            .max_elapsed(secs(10))
            .await;

        assert_eq!(result, Err("slow"));
        // 1 (→1s), 2 (→3s), 4 (→7s); next would be 8 → 7+8=15 ≥ 10 → stop.
        assert_eq!(clock.slept(), vec![secs(1), secs(2), secs(4)]);
    }

    // --- the wrapped operation may borrow locals and need not be `Send` ---

    #[tokio::test]
    async fn op_may_borrow_non_static_data() {
        // The op borrows locals (`greeting`, `attempts`); the retry future borrows only what the
        // op borrows, so it's never forced to be `'static`.
        let clock = MockClock::new();
        let greeting = String::from("hi");
        let attempts = AtomicUsize::new(0);

        let out: Result<String, &str> = retry(|| async {
            let n = attempts.fetch_add(1, SeqCst);
            if n < 2 {
                Err("transient")
            } else {
                Ok(format!("{greeting}!"))
            }
        })
        .backoff(backoff(5))
        .clock(clock.clone())
        .await;

        assert_eq!(out, Ok("hi!".to_string()));
        assert_eq!(attempts.load(SeqCst), 3);
        let _ = greeting; // still owned here — it was borrowed, not moved
    }

    #[tokio::test(flavor = "current_thread")]
    async fn op_may_be_non_send() {
        // The op captures an `Rc` (which is `!Send`); the retry future is `Send` only when its
        // parts are, so this runs fine on a single-threaded runtime.
        use std::cell::Cell;
        use std::rc::Rc;

        let clock = MockClock::new();
        let shared = Rc::new(Cell::new(0));

        let out: Result<i32, &str> = retry({
            let shared = shared.clone();
            move || {
                let shared = shared.clone();
                async move {
                    shared.set(shared.get() + 1);
                    if shared.get() < 2 {
                        Err("transient")
                    } else {
                        Ok(shared.get())
                    }
                }
            }
        })
        .backoff(backoff(5))
        .clock(clock.clone())
        .await;

        assert_eq!(out, Ok(2));
    }

    // --- realistic usage patterns ---

    #[tokio::test]
    async fn retries_transient_but_stops_on_fatal() {
        // The 5xx-retry / 4xx-stop pattern: retry some errors, give up at once on others.
        #[derive(Debug, PartialEq)]
        enum ApiError {
            Transient,
            Fatal,
        }
        let clock = MockClock::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = attempts.clone();
        let out: Result<i32, ApiError> = retry(move || {
            let a = a.clone();
            async move {
                match a.fetch_add(1, SeqCst) {
                    0 | 1 => Err(ApiError::Transient),
                    _ => Err(ApiError::Fatal),
                }
            }
        })
        .backoff(backoff(10))
        .clock(clock.clone())
        .when(|e| matches!(e, ApiError::Transient))
        .await;

        assert_eq!(out, Err(ApiError::Fatal));
        assert_eq!(attempts.load(SeqCst), 3); // transient, transient, fatal → stop
        assert_eq!(clock.slept(), vec![secs(1), secs(2)]); // slept only after the transients
    }

    #[tokio::test]
    async fn op_can_be_a_plain_fnmut() {
        // The op is `FnMut`, so it can mutate captured state directly — no Arc/atomic needed.
        let clock = MockClock::new();
        let mut calls = 0;
        let out: Result<i32, &str> = retry(|| {
            calls += 1;
            let n = calls;
            async move { if n < 3 { Err("transient") } else { Ok(n) } }
        })
        .backoff(backoff(5))
        .clock(clock.clone())
        .await;

        assert_eq!(out, Ok(3));
        assert_eq!(calls, 3); // mutated across attempts through a &mut capture
    }

    #[tokio::test]
    async fn reads_clock_only_when_a_budget_is_set() {
        // Perf contract: with no `max_elapsed` we never read the clock in the retry loop —
        // only the single `start` read at construction.
        let clock = MockClock::new();
        let _: Result<i32, &str> = retry(|| async { Err("x") })
            .backoff(backoff(3))
            .clock(clock.clone())
            .await;
        assert_eq!(clock.now_calls(), 1); // just `start`

        // With a budget, one extra read per retry decision (3 retries here).
        let clock = MockClock::new();
        let _: Result<i32, &str> = retry(|| async { Err("x") })
            .backoff(backoff(3))
            .clock(clock.clone())
            .max_elapsed(secs(1000))
            .await;
        assert_eq!(clock.now_calls(), 4); // start + 3
    }

    // --- suspension, wakers, and the real Tokio timer (the mock never goes Pending) ---

    #[tokio::test]
    async fn op_that_suspends_is_resumed() {
        // The op yields `Pending` before finishing — exercises the "attempt in flight" poll
        // path and confirms the future re-polls it to completion.
        let clock = MockClock::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = attempts.clone();
        let out: Result<i32, &str> = retry(move || {
            let a = a.clone();
            async move {
                tokio::task::yield_now().await; // suspend mid-attempt
                if a.fetch_add(1, SeqCst) < 1 {
                    Err("transient")
                } else {
                    Ok(7)
                }
            }
        })
        .backoff(backoff(5))
        .clock(clock.clone())
        .await;

        assert_eq!(out, Ok(7));
        assert_eq!(attempts.load(SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn drives_the_real_tokio_timer() {
        // Uses the default `TokioClock` with a real `tokio::time::Sleep`, not the mock. Paused
        // time auto-advances when the task parks on a sleep, so the delay's `Pending` path and
        // its waker wiring run for real — a missing waker registration would hang this test.
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = attempts.clone();
        let start = tokio::time::Instant::now();
        let out: Result<i32, &str> = retry(move || {
            let a = a.clone();
            async move {
                if a.fetch_add(1, SeqCst) < 2 {
                    Err("transient")
                } else {
                    Ok(9)
                }
            }
        })
        .backoff(backoff(5)) // delays: 1s, 2s
        .await;

        assert_eq!(out, Ok(9));
        assert_eq!(attempts.load(SeqCst), 3);
        assert_eq!(start.elapsed(), secs(3)); // really waited 1s + 2s of (virtual) time
    }

    #[tokio::test(start_paused = true)]
    async fn enforces_time_budget_with_the_real_clock() {
        // Same budget logic as `stops_on_time_budget`, but against the real `TokioClock`. Only
        // passes because `now` and `sleep` share a time source — otherwise it would never stop.
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = attempts.clone();
        let out: Result<i32, &str> = retry(move || {
            let a = a.clone();
            async move {
                a.fetch_add(1, SeqCst);
                Err("slow")
            }
        })
        .backoff(backoff(100)) // effectively unlimited retries: 1,2,4,8,… s
        .max_elapsed(secs(10))
        .await;

        assert_eq!(out, Err("slow"));
        assert_eq!(attempts.load(SeqCst), 4); // 1(→1s) 2(→3s) 4(→7s); next 8 → 15 ≥ 10, stop
    }

    #[tokio::test(start_paused = true)]
    async fn cancels_cleanly_inside_a_timeout() {
        // A user races retry against a timeout. When the timeout fires mid-attempt the retry
        // future is dropped in flight — the in-flight op must be dropped (cancelled) cleanly.
        use std::sync::atomic::AtomicBool;

        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let d = dropped.clone();
        let retrying = retry(move || {
            let g = Guard(d.clone());
            async move {
                let _g = g;
                tokio::time::sleep(secs(60)).await; // outlives the 1s timeout
                Ok::<i32, &str>(1)
            }
        });

        let outcome = tokio::time::timeout(secs(1), retrying).await;
        assert!(outcome.is_err()); // timed out
        assert!(dropped.load(SeqCst)); // the in-flight attempt was dropped when we were cancelled
    }

    // --- scale ---

    #[tokio::test]
    async fn handles_thousands_of_retries() {
        // A long sequence must not overflow, recurse, or stall — the state machine is a loop.
        let clock = MockClock::new();
        let big = ExponentialBackoff::new(ExponentialBackoffConfig {
            factor: 1,
            base: Duration::from_nanos(1),
            max_retries: 5000,
            max_delay: Duration::from_nanos(1),
        })
        .unwrap();
        let out: Result<i32, &str> = retry(|| async { Err("always") })
            .backoff(big)
            .clock(clock.clone())
            .await;

        assert_eq!(out, Err("always"));
        assert_eq!(clock.slept().len(), 5000); // 5000 retries, then give up
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn many_concurrent_retries_are_independent() {
        // Hundreds of retries in flight across threads: proves the future is `Send`/spawnable
        // and that concurrent runs don't corrupt one another's state.
        use tokio::task::JoinSet;

        let mut set = JoinSet::new();
        for i in 0..200usize {
            set.spawn(async move {
                let attempts = Arc::new(AtomicUsize::new(0));
                let a = attempts.clone();
                retry(move || {
                    let a = a.clone();
                    async move {
                        if a.fetch_add(1, SeqCst) < 2 {
                            Err("transient")
                        } else {
                            Ok(i)
                        }
                    }
                })
                .backoff(backoff(5))
                .clock(MockClock::new())
                .await
            });
        }

        let mut completed = 0;
        while let Some(res) = set.join_next().await {
            assert!(res.unwrap().is_ok());
            completed += 1;
        }
        assert_eq!(completed, 200);
    }

    #[tokio::test]
    async fn emits_a_tracing_event_per_retry() {
        // The op fails twice then succeeds → exactly two `mettle::retry` events. A global
        // counting subscriber (see `test_support`) tallies them for this thread.
        let events = crate::test_support::count_retry_events();
        let clock = MockClock::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = attempts.clone();

        let out: Result<i32, &str> = retry(move || {
            let a = a.clone();
            async move {
                if a.fetch_add(1, SeqCst) < 2 {
                    Err("boom")
                } else {
                    Ok(42)
                }
            }
        })
        .backoff(backoff(5))
        .clock(clock)
        .await;

        assert_eq!(out, Ok(42));
        assert_eq!(events.get(), 2); // one event per retry
    }
}
