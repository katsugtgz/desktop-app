//! OS-level notification integration (Slice 29):
//! 1. Windows Do Not Disturb / Focus Assist reader (`SHQueryUserNotificationState`) for
//!    `Snooze::SyncWithOs`, polled every [`DND_POLL_INTERVAL_S`] seconds on the frame clock.
//! 2. OS desktop notification delivery (`OsNotifier` seam + Windows tray balloon via
//!    `Shell_NotifyIconW` / `NIF_INFO`), gated by effective snooze and a per-minute rate cap.

use std::sync::Mutex;

/// How often `Snooze::SyncWithOs` polls the OS do-not-disturb state (seconds of frame clock).
pub const DND_POLL_INTERVAL_S: f64 = 5.0;

/// Rolling per-minute cap on OS notification dispatches so a noisy page cannot flood the OS.
pub const MAX_OS_NOTIFICATIONS_PER_MINUTE: usize = 12;

/// Rolling rate-limit window, in seconds.
pub const OS_NOTIFY_WINDOW_S: f64 = 60.0;

/// Maximum characters copied into a Win32 balloon title (`szInfoTitle[64]`).
pub const MAX_OS_TITLE_CHARS: usize = 63;

/// Maximum characters copied into a Win32 balloon body (`szInfo[256]`).
pub const MAX_OS_BODY_CHARS: usize = 255;

/// Win32 `QUERY_USER_NOTIFICATION_STATE` numeric values (`shellapi.h`), mirrored here so the
/// pure mapping is unit-testable on every platform.
#[cfg(test)]
pub const QUNS_NOT_PRESENT: i32 = 1;
pub const QUNS_BUSY: i32 = 2;
pub const QUNS_RUNNING_D3D_FULL_SCREEN: i32 = 3;
pub const QUNS_PRESENTATION_MODE: i32 = 4;
pub const QUNS_ACCEPTS_NOTIFICATIONS: i32 = 5;
pub const QUNS_QUIET_TIME: i32 = 6;
#[cfg(test)]
pub const QUNS_APP: i32 = 7;

/// Maps a Win32 `QUERY_USER_NOTIFICATION_STATE` value to whether OS Do Not Disturb is active.
pub fn quns_is_dnd(state: i32) -> bool {
    matches!(
        state,
        QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE | QUNS_QUIET_TIME
    )
}

/// Platform seam for reading whether the host OS currently suppresses notifications (DND).
pub trait OsDndReader {
    fn is_dnd_active(&self) -> bool;
}

/// Real OS DND reader: Windows `SHQueryUserNotificationState`, always `false` elsewhere.
pub struct SystemDndReader;

impl OsDndReader for SystemDndReader {
    fn is_dnd_active(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;
            let mut state: i32 = QUNS_ACCEPTS_NOTIFICATIONS;
            // SAFETY: `state` is a valid aligned out-pointer for `SHQueryUserNotificationState`.
            let hr = unsafe { SHQueryUserNotificationState(&mut state) };
            hr >= 0 && quns_is_dnd(state)
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }
}

/// Sanitizes and caps `(title, body)` for OS notification delivery.
pub fn sanitize_notification(app_name: &str, title: &str, body: Option<&str>) -> (String, String) {
    let clean = |s: &str, max: usize| -> String {
        s.chars()
            .filter(|c| !c.is_control())
            .take(max)
            .collect::<String>()
            .trim()
            .to_owned()
    };
    let clean_title = clean(title, MAX_OS_TITLE_CHARS);
    let clean_app = clean(app_name, MAX_OS_TITLE_CHARS);
    let final_title = if clean_title.is_empty() {
        if clean_app.is_empty() {
            "Station".to_owned()
        } else {
            clean_app
        }
    } else if !clean_app.is_empty() && !clean_title.eq_ignore_ascii_case(&clean_app) {
        let combined = format!("{clean_app} — {clean_title}");
        combined.chars().take(MAX_OS_TITLE_CHARS).collect()
    } else {
        clean_title
    };
    let final_body = body
        .map(|b| clean(b, MAX_OS_BODY_CHARS))
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| final_title.clone());
    (final_title, final_body)
}

