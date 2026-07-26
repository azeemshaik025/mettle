//! # mettle
//!
//! A resilience toolkit for Rust: composable, testable primitives for handling failure, so you
//! don't hand-roll retry-and-backoff logic in every project.
//!
//! Available now: `retry()` (async) and `blocking::retry()` (sync), both with configurable
//! backoff. Timeout and circuit breaking are planned.
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
//! # Observability
//!
//! Every retry emits a [`tracing`](https://docs.rs/tracing) event on target `mettle::retry` at
//! `WARN`, carrying the `attempt` number, `delay_ms`, and the `error`. Install any subscriber to
//! see them, filter with `RUST_LOG=mettle::retry=warn`, or silence with `RUST_LOG=mettle=off`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(not(any(feature = "async", feature = "blocking")))]
compile_error!("enable at least one of the `async` or `blocking` features");

pub mod backoff;

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

pub use backoff::{Backoff, BackoffConfigError, ExponentialBackoff, ExponentialBackoffConfig};
#[cfg(feature = "async")]
pub use clock::Clock;
#[cfg(feature = "async")]
pub use retry::retry;
