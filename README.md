# mettle

[![crates.io](https://img.shields.io/crates/v/mettle.svg)](https://crates.io/crates/mettle)
[![docs.rs](https://img.shields.io/docsrs/mettle)](https://docs.rs/mettle)
[![CI](https://github.com/azeemshaik025/mettle/actions/workflows/ci.yml/badge.svg)](https://github.com/azeemshaik025/mettle/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/mettle.svg)](#license)

**A resilience toolkit for Rust.**

Composable, testable primitives for handling failure. [Documentation](https://docs.rs/mettle).

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
use std::time::Duration;

// Retry with sensible defaults (exponential backoff, up to 3 retries),
// then override only what you need.
let body = retry(|| async { fetch(&url).await })
    .when(|e: &FetchError| e.is_transient())   // skip permanent errors
    .max_elapsed(Duration::from_secs(30))       // give up after ~30s total
    .await?;
```

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

Retry alone amplifies an outage, so there is a circuit breaker too. Put it inside the retry:

```rust
use mettle::clock::TokioClock;
use mettle::{retry, BreakerError, CircuitBreaker, CircuitBreakerConfig};
use std::sync::Arc;

// One per dependency, shared by everyone who calls it.
let breaker = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default(), TokioClock)?);

let body = retry(|| breaker.call_async(|| fetch(&url)))
    .when(BreakerError::is_inner)   // a shed call is not worth retrying
    .await?;
```

Once enough recent calls have failed, the breaker stops calling the dependency at all, waits, then
lets a probe through to check whether it came back. Because the clock is injected, you can test
that whole cycle without sleeping.

## Tools

Each tool comes with a runnable example. Start there:

- **retry**: async [examples/retry.rs](https://github.com/azeemshaik025/mettle/blob/main/examples/retry.rs) · blocking [examples/blocking_retry.rs](https://github.com/azeemshaik025/mettle/blob/main/examples/blocking_retry.rs)
- **circuit breaker**: [examples/breaker.rs](https://github.com/azeemshaik025/mettle/blob/main/examples/breaker.rs)

Retries emit `tracing` events out of the box (target `mettle::retry`). Install any subscriber
(e.g. `tracing_subscriber::fmt::init()`) to see them.

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
