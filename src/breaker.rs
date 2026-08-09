//! Circuit breaking: stop calling a dependency that is already failing.
//!
//! Retry on its own makes an outage worse. Every client that failed retries, so a service that is
//! struggling gets more load exactly when it can least take it. A breaker is the other half: once
//! enough recent calls have failed it stops letting calls through at all, gives the dependency a
//! quiet window, then lets a probe or two through to see if it recovered.
//!
//! **A rejection is not retryable.** When you put a retry around a breaker, say so with
//! `.when(BreakerError::is_inner)`, or the retry will sit through a full backoff schedule for
//! calls that never left the process.

use crate::time::Now;
use std::time::{Duration, Instant};

/// The most outcomes a window can weigh. The window is a single `u128` bitset, so this is where
/// that ends.
pub const MAX_WINDOW_SIZE: u32 = 128;

// Defaults for `CircuitBreakerConfig::default()`.
const DEFAULT_FAILURE_RATE_PERCENT: u8 = 50;
const DEFAULT_WINDOW_SIZE: u32 = 100;
const DEFAULT_MINIMUM_CALLS: u32 = 20;
const DEFAULT_WAIT_DURATION: Duration = Duration::from_secs(30);
const DEFAULT_HALF_OPEN_MAX_CALLS: u32 = 1;

/// Parameters for [`CircuitBreaker::new`]. Fill only what differs from [`Default`]:
///
/// ```
/// # use mettle::CircuitBreakerConfig;
/// # use std::time::Duration;
/// let _ = CircuitBreakerConfig {
///     wait_duration: Duration::from_secs(5),
///     ..Default::default()
/// };
/// ```
///
/// `minimum_calls` defaults to 20, so if you shrink `window_size` below that, lower
/// `minimum_calls` too. That combination is rejected rather than quietly clamped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CircuitBreakerConfig {
    /// Trip once this percentage of the window has failed (1..=100).
    ///
    /// A whole percent, not a float, so the trip boundary is exact: at 50 with a full window of
    /// 100, the 50th failure trips it and the 49th doesn't, on every machine.
    pub failure_rate_percent: u8,
    /// How many recent outcomes to weigh (1..=[`MAX_WINDOW_SIZE`]).
    pub window_size: u32,
    /// Don't trip until this many outcomes have been recorded (1..=`window_size`), counted from
    /// the last state change.
    ///
    /// Without this a rate breaker trips on the first failure, since one failure out of one call
    /// is a 100% failure rate.
    pub minimum_calls: u32,
    /// How long to stay open before letting a probe through. Also caps how long a half-open
    /// probe has to answer before the breaker gives up on it and re-opens.
    pub wait_duration: Duration,
    /// How many probes a half-open period lets through in total, and how many must succeed for
    /// the breaker to close (must be non-zero).
    pub half_open_max_calls: u32,
}

impl Default for CircuitBreakerConfig {
    /// Sensible defaults: trip at 50% of the last 100 calls, no sooner than 20 calls in, stay
    /// open 30 s, then close on a single successful probe.
    fn default() -> Self {
        Self {
            failure_rate_percent: DEFAULT_FAILURE_RATE_PERCENT,
            window_size: DEFAULT_WINDOW_SIZE,
            minimum_calls: DEFAULT_MINIMUM_CALLS,
            wait_duration: DEFAULT_WAIT_DURATION,
            half_open_max_calls: DEFAULT_HALF_OPEN_MAX_CALLS,
        }
    }
}

/// Why a [`CircuitBreakerConfig`] was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BreakerConfigError {
    /// `failure_rate_percent` was 0 or above 100. Zero would trip on the first outcome.
    FailureRateOutOfRange,
    /// `window_size` was 0 or above [`MAX_WINDOW_SIZE`].
    WindowSizeOutOfRange,
    /// `minimum_calls` was 0, or larger than `window_size`, which would mean it could never trip.
    MinimumCallsOutOfRange,
    /// `wait_duration` was zero, so the breaker would leave `Open` before anything could observe
    /// it and would never shed load.
    ZeroWaitDuration,
    /// `half_open_max_calls` was zero, so the breaker could never close again.
    ZeroHalfOpenMaxCalls,
}

impl std::fmt::Display for BreakerConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FailureRateOutOfRange => {
                f.write_str("breaker `failure_rate_percent` must be 1..=100")
            }
            Self::WindowSizeOutOfRange => {
                write!(f, "breaker `window_size` must be 1..={MAX_WINDOW_SIZE}")
            }
            Self::MinimumCallsOutOfRange => {
                f.write_str("breaker `minimum_calls` must be 1..=`window_size`")
            }
            Self::ZeroWaitDuration => f.write_str("breaker `wait_duration` must be non-zero"),
            Self::ZeroHalfOpenMaxCalls => {
                f.write_str("breaker `half_open_max_calls` must be non-zero")
            }
        }
    }
}

impl std::error::Error for BreakerConfigError {}

/// Check a config before anything is built from it, so a live breaker is always sane.
///
/// Every rejected case here is one that would make the breaker useless rather than merely
/// badly tuned: it would trip on the first call, never trip at all, or never close again.
fn validate(config: &CircuitBreakerConfig) -> Result<(), BreakerConfigError> {
    let CircuitBreakerConfig {
        failure_rate_percent,
        window_size,
        minimum_calls,
        wait_duration,
        half_open_max_calls,
    } = *config;

    if failure_rate_percent == 0 || failure_rate_percent > 100 {
        return Err(BreakerConfigError::FailureRateOutOfRange);
    }
    if window_size == 0 || window_size > MAX_WINDOW_SIZE {
        return Err(BreakerConfigError::WindowSizeOutOfRange);
    }
    if minimum_calls == 0 || minimum_calls > window_size {
        return Err(BreakerConfigError::MinimumCallsOutOfRange);
    }
    if wait_duration.is_zero() {
        return Err(BreakerConfigError::ZeroWaitDuration);
    }
    if half_open_max_calls == 0 {
        return Err(BreakerConfigError::ZeroHalfOpenMaxCalls);
    }
    Ok(())
}

/// What a breaker is currently doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BreakerState {
    /// Letting calls through and watching how they go.
    Closed,
    /// Shedding calls without attempting them.
    Open,
    /// Letting a limited number of probes through to see whether the dependency recovered.
    HalfOpen,
}

impl BreakerState {
    /// A stable snake_case token, for metric labels and log filters: `"closed"`, `"open"`,
    /// `"half_open"`. These strings are part of the API.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

impl std::fmt::Display for BreakerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why the breaker turned a call away. The operation was never run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Rejected {
    /// The circuit is open. `retry_after` is how long is left on the current wait, so a caller
    /// that wants to back off has a number to use.
    Open {
        /// Time remaining before a probe would be admitted.
        retry_after: Duration,
    },
    /// The circuit is half-open and has already admitted all the probes it will admit for this
    /// round. Another caller is testing the dependency; this one is shed.
    HalfOpenLimit,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open { retry_after } => {
                write!(f, "circuit is open, retry in {retry_after:?}")
            }
            Self::HalfOpenLimit => f.write_str("circuit is half-open and probing"),
        }
    }
}

