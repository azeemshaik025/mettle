# mettle

[![crates.io](https://img.shields.io/crates/v/mettle.svg)](https://crates.io/crates/mettle)
[![docs.rs](https://img.shields.io/docsrs/mettle)](https://docs.rs/mettle)
[![CI](https://github.com/azeemshaik025/mettle/actions/workflows/ci.yml/badge.svg)](https://github.com/azeemshaik025/mettle/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/mettle.svg)](#license)

**Retry for Rust, answered end to end.**

How long to wait, when to stop waiting on one attempt, when to give up, and what to report when
it's over. Every policy decision is a pure function of an injected clock, so a 30-second budget is
testable in microseconds with no real time passing. [Documentation](https://docs.rs/mettle).

## Install

```sh
cargo add mettle
```

Blocking only, without an async runtime (no `tokio`):

```sh
cargo add mettle --no-default-features --features blocking
```

## Example

```rust
use mettle::retry;

// Sensible defaults: exponential backoff, up to 3 retries.
let body = retry(|| async { fetch(&url).await }).await?;
```

Then override only what you need:

```rust
let body = retry(|| async { fetch(&url).await })
    .when(|e: &FetchError| e.is_transient())    // skip permanent errors
    .attempt_timeout(Duration::from_secs(5), || FetchError::Timeout)
    .max_elapsed(Duration::from_secs(30))
    .await?;
```

`attempt_timeout` bounds a single try, dropping the in-flight future and feeding the timeout into
the normal backoff. Reach for it whenever the call can hang: `max_elapsed` is only consulted
*between* attempts, so on its own it cannot stop a call that never returns. Its second argument is
the error to report, since a timed-out attempt never returned one of its own — for `io::Error` that
is `|| ErrorKind::TimedOut.into()`.

No async runtime? The blocking twin is identical but ends in `.call()` instead of `.await`.

Retrying on a fixed schedule means every client that failed together retries together, so a service
that is coming back up gets a synchronized wave. Jitter spreads them out:

```rust
use mettle::{Backoff, DecorrelatedBackoff, DecorrelatedBackoffConfig, ExponentialBackoff};

// Randomize any strategy's delays into 0 ..= delay ("full jitter")...
let backoff = ExponentialBackoff::default().jittered();

// ...or use decorrelated jitter, where each delay is drawn from the previous one.
let backoff = DecorrelatedBackoff::new(DecorrelatedBackoffConfig::default())?;
```

Which one: `.jittered()` works on any strategy, including one you wrote, and spreads delays as
widely as possible. `DecorrelatedBackoff` is its own strategy and never draws below its `base`, so
reach for it when you want a floor under every wait. The trade is that a floor also means never
retrying sooner than `base`, so a dependency that frees up early isn't picked up until then.

Both seed from entropy by default and take a fixed seed (`with_seed`) when you want a test to
replay the same delays.

When a retry gives up you get a `RetryError`, which says what stopped it:

```rust
match retry(|| async { fetch(&url).await }).await {
    Ok(body) => body,
    Err(e) => {
        // "max_elapsed after 5 attempts in 29.4s: connection refused"
        tracing::error!("{} after {} attempts in {:?}: {}",
            e.stop_reason().as_str(), e.attempts(), e.elapsed(), e.error());
        return Err(e.into());   // ?-able into Box<dyn Error> / anyhow
    }
}
```

Only want the underlying error? `.map_err(RetryError::into_error)`.

## Examples

Runnable, and the fastest way in:
[examples/retry.rs](https://github.com/azeemshaik025/mettle/blob/main/examples/retry.rs) (async) ·
[examples/blocking_retry.rs](https://github.com/azeemshaik025/mettle/blob/main/examples/blocking_retry.rs)
(blocking).

Retries emit `tracing` events out of the box (target `mettle::retry`). Install any subscriber
(e.g. `tracing_subscriber::fmt::init()`) to see them.

## Scope

mettle does retry, and does it completely, rather than being a shallow toolkit of five tools. The
test for anything new is whether it's about a failed call; that's what keeps bulkheads, rate
limiting and caching out. What's planned and what's been refused, with the reasoning, is in
[docs/ROADMAP.md](https://github.com/azeemshaik025/mettle/blob/main/docs/ROADMAP.md); design
decisions are in [docs/adr](https://github.com/azeemshaik025/mettle/tree/main/docs/adr).

## Status

v0.x, with async (Tokio) and blocking APIs. Expect breaking changes before 1.0.

## License

Licensed under either of

- [Apache License, Version 2.0](https://github.com/azeemshaik025/mettle/blob/main/LICENSE-APACHE)
- [MIT license](https://github.com/azeemshaik025/mettle/blob/main/LICENSE-MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
