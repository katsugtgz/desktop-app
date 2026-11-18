//! Durable session state for the native shell.
//!
//! One small JSON document holds what the shell restores on launch: window
//! placement, the active application, rail and dock order, installed
//! applications and tabs. The file carries an explicit [`FORMAT_VERSION`];
//! a version this build does not understand is refused instead of silently
//! discarded, and every [`SessionStore::save`] is atomic — a failure leaves
//! the previous session in place.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use appstore_service::{InstalledApplication, TabRecord};
use serde::{Deserialize, Serialize};

/// The only on-disk format this crate reads and writes.
pub const FORMAT_VERSION: u32 = 1;

/// The most a session document may occupy. A real one is a few KiB; anything near this is not a
/// session, and parsing it would only cost memory.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Caps on the collections a document may name. A real shell has a handful of apps; a hostile
/// document is refused instead of being materialized.
pub const MAX_INSTALLED: usize = 512;
pub const MAX_DOCK: usize = 512;
pub const MAX_TABS: usize = 2048;
pub const MAX_RAIL: usize = 512;

/// The window size a session may ask for, in points. Smaller is raised to the minimum, larger is
/// clamped: a hostile document cannot open a 10,000,000-pixel window.
///
/// The largest size a session may *store*, in points: 8192 keeps a 3840-pt 4K@100 % window and a
/// 5120×1440 ultrawide exactly as the user left them. This bound is scale-unaware by design — the
/// display scale is not part of the document — so the shell applies the device-pixel ceiling when
/// it restores the window (`station-shell`'s `restored_size`), not here.
pub const MIN_WINDOW_WIDTH: f32 = 320.0;
pub const MIN_WINDOW_HEIGHT: f32 = 240.0;
pub const MAX_WINDOW: f32 = 8192.0;

/// Where the window was and how big it was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WindowState {
    /// `None` when the shell has never placed the window (or the platform
    /// gave no position); the OS then chooses.
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            width: 1200.0,
            height: 800.0,
            maximized: false,
        }
    }
}

/// How the shell picks its appearance.
///
/// Added to format 1 after it shipped: `#[serde(default)]` keeps existing documents readable
/// (they open as `System`), and a build that does not know the field ignores it — so the format
/// version stays 1 rather than forcing a migration for an additive field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemePreference {
    /// Follow the desktop's appearance.
    #[default]
    System,
    Dark,
    Light,
}

/// Everything the shell restores on launch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionState {
    pub window: WindowState,
    pub theme: ThemePreference,
    pub active_application: Option<String>,
    /// Application ids in rail order.
    pub rail_order: Vec<String>,
    pub installed: Vec<InstalledApplication>,
    /// Application ids pinned to the dock.
    pub dock: Vec<String>,
    pub tabs: Vec<TabRecord>,
}

/// Why a session could not be read or written.
#[derive(Debug)]
pub enum StoreError {
    /// The file could not be read, written, or renamed.
    Io(io::Error),
    /// The bytes are not the JSON document this crate writes.
    Parse(serde_json::Error),
    /// The file was written by a newer (or older) format this build must
    /// not overwrite blindly.
    UnsupportedVersion(u32),
    /// The document is larger than [`MAX_FILE_BYTES`]; it is refused unread.
    TooLarge(u64),
    /// One collection is past its cap (see [`MAX_INSTALLED`] and friends).
    TooManyEntries {
        field: &'static str,
        count: usize,
        max: usize,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(error) => write!(f, "session store io error: {error}"),
            StoreError::Parse(error) => write!(f, "session store parse error: {error}"),
            StoreError::UnsupportedVersion(version) => write!(
                f,
                "session store version {version} is not supported (expected {FORMAT_VERSION})"
            ),
            StoreError::TooLarge(bytes) => write!(
                f,
                "session store file is {bytes} bytes; at most {MAX_FILE_BYTES} is read"
            ),
            StoreError::TooManyEntries { field, count, max } => {
                write!(
                    f,
                    "session store has {count} {field} entries; at most {max} is accepted"
                )
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Io(error) => Some(error),
            StoreError::Parse(error) => Some(error),
            StoreError::UnsupportedVersion(_)
            | StoreError::TooLarge(_)
            | StoreError::TooManyEntries { .. } => None,
        }
    }
}

