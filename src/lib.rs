//! # mettle
//!
//! A resilience toolkit for Rust: composable, testable primitives for handling failure, so you
//! don't hand-roll retry-and-backoff logic in every project.
//!
//! Wrap your operation, take the defaults, and override only what you need. Available now:
//! `retry()` (async) and `blocking::retry()` (sync), both with configurable backoff. Timeout and
//! circuit breaking are planned.
//!
//! Retries emit [`tracing`](https://docs.rs/tracing) events (target `mettle::retry`, level
//! `WARN`), so any subscriber will show them without extra wiring.

#![warn(missing_docs)]

#[cfg(not(any(feature = "async", feature = "blocking")))]
compile_error!("enable at least one of the `async` or `blocking` features");

pub mod backoff;

#[cfg(feature = "blocking")]
pub mod blocking;
#[cfg(feature = "async")]
pub mod clock;
#[cfg(feature = "async")]
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
