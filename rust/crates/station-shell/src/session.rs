//! Where the shell keeps its session, and the live window it reads placement from.
//!
//! The file is `session-store`'s JSON document: installed applications, the rail order, the
//! active application and the window's placement. `STATION_SESSION_PATH` overrides the path
//! (tests and smoke runs); otherwise it is `<config dir>/station/session.json` — `dirs`'s config
//! dir, the same convention `manifest-registry` keeps its private manifests under.
//!
//! Reading never writes: `ShellState::launch` loads once, and the file is only replaced after the
//! user acts (`session-store` writes atomically, so a failed save leaves the previous one).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use rune::WindowHandle;
use session_store::{SessionStore, WindowState};

/// The environment variable that points the shell at another session file.
pub const PATH_ENV: &str = "STATION_SESSION_PATH";

/// The largest window the shell *opens* with, in points. The display's scale is not known before
/// the window exists and the snapshot tooling defaults to 2×, so launch stays at a point cap that
/// is device-safe without the scale: 2048 pt is 4096 px at 2× and 8192 px at 4×, inside
/// [`MAX_DEVICE_PX`]. Large windows are restored by `apply` once the real scale is known.
pub const MAX_LAUNCH_POINTS: f32 = 2048.0;

/// The largest size the shell *restores*, in points — [`session_store::MAX_WINDOW`], repeated here
/// so the restore rule is self-contained.
pub const MAX_WINDOW_POINTS: f32 = 8192.0;

/// The largest device-pixel extent a frame may ask the renderer for:
/// **8192** px. Rune's headless snapshot reads the frame back into one buffer, and a 16 384²
/// frame is 1 GiB against wgpu's 256 MiB buffer limit (8192² × 4 B = 256 MiB exactly); textures
/// above 16 384 px are rejected too. Points × scale never exceed this bound.
pub const MAX_DEVICE_PX: f32 = 8192.0;

/// The window size to open with: `width` × `height` points, floored to sane defaults when
/// non-finite or non-positive, and capped so `points × scale` stays within [`MAX_DEVICE_PX`] and
/// `points` within [`MAX_LAUNCH_POINTS`]. At launch the scale is assumed to be 1 (it is unknown
/// before the window exists), so the launch point cap is the guard.
pub fn safe_window_size(width: f32, height: f32, scale: f32) -> (f32, f32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let cap = (MAX_DEVICE_PX / scale).min(MAX_LAUNCH_POINTS);
    let size = |value: f32, fallback: f32| {
        if value.is_finite() && value > 0.0 {
            value.min(cap)
        } else {
            fallback.min(cap)
        }
    };
    (size(width, 1200.0), size(height, 800.0))
}

/// The window size to restore once the display's scale is known: the stored points, floored to the
/// session's minimum, capped to [`MAX_WINDOW_POINTS`], and shrunk further when `points × scale`
/// would exceed [`MAX_DEVICE_PX`].
///
/// This is what keeps a legitimate large display intact (3840 pt at 1× stays 3840 pt, 1920 pt at
/// 2× stays 1920 pt) while a scale that cannot fit the window shrinks it instead of asking the
/// renderer for a texture it will refuse: 8192 pt at 2× becomes 4096 pt (8192 px), 16384 pt at 1×
/// becomes 8192 pt.
pub fn restored_size(width: f32, height: f32, scale: f32) -> (f32, f32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let cap = (MAX_DEVICE_PX / scale).min(MAX_WINDOW_POINTS);
    let size = |value: f32, fallback: f32, min: f32| {
        let value = if value.is_finite() && value > 0.0 {
            value
        } else {
            fallback
        };
        value.clamp(min, cap.max(min))
    };
    (
        size(width, 1200.0, session_store::MIN_WINDOW_WIDTH),
        size(height, 800.0, session_store::MIN_WINDOW_HEIGHT),
    )
}

/// Seconds since the process started (monotonic), for the save-failure log throttle.
pub fn seconds() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
}

/// The session file this process uses.
pub fn path() -> PathBuf {
    if let Some(path) = std::env::var_os(PATH_ENV) {
        return PathBuf::from(path);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("station")
        .join("session.json")
}

/// The window to open with: the stored placement, or the default when there is no readable
/// session. Unreadable sessions are reported once by `ShellState::launch`; this stays quiet
/// because it runs before the window exists and only needs a size.
pub fn window_state(path: &Path) -> WindowState {
    match SessionStore::new(path).load() {
        Ok(state) => state.window,
        Err(_) => WindowState::default(),
    }
}

thread_local! {
    /// The window this thread is showing (`App::on_window`), so a save can read where it is
    /// instead of caching a placement that moving the window would stale.
    static WINDOW: RefCell<Option<WindowHandle>> = const { RefCell::new(None) };
}

/// Remembers the window for placement reads (`App::on_window`).
pub fn remember(handle: &WindowHandle) {
    WINDOW.with(|window| *window.borrow_mut() = Some(handle.clone()));
}

/// The live window's placement as session state, once there is a window and the platform
/// reports one.
///
/// While maximized, what is stored is the frame the window restores to (Rune's
/// `WindowPlacement::restore`), with `maximized` set: the maximized content size is this
/// display's, and would be wrong on a smaller one next launch.
pub fn live_window_state() -> Option<WindowState> {
    let handle = WINDOW.with(|window| window.borrow().clone())?;
    let placement = handle.placement()?;
    let (position, size) = if placement.maximized {
        placement
            .restore
            .unwrap_or((placement.position, placement.content_size))
    } else {
        (placement.position, placement.content_size)
    };
    let scale = if placement.scale > 0.0 {
        placement.scale as f32
    } else {
        1.0
    };
    Some(WindowState {
        x: Some(position.0),
        y: Some(position.1),
        width: size.0 as f32 / scale,
        height: size.1 as f32 / scale,
        maximized: placement.maximized,
    })
}

/// Puts the window where the session left it (`App::on_window`). Size and maximized state come
/// from `AppSpec`/`App::maximized` at creation; only the position the OS would otherwise choose
/// needs applying here.
pub fn apply(handle: &WindowHandle, state: &WindowState) {
    if let (Some(x), Some(y)) = (state.x, state.y) {
        handle.set_position((x, y));
    }
    // The launch size was capped without knowing the scale; now it is known, restore the session's
    // own size where the display fits it (a 3840-pt 4K window comes back at 3840 pt) and shrink it
    // where it does not (8192 pt at 2× → 4096 pt).
    if let Some(placement) = handle.placement() {
        let launched = safe_window_size(state.width, state.height, 1.0);
        let restored = restored_size(state.width, state.height, placement.scale as f32);
        if restored != launched {
            handle.resize_instant(restored);
        }
    }
}