impl std::error::Error for Rejected {}

/// What went wrong: either the breaker refused to run the call, or the call ran and failed.
///
/// The two need telling apart, because a rejection means the dependency was never touched, so
/// retrying it immediately just burns the retry budget. [`is_inner`](BreakerError::is_inner) is
/// built to hand straight to retry's `.when(..)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BreakerError<E> {
    /// The breaker turned the call away. The operation never ran.
    Rejected(Rejected),
    /// The operation ran and returned this error.
    Inner(E),
}

impl<E> BreakerError<E> {
    /// True when the operation actually ran and failed, false when the breaker shed it.
    ///
    /// Pass this to retry's `.when(..)` so a shed call ends the retry immediately instead of
    /// sleeping through a backoff schedule for a request that never left the process.
    ///
    /// ```
    /// # use mettle::{BreakerError, Rejected};
    /// let shed = BreakerError::<std::io::Error>::Rejected(Rejected::HalfOpenLimit);
    /// assert!(!shed.is_inner());
    /// ```
    pub fn is_inner(&self) -> bool {
        matches!(self, Self::Inner(_))
    }

    /// True when the breaker shed the call without running it.
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected(_))
    }

    /// The operation's error, if it ran.
    pub fn inner(&self) -> Option<&E> {
        match self {
            Self::Inner(e) => Some(e),
            Self::Rejected(_) => None,
        }
    }

    /// Take the operation's error back, if it ran.
    pub fn into_inner(self) -> Option<E> {
        match self {
            Self::Inner(e) => Some(e),
            Self::Rejected(_) => None,
        }
    }
}

impl<E: std::fmt::Display> std::fmt::Display for BreakerError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(r) => write!(f, "{r}"),
            Self::Inner(e) => write!(f, "{e}"),
        }
    }
}

// Bounded the same way `RetryError` is, and for the same reason (ADR005): `E: Error + 'static`
// would conflict with this impl, and it would lock out `String`, `Box<dyn Error>`, and
// `anyhow::Error`, none of which implement `Error`.
impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for BreakerError<E> {}

// There is no `Outcome::Ignore` variant, because ignoring is the absence of a report rather than
// a kind of one: it touches neither the window nor the half-open success count. A half-open probe
// that reports nothing still burns its slot, since the slot was spent at permit time.

/// A state change worth reporting. Carries the window numbers that caused it so the log line can
/// explain itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Transition {
    pub(crate) from: BreakerState,
    pub(crate) to: BreakerState,
    pub(crate) failures: u32,
    pub(crate) samples: u32,
}

/// What the breaker is doing, plus the bookkeeping each state needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Closed,
    Open {
        since: Instant,
    },
    HalfOpen {
        /// When the oldest probe still waiting to report was admitted. Only meaningful while
        /// `in_flight > 0`; an idle period has nothing to time out.
        since: Instant,
        /// Probes admitted so far this period. A cumulative budget, never given back.
        admitted: u32,
        /// Probes that came back `Ok`.
        successes: u32,
        /// Probes admitted that haven't reported yet. The deadline applies only to these: a
        /// period sitting idle between slow callers isn't stuck, it's just quiet.
        in_flight: u32,
    },
}

/// The breaker's decision logic with no locking, no clock, and no logging: hand it "now" and an
/// outcome, get back a decision and possibly a transition.
///
/// Pure on purpose (ADR001). Everything interesting about a breaker is a state machine over time,
/// and keeping it separate from the sharing and the I/O means the whole transition table can be
/// tested by calling functions with hand-written `Instant`s, no mock and no threads.
#[derive(Debug)]
pub(crate) struct Machine {
    config: CircuitBreakerConfig,
    phase: Phase,
    /// Bumped on every transition. A permit carries the value it was issued under, so a report
    /// that arrives after the state moved on can be discarded instead of corrupting the new
    /// period's counters.
    generation: u64,
    /// One bit per recent outcome, set means failure. A `u128` because `window_size` is capped at
    /// 128, which keeps the whole window in a register and the breaker allocation-free.
    window: u128,
    /// How many outcomes are actually in the window, so a partly-filled window divides by the
    /// right denominator.
    filled: u32,
    /// Where the next outcome goes, wrapping at `window_size`.
    cursor: u32,
}

impl Machine {
    pub(crate) fn new(config: CircuitBreakerConfig) -> Result<Self, BreakerConfigError> {
        validate(&config)?;
        Ok(Self {
            config,
            phase: Phase::Closed,
            generation: 0,
            window: 0,
            filled: 0,
            cursor: 0,
        })
    }

    pub(crate) fn config(&self) -> &CircuitBreakerConfig {
        &self.config
    }

    /// What the breaker would report right now. Read-only: it never transitions, so calling it
    /// can't change behaviour.
    pub(crate) fn state(&self, now: Instant) -> BreakerState {
        match self.phase {
            Phase::Closed => BreakerState::Closed,
            // Both of these are due to move on the next call that arrives; report where they'd
            // land rather than where they've been sitting.
            Phase::Open { since } => {
                if self.waited(since, now) {
                    BreakerState::HalfOpen
                } else {
                    BreakerState::Open
                }
            }
            Phase::HalfOpen {
                since, in_flight, ..
            } => {
                if in_flight > 0 && self.waited(since, now) {
                    BreakerState::Open
                } else {
                    BreakerState::HalfOpen
                }
            }
        }
    }

    fn waited(&self, since: Instant, now: Instant) -> bool {
        now.saturating_duration_since(since) >= self.config.wait_duration
    }

    fn phase_state(&self) -> BreakerState {
        match self.phase {
            Phase::Closed => BreakerState::Closed,
            Phase::Open { .. } => BreakerState::Open,
            Phase::HalfOpen { .. } => BreakerState::HalfOpen,
        }
    }

    fn failures(&self) -> u32 {
        // Only bits below `window_size` are ever set, so this needs no mask.
        self.window.count_ones()
    }

    fn push(&mut self, failed: bool) {
        let bit = 1u128 << self.cursor;
        if failed {
            self.window |= bit;
        } else {
            self.window &= !bit;
        }
        self.cursor = (self.cursor + 1) % self.config.window_size;
        self.filled = (self.filled + 1).min(self.config.window_size);
    }

