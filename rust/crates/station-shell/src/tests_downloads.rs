//! Slice 22 tests: where a page's download lands.
//!
//! The COM half (`ICoreWebView2_4::add_DownloadStarting`, via wry's starter/completed closures) is
//! windowed-only and cannot run here. What *is* provable headlessly is everything that decides
//! the file: name sanitizing, conflict suffixing, the concurrency cap, the refusal reasons, the
//! URL fallback and the reservation that stops two simultaneous downloads from choosing one path.
//! `downloads::accept` is exercised for real against the OS Downloads folder — it chooses a path
//! and creates nothing (WebView2 creates the file), so these tests write no user file; every
//! accepted name is released before the test ends.

use std::fs;
use std::path::{Path, PathBuf};

use crate::downloads::{
    self, downloads_dir, plan, sanitize_file_name, unique_target, Refusal, MAX_CONCURRENT,
    MAX_NAME_CHARS,
};
use crate::test_support;

/// A scratch directory that no other test uses.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "station-shell-downloads-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

/// The reserved-list length without holding the lock beyond the read.
fn in_flight() -> usize {
    downloads::in_flight()
}

#[test]
fn separators_keep_only_the_last_component() {
    assert_eq!(
        sanitize_file_name("../../etc/passwd").as_deref(),
        Some("passwd")
    );
    assert_eq!(
        sanitize_file_name(r"..\..\Windows\System32\evil.exe").as_deref(),
        Some("evil.exe")
    );
    assert_eq!(
        sanitize_file_name("a/b\\c/report.pdf").as_deref(),
        Some("report.pdf")
    );
    // A suggestion that is only separators has no component left.
    assert_eq!(sanitize_file_name("/"), None);
    assert_eq!(sanitize_file_name(r"\\"), None);
}

#[test]
fn control_characters_are_dropped() {
    assert_eq!(
        sanitize_file_name("re\u{0}po\u{1}rt\u{7f}.pdf").as_deref(),
        Some("report.pdf")
    );
    assert_eq!(
        sanitize_file_name("a\nb\tc.txt").as_deref(),
        Some("abc.txt")
    );
}

