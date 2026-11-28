//! "Launch Station at sign-in": the Electron login item, written natively.
//!
//! `packages/app/src/services/services/autolaunch/main.ts` delegates to `auto-launch` 5.0.6.
//! Its Windows backend (read from the Yarn cache) puts one `REG_SZ` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, named after the app (`"Station"` here),
//! whose data is the executable path **quoted**, with no arguments — the Squirrel
//! `update.exe --processStart …` branch is Electron packaging this build does not have.
//! `isEnabled` means the value exists; disabling removes it and a missing value is success.
//! This module keeps that exactly.
//!
//! The write is `HKCU`, so it needs no elevation. On platforms without that key the item is
//! `Unsupported`: the Settings row is not drawn and no stub pretends otherwise.
//!
//! Caps: a command longer than [`MAX_COMMAND_CHARS`] UTF-16 units is refused rather than
//! truncated (the registry value has a size limit, and a truncated path must never be written);
//! reads use a fixed buffer and treat a longer stored value as "present".

use std::path::Path;

/// The `Run` value name the Electron build used: `app.name` (`productName` "Station").
pub(crate) const VALUE_NAME: &str = "Station";

/// The per-user login key. `HKCU` makes this an unelevated write.
pub(crate) const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The longest command this shell writes, in UTF-16 units (terminator excluded). A Windows path
/// is far below this; the cap is what keeps the value bounded.
pub(crate) const MAX_COMMAND_CHARS: usize = 4096;

/// The error text the non-Windows item reports; the Settings row is absent there, so this is
/// reached only by a caller that asks the item directly.
#[cfg(not(target_os = "windows"))]
pub(crate) const UNSUPPORTED: &str = "launching at sign-in is not supported on this platform";

/// Whether this build can manage a login item (the row is drawn only then).
pub(crate) fn supported() -> bool {
    cfg!(target_os = "windows")
}

/// The exact value data for `exe`: the path in double quotes, no arguments. Kept separate from
/// the registry call so the quoting is pinned by tests.
pub(crate) fn command_line(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// A login item: the registry on Windows, an in-memory fake in tests.
pub(crate) trait LoginItem {
    /// Whether the item exists (the TS `auto-launch.isEnabled`).
    fn enabled(&self) -> Result<bool, String>;
    /// Create the item pointing at this executable, or remove it.
    fn set(&self, enable: bool) -> Result<(), String>;
}

/// The real login item: the current executable under `HKCU`.
pub(crate) struct SystemLoginItem;

impl LoginItem for SystemLoginItem {
    fn enabled(&self) -> Result<bool, String> {
        #[cfg(target_os = "windows")]
        {
            win::enabled()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(UNSUPPORTED.to_owned())
        }
    }

    fn set(&self, enable: bool) -> Result<(), String> {
        #[cfg(target_os = "windows")]
        {
            let exe = std::env::current_exe()
                .map_err(|error| format!("could not resolve the shell executable: {error}"))?;
            win::set(enable, &exe)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = enable;
            Err(UNSUPPORTED.to_owned())
        }
    }
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn failure(what: &str, status: u32) -> String {
        format!("{what} failed (Win32 error {status})")
    }

    pub(super) fn enabled() -> Result<bool, String> {
        let key = wide(RUN_KEY);
        let value = wide(VALUE_NAME);
        let mut kind = 0u32;
        let mut buffer = [0u16; MAX_COMMAND_CHARS];
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: both names are NUL-terminated, and `buffer` is valid for `bytes` bytes;
        // `RegGetValueW` writes at most that many. A longer stored value reports ERROR_MORE_DATA.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        match status {
            ERROR_SUCCESS | ERROR_MORE_DATA => Ok(true),
            ERROR_FILE_NOT_FOUND => Ok(false),
            other => Err(failure("reading the login item", other)),
        }
    }

    pub(super) fn set(enable: bool, exe: &Path) -> Result<(), String> {
        if enable {
            write(exe)
        } else {
            remove()
        }
    }

    fn write(exe: &Path) -> Result<(), String> {
        let command = command_line(exe);
        let chars: Vec<u16> = command.encode_utf16().collect();
        if chars.len() > MAX_COMMAND_CHARS {
            return Err(format!(
                "the login item command is longer than {MAX_COMMAND_CHARS} characters"
            ));
        }
        let mut data = chars;
        data.push(0);
        let key = wide(RUN_KEY);
        let value = wide(VALUE_NAME);
        let mut hkey: HKEY = std::ptr::null_mut();
        // SAFETY: `key` is NUL-terminated; `hkey` receives an owned key handle on success.
        // `HKCU` and the default security descriptor make this an unelevated, per-user write.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut hkey,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(failure("opening the Run key", status));
        }
        // SAFETY: `hkey` is open, `value` is NUL-terminated, and `data` is a NUL-terminated
        // UTF-16 buffer of exactly this length.
        let status = unsafe {
            RegSetValueExW(
                hkey,
                value.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        // SAFETY: the key handle is open exactly once.
        unsafe { RegCloseKey(hkey) };
        if status != ERROR_SUCCESS {
            return Err(failure("writing the login item", status));
        }
        Ok(())
    }

    fn remove() -> Result<(), String> {
        let key = wide(RUN_KEY);
        let value = wide(VALUE_NAME);
        let mut hkey: HKEY = std::ptr::null_mut();
        // SAFETY: `key` is NUL-terminated; `hkey` receives an owned key handle on success.
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_SET_VALUE, &mut hkey) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if status != ERROR_SUCCESS {
            return Err(failure("opening the Run key", status));
        }
        // SAFETY: `hkey` is open, and `value` is NUL-terminated.
        let status = unsafe { RegDeleteValueW(hkey, value.as_ptr()) };
        // SAFETY: the key handle is open exactly once.
        unsafe { RegCloseKey(hkey) };
        match status {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            other => Err(failure("removing the login item", other)),
        }
    }
}