    /// Enough recent calls, and enough of them failed. Integer arithmetic, so the boundary is
    /// exact: at 50% of a full 100-window the 50th failure trips and the 49th doesn't.
    /// `filled`, not `window_size`, is the denominator, or a half-full window trips late.
    fn should_trip(&self) -> bool {
        self.filled >= self.config.minimum_calls
            && self.failures() * 100 >= u32::from(self.config.failure_rate_percent) * self.filled
    }

    fn transition(&mut self, to: Phase) -> Transition {
        let t = Transition {
            from: self.phase_state(),
            to: match to {
                Phase::Closed => BreakerState::Closed,
                Phase::Open { .. } => BreakerState::Open,
                Phase::HalfOpen { .. } => BreakerState::HalfOpen,
            },
            failures: self.failures(),
            samples: self.filled,
        };
        self.phase = to;
        self.generation += 1;
        // Every period starts from a clean window. Without this a recovered breaker still holds
        // the failures that tripped it, so the first post-recovery failure re-opens it forever.
        self.window = 0;
        self.filled = 0;
        self.cursor = 0;
        t
    }

    /// Ask permission to run a call. On success the caller gets the generation it must report
    /// under, and whether this call is a half-open probe (probes are the only ones whose report
    /// needs a timestamp).
    pub(crate) fn on_permit(
        &mut self,
        now: Instant,
    ) -> (Result<(u64, bool), Rejected>, Option<Transition>) {
        match self.phase {
            Phase::Closed => (Ok((self.generation, false)), None),

            Phase::Open { since } => {
                if !self.waited(since, now) {
                    let retry_after = self
                        .config
                        .wait_duration
                        .saturating_sub(now.saturating_duration_since(since));
                    return (Err(Rejected::Open { retry_after }), None);
                }
                // Lazily become half-open on the first call after the wait, so there's no timer
                // and no background task. This caller is probe number one.
                let t = self.transition(Phase::HalfOpen {
                    since: now,
                    admitted: 1,
                    successes: 0,
                    in_flight: 1,
                });
                (Ok((self.generation, true)), Some(t))
            }

            Phase::HalfOpen {
                since,
                admitted,
                in_flight,
                ..
            } => {
                // The deadline is about a probe that never came back, not about the period being
                // quiet. Timing out an idle period is how a breaker with more than one probe to
                // collect never closes when callers arrive slower than `wait_duration`.
                if in_flight > 0 && self.waited(since, now) {
                    let t = self.transition(Phase::Open { since: now });
                    return (
                        Err(Rejected::Open {
                            retry_after: self.config.wait_duration,
                        }),
                        Some(t),
                    );
                }
                if admitted >= self.config.half_open_max_calls {
                    return (Err(Rejected::HalfOpenLimit), None);
                }
                // A cumulative budget for the period, not a concurrency limit: handing the slot
                // back on release would let a recovering dependency take unbounded traffic.
                if let Phase::HalfOpen {
                    since,
                    admitted,
                    in_flight,
                    ..
                } = &mut self.phase
                {
                    *admitted += 1;
                    if *in_flight == 0 {
                        *since = now; // first outstanding probe starts the clock
                    }
                    *in_flight += 1;
                }
                (Ok((self.generation, true)), None)
            }
        }
    }

    /// Report that an admitted call succeeded.
    ///
    /// `now` is `Some` only for half-open probes, so an ordinary success in the closed state
    /// still costs no clock read at all. A probe needs it because answering after the period's
    /// deadline must re-open rather than close: without that check a probe could close a breaker
    /// that [`state`](Machine::state) is already reporting as `Open`.
    pub(crate) fn on_success(
        &mut self,
        generation: u64,
        now: Option<Instant>,
    ) -> Option<Transition> {
        if generation != self.generation {
            return None; // issued under a period that has since ended
        }
        match self.phase {
            Phase::Closed => {
                self.push(false);
                None
            }
            Phase::HalfOpen {
                since, successes, ..
            } => {
                // A `match` guard rather than a let-chain: let-chains need Rust 1.88 and this
                // crate's declared MSRV is 1.85.
                match now {
                    // This probe took longer than the whole open window to answer. Treat the
                    // period as stale rather than letting a very late `Ok` close the breaker.
                    Some(now) if self.waited(since, now) => {
                        return Some(self.transition(Phase::Open { since: now }));
                    }
                    _ => {}
                }
                let successes = successes + 1;
                if successes >= self.config.half_open_max_calls {
                    return Some(self.transition(Phase::Closed));
                }
                if let Phase::HalfOpen {
                    successes: s,
                    in_flight,
                    ..
                } = &mut self.phase
                {
                    *s = successes;
                    *in_flight = in_flight.saturating_sub(1);
                }
                None
            }
            // Unreachable: the generation check rejects anything issued before we opened.
            Phase::Open { .. } => None,
        }
    }

    /// Report that an admitted call finished but says nothing about the dependency's health.
    ///
    /// Nothing touches the window, and a probe keeps its spent budget, but the in-flight count
    /// drops so an ignored probe doesn't look like one that hung.
    pub(crate) fn on_ignore(&mut self, generation: u64) {
        if generation != self.generation {
            return;
        }
        if let Phase::HalfOpen { in_flight, .. } = &mut self.phase {
            *in_flight = in_flight.saturating_sub(1);
        }
    }

    /// Report that an admitted call failed. Needs `now` because this is the path that can open
    /// the breaker, and the open period is timed from that instant.
    pub(crate) fn on_failure(&mut self, generation: u64, now: Instant) -> Option<Transition> {
        if generation != self.generation {
            return None;
        }
        match self.phase {
            Phase::Closed => {
                self.push(true);
                self.should_trip()
                    .then(|| self.transition(Phase::Open { since: now }))
            }
            // One bad probe is enough. Averaging here would let a dependency that is still broken
            // drag a half-open breaker back closed.
            Phase::HalfOpen { .. } => Some(self.transition(Phase::Open { since: now })),
            Phase::Open { .. } => None,
        }
    }
}

/// A circuit breaker: shared across callers, so one caller's failures protect the rest.
///
/// This is the opposite ownership model from [`Backoff`](crate::Backoff), which each retry owns
/// privately. A breaker is only useful when everyone calling a dependency shares one, so hold it
/// in an `Arc` (or a `static`) and call through `&self`. Build one per dependency, not per call.
///
/// ```
/// # #[cfg(feature = "blocking")] {
/// use mettle::{CircuitBreaker, CircuitBreakerConfig};
/// use std::sync::Arc;
///
/// # fn demo() -> Result<(), Box<dyn std::error::Error>> {
/// let breaker = Arc::new(CircuitBreaker::new(
///     CircuitBreakerConfig::default(),
///     mettle::blocking::StdClock, // any `Now`; use a mock in tests
/// )?);
///
/// let out = breaker.call(|| Ok::<_, std::io::Error>(7));
/// assert_eq!(out.unwrap(), 7);
/// # Ok(())
/// # }
/// # demo().unwrap();
/// # }
/// ```
///
/// The time source is an explicit argument rather than a default, so a breaker built in a test
/// can't silently read the system clock and ignore the virtual one.
#[derive(Debug)]
pub struct CircuitBreaker<N> {
    machine: std::sync::Mutex<Machine>,
    now: N,
}

