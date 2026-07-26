//! Retrying a fallible async operation with `mettle::retry`.
//!
//! `retry(op)` runs `op`; if it returns `Err`, it backs off, waits, and runs it again, until `op`
//! succeeds, the error is non-retryable, or the retries (or time budget) run out. With no
//! configuration it uses exponential backoff: 100 ms before the first retry, doubling each time
//! (capped at 30 s), for up to 3 retries. Override any of it with the builder methods, then
//! `.await`:
//!
//! - `.when(pred)` retries only the errors `pred` accepts (default: every error)
//! - `.backoff(cfg)` swaps or tunes the backoff strategy
//! - `.max_elapsed(d)` gives up once the next wait would push total time past `d`
//! - `.clock(clock)` supplies the time source (default: Tokio)
//!
//! Run with: `cargo run --example retry`

use mettle::{ExponentialBackoff, ExponentialBackoffConfig, retry};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Duration;

#[derive(Debug)]
enum FetchError {
    Timeout,  // transient, worth retrying
    NotFound, // permanent, retrying won't help
}

#[tokio::main]
async fn main() {
    // 1) Simplest form: pass the operation, take every default.
    let result = retry(flaky_fetch).await;
    println!("1. defaults:  {result:?}"); // Ok("user data"), succeeds on attempt 3

    // 2) Retry only the errors worth retrying, so a permanent error stops at once.
    let result = retry(always_404)
        .when(|e| matches!(e, FetchError::Timeout))
        .await;
    println!("2. filtered:  {result:?}"); // Err(NotFound), no retries

    // 3) Full control: a faster backoff, a total-time budget, and an error filter.
    let result = retry(flaky_fetch)
        .backoff(fast_backoff())
        .max_elapsed(Duration::from_secs(5))
        .when(|e| matches!(e, FetchError::Timeout))
        .await;
    println!("3. full:      {result:?}"); // Ok("user data"), succeeds on attempt 3
}

/// A flaky call: fails with `Timeout` twice, then succeeds. It resets after each success, so it
/// stands in for a fresh operation in every example above.
async fn flaky_fetch() -> Result<&'static str, FetchError> {
    static ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
    if ATTEMPTS.fetch_add(1, SeqCst) < 2 {
        Err(FetchError::Timeout)
    } else {
        ATTEMPTS.store(0, SeqCst);
        Ok("user data")
    }
}

/// A call that always fails with a permanent error.
async fn always_404() -> Result<&'static str, FetchError> {
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
