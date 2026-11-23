//! Where a page's download lands (slice 22): the decision, not the COM wiring.
//!
//! wry 0.57.2 already wraps WebView2's download events, so this crate needs no raw COM and no
//! `with_webview` closure: `WebViewBuilder::with_download_started_handler` registers
//! `ICoreWebView2_4::add_DownloadStarting` on the page's own `ICoreWebView2`
//! (`wry-0.57.2/src/webview2/mod.rs:879-951`), reads the event's
//! `ICoreWebView2DownloadStartingEventArgs::ResultFilePath` — WebView2's default target, the
//! OS Downloads folder plus the response's suggested name — puts it through the closure, and
//! writes the answer back with `SetResultFilePath` + `SetHandled(true)` (allow) or
//! `SetCancel(true)` (refuse). Completion arrives through
//! `ICoreWebView2DownloadOperation::add_StateChanged` with `State`/`ResultFilePath`
//! (`:892-928`). The event token lives in that page's wry view, so the handler is registered
//! when the page is created and gone when the page is dropped.
//!
//! What is *not* wry's job is the trust decision, and that is here:
//!
//! - the **suggested name** is page-controlled. It is reduced to one path component, stripped of
//!   control characters, Windows-illegal characters replaced, reserved device names refused,
//!   trailing dots/spaces removed and the whole name capped at [`MAX_NAME_CHARS`];
//! - the **directory** is the OS Downloads folder ([`downloads_dir`]), never a path the page
//!   supplied;
//! - a name that already exists — on disk or reserved by a download still in flight — gets a
//!   ` (1)`, ` (2)` … suffix, up to [`MAX_CONFLICT_ATTEMPTS`]; nothing is overwritten;
//! - at most [`MAX_CONCURRENT`] downloads are accepted shell-wide; a page cannot make the shell
//!   stat the disk without bound.
//!
//! A visible toaster, cancel/pause/resume and "open file" are a later slice: this slice accepts,
//! places and reports. The log lines are `webview`'s existing bounded ones
//! ([`crate::webview::download_line`], at most [`crate::webview::MAX_DOWNLOAD_LINES`] per run).

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

/// The most downloads the shell accepts at once, across every page.
pub const MAX_CONCURRENT: usize = 4;

/// The longest file name this shell creates, in characters, suffix included. Windows' own limit
/// is 255; the remainder leaves room for the Downloads directory inside `MAX_PATH`.
pub const MAX_NAME_CHARS: usize = 120;

/// How many conflict suffixes are tried before the download is refused: ` (1)` … ` (64)`.
pub const MAX_CONFLICT_ATTEMPTS: u32 = 64;

/// The most URL characters a reservation keeps for matching its completion. The URL is only
/// compared, never turned into a path.
const MAX_RESERVED_URL_CHARS: usize = 2048;

/// The longest extension a name keeps when it is shortened. A longer "extension" is treated as
/// part of the stem and truncated with it.
const MAX_EXTENSION_CHARS: usize = 16;

/// Windows device names: a file with one of these stems is not creatable on a normal path.
const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Why a download was refused. Each variant has a fixed line for the log; page text is never
/// interpolated into it (the URL and the name go through the bounded log line instead).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The suggested name (and the URL's) had nothing usable after sanitizing.
    NoName,
    /// [`MAX_CONCURRENT`] downloads are already reserved.
    TooManyConcurrent,
    /// Every candidate name up to [`MAX_CONFLICT_ATTEMPTS`] is taken.
    NoFreeName,
    /// The OS has no Downloads folder and no home directory to fall back to.
    NoDownloadsFolder,
}

impl Refusal {
    /// The fixed log fragment for a refusal.
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::NoName => "the page's suggested name is not usable",
            Refusal::TooManyConcurrent => "too many downloads in flight",
            Refusal::NoFreeName => "no free file name",
            Refusal::NoDownloadsFolder => "no Downloads folder",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.reason())
    }
}

/// The OS's Downloads folder, or the conventional `~/Downloads` when the OS has no known folder.
/// Never a literal path and never anything a page chose.
pub fn downloads_dir() -> Option<PathBuf> {
    dirs::download_dir().or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
}

