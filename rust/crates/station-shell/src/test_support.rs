//! Test-only serialization for the crate's process-global state.
//!
//! `cargo test` runs one binary's tests in parallel threads, while these seams are process
//! state: `routing`'s request queue and rolling browser budget, the notification bridge's
//! pending queue, and whatever a frame poll drains. A test that enqueues and then routes (or
//! receives and then delivers) must own that state across both calls; every other test's poll
//! takes the same lock inside its drain, so it cannot steal the request or spend the budget.
//!
//! The lock is reentrant per thread (`lock()` twice on one thread only bumps a depth), so a
//! test can hold it for its whole body while the seams it calls take it again. Test binaries
//! are separate processes; nothing here couples them.

use std::cell::Cell;
use std::sync::{Mutex, MutexGuard};

/// The one process-state mutex. `tests::routing_lock` and `tests_pages::bridge_lock` both take
/// it (the earlier split locks let a poll from an unrelated test interleave).
static LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// How many `Guard`s this thread holds; above zero, a nested `lock()` does not touch the
    /// global mutex (which this thread already owns).
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// One hold of the process-state lock; nested holds on the same thread are counted, not blocked.
pub(crate) struct Guard {
    /// The global guard for the outermost hold; `None` for a nested one. Held only for its
    /// `Drop` (the mutex unlock).
    _inner: Option<MutexGuard<'static, ()>>,
}

/// Takes (or re-enters) the process-state lock for the calling thread.
pub(crate) fn lock() -> Guard {
    if DEPTH.with(Cell::get) > 0 {
        DEPTH.with(|depth| depth.set(depth.get() + 1));
        return Guard { _inner: None };
    }
    let inner = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    DEPTH.with(|depth| depth.set(1));
    Guard {
        _inner: Some(inner),
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}
