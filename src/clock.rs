//! Time as an injected dependency, so retries can be tested without real delays.

use std::future::Future;
use std::time::{Duration, Instant};

/// The two time effects retry needs: read "now", and wait for a duration.
///
/// Injected so tests can supply a mock that advances a *virtual* clock and completes
/// sleeps instantly. Open on purpose: implement it for a mock or a simulation runtime.
pub trait Clock {
    /// The future returned by [`sleep`](Clock::sleep).
    type Sleep: Future<Output = ()>;

    /// The current instant.
    fn now(&self) -> Instant;

    /// A future that completes after `dur`.
    fn sleep(&self, dur: Duration) -> Self::Sleep;
}

/// A [`Clock`] backed by Tokio's timer, so `now` and `sleep` read the same clock
/// (and both honor `tokio::time::pause`).
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioClock;

impl Clock for TokioClock {
    type Sleep = tokio::time::Sleep;

    fn now(&self) -> Instant {
        // Tokio's clock, not `std::time::Instant::now()`, so `now` and `sleep` stay
        // consistent when time is paused or advanced (e.g. in tests).
        tokio::time::Instant::now().into_std()
    }

    fn sleep(&self, dur: Duration) -> tokio::time::Sleep {
        tokio::time::sleep(dur)
    }
}
