//! Backoff strategies: whether to retry, and how long to wait before each attempt.
//!
//! A [`Backoff`] is a stateful sequence of delays. `retry` takes it by value and drives that
//! owned instance; reuse one policy across calls by passing a fresh value.

use std::num::NonZeroU32;
use std::time::Duration;

// Defaults shared by both config `Default` impls; `DEFAULT_FACTOR` is exponential-only.
const DEFAULT_FACTOR: u32 = 2;
const DEFAULT_BASE: Duration = Duration::from_millis(100);
const DEFAULT_MAX_RETRIES: u32 = 3;
const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(30);

/// The multiplier in decorrelated jitter's `rand(base, prev * 3)`. Part of the published
/// algorithm, not a tuning knob.
const DECORRELATED_MULTIPLIER: u32 = 3;

/// A stateful sequence of retry delays. Pure: no I/O, no sleeping, no clock reads.
///
/// A backoff is consumed as it runs, since each [`next_delay`](Backoff::next_delay) advances it,
/// so `retry` takes it **by value** and drives that owned instance. A freshly constructed value
/// must represent an un-started sequence, so reuse one policy across calls by passing a fresh
/// value each time. A deterministic strategy such as [`ExponentialBackoff`] can be cloned
/// instead; the randomized ones are deliberately not [`Clone`], since a copy would replay the
/// same delays.
///
/// Open on purpose: add a custom strategy by implementing it.
pub trait Backoff {
    /// Delay before the next retry, or `None` to give up (e.g. retries exhausted).
    fn next_delay(&mut self) -> Option<Duration>;

    /// Wrap this strategy so every delay becomes a uniform random value in `0 ..= delay`, seeding
    /// the RNG from entropy.
    ///
    /// This is "full jitter" from AWS's *Exponential Backoff and Jitter*. Composes with any
    /// strategy, including one you wrote, which is the point: reach for it to stop a fleet of
    /// clients retrying in lockstep.
    ///
    /// ```
    /// use mettle::{Backoff, ExponentialBackoff};
    ///
    /// let mut backoff = ExponentialBackoff::default().jittered();
    /// let delay = backoff.next_delay(); // somewhere in 0 ..= 100ms
    /// # let _ = delay;
    /// ```
    ///
    /// Because the floor is zero, a retry can fire almost immediately. That is the mechanism, not
    /// a flaw: it is what lets a freed-up dependency be picked up at once. If you need a floor
    /// under every wait, use [`DecorrelatedBackoff`] instead, which never goes below its `base`.
    fn jittered(self) -> Jittered<Self>
    where
        Self: Sized,
    {
        Jittered::new(self)
    }
}

/// Why a backoff configuration was rejected. Not every strategy can produce every variant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackoffConfigError {
    /// `base` was zero, so every delay would be zero (a busy-loop).
    ZeroBase,
    /// `factor` was zero, so delays would collapse to zero (a busy-loop).
    ZeroFactor,
    /// `max_delay` was smaller than `base`, which would cap delays below `base`.
    MaxDelayBelowBase,
}

impl std::fmt::Display for BackoffConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroBase => write!(f, "backoff `base` must be non-zero"),
            Self::ZeroFactor => write!(f, "backoff `factor` must be at least 1"),
            Self::MaxDelayBelowBase => write!(f, "backoff `max_delay` must be >= `base`"),
        }
    }
}

impl std::error::Error for BackoffConfigError {}

/// Parameters for [`ExponentialBackoff::new`]. Fill only what differs from [`Default`]:
///
/// ```
/// # use mettle::ExponentialBackoffConfig;
/// let _ = ExponentialBackoffConfig { max_retries: 8, ..Default::default() };
/// ```
#[derive(Debug, Clone)]
pub struct ExponentialBackoffConfig {
    /// Growth multiplier applied each retry (must be >= 1). Set it to `1` for a constant delay.
    pub factor: u32,
    /// Delay before the first retry (must be non-zero).
    pub base: Duration,
    /// Number of retries; `0` means one attempt, no retries
    /// (total attempts = `max_retries + 1`).
    pub max_retries: u32,
    /// Upper bound on any single delay (must be >= `base`).
    pub max_delay: Duration,
}

