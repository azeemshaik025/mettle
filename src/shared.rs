//! Retry internals shared by the async and blocking drivers: the decision of whether to keep
//! going, how the terminal error is built, and the `tracing` events. Kept here (not in either
//! driver) so both call the same code and neither depends on the other's feature being enabled.

use crate::backoff::Backoff;
use crate::error::{RetryError, StopReason};
use std::time::Duration;

/// What to do after a failed attempt.
///
/// `Stop` carries the reason so the driver doesn't have to re-derive it, and an `elapsed` that is
/// `Some` only on the `MaxElapsed` path, where the budget check already read the clock in this
/// same call. Reusing it there keeps the give-up path at one clock read, never two.
pub(crate) enum Decision {
    Retry(Duration),
    Stop {
        reason: StopReason,
        elapsed: Option<Duration>,
    },
}

/// The retry decision, shared by both drivers so they can't diverge on semantics.
///
/// Pure: `elapsed` is a thunk, so the clock is read only when a budget is actually set.
pub(crate) fn should_retry_after<E, P, B>(
    err: &E,
    when: &P,
    backoff: &mut B,
    max_elapsed: Option<Duration>,
    elapsed: impl FnOnce() -> Duration,
) -> Decision
where
    P: Fn(&E) -> bool,
    B: Backoff,
{
    if !when(err) {
        return Decision::Stop {
            reason: StopReason::NotRetryable,
            elapsed: None,
        };
    }
    let Some(delay) = backoff.next_delay() else {
        return Decision::Stop {
            reason: StopReason::RetriesExhausted,
            elapsed: None,
        };
    };
    if let Some(budget) = max_elapsed {
        let spent = elapsed();
        if spent.saturating_add(delay) >= budget {
            // No time for another attempt. We just read the clock, so hand the value on rather
            // than making the driver read it again.
            return Decision::Stop {
                reason: StopReason::MaxElapsed,
                elapsed: Some(spent),
            };
        }
    }
    Decision::Retry(delay)
}

/// Build the terminal [`RetryError`] and emit the give-up event.
///
/// `retries` is how many retries were performed, so the attempt count is one more than that: even
/// a first error rejected by `.when(..)` means the operation ran once. Both drivers go through
/// here so that `+1`, the fallback clock read, and the event gate can't drift apart.
pub(crate) fn give_up<E>(
    error: E,
    retries: u32,
    reason: StopReason,
    measured: Option<Duration>,
    measure: impl FnOnce() -> Duration,
) -> RetryError<E> {
    let elapsed = measured.unwrap_or_else(measure);
    let attempts = retries.saturating_add(1);
    if retries > 0 {
        trace_give_up(attempts, elapsed, reason);
    }
    RetryError::new(error, attempts, elapsed, reason)
}

/// Emit a `tracing` event for one retry. Shared by both drivers so they report identically.
/// `attempt` is the 1-based number of the attempt that just failed. Fires on target
/// `mettle::retry` at `WARN`; silence it with `RUST_LOG=mettle=off`.
pub(crate) fn trace_retry<E: std::fmt::Debug>(attempt: u32, error: &E, delay: Duration) {
    tracing::warn!(
        target: "mettle::retry",
        attempt,
        delay_ms = delay.as_millis() as u64,
        error = ?error,
        "retrying after error"
    );
}

/// Emit the one event that closes out a retry that gave up, on the same target as the per-retry
/// events. Carries no error: the caller now holds it, and the last retry event already logged it.
///
/// Only fires when at least one retry happened. Without that gate, putting a `.when(..)` filter in
/// front of an HTTP client would emit a `WARN` for every 404, on a path that is silent today.
fn trace_give_up(attempts: u32, elapsed: Duration, reason: StopReason) {
    tracing::warn!(
        target: "mettle::retry",
        attempts,
        elapsed_ms = elapsed.as_millis() as u64,
        reason = reason.as_str(),
        "gave up"
    );
}
