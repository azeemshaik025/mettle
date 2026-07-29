//! What a retry hands back when it gives up: the last error plus the context you need to tell
//! "the dependency is down" from "our timeout is too tight".

use std::time::Duration;

/// Why a retry stopped. Each variant names the knob that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StopReason {
    /// The backoff ran out of retries (set by `.backoff(..)`).
    RetriesExhausted,
    /// The `.when(..)` predicate rejected the error, so no retry was attempted for it.
    NotRetryable,
    /// The next delay wouldn't have fit in the `.max_elapsed(..)` budget.
    MaxElapsed,
}

impl StopReason {
    /// A stable snake_case token, for metric labels and log filters. These strings are part of
    /// the API and won't change under you: `"retries_exhausted"`, `"not_retryable"`,
    /// `"max_elapsed"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RetriesExhausted => "retries_exhausted",
            Self::NotRetryable => "not_retryable",
            Self::MaxElapsed => "max_elapsed",
        }
    }
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prose = match self {
            Self::RetriesExhausted => "retries exhausted",
            Self::NotRetryable => "error was not retryable",
            Self::MaxElapsed => "time budget spent",
        };
        f.write_str(prose)
    }
}

/// A retry that gave up: the error from the last attempt, plus how many attempts it made, how
/// long it spent, and what stopped it.
///
/// Both `retry` and `blocking::retry` return this as their error type. The last error alone
/// can't tell you whether you exhausted three retries in 700 ms or burned a 30 s budget, and
/// that difference is usually the whole question during an incident.
///
/// ```
/// # use mettle::{RetryError, StopReason};
/// # fn demo(err: RetryError<std::io::Error>) {
/// eprintln!(
///     "{} after {} attempts in {:?}: {}",
///     err.stop_reason().as_str(),
///     err.attempts(),
///     err.elapsed(),
///     err.error()
/// );
/// # }
/// ```
///
/// To go back to the bare error and keep an existing `Result<T, E>` signature, use
/// [`into_error`](RetryError::into_error):
///
/// ```
/// # use mettle::RetryError;
/// # fn demo(r: Result<u32, RetryError<std::io::Error>>) -> Result<u32, std::io::Error> {
/// r.map_err(RetryError::into_error)
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct RetryError<E> {
    error: E,
    attempts: u32,
    elapsed: Duration,
    stop_reason: StopReason,
}

impl<E> RetryError<E> {
    pub(crate) fn new(error: E, attempts: u32, elapsed: Duration, stop_reason: StopReason) -> Self {
        Self {
            error,
            attempts,
            elapsed,
            stop_reason,
        }
    }

    /// The error from the final attempt. Earlier errors appear only in the per-retry `tracing`
    /// events; they aren't collected, so retrying doesn't allocate.
    pub fn error(&self) -> &E {
        &self.error
    }

    /// Take the final error back, dropping the context around it.
    pub fn into_error(self) -> E {
        self.error
    }

    /// How many times the operation actually ran. Always at least 1, including when the very
    /// first error was rejected by `.when(..)`.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// How long it took, measured on the injected `Clock` from the start of the first attempt to
    /// giving up.
    ///
    /// Under [`StopReason::MaxElapsed`] this is always strictly *less* than the budget, since the
    /// budget is what stopped the next delay from being taken. Under
    /// [`StopReason::NotRetryable`] it's just one call's latency.
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Why the retry stopped.
    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason
    }
}

impl<E: std::fmt::Display> std::fmt::Display for RetryError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "gave up after {} attempt{} in {:?} ({}): {}",
            self.attempts,
            if self.attempts == 1 { "" } else { "s" },
            self.elapsed,
            self.stop_reason,
            self.error
        )
    }
}

// No `source()`. Returning the inner error there would need `E: Error + 'static`, and that impl
// conflicts with this one (E0119), so it's one or the other. `Debug + Display` covers strictly
// more error types: `String`, `Box<dyn Error>`, and `anyhow::Error` all satisfy it and none of
// them implement `Error`. The inner error's text still rides along in `Display`, and
// `downcast_ref::<RetryError<E>>()` recovers the whole structure.
impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for RetryError<E> {}

#[cfg(test)]
mod tests {
    use super::*;

    fn err() -> RetryError<&'static str> {
        RetryError::new(
            "connection refused",
            4,
            Duration::from_millis(7150),
            StopReason::RetriesExhausted,
        )
    }

    #[test]
    fn display_carries_the_inner_error() {
        assert_eq!(
            err().to_string(),
            "gave up after 4 attempts in 7.15s (retries exhausted): connection refused"
        );
    }

    #[test]
    fn display_does_not_say_one_attempts() {
        let e = RetryError::new("nope", 1, Duration::ZERO, StopReason::NotRetryable);
        assert!(e.to_string().contains("1 attempt in"), "{e}");
    }

    #[test]
    fn stop_reason_tokens_are_stable() {
        // These strings end up as metric labels, so pin them.
        assert_eq!(StopReason::RetriesExhausted.as_str(), "retries_exhausted");
        assert_eq!(StopReason::NotRetryable.as_str(), "not_retryable");
        assert_eq!(StopReason::MaxElapsed.as_str(), "max_elapsed");
    }

    #[test]
    fn accessors_need_no_bounds_on_the_error() {
        // `E` here is deliberately neither `Debug` nor `Display`.
        struct Opaque;
        let e = RetryError::new(Opaque, 2, Duration::from_secs(1), StopReason::MaxElapsed);
        assert_eq!(e.attempts(), 2);
        assert_eq!(e.elapsed(), Duration::from_secs(1));
        assert_eq!(e.stop_reason(), StopReason::MaxElapsed);
        let Opaque = e.into_error();
    }

    #[test]
    fn boxes_into_dyn_error_for_types_that_are_not_error() {
        // The whole reason `Error` is bounded on `Debug + Display` rather than `Error + 'static`.
        // None of these implement `std::error::Error`.
        fn boxed<E: std::fmt::Debug + std::fmt::Display + 'static>(
            e: RetryError<E>,
        ) -> Box<dyn std::error::Error> {
            Box::new(e)
        }
        let _ = boxed(err());
        let _ = boxed(RetryError::new(
            String::from("oops"),
            1,
            Duration::ZERO,
            StopReason::NotRetryable,
        ));
        let _: Box<dyn std::error::Error> = Box::new(RetryError::new(
            Box::<dyn std::error::Error>::from("inner"),
            1,
            Duration::ZERO,
            StopReason::NotRetryable,
        ));
    }
}