impl Default for ExponentialBackoffConfig {
    /// Sensible defaults: ×2 growth, 100 ms base, 3 retries, 30 s cap.
    fn default() -> Self {
        Self {
            factor: DEFAULT_FACTOR,
            base: DEFAULT_BASE,
            max_retries: DEFAULT_MAX_RETRIES,
            max_delay: DEFAULT_MAX_DELAY,
        }
    }
}

/// Exponential backoff: the delay grows by `factor` each retry, capped at `max_delay`, for at
/// most `max_retries` retries.
///
/// Construct via [`new`](ExponentialBackoff::new), which validates an
/// [`ExponentialBackoffConfig`]; a zero `base` or `factor` would cause a zero-delay busy-loop, so
/// those configs are rejected up front. A freshly built value is an un-started sequence, and
/// `retry` consumes it by value, so pass a fresh (or cloned) one to run the same policy again.
///
/// Delays are deterministic: no jitter is applied, so a given config always yields the same
/// sequence. Add randomness by wrapping it with [`Backoff::jittered`], for example
/// `ExponentialBackoff::default().jittered()`.
#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    factor: NonZeroU32,
    max_delay: Duration,
    next: Duration,    // the next delay to hand out; starts at `base`
    retries_left: u32, // starts at `max_retries`
}

impl ExponentialBackoff {
    /// Validate an [`ExponentialBackoffConfig`] into a ready-to-run backoff.
    ///
    /// # Errors
    /// Returns [`BackoffConfigError`] if the config would produce a degenerate
    /// (e.g. zero-delay) sequence.
    pub fn new(config: ExponentialBackoffConfig) -> Result<Self, BackoffConfigError> {
        let ExponentialBackoffConfig {
            factor,
            base,
            max_retries,
            max_delay,
        } = config;
        if base.is_zero() {
            return Err(BackoffConfigError::ZeroBase);
        }
        let factor = NonZeroU32::new(factor).ok_or(BackoffConfigError::ZeroFactor)?;
        if max_delay < base {
            return Err(BackoffConfigError::MaxDelayBelowBase);
        }
        Ok(Self {
            factor,
            max_delay,
            next: base,
            retries_left: max_retries,
        })
    }
}

impl Default for ExponentialBackoff {
    /// Sensible defaults: 100 ms base, ×2 growth, 30 s cap, 3 retries.
    fn default() -> Self {
        Self::new(ExponentialBackoffConfig::default())
            .expect("default exponential-backoff config is valid")
    }
}

impl Backoff for ExponentialBackoff {
    fn next_delay(&mut self) -> Option<Duration> {
        if self.retries_left == 0 {
            return None; // retries exhausted — give up
        }
        self.retries_left -= 1;

        // Hand out the current delay (capped); then grow it for next time.
        // `saturating_mul` so a large factor caps at `Duration::MAX` instead of panicking.
        let delay = self.next.min(self.max_delay);
        self.next = self
            .next
            .saturating_mul(self.factor.get())
            .min(self.max_delay);
        Some(delay)
        // Deterministic — no jitter, so a given config always yields the same delays.
    }
}

/// A uniform random `Duration` in `lo ..= hi`, or `lo` if `hi <= lo`. Computed in `u64`
/// nanoseconds; real delays sit well below that bound, and larger inputs saturate to it.
fn rand_duration(rng: &mut fastrand::Rng, lo: Duration, hi: Duration) -> Duration {
    let lo = lo.as_nanos().min(u64::MAX as u128) as u64;
    let hi = hi.as_nanos().min(u64::MAX as u128) as u64;
    if hi <= lo {
        return Duration::from_nanos(lo);
    }
    Duration::from_nanos(lo + rng.u64(0..=(hi - lo)))
}

