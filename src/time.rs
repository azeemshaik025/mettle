//! Reading the clock, without the sleeping.
//!
//! Both of mettle's `Clock` traits pair `now` with a `sleep`, and each is behind a
//! Cargo feature because their `sleep` differs. A circuit breaker only ever needs `now`, and one
//! breaker type has to compile whether you enabled `async`, `blocking`, or both, so it takes this
//! smaller trait instead.

use std::time::Instant;

/// A source of the current instant.
///
/// Injected for the same reason the `Clock` traits are: a breaker's whole behaviour is "how long
/// has it been since we tripped", and testing that against the real clock means sleeping. With a
/// mock, a test moves an hour forward and asserts the transition immediately.
///
/// Open on purpose, like the rest of mettle's traits. Both built-in clocks already implement it,
/// so passing `TokioClock` or `StdClock` works.
pub trait Now {
    /// The current instant.
    fn now(&self) -> Instant;
}

// A breaker holds its time source for its whole life, so without these a test can hand its mock
// over and then never touch it again: no advancing time, no asserting. `&C` when the breaker
// doesn't outlive the clock, `Arc<C>` when it does, which is the usual shape since a breaker is
// normally shared.
impl<C: Now + ?Sized> Now for &C {
    fn now(&self) -> Instant {
        (**self).now()
    }
}

impl<C: Now + ?Sized> Now for std::sync::Arc<C> {
    fn now(&self) -> Instant {
        (**self).now()
    }
}

// Concrete impls rather than a blanket `impl<C: Clock> Now for C`. Two blanket impls, one per
// `Clock` trait, overlap for any type implementing both (E0119), and every mock clock in this
// crate's tests implements both. One blanket impl would rule the other trait out entirely.
#[cfg(feature = "async")]
impl Now for crate::clock::TokioClock {
    fn now(&self) -> Instant {
        crate::clock::Clock::now(self)
    }
}

#[cfg(feature = "blocking")]
impl Now for crate::blocking::StdClock {
    fn now(&self) -> Instant {
        crate::blocking::Clock::now(self)
    }
}

// Deliberately no `SystemNow` reading `Instant::now()`, and no default type parameter on the
// breaker. A default that reads the system clock silently ignores `tokio::time::pause`, so a
// breaker built in a `#[tokio::test(start_paused = true)]` would never leave `Open` no matter how
// far the test advanced time. That is the bug the comment in `clock.rs` exists to prevent, and a
// default would reintroduce it one tool over. Making the time source an explicit argument keeps
// it visible.

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Instant);
    impl Now for Fixed {
        fn now(&self) -> Instant {
            self.0
        }
    }

    #[test]
    fn works_through_a_reference_and_an_arc() {
        // A breaker owns its time source for its whole life, so a test that can't keep a handle
        // can't advance time or assert. These impls are what make `&mock` and `Arc<mock>` usable.
        let t = Instant::now();
        let direct = Fixed(t);
        assert_eq!(direct.now(), t);
        assert_eq!(Now::now(&&direct), t);
        assert_eq!(std::sync::Arc::new(Fixed(t)).now(), t);
    }

    #[test]
    fn a_breaker_built_on_a_borrowed_clock_still_sees_time_move() {
        // The property that actually matters: the caller keeps the clock and the breaker follows
        // it. Without the `&C` impl this test cannot even be written.
        use crate::{BreakerState, CircuitBreaker, CircuitBreakerConfig};
        use std::cell::Cell;
        use std::time::Duration;

        struct Movable {
            start: Instant,
            offset: Cell<Duration>,
        }
        impl Now for Movable {
            fn now(&self) -> Instant {
                self.start + self.offset.get()
            }
        }

        let clock = Movable {
            start: Instant::now(),
            offset: Cell::new(Duration::ZERO),
        };
        let wait = Duration::from_secs(30);
        let breaker = CircuitBreaker::new(
            CircuitBreakerConfig {
                failure_rate_percent: 50,
                window_size: 4,
                minimum_calls: 2,
                wait_duration: wait,
                half_open_max_calls: 1,
            },
            &clock,
        )
        .unwrap();

        drop(breaker.permit().unwrap());
        drop(breaker.permit().unwrap());
        assert_eq!(breaker.state(), BreakerState::Open);

        clock.offset.set(wait); // the caller still owns the clock
        assert_eq!(breaker.state(), BreakerState::HalfOpen);
    }
}
