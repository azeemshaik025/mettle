//! Blocking (synchronous) retry: the sync twin of mettle's async `retry`, without async.
//!
//! Wrap your operation with [`retry()`] and run it with [`call`](Retry::call). The backoff
//! strategies and the retry decision are shared with the async API; only the driver differs: a
//! plain loop with [`std::thread::sleep`] instead of a future.
//!
//! ```no_run
//! # fn fetch() -> Result<u32, std::io::Error> { Ok(1) }
//! let value = mettle::blocking::retry(fetch).call()?;
//! # let _ = value;
//! # Ok::<(), std::io::Error>(())
//! ```

mod clock;
mod retry;

pub use clock::{Clock, StdClock};
pub use retry::{Retry, retry};
