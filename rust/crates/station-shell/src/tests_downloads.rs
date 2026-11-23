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

#[test]
fn accept_lands_in_downloads_and_reserves_until_finished() {
    let _guard = test_support::lock();
    downloads::reset_for_tests();

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

    // A blank suggestion and no usable URL segment: nothing is created, nothing reserved.
    let mut path = PathBuf::from("");
    assert!(!downloads::accept("", &mut path));
    assert_eq!(in_flight(), 0);
}

#[test]
fn accept_stops_at_the_concurrency_cap_and_recovers() {
    let _guard = test_support::lock();
    downloads::reset_for_tests();

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

    let mut path = PathBuf::from("no-path.bin");
    assert!(downloads::accept(
        "https://example.com/no-path.bin",
        &mut path
    ));
    assert_eq!(in_flight(), 1);
    downloads::finished("https://example.com/no-path.bin", None, false);
    assert_eq!(in_flight(), 0);
}