/// Any [`Backoff`] wrapped so each delay becomes a uniform random value in `0 ..= delay`.
///
/// This is "full jitter": the widest spread, so the density of clients attempting at any instant
/// is as low as it can be. There is no mode to choose. AWS's own measurements had the alternative
/// ("equal jitter", a floor at `delay/2`) doing more work *and* finishing later, so shipping it as
/// an option would only invite people to pick the worse one.
///
/// Want a floor under every wait? That's [`DecorrelatedBackoff`], which never draws below its
/// `base`. Note the trade: a floor means never retrying sooner than `base`, so a dependency that
/// frees up early isn't picked up until then.
///
/// The inner strategy stays deterministic; only this layer is random. Its RNG is seedable with
/// [`with_seed`](Jittered::with_seed) so jittered retries stay reproducible in tests. Usually
/// built with [`Backoff::jittered`] rather than named directly.
///
/// Not [`Clone`] on purpose: a copy would carry the RNG state and replay the same delays, which
/// is the lockstep jitter exists to prevent. Wrap a fresh inner strategy instead.
#[derive(Debug)]
pub struct Jittered<B> {
    inner: B,
    rng: fastrand::Rng,
}

impl<B> Jittered<B> {
    /// Wrap `inner`, seeding the RNG from entropy.
    pub fn new(inner: B) -> Self {
        Self {
            inner,
            rng: fastrand::Rng::new(),
        }
    }

    /// Wrap `inner` with a fixed `seed`, for reproducible tests.
    pub fn with_seed(inner: B, seed: u64) -> Self {
        Self {
            inner,
            rng: fastrand::Rng::with_seed(seed),
        }
    }
}

impl<B: Backoff> Backoff for Jittered<B> {
    fn next_delay(&mut self) -> Option<Duration> {
        let delay = self.inner.next_delay()?;
        Some(rand_duration(&mut self.rng, Duration::ZERO, delay))
    }
}

/// Parameters for [`DecorrelatedBackoff::new`]. Fill only what differs from [`Default`]:
///
/// ```
/// # use mettle::DecorrelatedBackoffConfig;
/// let _ = DecorrelatedBackoffConfig { max_retries: 8, ..Default::default() };
/// ```
#[derive(Debug, Clone)]
pub struct DecorrelatedBackoffConfig {
    /// Lower bound on every delay, and where the first draw starts from (must be non-zero).
    pub base: Duration,
    /// Number of retries; `0` means one attempt, no retries
    /// (total attempts = `max_retries + 1`).
    pub max_retries: u32,
    /// Upper bound on any single delay (must be >= `base`).
    pub max_delay: Duration,
}

impl Default for DecorrelatedBackoffConfig {
    /// Sensible defaults: 100 ms base, 3 retries, 30 s cap.
    fn default() -> Self {
        Self {
            base: DEFAULT_BASE,
            max_retries: DEFAULT_MAX_RETRIES,
            max_delay: DEFAULT_MAX_DELAY,
        }
    }
}

/// Decorrelated jitter: each delay is drawn uniformly from `base ..= prev * 3`, capped at
/// `max_delay`, for at most `max_retries` retries. The formula is the one from AWS's
/// "Exponential Backoff and Jitter".
///
/// ```
/// use mettle::{Backoff, DecorrelatedBackoff, DecorrelatedBackoffConfig};
///
/// let mut backoff = DecorrelatedBackoff::new(DecorrelatedBackoffConfig::default())?;
/// let delay = backoff.next_delay(); // somewhere in 100ms ..= 300ms
/// # let _ = delay;
/// # Ok::<_, mettle::BackoffConfigError>(())
/// ```
///
/// This is a strategy of its own rather than something [`Jittered`] could produce, because the
/// randomness lives in the recurrence: each range is set by the delay that was actually drawn last
/// time, so there is no deterministic sequence underneath to wrap.
///
/// One thing differs from [`ExponentialBackoff`]: the first delay is already random, somewhere in
/// `base ..= base * 3`, rather than exactly `base`. Against a jittered exponential, the difference
/// is the floor. Every delay here is at least `base`, where [`Backoff::jittered`] can return anything
/// down to zero. Don't stack the two by calling [`jittered`](Backoff::jittered) on this: the
/// randomness is already in the recurrence, and wrapping it throws the `base` floor away.
///
/// Build it with [`new`](DecorrelatedBackoff::new), or [`with_seed`](DecorrelatedBackoff::with_seed)
/// to make the delays reproducible in tests. Not [`Clone`] on purpose: a copy would carry the RNG
/// state and replay the same delays, which is the lockstep this strategy exists to prevent. Keep
/// the [`DecorrelatedBackoffConfig`] around and build a fresh one per call instead.
#[derive(Debug)]
pub struct DecorrelatedBackoff {
    base: Duration,
    max_delay: Duration,
    prev: Duration, // the last delay handed out; starts at `base`
    retries_left: u32,
    rng: fastrand::Rng,
}

