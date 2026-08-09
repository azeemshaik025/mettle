# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-08-09

### Added
- Jitter for backoff: `Backoff::jittered()` wraps any strategy so each delay becomes a uniform
  random value in `0 ..= delay` ("full jitter"), so a fleet of clients doesn't retry in lockstep.
  Opt-in; the RNG is seedable for reproducible tests.

  There is no mode to pick. AWS's measurements had the alternative ("equal jitter", a floor at
  `delay/2`) doing more work *and* finishing later than full jitter, and a simulation of mettle's
  own implementations reproduced that in every configuration tried, so offering it would only
  invite people to choose the worse one. For a floor under every wait, use `DecorrelatedBackoff`.
- `DecorrelatedBackoff`, built from a validated `DecorrelatedBackoffConfig`: each delay is drawn
  from `base ..= prev * 3`, capped at `max_delay` (the AWS "Exponential Backoff and Jitter"
  formula). It's a strategy rather than a `Jitter` mode because the randomness lives in the
  recurrence, so there's no deterministic sequence for `Jittered` to wrap. Seedable via
  `DecorrelatedBackoff::with_seed`.
- `Backoff::jittered_with_seed(seed)`, so the ergonomic combinator can be seeded too. Previously
  the readable form couldn't be made deterministic and the deterministic form meant naming
  `Jittered` directly, which made a test and its production config look nothing alike.
- `Clock` is now implemented for `&C` and `Arc<C>` (both the async and blocking traits). Writing a
  mock clock and passing `.clock(&mock)` used to be a compile error, forcing every mock to wrap its
  own state in `Rc`/`Arc` just to be usable. That was friction on the exact path this crate exists
  to make easy.

  **One way this can break you.** If you already wrote `impl Clock for &YourClock` yourself, most
  likely as a workaround for the above, that now collides with the impl this release adds and the
  build fails with `E0119: conflicting implementations`. The fix is to delete your impl, which this
  release makes redundant. Nothing else in 0.3.0 changes an existing signature, and
  `cargo-semver-checks` does not flag added impls, so this is called out here rather than left to
  be discovered.
- Documented that `ExponentialBackoffConfig { factor: 1, .. }` gives a constant delay, so there is
  no separate `ConstantBackoff` type to learn.

The randomized strategies (`DecorrelatedBackoff`, `Jittered`) are deliberately not `Clone`. A copy
would carry the RNG state and replay the same delays, which is the lockstep jitter exists to
prevent; clone the config and build a fresh strategy instead.

Jitter needs a random number generator, so this adds `fastrand` as a required dependency (1198
lines, no transitive dependencies of its own). It is not behind a feature flag; ADR004 records the
measurements and the reasoning.

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
