//! The notification-area icon and its two-item menu.
//!
//! The pinned Rune rev ships [`rune::Tray`]: `Shell_NotifyIcon` on a message-only window that
//! owns its thread and message loop, a `TrackPopupMenu` menu, `TaskbarCreated` recovery, and a
//! `Send + Sync` event handler (`tray/win32.rs` in Rune). The shell uses that rather than a
//! second hand-written Win32 loop: it is the mechanism the pinned Rune exposes safely, it keeps
//! tray callbacks off the window thread, and the handler can own the `AppHandle` this app
//! already has — so **Open** takes the existing focus path ([`rune::AppHandle::focus`]) and
//! **Exit** the existing quit path (`on_quit`: save the session, close the pages, shut the
//! instance listener down). There is no second shutdown path.
//!
//! The menu is the Electron tray's (`packages/app/src/services/services/electron-app/main.ts`):
//! Open, Exit. No settings entry is invented. Electron listened for `double-click`; Rune's tray
//! reports one primary click as [`rune::TrayEvent::Activate`], so a single primary click focuses
//! here — the same action, one click earlier — and a right click opens the menu.
//!
//! Windows only: the shell calls [`start`] only there. On other platforms Rune reports
//! `TrayError::Unsupported` and no tray is started (the web view's discipline: no stub).

#[cfg(target_os = "windows")]
use crate::state::ShellState;

/// The tooltip Electron set: `tray.setToolTip('Station')`.
pub(crate) const TOOLTIP: &str = "Station";

/// Station's icon asset for the tray (identical bytes to the window icon's
/// `packages/app/build/icon.ico`), embedded rather than read at run time.
pub(crate) static TRAY_ICO: &[u8] = include_bytes!("../../../assets/station.ico");

/// What a tray event asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Show/focus the window through `rune::AppHandle::focus` (restores a minimized window).
    Open,
    /// Quit through `rune::AppHandle::quit` (`on_quit` runs once).
    Quit,
}

/// One menu row: the id Rune reports back, the Electron label, and the command it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MenuItem {
    pub id: &'static str,
    pub label: &'static str,
    pub enabled: bool,
    pub command: Command,
}

/// The tray menu, in order: exactly Electron's `Open` and `Exit`.
pub(crate) const MENU: [MenuItem; 2] = [
    MenuItem {
        id: "open",
        label: "Open",
        enabled: true,
        command: Command::Open,
    },
    MenuItem {
        id: "quit",
        label: "Exit",
        enabled: true,
        command: Command::Quit,
    },
];

/// The tray menu as Rune takes it.
pub(crate) fn rune_menu() -> Vec<rune::TrayItem> {
    MENU.iter()
        .map(|item| rune::TrayItem::action(item.id, item.label).enabled(item.enabled))
        .collect()
}

/// Maps a tray event to the app action, or `None` for an event the shell has no work for.
pub(crate) fn command_for(event: &rune::TrayEvent) -> Option<Command> {
    match event {
        rune::TrayEvent::Activate { .. } => Some(Command::Open),
        rune::TrayEvent::Menu(id) => MENU
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.command),
        _ => None,
    }
}

/// The icon's side, following the notification area's small-icon metric at the current DPI.
#[cfg(target_os = "windows")]
fn icon_size() -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    // SAFETY: a pure system-metric read.
    let metric = unsafe { GetSystemMetrics(SM_CXSMICON) };
    u32::try_from(metric).unwrap_or(16).max(16)
}

/// Starts the tray on Rune's tray thread. Returns the [`rune::Tray`], which must be kept alive
/// for as long as the icon should stay in the notification area; dropping it removes the icon.
/// A failure (no icon bytes, no shell tray) is one stderr line and no tray — never a fatal path.
#[cfg(target_os = "windows")]
pub fn start(handle: rune::AppHandle<ShellState>) -> Option<rune::Tray> {
    use rune::{Tray, TrayIcon, TraySpec};
    let Some(pixels) = crate::ico::rgba(TRAY_ICO, icon_size()) else {
        eprintln!("station-shell: tray icon ICO did not decode; no tray icon");
        return None;
    };
    let spec = TraySpec::new(
        "station",
        TOOLTIP,
        TrayIcon::rgba(pixels.width, pixels.height, pixels.rgba),
    )
    .tooltip(TOOLTIP)
    .menu(rune_menu());
    match Tray::new(spec, move |event| {
        let Some(command) = command_for(&event) else {
            return;
        };
        match command {
            Command::Open => handle.focus(),
            Command::Quit => handle.quit(),
        }
    }) {
        Ok(tray) => Some(tray),
        Err(why) => {
            eprintln!("station-shell: tray icon unavailable: {why}");
            None
        }
    }
}
