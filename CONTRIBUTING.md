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

Keep the whole feature matrix green. CI runs `test`, `clippy`, `docs` and `msrv` against each of
these four configurations, so a change that only builds under `--all-features` still fails:

```sh
cargo test --all-features
cargo test                                              # defaults
cargo test --no-default-features --features async
cargo test --no-default-features --features blocking
```

MSRV is 1.85, and it applies to tests and examples too:

```sh
cargo +1.85.0 check --all-features --all-targets
```

The two checks that only CI used to catch, and that are worth running before you open a PR:

```sh
RUSTDOCFLAGS='-D warnings' cargo doc --all-features --no-deps   # broken intra-doc links
cargo semver-checks                                             # cargo install cargo-semver-checks
```

`cargo doc` is the only thing that catches a broken intra-doc link; tests, clippy and MSRV all pass
straight through them.

**If your change is breaking, bump the version in the same branch.** `cargo semver-checks` runs on
the pull request and compares your manifest against the published crate, so a breaking change left
at the current version fails CI no matter how good it is. In 0.x that means a minor bump: 0.4.0 to
0.5.0. Finalize the `CHANGELOG.md` heading with a date in the same commit and leave an empty
`## [Unreleased]` above it.

## Licensing

By contributing, you agree that your contributions are dual-licensed under `MIT OR Apache-2.0`,
matching the project.
