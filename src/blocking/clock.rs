//! A synchronous time source for [blocking retry](super).

use std::time::{Duration, Instant};

/// A synchronous time source: read "now", and block for a duration.
///
/// Injected so tests can supply a mock that advances a *virtual* clock and returns from
/// `sleep` instantly. Open on purpose: implement it for a mock or a simulation runtime.
///
/// This is the blocking counterpart of mettle's async `Clock` trait; the two are separate
/// because the async one's `sleep` returns a `Future` and this one just blocks.
pub trait Clock {
    /// The current instant.
    fn now(&self) -> Instant;

    /// Block the current thread for `dur`.
    fn sleep(&self, dur: Duration);
}

/// A [`Clock`] backed by [`std::thread::sleep`] and [`std::time::Instant`].
#[derive(Debug, Clone, Copy, Default)]
pub struct StdClock;

impl Clock for StdClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, dur: Duration) {
        std::thread::sleep(dur);
    }
}