impl<N: Now> CircuitBreaker<N> {
    /// Validate a [`CircuitBreakerConfig`] into a ready-to-use breaker reading time from `now`.
    ///
    /// # Errors
    /// Returns [`BreakerConfigError`] if the config describes a breaker that would trip on the
    /// first call, never trip, or never close again.
    pub fn new(config: CircuitBreakerConfig, now: N) -> Result<Self, BreakerConfigError> {
        Ok(Self {
            machine: std::sync::Mutex::new(Machine::new(config)?),
            now,
        })
    }

    /// What the breaker is doing right now.
    ///
    /// Read-only: observing a breaker never transitions it. A breaker whose wait has elapsed
    /// reports [`HalfOpen`](BreakerState::HalfOpen), since the next call through it will be a
    /// probe.
    pub fn state(&self) -> BreakerState {
        let now = self.now.now();
        self.lock().state(now)
    }

    /// The config this breaker was built with.
    pub fn config(&self) -> CircuitBreakerConfig {
        self.lock().config().clone()
    }

    /// Ask permission to call the dependency.
    ///
    /// Use this when [`call`](CircuitBreaker::call) doesn't fit, e.g. when success isn't simply
    /// "returned `Ok`". You must tell the returned [`Permit`] how the call went; dropping it
    /// without saying counts as a failure.
    ///
    /// # Errors
    /// Returns [`Rejected`] when the breaker is shedding load. The dependency was not called.
    pub fn permit(&self) -> Result<Permit<'_, N>, Rejected> {
        let now = self.now.now();
        // Take the transition out of the guard's scope so the tracing event is emitted with the
        // lock released. Logging under a lock is how a slow subscriber becomes a latency problem.
        let (result, transition) = {
            let mut machine = self.lock();
            machine.on_permit(now)
        };
        emit(transition);
        result.map(|(generation, probe)| Permit {
            breaker: self,
            generation,
            probe,
            armed: true,
        })
    }

    /// Run `op` if the breaker allows it, recording how it went.
    ///
    /// `Ok` counts as a success and `Err` as a failure. When that isn't the right reading (a 404
    /// says nothing about whether the service is healthy), use [`permit`](CircuitBreaker::permit)
    /// and [`Permit::ignore`].
    ///
    /// # Errors
    /// [`BreakerError::Rejected`] if the breaker shed the call, or [`BreakerError::Inner`]
    /// carrying the operation's own error.
    pub fn call<F, T, E>(&self, op: F) -> Result<T, BreakerError<E>>
    where
        F: FnOnce() -> Result<T, E>,
    {
        let permit = self.permit().map_err(BreakerError::Rejected)?;
        match op() {
            Ok(value) => {
                permit.success();
                Ok(value)
            }
            Err(err) => {
                permit.failure();
                Err(BreakerError::Inner(err))
            }
        }
    }

    /// The async twin of [`call`](CircuitBreaker::call).
    ///
    /// Takes anything that turns into a future, so a whole `retry(..)` builder can go inside one.
    /// If the returned future is dropped before it finishes (a `timeout` firing, a `select!`
    /// losing), that counts as a failure, on the grounds that a call which never came back is
    /// evidence against the dependency.
    ///
    /// # Errors
    /// [`BreakerError::Rejected`] if the breaker shed the call, or [`BreakerError::Inner`]
    /// carrying the operation's own error.
    pub async fn call_async<F, Fut, T, E>(&self, op: F) -> Result<T, BreakerError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::IntoFuture<Output = Result<T, E>>,
    {
        let permit = self.permit().map_err(BreakerError::Rejected)?;
        match op().into_future().await {
            Ok(value) => {
                permit.success();
                Ok(value)
            }
            Err(err) => {
                permit.failure();
                Err(BreakerError::Inner(err))
            }
        }
    }

    /// Recover a poisoned lock rather than propagating the panic. The breaker's state is a few
    /// integers and no user code runs while the lock is held, so there's nothing to be
    /// inconsistent; refusing to work would turn one unrelated panic into an outage.
    fn lock(&self) -> std::sync::MutexGuard<'_, Machine> {
        self.machine.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn report_success(&self, generation: u64, probe: bool) {
        // Read the clock before taking the lock, and only for probes.
        let now = probe.then(|| self.now.now());
        let transition = self.lock().on_success(generation, now);
        emit(transition);
    }

    fn report_ignored(&self, generation: u64) {
        self.lock().on_ignore(generation);
    }

    fn report_failure(&self, generation: u64) {
        let now = self.now.now();
        let transition = self.lock().on_failure(generation, now);
        emit(transition);
    }
}

/// Permission to make one call, which must report back.
///
/// Dropping it without calling [`success`](Permit::success), [`failure`](Permit::failure), or
/// [`ignore`](Permit::ignore) records a failure. That is deliberate: a call that was abandoned,
/// panicked, or hung is exactly the evidence a breaker exists to notice, and the alternative
/// (recording nothing) leaves a breaker blind to the dependency that never answers.
///
/// If you have a case where abandoning a call says nothing about the dependency, such as the
/// losing side of a `select!` or a hedged request, call [`ignore`](Permit::ignore).
#[must_use = "a `Permit` records a failure unless you report how the call went"]
#[derive(Debug)]
pub struct Permit<'a, N: Now> {
    breaker: &'a CircuitBreaker<N>,
    generation: u64,
    /// Whether this is a half-open probe. Probes are the only reports that need a timestamp, so
    /// an ordinary success stays free of clock reads.
    probe: bool,
    armed: bool,
}

impl<N: Now> Permit<'_, N> {
    /// The call succeeded.
    pub fn success(mut self) {
        self.armed = false;
        self.breaker.report_success(self.generation, self.probe);
    }

    /// The call failed, and that failure counts against the dependency.
    pub fn failure(mut self) {
        self.armed = false;
        self.breaker.report_failure(self.generation);
    }

    /// The call finished, but says nothing about whether the dependency is healthy. Nothing is
    /// recorded.
    ///
    /// A half-open probe still uses up its slot, because the slot was spent when the permit was
    /// issued. Otherwise an endless stream of ignored calls would probe a sick dependency with no
    /// backoff at all.
    pub fn ignore(mut self) {
        self.armed = false;
        self.breaker.report_ignored(self.generation);
    }
}

impl<N: Now> Drop for Permit<'_, N> {
    fn drop(&mut self) {
        if self.armed {
            self.breaker.report_failure(self.generation);
        }
    }
}

