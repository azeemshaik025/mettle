//! A circuit breaker taken through its whole cycle: healthy, tripped, shedding, recovered.
//!
//! Run with `cargo run --example breaker`.
//!
//! The interesting part is that this finishes instantly. The breaker reads time through the `Now`
//! trait, so the example hands it a clock it controls and jumps forward a minute at a time instead
//! of sleeping. Your tests can do exactly the same thing.

use std::cell::Cell;
use std::time::{Duration, Instant};

use mettle::{BreakerError, BreakerState, CircuitBreaker, CircuitBreakerConfig, Now};

/// A clock that only moves when we tell it to. A plain struct: the breaker takes `&FakeClock`,
/// so the example keeps its own handle and can still advance time. Your tests look the same.
struct FakeClock {
    start: Instant,
    offset: Cell<Duration>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            offset: Cell::new(Duration::ZERO),
        }
    }
    fn advance(&self, d: Duration) {
        self.offset.set(self.offset.get() + d);
    }
}

impl Now for FakeClock {
    fn now(&self) -> Instant {
        self.start + self.offset.get()
    }
}

/// A dependency that is down until we say otherwise.
struct Service {
    healthy: Cell<bool>,
    calls: Cell<u32>,
}

impl Service {
    fn fetch(&self) -> Result<&'static str, &'static str> {
        self.calls.set(self.calls.get() + 1);
        if self.healthy.get() {
            Ok("payload")
        } else {
            Err("connection refused")
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let clock = FakeClock::new();
    let service = Service {
        healthy: Cell::new(false),
        calls: Cell::new(0),
    };

    // Trip after 3 of the last 4 calls fail, then shed for a minute.
    let breaker = CircuitBreaker::new(
        CircuitBreakerConfig {
            failure_rate_percent: 75,
            window_size: 4,
            minimum_calls: 4,
            wait_duration: Duration::from_secs(60),
            half_open_max_calls: 1,
        },
        &clock,
    )?;

    println!("state: {}\n", breaker.state());

    // The dependency is down. Four calls get through and fail, and the fourth trips it.
    println!("-- dependency is down --");
    for i in 1..=4 {
        let outcome = breaker.call(|| service.fetch());
        println!(
            "  call {i}: {}   state: {}",
            describe(&outcome),
            breaker.state()
        );
    }

    // Now it sheds. These never reach the service at all, which is the whole point: the
    // dependency gets a quiet window instead of a retry storm.
    println!("\n-- shedding (the service is left alone) --");
    let before = service.calls.get();
    for i in 5..=8 {
        let outcome = breaker.call(|| service.fetch());
        println!("  call {i}: {}", describe(&outcome));
    }
    println!(
        "  service saw {} of those 4 calls",
        service.calls.get() - before
    );

    // The service comes back, but the breaker doesn't know that yet, so it keeps shedding
    // until the wait is up.
    service.healthy.set(true);
    println!("\n-- service recovered, breaker hasn't noticed --");
    println!("  call 9: {}", describe(&breaker.call(|| service.fetch())));

    // Jump a minute ahead. No sleeping: this is the injected clock earning its place.
    clock.advance(Duration::from_secs(60));
    println!("\n-- a minute later --");
    println!(
        "  state: {} (one probe will be let through)",
        breaker.state()
    );

    let outcome = breaker.call(|| service.fetch());
    println!(
        "  probe: {}   state: {}",
        describe(&outcome),
        breaker.state()
    );
    assert_eq!(breaker.state(), BreakerState::Closed);

    println!(
        "\nrecovered after {} calls to the service",
        service.calls.get()
    );
    Ok(())
}

fn describe(outcome: &Result<&'static str, BreakerError<&'static str>>) -> String {
    match outcome {
        Ok(v) => format!("ok ({v})"),
        Err(e) if e.is_rejected() => format!("shed ({e})"),
        Err(e) => format!("failed ({e})"),
    }
}
