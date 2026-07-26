//! Retrying a fallible *synchronous* operation with `mettle::blocking::retry`.
//!
//! The sync twin of `retry` (see `examples/retry.rs`): same builder, but the operation returns a
//! plain `Result` (no `async`) and you finish with `.call()` instead of `.await`, so no runtime
//! is needed.
//!
//! Run with: `cargo run --example blocking_retry`

use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Duration;

use mettle::blocking::retry;
use mettle::{ExponentialBackoff, ExponentialBackoffConfig};

#[derive(Debug)]
enum FetchError {
    Timeout,  // transient, worth retrying
    NotFound, // permanent, retrying won't help
}

fn main() {
    // 1) Simplest form: pass the operation, take every default, and `.call()`.
    let result = retry(flaky_fetch).call();
    println!("1. defaults:  {result:?}"); // Ok("user data"), succeeds on attempt 3

    // 2) Retry only the errors worth retrying, so a permanent error stops at once.
    let result = retry(always_404)
        .when(|e| matches!(e, FetchError::Timeout))
        .call();
    println!("2. filtered:  {result:?}"); // Err(NotFound), no retries

    // 3) Full control: a faster backoff, a total-time budget, and an error filter.
    let result = retry(flaky_fetch)
        .backoff(fast_backoff())
        .max_elapsed(Duration::from_secs(5))
        .when(|e| matches!(e, FetchError::Timeout))
        .call();
    println!("3. full:      {result:?}"); // Ok("user data"), succeeds on attempt 3
}

/// A flaky call: fails with `Timeout` twice, then succeeds. It resets after each success, so it
/// stands in for a fresh operation in every example above.
fn flaky_fetch() -> Result<&'static str, FetchError> {
    static ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
    if ATTEMPTS.fetch_add(1, SeqCst) < 2 {
        Err(FetchError::Timeout)
    } else {
        ATTEMPTS.store(0, SeqCst);
        Ok("user data")
    }
}

/// A call that always fails with a permanent error.
fn always_404() -> Result<&'static str, FetchError> {
    Err(FetchError::NotFound)
}

/// A snappier exponential backoff: 20 ms first delay, defaults for the rest.
fn fast_backoff() -> ExponentialBackoff {
    ExponentialBackoff::new(ExponentialBackoffConfig {
        base: Duration::from_millis(20),
        ..Default::default()
    })
    .expect("backoff config is valid")
}
