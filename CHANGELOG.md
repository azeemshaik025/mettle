# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-08-16

Retry can finally stop an attempt that hangs. Breaking.

### Added
- `Retry::attempt_timeout(duration, on_timeout)`. Bounds a single attempt, so an operation that
  hangs is finally stopped. When it fires the in-flight future is dropped and the attempt is
  treated as a failure, feeding the normal backoff and the normal `.when(..)` predicate.
  `on_timeout` supplies the error to report, because the operation never returned one.

  The wait runs on the injected `Clock`, not on Tokio directly, so a timeout is testable on a mock
  clock with no real time. `tokio::time::timeout` cannot be.
  [ADR007](https://github.com/azeemshaik025/mettle/blob/main/docs/adr/ADR007.md) covers why the
  error comes from a closure rather than making `RetryError::error()` an `Option`, and why there is
  no blocking equivalent.

### Fixed
- `max_elapsed` documented honestly. It is checked *between* attempts, so on its own it never
  bounded an operation that hangs, while the README said "give up after ~30s total". A future that
  is never ready gave the budget nothing to act on. Pairing it with `attempt_timeout` is what makes
  the budget enforceable; the blocking twin says plainly that it has no equivalent and points at
  the call's own timeout setting instead.

### Changed
- **Breaking:** `Retry` and `RetryFuture` take one more type parameter, for the on-timeout handler.
  Only affects code that names those types; `retry(..)` and every builder method are unchanged.
- The crate description and docs now say what mettle is — retry, answered end to end — rather than
  "a resilience toolkit", and no longer claim that timeout and circuit breaking are planned. Timeout
  shipped here; the circuit breaker was built and deliberately not shipped. Scope, including what
  has been refused and why, is in
  [docs/ROADMAP.md](https://github.com/azeemshaik025/mettle/blob/main/docs/ROADMAP.md).

### Upgrading

Nothing to do unless you *name* `Retry` or `RetryFuture`, which mostly means storing one in a
struct field or writing a function that returns one. `retry(..)` and every builder method are
unchanged, so the common inline use compiles as-is.

```diff
-fn build() -> Retry<F, ExponentialBackoff, TokioClock, fn(&E) -> bool> {
+fn build() -> Retry<F, ExponentialBackoff, TokioClock, fn(&E) -> bool, fn() -> E> {
```

The new parameter is the on-timeout handler. When no timeout is configured it is the function
pointer `fn() -> E` that `retry(..)` seeds, and it is never called. A default (`Q = NoTimeout`) was
tried and does not work: `NoTimeout` cannot implement `Fn() -> E`, so the no-timeout case would
need a second `IntoFuture` impl and the two would be seen as potentially overlapping.
[ADR007](https://github.com/azeemshaik025/mettle/blob/main/docs/adr/ADR007.md) has the detail.

## [0.4.0] - 2026-08-09

Retry now reports why it gave up, not just what failed last. Breaking.

### Added
- `RetryError<E>`, with `error()`, `into_error()`, `attempts()`, `elapsed()` and `stop_reason()`.
- `StopReason`: `RetriesExhausted`, `NotRetryable`, `MaxElapsed`, plus `as_str()` for metric labels.

### Changed
- **Breaking:** `retry(..).await` and `blocking::retry(..).call()` fail with `RetryError<E>` instead
  of `E`.
- **Breaking:** giving up emits one more `tracing` event on `mettle::retry`, carrying `attempts`,
  `elapsed_ms` and `reason`. It fires only when at least one retry happened, so a `.when(..)` filter
  rejecting the first error stays as quiet as it was.
- The async driver starts its clock on the first poll rather than at `.into_future()`, matching the
  blocking driver. A future parked before its first poll no longer bills that time to the operation.

### Upgrading

Wherever you name the error type:

```diff
-let value: Result<T, MyError> = retry(op).await;
+let value: Result<T, RetryError<MyError>> = retry(op).await;
```

`?` into `Box<dyn Error>` or `anyhow::Error` keeps working, and now covers error types it didn't
before: `String`, `Box<dyn Error>` and `anyhow::Error` all satisfy the new bound. To go back to the
bare error and keep an existing signature, add `.map_err(RetryError::into_error)`.

Matching on the error goes through an accessor, so `match e` becomes `match e.error()`.

Tests that asserted on the whole `Result` need the error unwrapped. `RetryError` is deliberately
not `PartialEq`, and could not usefully be: its fields are private with no public constructor, so
there is no way to build the right-hand side to compare against.

```diff
-assert_eq!(result, Err(MyError::Timeout));
+assert_eq!(*result.unwrap_err().error(), MyError::Timeout);
-assert_eq!(result, Ok(42));
+assert_eq!(result.unwrap(), 42);
```

`RetryError` deliberately has no `source()`. Why, and why its `Error` impl is bounded on
`E: Debug + Display` rather than `E: Error`:
[ADR006](https://github.com/azeemshaik025/mettle/blob/main/docs/adr/ADR006.md).

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
