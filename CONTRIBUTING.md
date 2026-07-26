# Contributing to mettle

Thanks for your interest. mettle is small and early, so the process is light.

## Before a large change

Open an issue first so we can agree on the approach. The design rationale lives in
[`docs/adr/`](docs/adr/) — worth a skim before proposing API changes.

## Development

```sh
cargo test --all-features        # full suite
cargo fmt --all                  # format
cargo clippy --all-targets --all-features -- -D warnings
```

Keep the whole feature matrix green:

```sh
cargo test --no-default-features --features async
cargo test --no-default-features --features blocking
```

MSRV is 1.85. CI runs all of the above plus a docs build (`-D warnings`) and a
semver-compatibility check, so run them locally before opening a PR.

## Licensing

By contributing, you agree that your contributions are dual-licensed under `MIT OR Apache-2.0`,
matching the project.
