//! Explicit least-privilege permission policy for hosted web views.
//!
//! Addresses the `docs/electron-retirement.md` finding that neither Electron nor
//! the earlier native shell wired a permission handler:
//! - Web `Notifications` are denied at the WebView2 layer because `INIT_SCRIPT`
//!   overrides `window.Notification` and routes notifications through Station's
//!   Notification Center (preserving snooze and rate caps).
//! - User-facing interactive permissions (`Camera`, `Microphone`, `DisplayCapture`,
//!   `Geolocation`, `ClipboardRead`, `Autoplay`) delegate to WebView2's native
//!   prompt (`Prompt` → `wry::PermissionResponse::Default`).
//! - Privileged / silent background capabilities (`AutomaticDownloads`,
//!   `FileSystemAccess`, `LocalFonts`, `WindowManagement`, `Midi`, `Sensors`,
//!   `PointerLock`, `Other`) are denied by default.

/// Platform-independent permission kinds mirroring `wry::PermissionKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    Microphone,
    Camera,
    Geolocation,
    Notifications,
    ClipboardRead,
    DisplayCapture,
    Midi,
    Sensors,
    MediaKeySystemAccess,
    LocalFonts,
    WindowManagement,
    PointerLock,
    AutomaticDownloads,
    FileSystemAccess,
    Autoplay,
    Other,
}

/// Decision returned to the web view backend (least-privilege: never silently auto-allow).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Deny,
    Prompt,
}

/// Pure permission decision function (cross-platform, unit-tested).
pub fn decide(kind: Permission) -> Decision {
    match kind {
        // Handled through Station's injected Notification bridge so snooze + caps hold.
        Permission::Notifications => Decision::Deny,
        // Interactive capabilities delegate to WebView2's native prompt.
        Permission::Microphone
        | Permission::Camera
        | Permission::DisplayCapture
        | Permission::Geolocation
        | Permission::ClipboardRead
        | Permission::Autoplay => Decision::Prompt,
        // Least-privilege default deny for silent/privileged APIs.
        Permission::Midi
        | Permission::Sensors
        | Permission::MediaKeySystemAccess
        | Permission::LocalFonts
        | Permission::WindowManagement
        | Permission::PointerLock
        | Permission::AutomaticDownloads
        | Permission::FileSystemAccess
        | Permission::Other => Decision::Deny,
    }
}

/// Maps `wry::PermissionKind` through [`decide`] into `wry::PermissionResponse`.
#[cfg(target_os = "windows")]
pub fn decide_wry(kind: wry::PermissionKind) -> wry::PermissionResponse {
    let mapped = match kind {
        wry::PermissionKind::Microphone => Permission::Microphone,
        wry::PermissionKind::Camera => Permission::Camera,
        wry::PermissionKind::Geolocation => Permission::Geolocation,
        wry::PermissionKind::Notifications => Permission::Notifications,
        wry::PermissionKind::ClipboardRead => Permission::ClipboardRead,
        wry::PermissionKind::DisplayCapture => Permission::DisplayCapture,
        wry::PermissionKind::Midi => Permission::Midi,
        wry::PermissionKind::Sensors => Permission::Sensors,
        wry::PermissionKind::MediaKeySystemAccess => Permission::MediaKeySystemAccess,
        wry::PermissionKind::LocalFonts => Permission::LocalFonts,
        wry::PermissionKind::WindowManagement => Permission::WindowManagement,
        wry::PermissionKind::PointerLock => Permission::PointerLock,
        wry::PermissionKind::AutomaticDownloads => Permission::AutomaticDownloads,
        wry::PermissionKind::FileSystemAccess => Permission::FileSystemAccess,
        wry::PermissionKind::Autoplay => Permission::Autoplay,
        _ => Permission::Other,
    };
    match decide(mapped) {
        Decision::Deny => wry::PermissionResponse::Deny,
        Decision::Prompt => wry::PermissionResponse::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_notifications_and_privileged_apis_are_denied() {
        assert_eq!(decide(Permission::Notifications), Decision::Deny);
        assert_eq!(decide(Permission::AutomaticDownloads), Decision::Deny);
        assert_eq!(decide(Permission::WindowManagement), Decision::Deny);
        assert_eq!(decide(Permission::FileSystemAccess), Decision::Deny);
        assert_eq!(decide(Permission::LocalFonts), Decision::Deny);
        assert_eq!(decide(Permission::Midi), Decision::Deny);
        assert_eq!(decide(Permission::Sensors), Decision::Deny);
        assert_eq!(decide(Permission::Other), Decision::Deny);
    }

    #[test]
    fn interactive_media_and_clipboard_delegate_to_prompt() {
        for kind in [
            Permission::Camera,
            Permission::Microphone,
            Permission::DisplayCapture,
            Permission::Geolocation,
            Permission::ClipboardRead,
            Permission::Autoplay,
        ] {
            assert_eq!(decide(kind), Decision::Prompt, "{kind:?} should prompt");
        }
    }
}
