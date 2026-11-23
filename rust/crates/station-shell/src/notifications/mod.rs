//! The notification center's own module: the panel view, and the page bridge that feeds it.

pub mod bridge;
mod panel;

#[cfg(test)]
mod tests_notification_override;

use notification_center::NotificationAction;

#[cfg(test)]
pub use bridge::clear;
pub use bridge::deliver_to;
#[cfg(any(target_os = "windows", test))]
pub use bridge::{receive, INIT_SCRIPT};
#[cfg(test)]
pub use panel::MAX_NOTIFICATION_ICON_URLS;
pub use panel::{icon_urls, panel};

// The frame time (seconds, Rune's poll clock) of the first observed poll, so the wall clock a
// snooze was stamped with (`ShellState::snooze_notifications`, the saga's `Date.now()`) and
// the frame clock the poll runs on can agree on "now".
thread_local! {
    static WALL_AT_FIRST_POLL: Cell<Option<i64>> = const { Cell::new(None) };
    /// The frame time of the last expiry scan, for the once-per-second throttle.
    static LAST_ADVANCE: Cell<Option<f64>> = const { Cell::new(None) };
}

/// Drives snooze expiry from the frame's poll: a finite snooze whose deadline has passed is
/// reset (`NotificationCenter`'s `advance_to` outcome), at most once per second of frame clock
/// — a 60 Hz window never scans per frame.
///
/// The deadline lives on the wall clock (the stamp `SetSnoozeDuration` recorded plus the
/// duration), so "now" is read on the wall clock once per poll and the *throttle* runs on the
/// frame clock: monotonic per process, never affected by a system clock step. A negative or
/// non-monotonic frame clock is refused (`NaN`/negative `now_s`), and so is a system clock
/// behind the process's start (`wall_now` younger than the first poll): an expiry needs both
/// clocks to have moved forward.
///
/// `Snooze::SyncWithOs` has no reader here (no OS do-not-disturb state is polled) and
/// `Snooze::Infinite` never ends: neither expires, by design. Recorded as a remaining gap in
/// `docs/rust-rune-port.md`.
pub fn expire_snooze(state: &mut ShellState, now_s: f64) -> bool {
    const THROTTLE_S: f64 = 1.0;
    if !now_s.is_finite() || now_s < 0.0 {
        return false;
    }
    let now_wall_ms = wall_now_ms();
    WALL_AT_FIRST_POLL.with(|slot| {
        if slot.get().is_none() {
            slot.set(Some(now_wall_ms));
        }
    });
    // Only a finite, started, elapsed snooze reaches the throttle; sentinels never expire.
    let Some(duration_ms) = state.notifications().snooze_duration_ms() else {
        return false;
    };
    let Some(started_wall_ms) = state.notifications().snooze_started_on() else {
        return false;
    };
    let end_wall_ms =
        started_wall_ms.saturating_add(i64::try_from(duration_ms).unwrap_or(i64::MAX));
    if end_wall_ms > now_wall_ms {
        return false;
    }
    // Throttled: at most one scan per second of frame clock, and the first eligible scan is
    // not consumed by stamping the clock (the deadline check above ran first).
    if let Some(last) = LAST_ADVANCE.with(|slot| slot.get()) {
        if now_s - last < THROTTLE_S {
            return false;
        }
    }
    LAST_ADVANCE.with(|slot| slot.set(Some(now_s)));
    let was_snoozed = state.notifications().is_snoozed();
    state
        .notifications_mut()
        .apply(NotificationAction::ResetSnoozeDuration);
    was_snoozed
}

fn wall_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(i64::MAX)
}

/// A notification row pressed — the panel's view action. `ShellState` marks the notification
/// read and routes the sender's tab (`click_notification`), then this layer asks the web view to
/// tell that page, which keeps the state handle-free (D5). Delivery is one message to the source
/// tab only; an absent page or a failed evaluation is dropped silently, and there is no retry.
pub fn click(state: &mut ShellState, notification_id: &str) {
    if let Some(tab_id) = state.click_notification(notification_id) {
        crate::webview::click(&tab_id, notification_id);
    }
}

use std::cell::Cell;

use crate::state::ShellState;