impl DecorrelatedBackoff {
    /// Validate a [`DecorrelatedBackoffConfig`] into a ready-to-run backoff, seeding the RNG from
    /// entropy.
    ///
    /// # Errors
    /// Returns [`BackoffConfigError`] if the config would produce a degenerate
    /// (e.g. zero-delay) sequence.
    pub fn new(config: DecorrelatedBackoffConfig) -> Result<Self, BackoffConfigError> {
        Self::build(config, fastrand::Rng::new())
    }

    /// As [`new`](DecorrelatedBackoff::new), but with a fixed `seed`, for reproducible tests. A
    /// seed replays the same delays within a build; the exact values are not part of this crate's
    /// API contract.
    ///
    /// # Errors
    /// Returns [`BackoffConfigError`] if the config would produce a degenerate
    /// (e.g. zero-delay) sequence.
    pub fn with_seed(
        config: DecorrelatedBackoffConfig,
        seed: u64,
    ) -> Result<Self, BackoffConfigError> {
        Self::build(config, fastrand::Rng::with_seed(seed))
    }

    fn build(
        config: DecorrelatedBackoffConfig,
        rng: fastrand::Rng,
    ) -> Result<Self, BackoffConfigError> {
        let DecorrelatedBackoffConfig {
            base,
            max_retries,
            max_delay,
        } = config;
        if base.is_zero() {
            // Zero is absorbing here: prev = 0 makes every later range [0, 0] too.
            return Err(BackoffConfigError::ZeroBase);
        }
        if max_delay < base {
            return Err(BackoffConfigError::MaxDelayBelowBase);
        }
        Ok(Self {
            base,
            max_delay,
            prev: base,
            retries_left: max_retries,
            rng,
        })
    }
}

