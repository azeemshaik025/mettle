//! # mettle
//!
//! A resilience toolkit for Rust: composable, testable primitives for handling failure, so you
//! don't hand-roll retry-and-backoff logic in every project.
//!
//! Two tools: `retry()` (async) and `blocking::retry()` (sync) with configurable backoff and
//! optional jitter, and a `CircuitBreaker` that sheds load once a dependency is already failing.
//! They are meant to be used together, since retry on its own amplifies an outage.
//!
//! # Quickstart
//!
//! Retry an async operation with sensible defaults (exponential backoff, up to 3 retries):
//!
//! ```no_run
//! # #[cfg(feature = "async")]
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! use mettle::retry;
//!
//! let value = retry(|| async { fetch().await }).await?;
//! # let _ = value;
//! # Ok(())
//! # }
//! # #[cfg(feature = "async")]
//! # async fn fetch() -> Result<u32, std::io::Error> { Ok(1) }
//! ```
//!
//! Override the backoff, clock, retry predicate (`.when`), or time budget (`.max_elapsed`) with
//! the builder methods, then `.await`. No async runtime? The blocking twin is identical but ends
//! in `.call()` instead of `.await`.
//!
//! # Jitter
//!
//! A fixed schedule means every client that failed together retries together, so a service coming
//! back up gets a synchronized wave. Two ways to spread that out, both opt-in:
//!
//! ```
//! use mettle::{Backoff, DecorrelatedBackoff, DecorrelatedBackoffConfig, ExponentialBackoff};
//!
//! // Randomize any strategy's delays into `0 ..= delay` ("full jitter").
//! let spread = ExponentialBackoff::default().jittered();
//!
//! // Or draw each delay from the previous one, never below `base`.
//! let floored = DecorrelatedBackoff::new(DecorrelatedBackoffConfig::default())?;
//! # let _ = (spread, floored);
//! # Ok::<_, mettle::BackoffConfigError>(())
//! ```
//!
//! [`jittered`](Backoff::jittered) wraps any strategy, including one you wrote.
//! [`DecorrelatedBackoff`] is its own strategy and keeps a floor under every wait, at the cost of
//! never retrying sooner than `base`. Both seed from entropy; use
//! [`jittered_with_seed`](Backoff::jittered_with_seed) or
//! [`DecorrelatedBackoff::with_seed`] when a test needs the delays to repeat.
//!
//! # When it fails
//!
//! Both APIs fail with a [`RetryError<E>`](RetryError): the last error, plus how many attempts ran,
//! how long they took, and a [`StopReason`] saying which limit stopped it. During an incident that
//! difference is usually the whole question, since the last error alone can't tell you whether you
//! burned three retries in 700 ms or a 30 s budget.
//!
//! It `?`s straight into `Box<dyn Error>` and `anyhow::Error`. To go back to the bare error, use
//! `.map_err(RetryError::into_error)`.
//!
//! # Circuit breaking
//!
//! A [`CircuitBreaker`] is shared across callers, so one caller's failures protect the rest. Put
//! it *inside* the retry, and tell the retry that a shed call is not worth retrying:
//!
//! ```no_run
//! # #[cfg(feature = "async")]
//! # async fn demo(breaker: &mettle::CircuitBreaker<mettle::clock::TokioClock>)
//! # -> Result<(), Box<dyn std::error::Error>> {
//! # async fn fetch() -> Result<u32, std::io::Error> { Ok(1) }
//! use mettle::{retry, BreakerError};
//!
//! let value = retry(|| breaker.call_async(fetch))
//!     .when(BreakerError::is_inner)
//!     .await?;
//! # let _ = value;
//! # Ok(())
//! # }
//! ```
//!
//! The other order (breaker outside the retry) looks equivalent and isn't: the breaker would see
//! one outcome per retry schedule instead of one per attempt, which hides the amplification it
//! exists to stop.
//!
//! # Observability
//!
//! Every retry emits a [`tracing`](https://docs.rs/tracing) event on target `mettle::retry` at
//! `WARN`, carrying the `attempt` number, `delay_ms`, and the `error`. If at least one retry
//! happened, giving up emits one more on the same target with `attempts`, `elapsed_ms`, and
//! `reason`. The breaker emits one on `mettle::breaker` per state change, never per call, with
//! `from`, `to`, `failures`, and `samples`. Install any subscriber to see them, filter with
//! `RUST_LOG=mettle::retry=warn`, or silence with `RUST_LOG=mettle=off`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(not(any(feature = "async", feature = "blocking")))]
compile_error!("enable at least one of the `async` or `blocking` features");

pub mod backoff;
pub mod breaker;
pub mod error;
pub mod time;

#[cfg(feature = "blocking")]
#[cfg_attr(docsrs, doc(cfg(feature = "blocking")))]
pub mod blocking;
#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
pub mod clock;
#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
pub mod retry;

#[cfg(any(feature = "async", feature = "blocking"))]
mod shared;

#[cfg(test)]
mod test_support;

pub use backoff::{
    Backoff, BackoffConfigError, DecorrelatedBackoff, DecorrelatedBackoffConfig,
    ExponentialBackoff, ExponentialBackoffConfig, Jittered,
};
pub use breaker::{
    BreakerConfigError, BreakerError, BreakerState, CircuitBreaker, CircuitBreakerConfig,
    MAX_WINDOW_SIZE, Permit, Rejected,
};
#[cfg(feature = "async")]
pub use clock::Clock;
pub use error::{RetryError, StopReason};
#[cfg(feature = "async")]
pub use retry::retry;
pub use time::Now;
