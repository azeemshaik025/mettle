# Architecture Decision Records

The non-obvious calls behind mettle, grouped by theme so the reasoning doesn't get lost.
(ADR = Architecture Decision Record.) Each file groups a few related decisions. When one
changes, edit its file and say what changed and why.

| ADR | Area | Status |
|-----|------|--------|
| [001](ADR001.md) | Sans-IO core, hand-written async, one crate | Accepted |
| [002](ADR002.md) | Public API, open traits, one validated `Backoff` | Accepted |
| [003](ADR003.md) | Dependencies and Cargo features | Accepted |
| [004](ADR004.md) | Jitter, a fourth dependency, and `Clone` | Accepted |
| [005](ADR005.md) | Making the tested path as usable as the production path | Accepted |
| [006](ADR006.md) | `RetryError<E>` and the `Error` bound | Accepted |
| [007](ADR007.md) | Bounding one attempt, and where the error comes from | Accepted |

What's planned and what's deliberately refused lives in [../ROADMAP.md](../ROADMAP.md).