/// A session document at a fixed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The stored session, or the default one when no file exists yet.
    ///
    /// A future [`FORMAT_VERSION`] is reported as
    /// [`StoreError::UnsupportedVersion`]; it is never replaced by a default.
    pub fn load(&self) -> Result<SessionState, StoreError> {
        // The size gate runs before the read: a 4 GiB "session" is refused, never loaded.
        let size = match fs::metadata(&self.path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SessionState::default())
            }
            Err(error) => return Err(StoreError::Io(error)),
        };
        if size > MAX_FILE_BYTES {
            return Err(StoreError::TooLarge(size));
        }
        let bytes = fs::read(&self.path).map_err(StoreError::Io)?;
        let header: VersionHeader = serde_json::from_slice(&bytes).map_err(StoreError::Parse)?;
        if header.version != FORMAT_VERSION {
            return Err(StoreError::UnsupportedVersion(header.version));
        }
        // Unknown fields (including `version`) are ignored by serde.
        let mut state: SessionState = serde_json::from_slice(&bytes).map_err(StoreError::Parse)?;
        sanitize(&mut state)?;
        Ok(state)
    }

    /// Atomically replace the stored session: write a sibling temp file,
    /// fsync it, then rename it over `path`. On failure the previous file is
    /// untouched and the temp file is removed.
    pub fn save(&self, state: &SessionState) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec_pretty(&WireState {
            version: FORMAT_VERSION,
            state,
        })
        .map_err(StoreError::Parse)?;
        write_atomically(&self.path, &bytes).map_err(StoreError::Io)
    }
}

/// Brings a parsed document into the range the shell can use: window sizes are clamped to
/// something a desktop can show, and collections past their caps are refused (a refusal is safer
/// than truncating a session whose dock and tabs would then point at dropped applications).
fn sanitize(state: &mut SessionState) -> Result<(), StoreError> {
    state.window.width = sane_length(state.window.width, 1200.0, MIN_WINDOW_WIDTH);
    state.window.height = sane_length(state.window.height, 800.0, MIN_WINDOW_HEIGHT);
    for (field, count, max) in [
        ("installed", state.installed.len(), MAX_INSTALLED),
        ("dock", state.dock.len(), MAX_DOCK),
        ("tab", state.tabs.len(), MAX_TABS),
        ("rail", state.rail_order.len(), MAX_RAIL),
    ] {
        if count > max {
            return Err(StoreError::TooManyEntries { field, count, max });
        }
    }
    Ok(())
}

/// A window length: a non-finite or non-positive value falls back to the default, and anything
/// outside `[min, MAX_WINDOW]` is clamped into it.
fn sane_length(value: f32, fallback: f32, min: f32) -> f32 {
    if !value.is_finite() || value <= 0.0 {
        fallback
    } else {
        value.clamp(min, MAX_WINDOW)
    }
}

/// The on-disk document: the format version plus the state, flattened so the
/// state's own field names stay top-level (and camelCase).
#[derive(Serialize)]
struct WireState<'a> {
    version: u32,
    #[serde(flatten)]
    state: &'a SessionState,
}

/// The version gate. Parsed before the state so a newer document is refused
/// rather than coerced into a partial `SessionState`.
#[derive(Deserialize)]
struct VersionHeader {
    version: u32,
}

/// Write `bytes` to `path` via a sibling temp file and a rename.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }
    let directory = parent.unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "session store path has no file name",
        )
    })?;
    let temp = directory.join(temp_name(file_name));

    let result = write_and_rename(&temp, path, bytes);
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_and_rename(temp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp, path)
}

/// A sibling name unlikely to collide with another writer, including a
/// crashed one whose counter restarted.
fn temp_name(file_name: &std::ffi::OsStr) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!(
        ".{}.{}.{nanos}.{serial}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    )
}
