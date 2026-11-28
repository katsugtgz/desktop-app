//! Local crash reporting: installs a panic hook that writes a bounded, sanitized
//! crash report under `<config dir>/station/crashes/` so post-mortem diagnostics
//! exist without shipping telemetry or leaking secrets.
//!
//! Security & bounds:
//! - Reports are capped at [`MAX_REPORT_BYTES`] (8 KiB).
//! - At most [`MAX_CRASH_FILES`] (20) files are retained; oldest are pruned.
//! - Environment variables, session contents, and tokens are never written.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum bytes written for a single crash report.
pub const MAX_REPORT_BYTES: usize = 8192;

/// Maximum crash files retained on disk before oldest files are pruned.
pub const MAX_CRASH_FILES: usize = 20;

/// Directory where crash reports are saved: beside `session.json` under `crashes/`.
pub fn crash_dir() -> PathBuf {
    crate::session::path()
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("crashes")
}

/// Formats a sanitized, bounded crash report body.
pub fn format_report(
    timestamp_ms: u128,
    pid: u32,
    location: Option<(&str, u32, u32)>,
    message: &str,
) -> String {
    let loc = match location {
        Some((file, line, col)) => format!("{file}:{line}:{col}"),
        None => "unknown".to_owned(),
    };
    let clean_msg: String = message
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect();
    let mut out = format!(
        "Station Native Shell Crash Report\n\
         version: {}\n\
         os: {} ({})\n\
         timestamp_ms: {timestamp_ms}\n\
         pid: {pid}\n\
         location: {loc}\n\
         message: {clean_msg}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    if out.len() > MAX_REPORT_BYTES {
        let mut end = MAX_REPORT_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

/// Writes `report` into `dir` and prunes older reports beyond `max_files`.
pub fn write_report(
    dir: &Path,
    timestamp_ms: u128,
    pid: u32,
    report: &str,
    max_files: usize,
) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let filename = format!("crash-{timestamp_ms:020}-{pid}.txt");
    let path = dir.join(filename);
    let capped = if report.len() > MAX_REPORT_BYTES {
        let mut end = MAX_REPORT_BYTES;
        while !report.is_char_boundary(end) {
            end -= 1;
        }
        &report[..end]
    } else {
        report
    };
    fs::write(&path, capped.as_bytes())?;
    let _ = prune_old_reports(dir, max_files);
    Ok(path)
}

/// Removes the oldest `crash-*.txt` files in `dir` when the count exceeds `max_files`.
pub fn prune_old_reports(dir: &Path, max_files: usize) -> io::Result<usize> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt"))
        })
        .collect();
    if files.len() <= max_files {
        return Ok(0);
    }
    // Filenames carry zero-padded millisecond timestamps, so lexicographic order is chronological.
    files.sort();
    let to_remove = files.len() - max_files;
    let mut removed = 0;
    for path in files.into_iter().take(to_remove) {
        if fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Installs the shell's panic hook once at startup (chaining the existing hook).
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let pid = std::process::id();
        let location = info
            .location()
            .map(|loc| (loc.file(), loc.line(), loc.column()));
        let message = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_owned()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_owned()
        };
        let report = format_report(ts, pid, location, &message);
        let dir = crash_dir();
        if let Ok(path) = write_report(&dir, ts, pid, &report, MAX_CRASH_FILES) {
            eprintln!("station-shell: crash report written to {}", path.display());
        }
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_sanitized_and_capped() {
        let huge = format!("bad\u{0}msg {}", "x".repeat(MAX_REPORT_BYTES * 2));
        let report = format_report(12345, 99, Some(("src/main.rs", 10, 2)), &huge);
        assert!(report.len() <= MAX_REPORT_BYTES);
        assert!(report.contains("src/main.rs:10:2"));
        assert!(!report.contains('\u{0}'));
    }

    #[test]
    fn write_and_prune_keeps_newest_reports() {
        let dir = std::env::temp_dir().join(format!("station-crash-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for i in 1..=5u128 {
            write_report(&dir, i * 1000, 1, "boom", 3).unwrap();
        }
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        assert_eq!(names.len(), 3);
        assert!(names[0].contains("00000000000000003000"));
        assert!(names[2].contains("00000000000000005000"));
        let _ = fs::remove_dir_all(&dir);
    }
}
