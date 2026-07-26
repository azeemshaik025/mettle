//! Retry internals shared by the async and blocking drivers: the decision of whether to keep
//! going, and the per-retry `tracing` event. Kept here (not in either driver) so both call the
//! same code and neither depends on the other's feature being enabled.

use crate::backoff::Backoff;
use std::time::Duration;

/// The retry decision, shared by both drivers so they can't diverge on semantics.
///
/// After a failed attempt, returns `Some(delay)` to wait `delay` and try again, or `None` to
/// give up (non-retryable error, retries exhausted, or the time budget is spent). Pure:
/// `elapsed` is a thunk, so the clock is read only when a budget is actually set.
pub(crate) fn should_retry_after<E, P, B>(
    err: &E,
    when: &P,
    backoff: &mut B,
    max_elapsed: Option<Duration>,
    elapsed: impl FnOnce() -> Duration,
) -> Option<Duration>
where
    P: Fn(&E) -> bool,
    B: Backoff,
{
    if !when(err) {
        return None; // non-retryable
    }
    let delay = backoff.next_delay()?; // None = retries exhausted
    if let Some(budget) = max_elapsed {
        if elapsed().saturating_add(delay) >= budget {
            return None; // no time for another attempt
        }
    }
    Some(delay)
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