/// One suggested name reduced to a name this shell will create, or `None` when nothing usable is
/// left. Reduction order: last path component, control characters dropped, `< > : " | ? *`
/// replaced with `_`, leading whitespace and trailing whitespace/dots removed, reserved device
/// names refused, then capped at [`MAX_NAME_CHARS`] with the extension kept.
pub fn sanitize_file_name(suggested: &str) -> Option<String> {
    // A page may suggest `..\..\Windows\System32\evil.exe`; only the last component survives.
    // A component of only separators (`/` or `\\`) leaves nothing.
    let component = suggested
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())?;

    let mut cleaned = String::with_capacity(component.len());
    for character in component.chars() {
        if character.is_control() {
            continue;
        }
        match character {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => cleaned.push('_'),
            _ => cleaned.push(character),
        }
    }

    // Windows strips trailing dots and spaces from a created file's name; do the same here so the
    // name chosen is the name on disk. Leading whitespace is dropped too (it is legal but always
    // an accident in a suggested name).
    let cleaned = cleaned.trim_start();
    let cleaned = cleaned.trim_end_matches([' ', '.']);
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return None;
    }

    let (stem, extension) = split_extension(cleaned);
    // "CON" and "con.txt" are the same device on Windows; the stem is what counts.
    if RESERVED_NAMES.contains(
        &stem
            .trim_end_matches([' ', '.'])
            .to_ascii_uppercase()
            .as_str(),
    ) {
        return None;
    }

    fit(stem, extension, "")
}

/// A free target under `dir` for `suggested`, or `None` when every candidate up to
/// [`MAX_CONFLICT_ATTEMPTS`] is taken. `taken` is the caller's notion of "in use" (the file
/// exists, or another download reserved the name); it is never called for a candidate beyond the
/// attempt cap, so a page cannot make the shell stat the disk without bound.
pub fn unique_target(
    dir: &Path,
    suggested: &str,
    taken: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let name = sanitize_file_name(suggested)?;
    let (stem, extension) = split_extension(&name);

    let first = dir.join(&name);
    if !taken(&first) {
        return Some(first);
    }

    for attempt in 1..=MAX_CONFLICT_ATTEMPTS {
        let suffix = format!(" ({attempt})");
        let candidate = fit(stem, extension, &suffix)?;
        let path = dir.join(candidate);
        if !taken(&path) {
            return Some(path);
        }
    }
    None
}

/// The whole decision for one download, as a function of the directory, the page's suggestion,
/// the in-flight count and the caller's "taken" predicate. Pure: no disk access beyond `taken`.
pub fn plan(
    dir: &Path,
    suggested: Option<&str>,
    in_flight: usize,
    taken: impl Fn(&Path) -> bool,
) -> Result<PathBuf, Refusal> {
    if in_flight >= MAX_CONCURRENT {
        return Err(Refusal::TooManyConcurrent);
    }
    let name = suggested
        .and_then(sanitize_file_name)
        .ok_or(Refusal::NoName)?;
    unique_target(dir, &name, taken).ok_or(Refusal::NoFreeName)
}

/// The last non-empty path segment of a URL, as a candidate name: `https://x/y/report.pdf?t=1`
/// → `report.pdf`. No decoding and no parsing library — [`sanitize_file_name`] is what decides.
pub fn name_from_url(url: &str) -> Option<String> {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    // Only a hierarchical URL has a last segment: `https://x/y/report.pdf` does, while
    // `data:,`, `about:blank` and `javascript:…` are their own payload.
    let index = without_query.find("://")?;
    without_query[index + 3..]
        .rsplit('/')
        .find(|part| !part.is_empty())
        .map(str::to_owned)
}

/// The names reserved by downloads that have started and not yet completed: the reservation
/// keeps two simultaneous downloads of `report.pdf` from choosing the same target, which the
/// filesystem alone cannot prevent before WebView2 creates the file.
static RESERVED: LazyLock<Mutex<Vec<(PathBuf, String)>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn reservations() -> MutexGuard<'static, Vec<(PathBuf, String)>> {
    // A page's input never holds this lock across a panic-free section; a poisoned lock still
    // guards a consistent vector, so recovering is right.
    RESERVED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The wry started-handler's answer: choose `path`'s replacement and return whether to accept.
