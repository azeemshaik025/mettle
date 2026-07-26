//! Backoff strategies: whether to retry, and how long to wait before each attempt.
//!
//! A [`Backoff`] is a stateful sequence of delays. `retry` takes it by value and drives that
//! owned instance; reuse one policy across calls by passing a fresh (or cloned) value.

use std::num::NonZeroU32;
use std::time::Duration;

// Defaults for `ExponentialBackoffConfig::default()`.
const DEFAULT_FACTOR: u32 = 2;
const DEFAULT_BASE: Duration = Duration::from_millis(100);
const DEFAULT_MAX_RETRIES: u32 = 3;
const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(30);

/// A stateful sequence of retry delays. Pure: no I/O, no sleeping, no clock reads.
///
/// A backoff is consumed as it runs, since each [`next_delay`](Backoff::next_delay) advances it,
/// so `retry` takes it **by value** and drives that owned instance. A freshly constructed value
/// must represent an un-started sequence; to reuse one policy across calls, pass a fresh (or
/// cloned) value each time.
///
/// Open on purpose: add a custom strategy by implementing it.
pub trait Backoff {
    /// Delay before the next retry, or `None` to give up (e.g. retries exhausted).
    fn next_delay(&mut self) -> Option<Duration>;
}

/// Why an [`ExponentialBackoff`] configuration was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackoffConfigError {
    /// `base` was zero, so every delay would be zero (a busy-loop).
    ZeroBase,
    /// `factor` was zero, so delays would collapse to zero (a busy-loop).
    ZeroFactor,
    /// `max_delay` was smaller than `base`, which would cap the first delay below `base`.
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
    /// Growth multiplier applied each retry (must be >= 1).
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
/// sequence. Jitter is planned; until then, wrap this in a custom [`Backoff`] if you need it.
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
        let _ = b.next_delay(); // saturating_mul must not overflow-panic
        let _ = b.next_delay();
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
}
