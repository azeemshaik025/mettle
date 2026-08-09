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

// A mock clock is normally something the test wants to keep hold of, so it can advance time and
// read back what was slept. Without these, `.clock(&mock)` fails to compile and the mock has to
// wrap its own state in `Rc`/`Arc` just to be usable, which is friction on the exact path this
// crate exists to make easy. Use `&C` when the retry doesn't outlive the clock, `Arc<C>` when it
// does.
impl<C: Clock + ?Sized> Clock for &C {
    type Sleep = C::Sleep;

    fn now(&self) -> Instant {
        (**self).now()
    }

    fn sleep(&self, dur: Duration) -> Self::Sleep {
        (**self).sleep(dur)
    }
}

impl<C: Clock + ?Sized> Clock for std::sync::Arc<C> {
    type Sleep = C::Sleep;

    fn now(&self) -> Instant {
        (**self).now()
    }

    fn sleep(&self, dur: Duration) -> Self::Sleep {
        (**self).sleep(dur)
    }
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