#[test]
fn windows_illegal_characters_become_underscores() {
    assert_eq!(
        sanitize_file_name(r#"a<b>c:d"e|f?g*h.txt"#).as_deref(),
        Some("a_b_c_d_e_f_g_h.txt")
    );
}

#[test]
fn reserved_device_names_are_refused() {
    for name in [
        "CON",
        "con.txt",
        "PRN",
        "NUL.tar.gz",
        "COM1",
        "com9.log",
        "LPT1",
    ] {
        assert_eq!(sanitize_file_name(name), None, "{name:?} must be refused");
    }
    // Only the exact stem is a device name; a longer stem is an ordinary file.
    assert_eq!(
        sanitize_file_name("CONSOLE.txt").as_deref(),
        Some("CONSOLE.txt")
    );
    assert_eq!(
        sanitize_file_name("COM10.txt").as_deref(),
        Some("COM10.txt")
    );
    assert_eq!(sanitize_file_name("LPT0").as_deref(), Some("LPT0"));
}

#[test]
fn trailing_dots_and_spaces_are_trimmed() {
    assert_eq!(
        sanitize_file_name("report.pdf. . ").as_deref(),
        Some("report.pdf")
    );
    assert_eq!(
        sanitize_file_name("  spaced.txt  ").as_deref(),
        Some("spaced.txt")
    );
    // Trimming everything leaves nothing.
    assert_eq!(sanitize_file_name(" . . "), None);
    assert_eq!(sanitize_file_name("."), None);
    assert_eq!(sanitize_file_name(".."), None);
    assert_eq!(sanitize_file_name(""), None);
    assert_eq!(sanitize_file_name("   "), None);
}

#[test]
fn the_name_is_capped_and_keeps_its_extension() {
    let long = format!("{}.pdf", "a".repeat(10_000));
    let cleaned = sanitize_file_name(&long).expect("a long name is shortened, not refused");
    assert_eq!(cleaned.chars().count(), MAX_NAME_CHARS);
    assert!(cleaned.ends_with(".pdf"), "{cleaned}");

    // No extension: the whole name is capped.
    let plain = sanitize_file_name(&"b".repeat(500)).unwrap();
    assert_eq!(plain.chars().count(), MAX_NAME_CHARS);

    // An absurd "extension" is treated as stem text.
    let weird = sanitize_file_name(&format!("x.{}", "c".repeat(400))).unwrap();
    assert_eq!(weird.chars().count(), MAX_NAME_CHARS);
    assert!(weird.ends_with('c'));
}

#[test]
fn unicode_survives() {
    assert_eq!(
        sanitize_file_name("café-résumé.pdf").as_deref(),
        Some("café-résumé.pdf")
    );
    assert_eq!(
        sanitize_file_name("日本語.txt").as_deref(),
        Some("日本語.txt")
    );
}

#[test]
fn a_free_name_is_used_as_is() {
    let dir = scratch("free");
    let taken = |path: &Path| path.exists();
    assert_eq!(
        unique_target(&dir, "report.pdf", taken),
        Some(dir.join("report.pdf"))
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn an_existing_file_gets_a_suffix() {
    let dir = scratch("suffix");
    fs::write(dir.join("report.pdf"), b"one").unwrap();

    let taken = |path: &Path| path.exists();
    assert_eq!(
        unique_target(&dir, "report.pdf", taken),
        Some(dir.join("report (1).pdf"))
    );

    fs::write(dir.join("report (1).pdf"), b"two").unwrap();
    assert_eq!(
        unique_target(&dir, "report.pdf", taken),
        Some(dir.join("report (2).pdf"))
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_long_name_still_fits_with_a_suffix() {
    let stem = "n".repeat(MAX_NAME_CHARS);
    let name = format!("{stem}.pdf");
    let dir = scratch("longsuffix");
    fs::write(dir.join(sanitize_file_name(&name).unwrap()), b"x").unwrap();

    let taken = |path: &Path| path.exists();
    let target = unique_target(&dir, &name, taken).expect("a free candidate");
    let chosen = target.file_name().unwrap().to_str().unwrap();
    assert!(
        chosen.chars().count() <= MAX_NAME_CHARS,
        "{chosen} is {} chars",
        chosen.chars().count()
    );
    assert!(chosen.ends_with(" (1).pdf"), "{chosen}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn every_taken_name_refuses_instead_of_overwriting() {
    let dir = scratch("full");
    // The cap is 64 attempts; make all 65 candidates "exist" without creating them.
    let taken = |_: &Path| true;
    assert_eq!(unique_target(&dir, "only.tar.gz", taken), None);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn plan_refuses_a_name_that_sanitizes_to_nothing() {
    let dir = scratch("noname");
    let taken = |_: &Path| false;
    assert_eq!(plan(&dir, None, 0, taken), Err(Refusal::NoName));
    assert_eq!(plan(&dir, Some("../"), 0, taken), Err(Refusal::NoName));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn plan_refuses_past_the_concurrency_cap() {
    let dir = scratch("cap");
    let taken = |_: &Path| false;
    assert_eq!(
        plan(&dir, Some("a.zip"), MAX_CONCURRENT, taken),
        Err(Refusal::TooManyConcurrent)
    );
    // One below the cap is still accepted.
    assert!(plan(&dir, Some("a.zip"), MAX_CONCURRENT - 1, taken).is_ok());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_url_supplies_a_name_when_the_page_does_not() {
    assert_eq!(
        downloads::name_from_url("https://example.com/files/report.pdf?token=1#page").as_deref(),
        Some("report.pdf")
    );
    assert_eq!(
        downloads::name_from_url("https://example.com/").as_deref(),
        Some("example.com")
    );
    assert_eq!(downloads::name_from_url("data:,"), None);
    assert_eq!(downloads::name_from_url("about:blank"), None);
}

#[test]
fn the_directory_is_the_os_downloads_folder() {
    let dir = downloads_dir().expect("this machine has a Downloads folder");
    assert!(dir.is_absolute(), "{dir:?} must be absolute");
    // Contract: the OS helper's answer, never a literal in this crate. When the OS knows the
    // folder (it does here), the helper's value is what the shell uses.
    if let Some(os) = dirs::download_dir() {
        assert_eq!(dir, os);
    }
}

struct ResetOnDrop;
impl Drop for ResetOnDrop {
    fn drop(&mut self) {
        downloads::reset_for_tests();
    }
}

#[test]
fn accept_lands_in_downloads_and_reserves_until_finished() {
    let _guard = test_support::lock();
    downloads::reset_for_tests();
    let _reset = ResetOnDrop;

    let mut path = PathBuf::from("ignored/report.pdf");
    assert!(downloads::accept(
        "https://example.com/report.pdf",
        &mut path
    ));
    let dir = downloads_dir().unwrap();
    assert_eq!(path.parent(), Some(dir.as_path()));
    // The name is either the suggestion or its first free suffix — the folder may already hold
    // a `report.pdf` on this machine.
    assert!(path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("report"));
    assert_eq!(in_flight(), 1);

    // The completion releases the name; a second accept then has the same starting point.
    downloads::finished("https://example.com/report.pdf", Some(&path), true);
    assert_eq!(in_flight(), 0);
}

#[test]
fn accept_refuses_a_page_that_can_only_offer_junk() {
    let _guard = test_support::lock();
    downloads::reset_for_tests();
    let _reset = ResetOnDrop;

    // A blank suggestion and no usable URL segment: nothing is created, nothing reserved.
    let mut path = PathBuf::from("");
    assert!(!downloads::accept("", &mut path));
    assert_eq!(in_flight(), 0);
}

#[test]
fn accept_stops_at_the_concurrency_cap_and_recovers() {
    let _guard = test_support::lock();
    downloads::reset_for_tests();
    let _reset = ResetOnDrop;

    let urls: Vec<String> = (0..MAX_CONCURRENT)
        .map(|index| format!("https://example.com/file-{index}.bin"))
        .collect();
    let mut paths = Vec::new();
    for url in &urls {
        let mut path = PathBuf::from("x");
        assert!(
            downloads::accept(url, &mut path),
            "{url} should be accepted"
        );
        paths.push(path);
    }
    assert_eq!(in_flight(), MAX_CONCURRENT);

    let mut refused = PathBuf::from("y.bin");
    assert!(!downloads::accept(
        "https://example.com/one-too-many.bin",
        &mut refused
    ));
    // The refusal never rewrote the caller's path.
    assert_eq!(refused, PathBuf::from("y.bin"));

    for (url, path) in urls.iter().zip(paths.iter()) {
        downloads::finished(url, Some(path), true);
    }
    assert_eq!(in_flight(), 0);
}

#[test]
fn a_completion_with_a_verbatim_path_still_releases() {
    let _guard = test_support::lock();
    downloads::reset_for_tests();
    let _reset = ResetOnDrop;

    let mut path = PathBuf::from("release-me.txt");
    assert!(downloads::accept(
        "https://example.com/release-me.txt",
        &mut path
    ));
    assert_eq!(in_flight(), 1);

    // WebView2 reports `\\?\C:\...` for the same target; the reservation must still be found.
    let verbatim = PathBuf::from(format!(r"\\?\{}", path.display()));
    downloads::finished("https://example.com/release-me.txt", Some(&verbatim), true);
    assert_eq!(in_flight(), 0);
}

#[test]
fn a_completion_without_a_path_falls_back_to_the_url() {
    let _guard = test_support::lock();
    downloads::reset_for_tests();
    let _reset = ResetOnDrop;

    let mut path = PathBuf::from("no-path.bin");
    assert!(downloads::accept(
        "https://example.com/no-path.bin",
        &mut path
    ));
    assert_eq!(in_flight(), 1);
    downloads::finished("https://example.com/no-path.bin", None, false);
    assert_eq!(in_flight(), 0);
}

#[test]
fn downloads_bar_shrinks_content_handles_open_folder_dismiss_and_clear_all() {
    use appstore_service::ApplicationService;
    use rune::{Env, Runtime};

    let _guard = test_support::lock();
    downloads::reset_for_tests();
    let _reset = ResetOnDrop;
    let opened = downloads::record_local_opens();

    let custom_dir = scratch("bar-ui");
    downloads::set_custom_dir(Some(custom_dir.clone()));

    let mut rt = Runtime::new(
        crate::state::ShellState::new(ApplicationService::for_tests("downloads-bar-ui")),
        crate::shell::view,
        (1200.0, 800.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.frame(1.0 / 60.0);
    let base_rect = crate::webview::pane_rect(&rt).expect("base content rect");
    assert!(rt.find("downloads-bar").is_none());

    // Start two downloads via the real accept/finished hooks and poll them into the UI.
    let mut first = PathBuf::from("a.pdf");
    assert!(downloads::accept("https://example.com/a.pdf", &mut first));
    assert_eq!(first.parent(), Some(custom_dir.as_path()));
    downloads::finished("https://example.com/a.pdf", Some(&first), true);

    let mut second = PathBuf::from("b.zip");
    assert!(downloads::accept("https://example.com/b.zip", &mut second));
    downloads::finished("https://example.com/b.zip", Some(&second), true);

    assert!(crate::webview::poll_at(&mut rt.state, 1.0));
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);

    assert!(rt.find("downloads-bar").is_some());
    assert!(rt.find("downloads-clear-all").is_some());
    let shrunk_rect = crate::webview::pane_rect(&rt).expect("shrunk content rect");
    assert!(
        (base_rect.h - shrunk_rect.h - downloads::DOWNLOADS_BAR_HEIGHT).abs() < 1.0,
        "downloads bar must shrink content by {} pt: before={} after={}",
        downloads::DOWNLOADS_BAR_HEIGHT,
        base_rect.h,
        shrunk_rect.h
    );

    // Open downloaded file and open its containing folder.
    for key in ["downloads-open", "downloads-folder"] {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }
    let opens = downloads::recorded_local_opens(&opened);
    assert_eq!(opens, vec![second.clone(), custom_dir.clone()]);

    // Dismiss removes latest item (leaving 1 item -> Clear all disappears).
    let (dx, dy) = rt.find("downloads-dismiss").expect("dismiss button");
    rt.pointer_down(dx, dy);
    rt.pointer_up(dx, dy);
    rt.frame(1.0 / 60.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.downloads().len(), 1);
    assert!(rt.find("downloads-clear-all").is_none());

    // Add another item and press Clear all to hide the bar and restore full content height.
    let mut third = PathBuf::from("c.txt");
    assert!(downloads::accept("https://example.com/c.txt", &mut third));
    downloads::finished("https://example.com/c.txt", Some(&third), true);
    assert!(crate::webview::poll_at(&mut rt.state, 2.0));
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);
    let (cx, cy) = rt.find("downloads-clear-all").expect("clear-all button");
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(1.0 / 60.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);
    assert!(rt.find("downloads-bar").is_none());
    let restored_rect = crate::webview::pane_rect(&rt).expect("restored content rect");
    assert!((restored_rect.h - base_rect.h).abs() < 1.0);

    let _ = fs::remove_dir_all(&custom_dir);
}

#[test]
fn settings_download_dir_persists_validates_and_resets() {
    use appstore_service::ApplicationService;
    use rune::{Env, Runtime, TextEdit};

    let _guard = test_support::lock();
    downloads::reset_for_tests();
    let _reset = ResetOnDrop;

    let session_path = std::env::temp_dir().join(format!(
        "station-shell-download-dir-setting-{}.json",
        std::process::id()
    ));
    let _ = fs::remove_file(&session_path);
    let custom_dir = scratch("settings-dir");

    let mut rt = Runtime::new(
        crate::state::ShellState::launch_in(
            ApplicationService::for_tests("dl-dir-settings"),
            &session_path,
        ),
        crate::shell::view,
        (1200.0, 800.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.frame(1.0 / 60.0);

    // Open Settings -> Change downloads folder.
    let (sx, sy) = rt.find("rail-settings").expect("settings");
    rt.pointer_down(sx, sy);
    rt.pointer_up(sx, sy);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);

    let (ex, ey) = rt.find("settings-download-dir-edit").expect("edit button");
    rt.pointer_down(ex, ey);
    rt.pointer_up(ex, ey);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.find("settings-download-dir-editor").is_some());

    // Relative path is refused without saving.
    rt.state.download_dir_draft_mut().unwrap().edit = TextEdit::new("relative/downloads");
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.save_download_dir_draft());
    assert!(rt.state.download_dir().is_none());

    // Absolute path saves and persists across relaunch.
    rt.state.download_dir_draft_mut().unwrap().edit =
        TextEdit::new(custom_dir.display().to_string());
    assert!(rt.state.save_download_dir_draft());
    assert_eq!(
        rt.state.download_dir(),
        Some(custom_dir.display().to_string().as_str())
    );
    assert_eq!(
        downloads::effective_downloads_dir(),
        Some(custom_dir.clone())
    );

    let relaunched = crate::state::ShellState::launch_in(
        ApplicationService::for_tests("dl-dir-relaunch"),
        &session_path,
    );
    assert_eq!(
        relaunched.download_dir(),
        Some(custom_dir.display().to_string().as_str())
    );

    // Reset reverts to OS default.
    rt.state.reset_download_dir();
    assert_eq!(rt.state.download_dir(), None);
    assert_eq!(downloads::effective_downloads_dir(), downloads_dir());

    let _ = fs::remove_file(&session_path);
    let _ = fs::remove_dir_all(&custom_dir);
}
