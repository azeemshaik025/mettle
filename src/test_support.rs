//! Test-only helpers shared across modules.

use std::cell::Cell;
use std::sync::Once;

thread_local! {
    /// Per-thread count of `mettle::retry` events. Thread-local so parallel tests don't
    /// clobber each other; each test reads only what its own thread emitted.
    static RETRY_EVENTS: Cell<usize> = const { Cell::new(0) };
}

/// A [`tracing::Subscriber`] that counts `mettle::retry` events into the emitting thread's
/// [`RETRY_EVENTS`]. Installed *once* as the global default (see [`count_retry_events`]) so the
/// retry callsite is always registered as "interested", which avoids the global interest-cache
/// race a per-thread `with_default` subscriber suffers under parallel tests.
struct Counter;

impl tracing::Subscriber for Counter {
    // Only mettle's retry events; everything else is filtered here.
    fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
        meta.target() == "mettle::retry"
    }

    fn event(&self, _event: &tracing::Event<'_>) {
        RETRY_EVENTS.with(|c| c.set(c.get() + 1));
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

/// Install the global retry-event counter (once), reset this thread's count, and return a handle
/// to read it after running a retry on this thread.
pub(crate) fn count_retry_events() -> RetryEventCount {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = tracing::subscriber::set_global_default(Counter);
    });
    // Flush any callsite interest cached before this subscriber was installed. Another retry test
    // may have hit the `mettle::retry` callsite first with no subscriber, caching it as "never
    // interested", which would silently drop our events. Rebuilding forces a re-query.
    tracing::callsite::rebuild_interest_cache();
    RETRY_EVENTS.with(|c| c.set(0));
    RetryEventCount
}

/// Reads the calling thread's `mettle::retry` event count.
pub(crate) struct RetryEventCount;

impl RetryEventCount {
    pub(crate) fn get(&self) -> usize {
        RETRY_EVENTS.with(|c| c.get())
    }
}
