# mettle

[![crates.io](https://img.shields.io/crates/v/mettle.svg)](https://crates.io/crates/mettle)
[![docs.rs](https://img.shields.io/docsrs/mettle)](https://docs.rs/mettle)
[![CI](https://github.com/azeemshaik025/mettle/actions/workflows/ci.yml/badge.svg)](https://github.com/azeemshaik025/mettle/actions/workflows/ci.yml)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**A resilience toolkit for Rust.**

Composable, testable primitives for handling failure.

## Install

```sh
cargo add mettle
```

Blocking only, without an async runtime (no `tokio`):

```sh
cargo add mettle --no-default-features --features blocking
```

## Tools

Each tool comes with a runnable example. Start there:

- **retry**: async [`examples/retry.rs`](examples/retry.rs) · blocking [`examples/blocking_retry.rs`](examples/blocking_retry.rs)

Retries emit `tracing` events out of the box. Install any subscriber
(e.g. `tracing_subscriber::fmt::init()`) to see them.

## Status

v0.1, with async (Tokio) and blocking APIs. Expect breaking changes before 1.0.

## License

MIT
