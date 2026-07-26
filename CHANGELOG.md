# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-07-26

### Changed
- Trimmed the crate-root re-exports to the symbols developers actually name. `TokioClock`,
  `Retry`, and `RetryFuture` are no longer exported at the crate root; reach them via
  `mettle::clock::TokioClock`, `mettle::retry::Retry`, and `mettle::retry::RetryFuture`.

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
