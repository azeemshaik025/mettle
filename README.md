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

## Tools

Each tool comes with a runnable example. Start there:

- **retry**: async [examples/retry.rs](https://github.com/azeemshaik025/mettle/blob/main/examples/retry.rs) · blocking [examples/blocking_retry.rs](https://github.com/azeemshaik025/mettle/blob/main/examples/blocking_retry.rs)

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