impl Backoff for DecorrelatedBackoff {
    fn next_delay(&mut self) -> Option<Duration> {
        if self.retries_left == 0 {
            return None; // retries exhausted — give up
        }
        self.retries_left -= 1;

        // Draw from [base, prev * 3], then cap, which is the formula as published.
        // `saturating_mul` so an enormous `prev` pins the top of the range at `Duration::MAX`
        // instead of panicking.
        let hi = self.prev.saturating_mul(DECORRELATED_MULTIPLIER);
        let delay = rand_duration(&mut self.rng, self.base, hi)
            .min(self.max_delay)
            // `.min` alone can't drop below `base`, since `max_delay >= base` is validated. The
            // `.max` is for a `base` past the u64-nanosecond range `rand_duration` works in,
            // where the draw itself saturates low. Absurd as an input, but it keeps the
            // "every delay is at least `base`" guarantee total rather than almost-total.
            .max(self.base);
        self.prev = delay; // the capped value seeds the next draw
        Some(delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn exp(base: u64, factor: u32, max_delay: u64, max_retries: u32) -> ExponentialBackoff {
        ExponentialBackoff::new(ExponentialBackoffConfig {
            factor,
            base: secs(base),
            max_retries,
            max_delay: secs(max_delay),
        })
        .unwrap()
    }

    #[test]
    fn grows_exponentially_then_gives_up() {
        let mut b = exp(1, 2, 100, 5);
        assert_eq!(b.next_delay(), Some(secs(1)));
        assert_eq!(b.next_delay(), Some(secs(2)));
        assert_eq!(b.next_delay(), Some(secs(4)));
        assert_eq!(b.next_delay(), Some(secs(8)));
        assert_eq!(b.next_delay(), Some(secs(16)));
        assert_eq!(b.next_delay(), None); // 5 retries used up
    }

    #[test]
    fn factor_of_one_holds_the_delay_constant() {
        // Documented on `ExponentialBackoffConfig::factor`, and the reason there's no separate
        // `ConstantBackoff` type.
        assert_eq!(drain(exp(7, 1, 100, 4)), vec![secs(7); 4]);
    }

    #[test]
    fn delay_is_capped_at_max() {
        let mut b = exp(10, 10, 30, 4);
        assert_eq!(b.next_delay(), Some(secs(10)));
        assert_eq!(b.next_delay(), Some(secs(30))); // 100 → capped to 30
        assert_eq!(b.next_delay(), Some(secs(30))); // stays capped
    }

    #[test]
    fn zero_retries_means_one_attempt() {
        let mut b = exp(1, 2, 100, 0);
        assert_eq!(b.next_delay(), None); // no retries — caller makes exactly one attempt
    }

    #[test]
    fn huge_factor_does_not_panic() {
        let mut b = ExponentialBackoff::new(ExponentialBackoffConfig {
            factor: u32::MAX,
            base: Duration::from_secs(u64::MAX / 2),
            max_retries: 3,
            max_delay: Duration::MAX,
        })
        .unwrap();
        // `saturating_mul` must not overflow-panic, and must saturate rather than wrap to zero:
        // a zero delay here would be the busy-loop the config validation exists to prevent.
        assert!(b.next_delay().unwrap() > Duration::ZERO);
        assert!(b.next_delay().unwrap() > Duration::ZERO);
    }

    #[test]
    fn rejects_degenerate_configs() {
        // Each config is valid except the one field under test (defaults fill the rest).
        assert!(matches!(
            ExponentialBackoff::new(ExponentialBackoffConfig {
                base: secs(0),
                ..Default::default()
            }),
            Err(BackoffConfigError::ZeroBase)
        ));
        assert!(matches!(
            ExponentialBackoff::new(ExponentialBackoffConfig {
                factor: 0,
                ..Default::default()
            }),
            Err(BackoffConfigError::ZeroFactor)
        ));
        assert!(matches!(
            ExponentialBackoff::new(ExponentialBackoffConfig {
                base: secs(10),
                max_delay: secs(5),
                ..Default::default()
            }),
            Err(BackoffConfigError::MaxDelayBelowBase)
        ));
    }

    // --- jitter ---

    fn exp6() -> ExponentialBackoff {
        exp(1, 2, 100, 6) // underlying: 1, 2, 4, 8, 16, 32 (seconds)
    }

    fn drain(mut b: impl Backoff) -> Vec<Duration> {
        std::iter::from_fn(move || b.next_delay()).collect()
    }

    #[test]
    fn full_jitter_stays_within_bounds() {
        // Full jitter: every delay lies in [0, the underlying delay], and the sequence still ends
        // exactly when the inner strategy is exhausted.
        let plain = drain(exp6());
        let mut j = Jittered::with_seed(exp6(), 42);
        for p in &plain {
            let d = j.next_delay().unwrap();
            assert!(
                d <= *p,
                "full jitter exceeded the base delay: {d:?} > {p:?}"
            );
        }
        assert_eq!(j.next_delay(), None);
    }

    #[test]
    fn seed_makes_jitter_reproducible() {
        // Same seed yields an identical sequence (so jittered retries stay testable); different
        // seeds generally differ, which guards against a constant or broken RNG.
        let seq = |seed| drain(Jittered::with_seed(exp6(), seed));
        assert_eq!(seq(7), seq(7));
        assert_ne!(seq(1), seq(2));
    }

    #[test]
    fn jitter_actually_moves_the_delay() {
        // `d <= p` alone is satisfied by the identity function, so pin that jitter really
        // randomizes: across the sequence it must land both below and above the halfway mark.
        let jittered = drain(Jittered::with_seed(exp6(), 5));
        let plain = drain(exp6());
        assert!(
            jittered.iter().zip(&plain).any(|(d, p)| *d < *p / 2),
            "jitter never dropped below half the plain delay"
        );
        assert!(
            jittered.iter().zip(&plain).any(|(d, p)| *d > *p / 2),
            "jitter never rose above half the plain delay"
        );
    }

    #[test]
    fn jittered_combinator_wraps_the_inner_strategy() {
        // `Backoff::jittered` is the documented entry point, so drive it rather than only the
        // `Jittered::` constructors. It must bound each delay by the inner strategy's own value
        // and end exactly when the inner one does.
        let mut b = exp6().jittered();
        let plain = drain(exp6());
        for p in &plain {
            let d = b.next_delay().unwrap();
            assert!(
                d <= *p,
                "combinator exceeded the inner delay: {d:?} for {p:?}"
            );
        }
        assert_eq!(b.next_delay(), None);
    }

    #[test]
    fn unseeded_jitter_differs_between_instances() {
        // The entropy-seeded path is what stops a fleet retrying in lockstep, and it's the reason
        // these types aren't `Clone`. A regression to a fixed seed would pass every other test
        // here. Two independent RNGs colliding across six delays is a 2^-64 event.
        assert_ne!(drain(exp6().jittered()), drain(exp6().jittered()));
    }

    #[test]
    fn jitter_handles_zero_and_extreme_delays() {
        // A custom strategy can hand back zero or enormous delays; jitter must not panic.
        struct Fixed(std::vec::IntoIter<Duration>);
        impl Backoff for Fixed {
            fn next_delay(&mut self) -> Option<Duration> {
                self.0.next()
            }
        }
        let inner = Fixed(vec![Duration::ZERO, Duration::MAX, secs(1)].into_iter());
        let mut j = Jittered::with_seed(inner, 1);
        assert_eq!(j.next_delay(), Some(Duration::ZERO)); // rand(0..=0)
        let _ = j.next_delay().unwrap(); // Duration::MAX saturates, no panic
        assert!(j.next_delay().unwrap() <= secs(1));
        assert_eq!(j.next_delay(), None);
    }

    // --- decorrelated jitter ---

    fn dec(base: u64, max_delay: u64, max_retries: u32, seed: u64) -> DecorrelatedBackoff {
        DecorrelatedBackoff::with_seed(
            DecorrelatedBackoffConfig {
                base: secs(base),
                max_retries,
                max_delay: secs(max_delay),
            },
            seed,
        )
        .unwrap()
    }

    #[test]
    fn decorrelated_draws_each_delay_from_the_previous_one() {
        // The recurrence, checked step by step across seeds: every delay sits in
        // [base, min(cap, prev * 3)]. The `base` floor is what stops it collapsing toward zero,
        // and it's the first non-zero `lo` anything passes to `rand_duration`.
        let (base, cap) = (secs(1), secs(30));
        let mut ever_above_double = false;
        for seed in 0..16 {
            let mut prev = base;
            let mut b = dec(1, 30, 40, seed);
            while let Some(d) = b.next_delay() {
                let hi = (prev * 3).min(cap);
                assert!(
                    d >= base && d <= hi,
                    "{d:?} outside [{base:?}, {hi:?}] (seed {seed})"
                );
                ever_above_double |= d > prev * 2 && d < cap;
                prev = d;
            }
        }
        // That bound is one-sided, so a smaller multiplier would satisfy it too. Only a range that
        // really runs to `prev * 3` can land a delay past `prev * 2`.
        assert!(ever_above_double, "no delay ever exceeded `prev * 2`");
    }

    #[test]
    fn decorrelated_feeds_the_capped_delay_into_the_next_draw() {
        // `prev` must be the delay handed out, not the raw draw. Feeding the uncapped draw back in
        // lets `prev` grow without bound, so the sequence pins to the cap and stops coming down.
        // Every per-delay bound still holds under that mutation, so what separates the two is how
        // often the sequence recovers below the cap.
        let cap = secs(2);
        let (mut below, mut total) = (0u32, 0u32);
        for seed in 0..32 {
            for d in drain(dec(1, 2, 32, seed)) {
                below += u32::from(d < cap);
                total += 1;
            }
        }
        // Feeding back the capped value holds this near 20%. Feeding back the raw draw collapses
        // it to roughly 3%, since `prev` runs away after a handful of retries.
        assert!(
            below * 10 >= total,
            "sequence stopped recovering below the cap: {below}/{total}"
        );
    }

    #[test]
    fn unseeded_decorrelated_differs_between_instances() {
        // Same reasoning as the jitter twin: the entropy-seeded path is the whole reason this type
        // isn't `Clone`, and a regression to a fixed seed would pass every other test here.
        let cfg = || DecorrelatedBackoffConfig {
            base: secs(1),
            max_retries: 8,
            max_delay: secs(60),
        };
        assert_ne!(
            drain(DecorrelatedBackoff::new(cfg()).unwrap()),
            drain(DecorrelatedBackoff::new(cfg()).unwrap())
        );
    }

    #[test]
    fn decorrelated_first_delay_is_already_random() {
        // Unlike `ExponentialBackoff`, whose first delay is exactly `base`, `prev` starts at
        // `base` so the very first delay is drawn from [base, base * 3].
        let firsts: Vec<_> = (0..16)
            .map(|seed| dec(1, 1000, 1, seed).next_delay().unwrap())
            .collect();
        assert!(firsts.iter().all(|d| *d >= secs(1) && *d <= secs(3)));
        assert!(
            firsts.iter().any(|d| *d != secs(1)),
            "first delay never moved off `base`"
        );
    }

    #[test]
    fn decorrelated_gives_up_after_max_retries() {
        assert_eq!(drain(dec(1, 100, 5, 3)).len(), 5);
        assert_eq!(dec(1, 100, 0, 3).next_delay(), None); // no retries — exactly one attempt
    }

    #[test]
    fn decorrelated_seed_is_reproducible() {
        // Same seed, same sequence, so a jittered retry stays testable; different seeds differ,
        // which guards against a constant or broken RNG.
        let seq = |seed| drain(dec(1, 60, 8, seed));
        assert_eq!(seq(7), seq(7));
        assert_ne!(seq(1), seq(2));
    }

    #[test]
    fn decorrelated_reaches_the_cap() {
        // The cap applies after the draw, so `max_delay` is a value the sequence actually hits
        // rather than an asymptote it approaches.
        let hit = (0..32).any(|seed| drain(dec(1, 4, 24, seed)).contains(&secs(4)));
        assert!(hit, "cap was never reached");
    }

    #[test]
    fn decorrelated_base_equal_to_cap_is_constant() {
        // Degenerate but legal: every draw is capped straight back to `base`, so the RNG can't
        // move it.
        assert_eq!(drain(dec(5, 5, 4, 12345)), vec![secs(5); 4]);
    }

    #[test]
    fn decorrelated_huge_base_does_not_panic() {
        let mut b = DecorrelatedBackoff::with_seed(
            DecorrelatedBackoffConfig {
                base: Duration::from_secs(u64::MAX / 2),
                max_retries: 3,
                max_delay: Duration::MAX,
            },
            1,
        )
        .unwrap();
        // `prev * 3` must saturate rather than overflow-panic. This `base` is also past the
        // u64-nanosecond range `rand_duration` works in, where the draw saturates low, so it
        // pins the `.max(base)` that keeps the "every delay is at least `base`" guarantee total.
        let base = Duration::from_secs(u64::MAX / 2);
        assert!(b.next_delay().unwrap() >= base);
        assert!(b.next_delay().unwrap() >= base);
    }

    #[test]
    fn decorrelated_rejects_degenerate_configs() {
        assert!(matches!(
            DecorrelatedBackoff::new(DecorrelatedBackoffConfig {
                base: secs(0),
                ..Default::default()
            }),
            Err(BackoffConfigError::ZeroBase)
        ));
        assert!(matches!(
            DecorrelatedBackoff::new(DecorrelatedBackoffConfig {
                base: secs(10),
                max_delay: secs(5),
                ..Default::default()
            }),
            Err(BackoffConfigError::MaxDelayBelowBase)
        ));
    }
}
