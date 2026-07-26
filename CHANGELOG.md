# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-07-26

Supersedes the yanked 0.1.1: that release removed public items in a patch, which was a breaking
change. This makes it a proper minor bump.

### Changed
- **Breaking:** the crate-root re-exports were trimmed. Reach `TokioClock`, `Retry`, and
  `RetryFuture` via `mettle::clock::TokioClock`, `mettle::retry::Retry`, and
  `mettle::retry::RetryFuture` instead of the crate root.
- Dual-licensed under `MIT OR Apache-2.0` (previously `MIT`).

### Added
- `#![forbid(unsafe_code)]`.
- Expanded crate docs: a landing-page quickstart, plus cancellation, `Send`/`'static`,
  determinism, and observability notes. Feature-gated items are now labeled on docs.rs.

## [0.1.1] - 2026-07-26 — Yanked

Yanked: it removed public re-exports in a patch release, which is a breaking change. Use 0.2.0.

### Changed
- Trimmed the crate-root re-exports (`TokioClock`, `Retry`, `RetryFuture` moved to
  `mettle::clock` / `mettle::retry`).

## [0.1.0] - 2026-07-26

_First release._

### Added
- `retry(op).await`: retry for fallible async operations. Sane defaults, with `.backoff()`,
  `.clock()`, `.when()`, and `.max_elapsed()` to adjust.
- `blocking::retry(op).call()`: the same retry without async, so no runtime is needed. Has its
  own injectable `Clock` (`StdClock` by default), so it's testable with a mock too.
- Exponential backoff (`ExponentialBackoff`, built from a validated `ExponentialBackoffConfig`).
  Delay arithmetic saturates instead of overflowing.
- `Backoff` trait for writing your own strategy: one method to implement.
- `Clock` trait with a `TokioClock` adapter, so time can be mocked in tests.
- A `tracing` event on every retry (target `mettle::retry`, `WARN`) with the attempt number,
  delay, and error. Any subscriber picks it up.
- Cargo features `async` and `blocking`, both on by default. For a blocking-only build with no
  async-runtime dependency: `default-features = false, features = ["blocking"]`.
