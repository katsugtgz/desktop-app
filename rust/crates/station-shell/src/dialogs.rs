//! Native fatal error dialogs (Windows `MessageBoxW`, stderr fallback elsewhere).
//!
//! Page load failures stay inline inside the Rune pane (`placeholder::page_error`),
//! while unrecoverable startup/runtime errors use this bounded OS dialog so a user
//! launching without a console still sees why Station could not continue.

/// Maximum length of a dialog title in characters.
pub const MAX_TITLE_CHARS: usize = 128;

/// Maximum length of a dialog body message in characters.
pub const MAX_MESSAGE_CHARS: usize = 2048;

/// Sanitizes and bounds `(title, message)` before showing a native dialog:
/// control characters are stripped (newlines preserved in `message`), empty
/// titles fall back to `"Station"`.
pub fn sanitize(title: &str, message: &str) -> (String, String) {
    let clean_title: String = title
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TITLE_CHARS)
        .collect::<String>()
        .trim()
        .to_owned();
    let clean_title = if clean_title.is_empty() {
        "Station".to_owned()
    } else {
        clean_title
    };

    let clean_message: String = message
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(MAX_MESSAGE_CHARS)
        .collect::<String>()
        .trim()
        .to_owned();

    (clean_title, clean_message)
}

/// Presents a fatal error to the user: modal `MessageBoxW` on Windows, stderr elsewhere.
pub fn show_fatal_error(title: &str, message: &str) {
    let (title, message) = sanitize(title, message);
    eprintln!("station-shell fatal [{title}]: {message}");

    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let wide_title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        let wide_msg: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
        // SAFETY: both strings are NUL-terminated UTF-16 buffers valid for the call; null HWND
        // displays a standalone desktop-owned error box.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                wide_msg.as_ptr(),
                wide_title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_control_chars_and_bounds_lengths() {
        let (title, msg) = sanitize("Bad\u{0}\nTitle", "line1\nline2\u{7}\u{0}");
        assert_eq!(title, "BadTitle");
        assert_eq!(msg, "line1\nline2");

        let (empty_title, _) = sanitize("   \u{1} ", "ok");
        assert_eq!(empty_title, "Station");

        let long = "a".repeat(5000);
        let (t, m) = sanitize(&long, &long);
        assert_eq!(t.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(m.chars().count(), MAX_MESSAGE_CHARS);
    }
}