/// One event per state change, never per call. Emitted with the lock released.
fn emit(transition: Option<Transition>) {
    let Some(t) = transition else { return };
    tracing::warn!(
        target: "mettle::breaker",
        from = t.from.as_str(),
        to = t.to.as_str(),
        failures = t.failures,
        samples = t.samples,
        "circuit breaker state changed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_degenerate_configs() {
        // Each config is valid except the field under test.
        let cases: Vec<(CircuitBreakerConfig, BreakerConfigError)> = vec![
            (
                CircuitBreakerConfig {
                    failure_rate_percent: 0,
                    ..Default::default()
                },
                BreakerConfigError::FailureRateOutOfRange,
            ),
            (
                CircuitBreakerConfig {
                    failure_rate_percent: 101,
                    ..Default::default()
                },
                BreakerConfigError::FailureRateOutOfRange,
            ),
            (
                CircuitBreakerConfig {
                    window_size: 0,
                    ..Default::default()
                },
                BreakerConfigError::WindowSizeOutOfRange,
            ),
            (
                CircuitBreakerConfig {
                    window_size: MAX_WINDOW_SIZE + 1,
                    ..Default::default()
                },
                BreakerConfigError::WindowSizeOutOfRange,
            ),
            (
                CircuitBreakerConfig {
                    minimum_calls: 0,
                    ..Default::default()
                },
                BreakerConfigError::MinimumCallsOutOfRange,
            ),
            (
                // minimum_calls above window_size could never be reached.
                CircuitBreakerConfig {
                    window_size: 10,
                    minimum_calls: 11,
                    ..Default::default()
                },
                BreakerConfigError::MinimumCallsOutOfRange,
            ),
            (
                CircuitBreakerConfig {
                    wait_duration: Duration::ZERO,
                    ..Default::default()
                },
                BreakerConfigError::ZeroWaitDuration,
            ),
            (
                CircuitBreakerConfig {
                    half_open_max_calls: 0,
                    ..Default::default()
                },
                BreakerConfigError::ZeroHalfOpenMaxCalls,
            ),
        ];
        for (config, want) in cases {
            assert_eq!(validate(&config), Err(want.clone()), "for {config:?}");
        }
    }

    #[test]
    fn accepts_the_boundaries() {
        // The edges of every range must be legal, or the documented range is a lie.
        for config in [
            CircuitBreakerConfig {
                failure_rate_percent: 1,
                ..Default::default()
            },
            CircuitBreakerConfig {
                failure_rate_percent: 100,
                ..Default::default()
            },
            CircuitBreakerConfig {
                window_size: MAX_WINDOW_SIZE,
                ..Default::default()
            },
            CircuitBreakerConfig {
                window_size: 1,
                minimum_calls: 1,
                ..Default::default()
            },
            CircuitBreakerConfig::default(),
        ] {
            assert_eq!(validate(&config), Ok(()), "for {config:?}");
        }
    }

    #[test]
    fn state_tokens_are_stable() {
        // These end up as metric labels.
        assert_eq!(BreakerState::Closed.as_str(), "closed");
        assert_eq!(BreakerState::Open.as_str(), "open");
        assert_eq!(BreakerState::HalfOpen.as_str(), "half_open");
    }

    #[test]
    fn breaker_error_separates_shed_from_failed() {
        let shed: BreakerError<&str> = BreakerError::Rejected(Rejected::HalfOpenLimit);
        let failed: BreakerError<&str> = BreakerError::Inner("boom");

        assert!(!shed.is_inner() && shed.is_rejected());
        assert!(failed.is_inner() && !failed.is_rejected());
        assert_eq!(shed.inner(), None);
        assert_eq!(failed.inner(), Some(&"boom"));
        assert_eq!(shed.into_inner(), None);
        assert_eq!(failed.into_inner(), Some("boom"));
    }

    #[test]
    fn rejection_display_carries_the_wait() {
        let r = Rejected::Open {
            retry_after: Duration::from_secs(7),
        };
        assert_eq!(
            BreakerError::<&str>::Rejected(r).to_string(),
            "circuit is open, retry in 7s"
        );
    }

    #[test]
    fn accessors_need_no_bounds_on_the_error() {
        // `E` here is neither `Debug` nor `Display`.
        struct Opaque;
        let e = BreakerError::Inner(Opaque);
        assert!(e.is_inner());
        assert!(e.into_inner().is_some());
    }

    #[test]
    fn boxes_into_dyn_error_for_types_that_are_not_error() {
        // Same reasoning as `RetryError` (ADR005): these don't implement `Error`.
        let _: Box<dyn std::error::Error> = Box::new(BreakerError::Inner(String::from("oops")));
        let _: Box<dyn std::error::Error> =
            Box::new(BreakerError::<String>::Rejected(Rejected::HalfOpenLimit));
    }

    // --- the state machine, driven directly with hand-fed instants ---
    //
    // No mock clock and no threads here on purpose: the machine is pure, so the whole transition
    // table can be proved by calling functions. The locking wrapper is tested separately.

    const WAIT: Duration = Duration::from_secs(30);

    fn machine(window: u32, minimum: u32, pct: u8, half_open: u32) -> Machine {
        Machine::new(CircuitBreakerConfig {
            failure_rate_percent: pct,
            window_size: window,
            minimum_calls: minimum,
            wait_duration: WAIT,
            half_open_max_calls: half_open,
        })
        .unwrap()
    }

    fn t0() -> Instant {
        Instant::now()
    }

    /// Admit a call and report it, asserting it was admitted. Returns any transition.
    fn call(m: &mut Machine, now: Instant, ok: bool) -> Option<Transition> {
        let (permit, _) = m.on_permit(now);
        let (generation, probe) = permit.expect("expected the call to be admitted");
        if ok {
            m.on_success(generation, probe.then_some(now))
        } else {
            m.on_failure(generation, now)
        }
    }

    #[test]
    fn closed_stays_closed_below_the_threshold() {
        let mut m = machine(10, 4, 50, 1);
        let now = t0();
        for _ in 0..4 {
            assert!(call(&mut m, now, true).is_none());
        }
        // 1 failure out of 5 is 20%, under the 50% threshold.
        assert!(call(&mut m, now, false).is_none());
        assert_eq!(m.state(now), BreakerState::Closed);
    }

    #[test]
    fn minimum_calls_stops_a_one_of_one_trip() {
        // Without the gate, the first failure is a 100% failure rate and would trip instantly.
        let mut m = machine(10, 5, 50, 1);
        let now = t0();
        assert!(call(&mut m, now, false).is_none());
        assert_eq!(m.state(now), BreakerState::Closed);
        assert!(call(&mut m, now, false).is_none());
        assert_eq!(m.state(now), BreakerState::Closed);
    }

    #[test]
    fn trips_at_exactly_the_threshold_and_not_before() {
        // Boundary check: 50% of a filled window trips, one failure fewer does not. This is what
        // integer percentages buy, and it must hold exactly.
        let mut m = machine(10, 10, 50, 1);
        let now = t0();
        for _ in 0..5 {
            assert!(call(&mut m, now, true).is_none());
        }
        for _ in 0..4 {
            assert!(call(&mut m, now, false).is_none()); // 4/10 = 40%
        }
        assert_eq!(m.state(now), BreakerState::Closed);
        let t = call(&mut m, now, false).expect("the 5th failure is 50% and must trip");
        assert_eq!((t.from, t.to), (BreakerState::Closed, BreakerState::Open));
        assert_eq!((t.failures, t.samples), (5, 10));
    }

    #[test]
    fn partly_filled_window_divides_by_what_it_has() {
        // The denominator is what's in the window, not its capacity. Using capacity would make a
        // half-full window trip late, which is the difference between shedding during an incident
        // and shedding after it.
        let mut m = machine(100, 4, 50, 1);
        let now = t0();
        for _ in 0..2 {
            assert!(call(&mut m, now, true).is_none());
        }
        assert!(call(&mut m, now, false).is_none()); // 1/3
        let t = call(&mut m, now, false).expect("2 of 4 is 50% and must trip");
        assert_eq!(t.samples, 4);
    }

    #[test]
    fn open_sheds_until_the_wait_elapses() {
        let mut m = machine(4, 2, 50, 1);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false); // tripped
        assert_eq!(m.state(start), BreakerState::Open);

        let (r, transition) = m.on_permit(start + Duration::from_secs(10));
        assert_eq!(
            r,
            Err(Rejected::Open {
                retry_after: Duration::from_secs(20) // what's left, not the whole wait
            })
        );
        assert!(transition.is_none());
    }

    #[test]
    fn open_becomes_half_open_on_the_first_call_after_the_wait() {
        // Lazily, driven by a call rather than a timer, so there's no background task.
        let mut m = machine(4, 2, 50, 1);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);

        let later = start + WAIT;
        let (r, t) = m.on_permit(later);
        assert!(r.is_ok(), "the first call after the wait is the probe");
        let t = t.expect("Open -> HalfOpen is a transition");
        assert_eq!((t.from, t.to), (BreakerState::Open, BreakerState::HalfOpen));
    }

    #[test]
    fn half_open_admits_a_bounded_number_of_probes() {
        // The budget is cumulative for the period, not a concurrency limit. Handing slots back
        // would let a recovering dependency take unbounded traffic.
        let mut m = machine(4, 2, 50, 2);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);
        let later = start + WAIT;

        assert!(m.on_permit(later).0.is_ok()); // probe 1
        assert!(m.on_permit(later).0.is_ok()); // probe 2
        assert_eq!(m.on_permit(later).0, Err(Rejected::HalfOpenLimit));
    }

    #[test]
    fn half_open_closes_after_enough_successes() {
        let mut m = machine(4, 2, 50, 2);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);
        let later = start + WAIT;

        let (g1, _) = m.on_permit(later).0.unwrap();
        let (g2, _) = m.on_permit(later).0.unwrap();
        assert!(
            m.on_success(g1, Some(later)).is_none(),
            "1 of 2 successes isn't enough"
        );
        let t = m
            .on_success(g2, Some(later))
            .expect("the 2nd success closes it");
        assert_eq!(
            (t.from, t.to),
            (BreakerState::HalfOpen, BreakerState::Closed)
        );
        assert_eq!(m.state(later), BreakerState::Closed);
    }

    #[test]
    fn one_failed_probe_reopens() {
        let mut m = machine(4, 2, 50, 3);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);
        let later = start + WAIT;

        let (g, _) = m.on_permit(later).0.unwrap();
        let t = m.on_failure(g, later).expect("a failed probe re-opens");
        assert_eq!((t.from, t.to), (BreakerState::HalfOpen, BreakerState::Open));
    }

    #[test]
    fn closing_clears_the_window() {
        // Without this the breaker carries the failures that tripped it into the recovered state,
        // so the first failure after recovery re-opens it, forever. Every per-call assertion above
        // still passes without the clear, which is why this test exists.
        let mut m = machine(4, 2, 50, 1);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);

        let later = start + WAIT;
        let (g, _) = m.on_permit(later).0.unwrap();
        m.on_success(g, Some(later)).expect("closes");

        // A single failure now must not trip it: the window starts empty, so `minimum_calls`
        // isn't met.
        assert!(call(&mut m, later, false).is_none());
        assert_eq!(m.state(later), BreakerState::Closed);
    }

    #[test]
    fn a_wedged_half_open_period_reopens_after_the_wait() {
        // If probes never report back (a leaked permit, a task that vanished), the breaker must
        // not sit half-open forever. The period gets the same wait the open state does.
        let mut m = machine(4, 2, 50, 1);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);

        let probe_at = start + WAIT;
        assert!(m.on_permit(probe_at).0.is_ok()); // probe admitted, never reports
        assert_eq!(m.on_permit(probe_at).0, Err(Rejected::HalfOpenLimit));

        let stuck_until = probe_at + WAIT;
        let (r, t) = m.on_permit(stuck_until);
        assert!(matches!(r, Err(Rejected::Open { .. })));
        let t = t.expect("the wedged period re-opens");
        assert_eq!((t.from, t.to), (BreakerState::HalfOpen, BreakerState::Open));
    }

    #[test]
    fn a_slow_but_healthy_caller_stream_still_closes_the_breaker() {
        // Regression: the half-open deadline used to be anchored at the start of the period and
        // never moved, so collecting more than one probe required every probe to arrive inside a
        // single `wait_duration`. Callers arriving slower than that could never close the breaker,
        // no matter how healthy the dependency was, and it shed forever.
        let mut m = machine(100, 2, 50, 3); // needs 3 good probes to close
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);
        assert_eq!(m.state(start), BreakerState::Open);

        // One call a minute against a wait of 30s: each probe lands well after the last.
        let gap = Duration::from_secs(60);
        let mut now = start + WAIT;
        for _ in 0..3 {
            let (permit, _) = m.on_permit(now);
            let (generation, probe) = permit.expect("a healthy probe must be admitted");
            m.on_success(generation, probe.then_some(now));
            now += gap;
        }
        assert_eq!(
            m.state(now),
            BreakerState::Closed,
            "a healthy dependency must eventually close the breaker"
        );
    }

    #[test]
    fn an_idle_half_open_period_does_not_time_out() {
        // The deadline is about a probe that never came back, not about the period being quiet.
        let mut m = machine(100, 2, 50, 2);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);

        let probe_at = start + WAIT;
        let (permit, _) = m.on_permit(probe_at);
        let (generation, probe) = permit.unwrap();
        m.on_success(generation, probe.then_some(probe_at)); // answers at once, nothing in flight

        // Long quiet stretch, then the second probe. Nothing was outstanding, so the period lives.
        let much_later = probe_at + WAIT * 10;
        assert_eq!(m.state(much_later), BreakerState::HalfOpen);
        assert!(
            m.on_permit(much_later).0.is_ok(),
            "an idle period must still admit its remaining probes"
        );
    }

    #[test]
    fn a_probe_answering_after_the_deadline_reopens_rather_than_closes() {
        // Regression: `on_success` didn't check the deadline, so a probe that took longer than the
        // whole open window could close a breaker that `state()` was already reporting as `Open`.
        let mut m = machine(100, 2, 50, 1);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);

        let probe_at = start + WAIT;
        let (permit, _) = m.on_permit(probe_at);
        let (generation, probe) = permit.unwrap();
        assert!(probe, "the first call after the wait is a probe");

        let far_too_late = probe_at + WAIT * 4;
        assert_eq!(m.state(far_too_late), BreakerState::Open);
        let t = m
            .on_success(generation, Some(far_too_late))
            .expect("a very late probe must move the breaker");
        assert_eq!((t.from, t.to), (BreakerState::HalfOpen, BreakerState::Open));
        assert_eq!(m.state(far_too_late), BreakerState::Open);
    }

    #[test]
    fn an_ignored_probe_frees_its_in_flight_slot_but_not_its_budget() {
        // Ignoring says "this told us nothing", so the probe shouldn't look like one that hung.
        // The budget stays spent, or an endless stream of ignored calls probes a sick dependency
        // with no backoff at all.
        let mut m = machine(100, 2, 50, 2);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);

        let probe_at = start + WAIT;
        let (permit, _) = m.on_permit(probe_at);
        let (generation, _) = permit.unwrap();
        m.on_ignore(generation);

        // Nothing outstanding, so a long gap doesn't expire the period.
        let later = probe_at + WAIT * 3;
        assert_eq!(m.state(later), BreakerState::HalfOpen);
        assert!(m.on_permit(later).0.is_ok(), "budget slot 2 of 2");
        // Budget is spent even though one probe was ignored.
        assert_eq!(m.on_permit(later).0, Err(Rejected::HalfOpenLimit));
    }

    #[test]
    fn stale_reports_are_discarded() {
        // A permit issued before a state change must not touch the counters of the period that
        // replaced it. Without the generation check, a slow success from a dead period closes a
        // breaker that just re-opened.
        let mut m = machine(4, 2, 50, 1);
        let start = t0();
        let (stale, _) = m.on_permit(start);
        let (stale, _) = stale.unwrap();

        call(&mut m, start, false);
        call(&mut m, start, false); // now Open, generation bumped
        assert_eq!(m.state(start), BreakerState::Open);

        let later = start + WAIT;
        m.on_permit(later).0.unwrap(); // half-open probe issued
        assert_eq!(m.state(later), BreakerState::HalfOpen);

        assert!(
            m.on_success(stale, None).is_none(),
            "a success from the pre-trip period must not close the breaker"
        );
        assert_eq!(m.state(later), BreakerState::HalfOpen);
    }

    #[test]
    fn state_never_mutates() {
        // `state()` is a read-only projection. If it transitioned, merely observing a breaker
        // would change its behaviour.
        let mut m = machine(4, 2, 50, 1);
        let start = t0();
        call(&mut m, start, false);
        call(&mut m, start, false);

        let later = start + WAIT;
        for _ in 0..5 {
            assert_eq!(m.state(later), BreakerState::HalfOpen);
        }
        // Still genuinely Open underneath: the next permit is what performs the transition.
        let (r, t) = m.on_permit(later);
        assert!(r.is_ok());
        assert_eq!(t.unwrap().from, BreakerState::Open);
    }

    #[test]
    fn window_wraps_without_losing_track() {
        // Push well past `window_size` so the bitset wraps, then check it still weighs only the
        // most recent outcomes.
        let mut m = machine(8, 8, 100, 1);
        let now = t0();
        for _ in 0..50 {
            assert!(call(&mut m, now, true).is_none());
        }
        assert_eq!(m.failures(), 0);
        assert_eq!(m.filled, 8);
        // 7 failures out of 8 is under 100%.
        for _ in 0..7 {
            assert!(call(&mut m, now, false).is_none());
        }
        let t = call(&mut m, now, false).expect("8 of 8 at 100% trips");
        assert_eq!((t.failures, t.samples), (8, 8));
    }

    // --- the shared wrapper: locking, permits, and the things only concurrency can break ---

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    /// A time source that only moves when a test says so.
    #[derive(Clone)]
    struct MockNow {
        start: Instant,
        elapsed: Arc<std::sync::Mutex<Duration>>,
        calls: Arc<AtomicUsize>,
    }
    impl MockNow {
        fn new() -> Self {
            Self {
                start: Instant::now(),
                elapsed: Arc::new(std::sync::Mutex::new(Duration::ZERO)),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
        fn advance(&self, d: Duration) {
            *self.elapsed.lock().unwrap() += d;
        }
        fn calls(&self) -> usize {
            self.calls.load(SeqCst)
        }
    }
    impl Now for MockNow {
        fn now(&self) -> Instant {
            self.calls.fetch_add(1, SeqCst);
            self.start + *self.elapsed.lock().unwrap()
        }
    }

    fn breaker(half_open: u32) -> (CircuitBreaker<MockNow>, MockNow) {
        let clock = MockNow::new();
        let b = CircuitBreaker::new(
            CircuitBreakerConfig {
                failure_rate_percent: 50,
                window_size: 4,
                minimum_calls: 2,
                wait_duration: WAIT,
                half_open_max_calls: half_open,
            },
            clock.clone(),
        )
        .unwrap();
        (b, clock)
    }

    #[test]
    fn dropping_a_permit_counts_as_a_failure() {
        // The whole reason `Drop` is wired up: a call that was abandoned, panicked, or hung never
        // reports, and a breaker that ignores those is blind to the dependency that stops
        // answering, which is the outage it most needs to catch.
        let (b, _clock) = breaker(1);
        drop(b.permit().unwrap());
        assert_eq!(b.state(), BreakerState::Closed); // 1 of 1, under `minimum_calls`
        drop(b.permit().unwrap());
        assert_eq!(b.state(), BreakerState::Open);
    }

    #[test]
    fn ignore_records_nothing() {
        let (b, _clock) = breaker(1);
        for _ in 0..50 {
            b.permit().unwrap().ignore();
        }
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[test]
    fn call_maps_ok_and_err_onto_the_window() {
        let (b, _clock) = breaker(1);
        assert_eq!(b.call(|| Ok::<_, &str>(1)).unwrap(), 1);
        assert!(b.call(|| Err::<i32, _>("boom")).unwrap_err().is_inner());
        // 1 failure out of 2 samples is exactly the 50% threshold, so that trips it.
        assert_eq!(b.state(), BreakerState::Open);

        // Now shedding: the operation must not run at all.
        let ran = std::cell::Cell::new(false);
        let err = b
            .call(|| {
                ran.set(true);
                Ok::<_, &str>(1)
            })
            .unwrap_err();
        assert!(err.is_rejected());
        assert!(!ran.get(), "a shed call must not touch the dependency");
    }

    #[test]
    fn a_shed_call_does_not_feed_the_window() {
        // Rejections are not outcomes. If they counted, an open breaker would keep itself open.
        let (b, clock) = breaker(1);
        drop(b.permit().unwrap());
        drop(b.permit().unwrap());
        assert_eq!(b.state(), BreakerState::Open);

        for _ in 0..1000 {
            assert!(b.permit().is_err());
        }
        // Still exactly one wait away from probing, not pushed further out.
        clock.advance(WAIT);
        assert_eq!(b.state(), BreakerState::HalfOpen);
    }

    #[test]
    fn a_dropped_future_records_a_failure() {
        // Cancellation is the async shape of the same problem `Drop` exists for: a `timeout` or a
        // losing `select!` branch drops the future mid-call. Polled by hand, so this needs no
        // runtime and no real time.
        use std::future::Future;
        use std::task::{Context, Waker};

        let (b, _clock) = breaker(1);
        for _ in 0..2 {
            let mut cx = Context::from_waker(Waker::noop());
            let mut fut = Box::pin(b.call_async(std::future::pending::<Result<(), &str>>));
            assert!(fut.as_mut().poll(&mut cx).is_pending());
            drop(fut); // cancelled before it ever answered
        }
        assert_eq!(b.state(), BreakerState::Open);
    }

    #[test]
    fn concurrent_callers_get_exactly_one_probe() {
        // The race the half-open state exists to lose: many threads arrive the instant the wait
        // expires, and exactly one may through. Repeated, because a race that fires one time in
        // twenty still fires in production.
        for round in 0..200 {
            let (b, clock) = breaker(1);
            let b = Arc::new(b);
            drop(b.permit().unwrap());
            drop(b.permit().unwrap());
            assert_eq!(b.state(), BreakerState::Open);
            clock.advance(WAIT);

            let threads = 16;
            let gate = Arc::new(std::sync::Barrier::new(threads));
            let admitted = Arc::new(AtomicUsize::new(0));
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    let (b, gate, admitted) = (b.clone(), gate.clone(), admitted.clone());
                    std::thread::spawn(move || {
                        gate.wait();
                        if let Ok(p) = b.permit() {
                            admitted.fetch_add(1, SeqCst);
                            p.ignore();
                        }
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap();
            }
            assert_eq!(admitted.load(SeqCst), 1, "round {round}");
        }
    }

    #[test]
    fn clock_reads_do_not_scale_with_calls() {
        // One read per permit, one more per recorded failure, none on success. Checked at two
        // sizes so the slope is pinned, not just a magic number.
        for n in [5usize, 50] {
            let (b, clock) = breaker(1);
            for _ in 0..n {
                b.permit().unwrap().success();
            }
            assert_eq!(clock.calls(), n, "successes: {n}");
        }
        for n in [5usize, 50] {
            let (b, clock) = breaker(1);
            for _ in 0..n {
                b.permit().unwrap().ignore();
            }
            assert_eq!(clock.calls(), n, "ignored: {n}");
        }
    }

    #[test]
    fn emits_one_event_per_transition_and_none_per_call() {
        // A breaker that logged per call would be the loudest thing in the process during an
        // incident, which is exactly when you need the log readable.
        let events = crate::test_support::count_breaker_events();
        let (b, clock) = breaker(1);

        for _ in 0..20 {
            b.permit().unwrap().success();
        }
        assert_eq!(events.get(), 0, "steady-state success must be silent");

        drop(b.permit().unwrap());
        drop(b.permit().unwrap());
        assert_eq!(events.get(), 1, "Closed -> Open");

        for _ in 0..50 {
            assert!(b.permit().is_err());
        }
        assert_eq!(events.get(), 1, "shedding must be silent");

        clock.advance(WAIT);
        let probe = b.permit().unwrap();
        assert_eq!(events.get(), 2, "Open -> HalfOpen");
        probe.success();
        assert_eq!(events.get(), 3, "HalfOpen -> Closed");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn retry_through_an_open_breaker_gives_up_immediately() {
        // The composition this crate exists to get right. A rejection is not retryable, so
        // `.when(BreakerError::is_inner)` must end the retry on the first shed call rather than
        // sleeping through a backoff schedule for a request that never left the process.
        let (b, _clock) = breaker(1);
        drop(b.permit().unwrap());
        drop(b.permit().unwrap());
        assert_eq!(b.state(), BreakerState::Open);

        let ran = AtomicUsize::new(0);
        let out: Result<i32, _> = crate::retry(|| {
            b.call_async(|| {
                ran.fetch_add(1, SeqCst);
                std::future::ready(Ok::<i32, &str>(1))
            })
        })
        .when(BreakerError::is_inner)
        .await;

        let err = out.unwrap_err();
        assert_eq!(err.stop_reason(), crate::StopReason::NotRetryable);
        assert_eq!(err.attempts(), 1, "no backoff schedule for a shed call");
        assert!(err.error().is_rejected());
        assert_eq!(ran.load(SeqCst), 0, "the dependency was never touched");
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn call_async_records_through_the_breaker() {
        let (b, _clock) = breaker(1);
        assert_eq!(
            b.call_async(|| std::future::ready(Ok::<_, &str>(5)))
                .await
                .unwrap(),
            5
        );
        assert!(
            b.call_async(|| std::future::ready(Err::<i32, _>("boom")))
                .await
                .unwrap_err()
                .is_inner()
        );
        // 1 failure of 2 samples is the 50% threshold.
        assert_eq!(b.state(), BreakerState::Open);
    }

    #[test]
    fn breaker_is_send_and_sync() {
        // It is useless unless it can be shared, so pin that rather than discovering it later.
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<CircuitBreaker<MockNow>>();
        assert_sync::<CircuitBreaker<MockNow>>();
        assert_send::<Permit<'static, MockNow>>();
    }

    #[test]
    fn config_is_validated_through_the_machine() {
        assert_eq!(
            Machine::new(CircuitBreakerConfig {
                window_size: 0,
                ..Default::default()
            })
            .map(|_| ())
            .unwrap_err(),
            BreakerConfigError::WindowSizeOutOfRange
        );
    }
}
