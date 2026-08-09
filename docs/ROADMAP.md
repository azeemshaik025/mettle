# Roadmap and scope

What mettle intends to build, and what it refuses to. The refusals matter more than the plans:
a toolkit with an edge is worth more than a toolkit with everything, and every tool costs a feature
matrix, a blocking twin, and an ADR forever.

This file is the record. If a tool isn't here, it hasn't been decided.

## The claim

Every policy decision in this crate is a pure function of an injected time source, so you can test
your resilience configuration exactly, at boundaries, with no sleeping and no real clock.

That is narrow on purpose. mettle is not competing on breadth with `tower-resilience` or
resilience4j. It is a small set of tools that bound each other correctly, tested to a standard the
alternatives don't meet. The second, smaller claim is shape: operations are `FnMut() -> Fut`
factories, so nothing needs `Clone`, `Send`, or `'static`, which is why `tower::retry` and
`tower::hedge` don't work for tonic users whose `http::Request` isn't `Clone`.

## Shipped

| Tool | Notes |
|------|-------|
| retry | Async and blocking twins over one decision core. Exponential backoff, jitter (full, equal, decorrelated), `RetryError<E>` carrying attempts, elapsed, and why it stopped. |
| circuit breaker | Shared through `&self`, pure state machine, RAII permits, injected `Now`. |

## Planned

In order. Each is a prerequisite for the ones under it.

**1. Per-attempt bound: `Retry::attempt_timeout` plus a `Deadline` value.**
`max_elapsed` is only checked between attempts, so an operation that hangs is never bounded. The
README currently implies otherwise, which makes this a correctness fix to a shipped promise rather
than a new feature. Async gets `attempt_timeout`, which drops the in-flight future and feeds the
normal backoff sequence. Blocking gets `Deadline`, an absolute instant you hand to whatever actually
waits (`set_read_timeout`, `wait_timeout`, a channel recv), because you cannot interrupt a blocking
closure without forcing `Send + 'static` on it, and two existing tests exist specifically to forbid
that.

**2. Retry budget.**
A shared, rate-limited allowance for retries, so a partial outage can't turn into a retry storm.
Universal outside Rust (gRPC `retryThrottling`, Envoy `budget_percent`, Linkerd `retryRatio`,
Finagle `RetryBudget`, AWS retry quotas) and effectively absent inside it: the only working
implementation is buried in `aws-smithy-runtime` behind about twenty crates. The per-call retry
crates can't add one, because a budget needs a handle shared across calls and they hand out values.
mettle already has that shape from the breaker.

Two decisions belong in the ADR before any code. Use a rate-with-TTL budget rather than the timeless
token bucket, because "does the budget refill after sixty idle seconds" is then a hand-fed-`Instant`
test nobody else in Rust can write, and because a budget that only refills on traffic locks out a
low-QPS caller forever. And name the debit ordering: `when(err)` runs before the backoff is drawn,
so a naive implementation debits for a retry that may never happen.

**3. `cargo-mutants` in CI.**
Not a tool. The three worst bugs this crate has had (the cleared window, the generation token, the
half-open livelock) were all mutation-detectable and all found by hand. Do this before hedging.

**4. Hedging, one backup request.**
Send a backup when the first request passes a latency threshold, take whichever answers first. The
one candidate where someone would choose mettle for the tool itself: `tower::hedge` has had no
functional change since 2022, and the standalone crates have four-figure lifetime downloads. It is
also the only planned tool that is inherently async-only, which spends the crate's single permitted
exception to the twin rule.

Strictly after the budget. Hedging at a fixed delay doubles load on a degraded backend exactly when
it is degraded, which is why gRPC gates hedging on `retryThrottling` and Finagle builds its backup
requests on `RetryBudget`.

Only one backup, ever. `#![forbid(unsafe_code)]` means there is no safe projection from
`Pin<&mut [Option<Fut>; N]>` to element *i*, and boxing would violate ADR001.

## Refused

Recorded so nobody relitigates them. Each can be revisited if a real user asks, but the default is no.

- **Bulkhead.** Reads no clock, so it exercises none of what this crate is for, and
  `Semaphore::try_acquire` is five lines. A queueing bulkhead is worse: it needs async queueing
  machinery mettle doesn't have, and the blocking twin is untestable because `Condvar::wait_timeout`
  has no injection point.
- **General rate limiting.** `governor` and `ratelimit` own this, and `ratelimit` already implements
  the injected-clock thesis. Being third here would turn our differentiator into table stakes.
- **Adaptive concurrency limits (AIMD, Vegas, Gradient).** The value is the published control law,
  not deterministic replay. Deterministic testing proves a limiter is repeatable, not that it is
  well tuned.
- **Fallback.** `Result::or_else` already is the combinator.
- **Cache.** Owned by `moka` and `foyer`, and there is no decision logic to make pure, so the
  architecture buys nothing.
- **Health checks.** A fleet concern that belongs with discovery and load balancing. mettle has no
  notion of a set of endpoints.
- **Ambient deadline propagation.** The hard part is a task-local convention that survives `spawn`,
  which is a runtime and ecosystem problem. mettle ships the `Deadline` value and stops there.
- **Graceful degradation.** A posture, not a primitive.

## The ceiling

One maintainer, with a commitment to deterministic tests and mutation-checked fixes. The binding
constraint isn't lines of code, it's feature matrix times twin count times ADR surface. Realistically
that is about five tools and one async-only exception. Hedging is the exception. Anything after it
has to displace something.

Budget for correction, too. The circuit breaker shipped a fix worth roughly a fifth of its final size
*after* it was otherwise complete, and that was the half-open livelock, which no per-call assertion
could have caught. Assume the same on the retry budget and on hedging.
