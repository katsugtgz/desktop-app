//! The window's icon: Station's own, on the title bar and the taskbar.
//!
//! Rune's pinned rev has no per-window icon setter (`WindowHandle` exposes placement, taskbar
//! progress and the raw handle — no icon), and `rune::platform::app_icon::set_app_icon` sets a
//! process-wide icon only *before* windows open (it feeds winit's `WindowAttributes`); calling
//! it from `on_window` — after the window exists — would do nothing. So the same rule as the
//! web view (D4) applies: one place a raw handle exists (`webview::window_created`, from
//! `App::on_window`), one `WM_SETICON` per icon size, and nothing else in the shell touches the
//! HWND.
//!
//! Source bytes: `packages/app/build/icon.ico`, the Electron build's own icon asset. An ICO
//! entry is already an `RT_ICON` resource — a 40-byte `BITMAPINFOHEADER` whose `biHeight` is
//! doubled for the XOR bitmap plus AND mask, followed by the pixels and the mask — which is
//! exactly what `CreateIconFromResourceEx` consumes. [`crate::ico`] parses the directory; the
//! 32×32 entry goes out as `ICON_SMALL` (title bar, small taskbar mode) and the 64×64 entry as
//! `ICON_BIG` (taskbar/Alt+Tab scales it per DPI), with the nearest entry at least that large as
//! the fallback. A failure is one stderr line; the shell keeps the default icon rather than
//! failing anything.
//!
//! Nothing is synthesised here: an earlier revision decoded `icon_512x512.png` and built a
//! BMP-shaped buffer by hand, which `CreateIconFromResourceEx` rejected at runtime. The ICO's
//! own entries are the resource bytes the call wants.

#![cfg(target_os = "windows")]

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, SendMessageW, ICON_BIG, ICON_SMALL, LR_DEFAULTCOLOR,
};

use crate::ico;

/// Station's icon, at compile time, from the Electron build's own asset.
static ICON_ICO: &[u8] = include_bytes!("../../../../packages/app/build/icon.ico");

/// The sizes one window gets: (width, height) pairs, small first.
const SIZES: [(u32, u32); 2] = [(32, 32), (64, 64)];

/// The ICO entry's own `RT_ICON` bytes turned into an `HICON`, or `None` on a failure.
fn hicon(size: u32) -> Option<*mut core::ffi::c_void> {
    let entries = ico::entries(ICON_ICO).ok()?;
    let entry = ico::entry_for(&entries, size)?;
    // SAFETY: the payload borrows the `static` ICO bytes and outlives the call;
    // `CreateIconFromResourceEx` copies what it needs before returning. The size and flags are
    // the documented icon ones.
    let icon = unsafe {
        CreateIconFromResourceEx(
            entry.payload.as_ptr(),
            entry.payload.len() as u32,
            1, // fIcon
            0x00030000,
            size as i32,
            size as i32,
            LR_DEFAULTCOLOR,
        )
    };
    (!icon.is_null()).then_some(icon)
}

/// Applies Station's icon to the window `raw` (rune's `RawWindowHandle::Win32`): the ICO's
/// 32×32 and 64×64 resource bytes, sent with one `WM_SETICON` each. Failure logs once and the
/// window keeps its default.
pub fn apply(raw: rune::raw_window_handle::RawWindowHandle) {
    let rune::raw_window_handle::RawWindowHandle::Win32(win32) = raw else {
        return;
    };
    let hwnd = win32.hwnd.get() as HWND;
    for (index, (width, height)) in SIZES.iter().copied().enumerate() {
        let Some(icon) = hicon(width) else {
            // SAFETY: no arguments; reads the calling thread's last-error value, which belongs
            // to the failed call just above.
            let error = unsafe { GetLastError() };
            eprintln!(
                "station-shell: window icon {width}×{height} could not be created (error {error})"
            );
            continue;
        };
        let message = if index == 0 { ICON_SMALL } else { ICON_BIG };
        // SAFETY: a live window handle from `on_window`, and an HICON the window keeps (the
        // icon intentionally leaks for the window's life — the OS owns it once sent, and the
        // shell creates one window per process).
        unsafe {
            SendMessageW(
                hwnd,
                0x0080, /* WM_SETICON */
                message as WPARAM,
                icon as LPARAM,
            );
        }
    }
}