/// Copies `text` into a fixed UTF-16 buffer without splitting surrogate pairs, always NUL-terminated.
pub fn write_utf16_fixed<const N: usize>(dst: &mut [u16; N], text: &str) {
    dst.fill(0);
    if N <= 1 {
        return;
    }
    let mut pos = 0usize;
    let mut tmp = [0u16; 2];
    for ch in text.chars() {
        let encoded = ch.encode_utf16(&mut tmp);
        if pos + encoded.len() >= N {
            break;
        }
        for &unit in &*encoded {
            dst[pos] = unit;
            pos += 1;
        }
    }
    dst[pos] = 0;
}

/// Platform seam for delivering an OS-level notification toast/balloon.
pub trait OsNotifier {
    fn notify(&self, app_name: &str, title: &str, body: Option<&str>) -> bool;
}

/// Real OS notifier: Windows `Shell_NotifyIconW` (`NIF_INFO` balloon) respecting Quiet Time;
/// no-op returning `false` on non-Windows hosts.
pub struct SystemOsNotifier;

#[cfg(target_os = "windows")]
const BALLOON_ICON_UID: u32 = 0x5354_4E46; // "STNF"

#[cfg(target_os = "windows")]
static BALLOON_ADDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl OsNotifier for SystemOsNotifier {
    fn notify(&self, app_name: &str, title: &str, body: Option<&str>) -> bool {
        let (title, body) = sanitize_notification(app_name, title, body);
        #[cfg(target_os = "windows")]
        {
            use std::sync::atomic::Ordering;
            use windows_sys::Win32::UI::Shell::{
                Shell_NotifyIconW, NIF_INFO, NIIF_INFO, NIIF_RESPECT_QUIET_TIME, NIM_ADD,
                NIM_MODIFY, NOTIFYICONDATAW,
            };
            let Some(hwnd) = crate::webview::parent_hwnd() else {
                return false;
            };
            // SAFETY: `NOTIFYICONDATAW` is a plain C struct; zero-init + cbSize is standard Win32.
            let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
            data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            data.hWnd = hwnd;
            data.uID = BALLOON_ICON_UID;
            data.uFlags = NIF_INFO;
            data.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
            write_utf16_fixed(&mut data.szInfoTitle, &title);
            write_utf16_fixed(&mut data.szInfo, &body);

            let msg = if BALLOON_ADDED.load(Ordering::Relaxed) {
                NIM_MODIFY
            } else {
                NIM_ADD
            };
            // SAFETY: `data` has valid `cbSize`, live `hwnd`, and NUL-terminated UTF-16 buffers.
            let ok = unsafe { Shell_NotifyIconW(msg, &data) != 0 };
            if ok {
                BALLOON_ADDED.store(true, Ordering::Relaxed);
            }
            ok
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (title, body);
            false
        }
    }
}

/// Removes the Windows balloon icon entry on shutdown if one was added.
pub fn shutdown() {
    #[cfg(target_os = "windows")]
    {
        use std::sync::atomic::Ordering;
        use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW};
        if !BALLOON_ADDED.swap(false, Ordering::Relaxed) {
            return;
        }
        let Some(hwnd) = crate::webview::parent_hwnd() else {
            return;
        };
        // SAFETY: zero-initialized `NOTIFYICONDATAW` with matching `hWnd` + `uID`.
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = BALLOON_ICON_UID;
        // SAFETY: removes the icon we previously registered with `NIM_ADD`.
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }
}

/// Rolling rate-limiter for OS notifications.
static NOTIFY_TIMESTAMPS: Mutex<Vec<f64>> = Mutex::new(Vec::new());