///
/// `path` is WebView2's default target (Downloads directory + suggested name). Its file name is
/// the page's suggestion; its directory is ignored. On acceptance `path` is rewritten to the
/// chosen target (absolute) and the name is reserved until [`finished`]; `false` makes wry call
/// `SetCancel(true)` and the download never starts.
pub fn accept(url: &str, path: &mut PathBuf) -> bool {
    let stored_url: String = url.chars().take(MAX_RESERVED_URL_CHARS).collect();
    let Some(dir) = downloads_dir() else {
        refuse(&stored_url, Refusal::NoDownloadsFolder);
        return false;
    };

    // The suggestion chain: what WebView2 proposed, else the URL's last segment. Nothing usable
    // refuses the download — the shell does not invent a name for a page that offers none.
    let suggested = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .or_else(|| name_from_url(url));

    let mut reserved = reservations();
    let outcome = plan(&dir, suggested.as_deref(), reserved.len(), |candidate| {
        candidate.exists() || reserved.iter().any(|(held, _)| same_path(held, candidate))
    });
    match outcome {
        Ok(target) => {
            reserved.push((target.clone(), stored_url));
            *path = target;
            true
        }
        Err(refusal) => {
            drop(reserved);
            refuse(&stored_url, refusal);
            false
        }
    }
}

/// The wry completed-handler's bookkeeping: release the reservation that completion belongs to.
/// Matched by path (verbatim or `\\?\`-simplified), then by file name, then by URL — WebView2's
/// completion path is the same target in the normal case, and the fallbacks keep one odd report
/// from leaking a slot for the rest of the run.
pub fn finished(url: &str, path: Option<&Path>, _success: bool) {
    let mut reserved = reservations();
    if let Some(path) = path {
        let simplified = simplify(path);
        if let Some(index) = reserved
            .iter()
            .position(|(held, _)| simplify(held) == simplified)
        {
            reserved.remove(index);
            return;
        }
        if let Some(name) = simplified.file_name().and_then(|name| name.to_str()) {
            if let Some(index) = reserved.iter().position(|(held, _)| {
                held.file_name()
                    .and_then(|held_name| held_name.to_str())
                    .is_some_and(|held_name| held_name.eq_ignore_ascii_case(name))
            }) {
                reserved.remove(index);
                return;
            }
        }
    }
    if let Some(index) = reserved.iter().position(|(_, held_url)| held_url == url) {
        reserved.remove(index);
    }
}

/// How many downloads are reserved right now (a test seam; the shell never asks).
pub fn in_flight() -> usize {
    reservations().len()
}

/// Empties the reservation list (tests only: a failed assertion mid-test would otherwise leave a
/// slot held for every later test in the process).
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    reservations().clear();
}

/// True when two paths name the same target: equal after stripping the Windows verbatim prefix.
fn same_path(left: &Path, right: &Path) -> bool {
    simplify(left) == simplify(right)
}

/// Strips a `\\?\` (or `\\?\UNC\`) verbatim prefix, which WebView2's completion path may carry
/// and our chosen path never does.
fn simplify(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

/// `(stem, extension-with-dot)`; the extension is only recognised when it is short enough to be
/// one, so `archive.tar.gz` keeps `.gz` and a name of dots keeps everything.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(index) if index > 0 => {
            let extension = &name[index..];
            let length = extension.chars().count();
            if length > 1 && length <= MAX_EXTENSION_CHARS + 1 {
                (&name[..index], extension)
            } else {
                (name, "")
            }
        }
        _ => (name, ""),
    }
}

/// `stem + suffix + extension`, shortened so the result is at most [`MAX_NAME_CHARS`], with the
/// extension kept when there is one. `None` when the suffix and extension leave no room.
fn fit(stem: &str, extension: &str, suffix: &str) -> Option<String> {
    let used = extension.chars().count() + suffix.chars().count();
    let room = MAX_NAME_CHARS.checked_sub(used)?;
    if room == 0 {
        return None;
    }
    let stem: String = stem.chars().take(room).collect();
    let stem = stem.trim_end_matches([' ', '.']);
    if stem.is_empty() {
        return None;
    }
    Some(format!("{stem}{suffix}{extension}"))
}

/// One bounded refusal line through the shell's existing download log (same 32-line budget as
/// the start/finish lines). The reason is a fixed phrase; only the URL is page text, and it is
/// stripped of control characters and capped before it reaches the line.
fn refuse(url: &str, refusal: Refusal) {
    let url: String = url
        .chars()
        .filter(|character| !character.is_control())
        .take(crate::webview::MAX_DOWNLOAD_TEXT_CHARS)
        .collect();
    crate::webview::log_download(format!(
        "station-shell: download refused: {url} ({})",
        refusal.reason()
    ));
}
