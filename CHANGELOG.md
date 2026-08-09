# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-08-09

Adds jitter. Purely additive apart from one collision noted under Upgrading.

### Added
- `Backoff::jittered()` and `Backoff::jittered_with_seed(seed)`. Wraps any strategy, including one
  you wrote, so each delay becomes a uniform random value in `0 ..= delay` ("full jitter"). Opt-in:
  the default schedule stays deterministic. The seeded form is for tests.
- `DecorrelatedBackoff`, from a validated `DecorrelatedBackoffConfig`. Draws each delay from
  `base ..= prev * 3`, capped at `max_delay`, and never below `base`. Seedable with `with_seed`.
- `Clock` for `&C` and `Arc<C>`, on both the async and blocking traits, so a mock clock can be
  passed as `.clock(&mock)` and still be read afterwards.
- `ExponentialBackoffConfig { factor: 1, .. }` documented as the way to get a constant delay; there
  is no separate `ConstantBackoff` type.

### Changed
- New required dependency: `fastrand` (no transitive dependencies, not feature-gated).

### Upgrading

0.2.0 code compiles unchanged, with one exception. If you wrote `impl Clock for &YourClock`
yourself, it now collides with the impl this release adds and the build fails with
`E0119: conflicting implementations`. Delete yours; this release makes it redundant.
`cargo-semver-checks` does not flag added impls, so it would not have warned you.
[ADR005](https://github.com/azeemshaik025/mettle/blob/main/docs/adr/ADR005.md) explains why the
impls are there and why we shipped them anyway.

`Jittered` and `DecorrelatedBackoff` are deliberately not `Clone`, because a copy carries the RNG
state and replays the same delays. Keep the config and build a fresh strategy from it.

Why the jitter side is shaped the way it is, including why there is no *mode* to choose:
[ADR004](https://github.com/azeemshaik025/mettle/blob/main/docs/adr/ADR004.md).

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