/// Checks and records whether an OS notification may fire at frame clock `now_s`.
pub fn rate_allows(now_s: f64) -> bool {
    let now_s = if now_s.is_finite() { now_s } else { 0.0 };
    let mut times = NOTIFY_TIMESTAMPS.lock().unwrap_or_else(|p| p.into_inner());
    times.retain(|at| now_s - at < OS_NOTIFY_WINDOW_S);
    if times.len() >= MAX_OS_NOTIFICATIONS_PER_MINUTE {
        return false;
    }
    times.push(now_s);
    true
}

/// Resets the OS notification rate limiter (tests).
#[cfg(test)]
pub fn reset_rate_limit() {
    NOTIFY_TIMESTAMPS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear();
}

/// Dispatches an OS notification only if not snoozed and within the rolling rate cap.
pub fn dispatch_if_allowed(
    snoozed: bool,
    now_s: f64,
    notifier: &dyn OsNotifier,
    app_name: &str,
    title: &str,
    body: Option<&str>,
) -> bool {
    if snoozed || !rate_allows(now_s) {
        return false;
    }
    notifier.notify(app_name, title, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeNotifier(RefCell<Vec<(String, String, Option<String>)>>);

    impl OsNotifier for FakeNotifier {
        fn notify(&self, app_name: &str, title: &str, body: Option<&str>) -> bool {
            self.0.borrow_mut().push((
                app_name.to_owned(),
                title.to_owned(),
                body.map(str::to_owned),
            ));
            true
        }
    }

    #[test]
    fn quns_dnd_mapping_covers_busy_fullscreen_presentation_and_quiet_time() {
        assert!(quns_is_dnd(QUNS_BUSY));
        assert!(quns_is_dnd(QUNS_RUNNING_D3D_FULL_SCREEN));
        assert!(quns_is_dnd(QUNS_PRESENTATION_MODE));
        assert!(quns_is_dnd(QUNS_QUIET_TIME));
        assert!(!quns_is_dnd(QUNS_ACCEPTS_NOTIFICATIONS));
        assert!(!quns_is_dnd(QUNS_NOT_PRESENT));
        assert!(!quns_is_dnd(QUNS_APP));
    }

    #[test]
    fn sanitize_and_utf16_copy_preserve_surrogates_and_nul() {
        let (title, body) = sanitize_notification("Gmail", "New\u{0}\nMail", Some("Hello\u{7}"));
        assert_eq!(title, "Gmail — NewMail");
        assert_eq!(body, "Hello");

        let mut buf = [0u16; 4]; // 3 UTF-16 code units + NUL
                                 // '😀' takes 2 UTF-16 code units: two of them would need 4 units -> only 1 fits.
        write_utf16_fixed(&mut buf, "😀😀");
        assert_eq!(buf[2], 0);
        assert_eq!(buf[3], 0);
        let decoded = String::from_utf16(&buf[..2]).expect("valid utf-16");
        assert_eq!(decoded, "😀");
    }

    #[test]
    fn dispatch_respects_snooze_and_rate_limit() {
        let _guard = crate::test_support::lock();
        reset_rate_limit();
        let fake = FakeNotifier(RefCell::new(Vec::new()));

        // Suppressed while snoozed
        assert!(!dispatch_if_allowed(
            true, 1.0, &fake, "Gmail", "Ping", None
        ));
        assert!(fake.0.borrow().is_empty());

        // Allowed up to cap
        for i in 0..MAX_OS_NOTIFICATIONS_PER_MINUTE {
            assert!(dispatch_if_allowed(
                false,
                10.0 + (i as f64) * 0.1,
                &fake,
                "Gmail",
                "Ping",
                Some("Body")
            ));
        }
        // Next one within 60 s is dropped
        assert!(!dispatch_if_allowed(
            false, 12.0, &fake, "Gmail", "Flood", None
        ));
        assert_eq!(fake.0.borrow().len(), MAX_OS_NOTIFICATIONS_PER_MINUTE);

        // Window passes -> allowed again
        assert!(dispatch_if_allowed(
            false,
            80.0,
            &fake,
            "Gmail",
            "After window",
            None
        ));
        reset_rate_limit();
    }
}
