# Roadmap and scope

What mettle intends to build, and what it refuses to. The refusals matter more than the plans: a
crate with an edge is worth more than a crate with everything, and every feature costs a feature
matrix, a blocking twin, and an ADR forever.

This file is the record. If something isn't here, it hasn't been decided.

## What mettle is

**Retry, solved completely, and the only one you can test exactly without sleeping.**

Not a toolkit of five shallow tools. One problem, answered end to end: how long to wait, when to
stop waiting on one try, when the fleet should stop retrying, when to try again before the first
finishes, and what to report when it's over. Every policy decision is a pure function of an injected
time source, so you can test your configuration at its boundaries with no real clock.

That gives a test for every future request: **is this about a failed call?** If not, it's out. That
test is what refused most of the list below, and it's the reason this file exists.

The second, smaller claim is shape. Operations are `FnMut() -> Fut` factories, so nothing needs
`Clone`, `Send`, or `'static`. That's why `tower::retry` doesn't work for tonic users whose
`http::Request` isn't `Clone`.

## Shipped

| | |
|------|-------|
| retry, async and blocking twins over one decision core | 0.1 |
| exponential backoff, validated config, saturating arithmetic | 0.1 |
| jitter (full) and `DecorrelatedBackoff` | 0.3.0 |
| `RetryError<E>` with attempts, elapsed, and why it stopped | 0.4.0 |
| `attempt_timeout`, so a hung attempt is finally bounded | 0.5.0 |

## Planned

In order. Each is a prerequisite for the ones under it.

**1. Retry budget.**
A shared, rate-limited allowance for retries, so a partial outage can't turn into a retry storm.
Universal outside Rust (gRPC `retryThrottling`, Envoy `budget_percent`, Linkerd `retryRatio`,
Finagle `RetryBudget`, AWS retry quotas) and effectively absent inside it: the only working
implementation is buried in `aws-smithy-runtime` behind about twenty crates.

This is the one the competition structurally cannot copy. A budget needs a handle shared across
calls; `backon`, `tokio-retry`, `tryhard` and `again` all hand out per-call values. Verified against
`backon` (25M downloads/90d): no budget, no hedging, no per-attempt timeout.

Two decisions belong in the ADR before any code. Use a rate-with-TTL budget rather than the timeless
token bucket, because "does the budget refill after sixty idle seconds" is then a hand-fed-`Instant`
test nobody else in Rust can write, and because a budget that only refills on traffic locks out a
low-QPS caller forever. And name the debit ordering: `when(err)` runs before the backoff is drawn,
so a naive implementation debits for a retry that may never happen.

**2. `cargo-mutants` in CI.**
Not a feature. The worst bugs this crate has had (the cleared window, the generation token, the
half-open livelock, the mock clock advancing on creation) were all mutation-detectable and all found
by hand. Do this before hedging.

**3. Hedging, one backup request.**
Send a backup when the first request passes a latency threshold, take whichever answers first. Not a
separate tool: it's speculative retry. Weak incumbents — `tower::hedge` has had no functional change
since 2022, and the standalone crates have four-figure lifetime downloads.

Strictly after the budget. Hedging at a fixed delay doubles load on a degraded backend exactly when
it's degraded, which is why gRPC gates hedging on `retryThrottling` and Finagle builds its backup
requests on `RetryBudget`.

Only ever one backup. `#![forbid(unsafe_code)]` means there's no safe projection from
`Pin<&mut [Option<Fut>; N]>` to element *i*, and boxing would violate ADR001.

## Refused

Recorded so nobody relitigates them. Each can be revisited if a real user asks, but the default is
no.

- **Circuit breaker.** Built, tested, and not shipped — see below. The one refusal we reached by
  building the thing.
- **Bulkhead.** Reads no clock, so it exercises none of what this crate is for, and
  `Semaphore::try_acquire` is five lines. A queueing bulkhead is worse: it needs async queueing
  machinery mettle doesn't have, and the blocking twin is untestable because `Condvar::wait_timeout`
  has no injection point.
- **General rate limiting.** `governor` (13.6M downloads/90d) and `ratelimit` (4M) own this, and
  `ratelimit` already implements the injected-clock thesis. Being third here would turn our
  differentiator into table stakes.
- **Adaptive concurrency limits (AIMD, Vegas, Gradient).** The value is the published control law,
  not deterministic replay. Deterministic testing proves a limiter is repeatable, not well tuned.
- **Standalone timeout.** `tokio::time::timeout` exists, and a blocking version would force
  `Send + 'static` onto the caller's operation. What was genuinely missing was the *per-attempt*
  bound, which shipped as `attempt_timeout` (ADR007).
- **Fallback.** `Result::or_else` already is the combinator.
- **Cache.** Owned by `moka` and `foyer`, and there's no decision logic to make pure, so the
  architecture buys nothing.
- **Health checks.** A fleet concern that belongs with discovery and load balancing. mettle has no
  notion of a set of endpoints.
- **Ambient deadline propagation.** The hard part is a task-local convention that survives `spawn`,
  which is a runtime and ecosystem problem.
- **Graceful degradation.** A posture, not a primitive.

## The circuit breaker, in detail

It exists, on the `circuit-breaker` branch: a pure state machine, a shared wrapper, RAII permits, an
injected `Now`, 100 tests, and a half-open livelock found and fixed. It is not shipped.

The demand is roughly fiftyfold smaller than retry — about 1.2M downloads per 90 days across every
Rust breaker (`failsafe`, `recloser`, `circuit_breaker`) against about 60M for the retry crates.
Infrastructure has also absorbed much of the use case: Istio, Linkerd and Envoy break circuits at
the proxy, so anyone on a mesh already has one.

And it doesn't fit the spine. Everything else here answers "a call failed, now what". A breaker
answers "should I call at all", which is why it was the only thing needing shared mutable state, a
new trait, and a concurrency story. Shipping it would double the public surface for a tool the
evidence says few would reach for.

The work isn't wasted; it's how we found out. If a user asks, the branch is ready.

## The ceiling

One maintainer, with a commitment to deterministic tests and mutation-checked fixes. The binding
constraint isn't lines of code, it's feature matrix times twin count times ADR surface. Every
async-only feature is a documented exception to the twin rule: `attempt_timeout` is the first, and
hedging would be the second, which is as many as this crate should have.

Budget for correction too. The circuit breaker needed a fix worth roughly a fifth of its size
*after* it was otherwise complete, and that was the half-open livelock, which no per-call assertion
could have caught. Assume the same on the retry budget.
