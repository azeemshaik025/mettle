//! Test-only helpers shared across modules.

use std::cell::Cell;
use std::sync::Once;

thread_local! {
    /// Per-thread count of `mettle::retry` events. Thread-local so parallel tests don't
    /// clobber each other; each test reads only what its own thread emitted.
    static RETRY_EVENTS: Cell<usize> = const { Cell::new(0) };
    /// The same, for `mettle::breaker`. Counted separately so a test can assert on one tool's
    /// events without the other's noise.
    static BREAKER_EVENTS: Cell<usize> = const { Cell::new(0) };
}

/// A [`tracing::Subscriber`] that counts mettle's events into the emitting thread's counters.
/// Installed *once* as the global default (see [`count_retry_events`]) so the callsites are
/// always registered as "interested", which avoids the global interest-cache race a per-thread
/// `with_default` subscriber suffers under parallel tests.
struct Counter;

impl tracing::Subscriber for Counter {
    // Only mettle's own events; everything else is filtered here.
    fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
        matches!(meta.target(), "mettle::retry" | "mettle::breaker")
    }

    fn event(&self, event: &tracing::Event<'_>) {
        match event.metadata().target() {
            "mettle::retry" => RETRY_EVENTS.with(|c| c.set(c.get() + 1)),
            "mettle::breaker" => BREAKER_EVENTS.with(|c| c.set(c.get() + 1)),
            _ => {}
        }
    }

    // Spans are unused by mettle's events; satisfy the trait with no-ops.
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

fn install() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = tracing::subscriber::set_global_default(Counter);
    });
    // Flush any callsite interest cached before this subscriber was installed. Another test may
    // have hit a mettle callsite first with no subscriber, caching it as "never interested",
    // which would silently drop our events. Rebuilding forces a re-query.
    tracing::callsite::rebuild_interest_cache();
}

/// Install the global counter (once), reset this thread's retry count, and return a handle to
/// read it after running a retry on this thread.
#[cfg(any(feature = "async", feature = "blocking"))]
pub(crate) fn count_retry_events() -> RetryEventCount {
    install();
    RETRY_EVENTS.with(|c| c.set(0));
    RetryEventCount
}

/// Reads the calling thread's `mettle::retry` event count.
#[cfg(any(feature = "async", feature = "blocking"))]
pub(crate) struct RetryEventCount;

#[cfg(any(feature = "async", feature = "blocking"))]
impl RetryEventCount {
    pub(crate) fn get(&self) -> usize {
        RETRY_EVENTS.with(|c| c.get())
    }
}

/// The breaker twin of [`count_retry_events`].
pub(crate) fn count_breaker_events() -> BreakerEventCount {
    install();
    BREAKER_EVENTS.with(|c| c.set(0));
    BreakerEventCount
}

/// Reads the calling thread's `mettle::breaker` event count.
pub(crate) struct BreakerEventCount;

impl BreakerEventCount {
    pub(crate) fn get(&self) -> usize {
        BREAKER_EVENTS.with(|c| c.get())
    }
}
