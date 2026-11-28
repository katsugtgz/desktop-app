//! Slice 34 tests: GitHub Releases update checker, beta channel persistence, and About Station UI.

use std::sync::Arc;
use std::time::{Duration, Instant};

use appstore_service::ApplicationService;
use rune::{Env, Runtime};
use session_store::SessionStore;

use crate::routing;
use crate::shell;
use crate::state::ShellState;
use crate::test_support;
use crate::updater::{self, ReleaseInfo, UpdateError, UpdateFetcher};
use crate::webview;

struct ResetUpdaterOnDrop;
impl Drop for ResetUpdaterOnDrop {
    fn drop(&mut self) {
        updater::reset_for_tests();
    }
}

struct FakeFetcher {
    result: Result<Vec<ReleaseInfo>, UpdateError>,
}

impl UpdateFetcher for FakeFetcher {
    fn fetch_releases(&self) -> Result<Vec<ReleaseInfo>, UpdateError> {
        self.result.clone()
    }
}

fn press(rt: &mut Runtime<ShellState>, key: &str) {
    let (x, y) = rt
        .find(key)
        .unwrap_or_else(|| panic!("{key} is not laid out"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
}

fn element_text(rt: &Runtime<ShellState>, key: &str) -> Option<String> {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .and_then(|e| e.text.or(e.label))
}

fn wait_for_update_poll(state: &mut ShellState) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if webview::poll_at(state, 1.0) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("background update check did not complete within 3s");
}

#[test]
fn about_update_check_shows_available_release_opens_trusted_url_and_handles_errors() {
    let _serial = test_support::lock();
    updater::reset_for_tests();
    let _reset = ResetUpdaterOnDrop;
    let opened = routing::record_external();

    let mut rt = Runtime::new(
        ShellState::new(ApplicationService::for_tests("updater-ui")),
        shell::view,
        (1200.0, 800.0),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.state.open_about();
    rt.invalidate();
    rt.frame(1.0 / 60.0);

    // 1. Update available on stable channel.
    let fetcher = Arc::new(FakeFetcher {
        result: Ok(vec![
            ReleaseInfo {
                tag_name: "v9.9.9".to_owned(),
                name: "Hostile Mirror".to_owned(),
                html_url: "https://evil.example/releases/v9.9.9".to_owned(),
                draft: false,
                prerelease: false,
            },
            ReleaseInfo {
                tag_name: "v2.0.0-beta.1".to_owned(),
                name: "Station 2.0 Beta".to_owned(),
                html_url: "https://github.com/katsugtgz/desktop-app/releases/tag/v2.0.0-beta.1"
                    .to_owned(),
                draft: false,
                prerelease: true,
            },
            ReleaseInfo {
                tag_name: "v1.4.0".to_owned(),
                name: "Station 1.4.0".to_owned(),
                html_url: "https://github.com/katsugtgz/desktop-app/releases/tag/v1.4.0".to_owned(),
                draft: false,
                prerelease: false,
            },
        ]),
    });
    rt.state.check_for_updates_with("1.0.0", fetcher);
    assert!(rt.state.about_state().checking_update);
    wait_for_update_poll(&mut rt.state);
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);

    assert!(!rt.state.about_state().checking_update);
    assert!(rt.state.about_state().update_available);
    assert_eq!(
        element_text(&rt, "about-updates-status").as_deref(),
        Some("Update available: Station 1.4.0")
    );
    assert!(rt.find("about-open-release").is_some());

    // Pressing Download release opens the trusted GitHub URL via routing::open_external.
    press(&mut rt, "about-open-release");
    assert_eq!(
        routing::recorded_external(&opened),
        vec!["https://github.com/katsugtgz/desktop-app/releases/tag/v1.4.0".to_owned()]
    );

    // 2. Enabling beta channel picks 2.0.0-beta.1 on the next check.
    press(&mut rt, "about-include-beta");
    assert!(rt.state.include_beta_updates());
    assert_eq!(
        element_text(&rt, "about-include-beta-value").as_deref(),
        Some("On")
    );
    let beta_fetcher = Arc::new(FakeFetcher {
        result: Ok(vec![ReleaseInfo {
            tag_name: "v2.0.0-beta.1".to_owned(),
            name: "Station 2.0 Beta".to_owned(),
            html_url: "https://github.com/katsugtgz/desktop-app/releases/tag/v2.0.0-beta.1"
                .to_owned(),
            draft: false,
            prerelease: true,
        }]),
    });
    rt.state.check_for_updates_with("1.4.0", beta_fetcher);
    wait_for_update_poll(&mut rt.state);
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(
        element_text(&rt, "about-updates-status").as_deref(),
        Some("Update available: Station 2.0 Beta")
    );

    // 3. Up-to-date clears release button and reports current status.
    let current_fetcher = Arc::new(FakeFetcher {
        result: Ok(vec![ReleaseInfo {
            tag_name: "v1.4.0".to_owned(),
            name: "Station 1.4.0".to_owned(),
            html_url: "https://github.com/katsugtgz/desktop-app/releases/tag/v1.4.0".to_owned(),
            draft: false,
            prerelease: false,
        }]),
    });
    rt.state.check_for_updates_with("1.4.0", current_fetcher);
    wait_for_update_poll(&mut rt.state);
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert!(rt.find("about-open-release").is_none());
    let status = element_text(&rt, "about-updates-status").expect("status");
    assert!(status.contains("up to date"), "unexpected status: {status}");

    // 4. Network error surfaces error text cleanly.
    let failing = Arc::new(FakeFetcher {
        result: Err(UpdateError::Network(
            "HTTP 503 from GitHub Releases".to_owned(),
        )),
    });
    rt.state.check_for_updates_with("1.4.0", failing);
    wait_for_update_poll(&mut rt.state);
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let err_status = element_text(&rt, "about-updates-status").expect("err status");
    assert!(
        err_status.contains("HTTP 503"),
        "expected HTTP 503 in error status, got: {err_status}"
    );
}

#[test]
fn include_beta_updates_toggle_persists_across_relaunch() {
    let _serial = test_support::lock();
    updater::reset_for_tests();
    let _reset = ResetUpdaterOnDrop;

    let session_path = std::env::temp_dir().join(format!(
        "station-shell-beta-updates-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&session_path);

    let mut state = ShellState::launch_in(
        ApplicationService::for_tests("updater-beta-persist"),
        &session_path,
    );
    assert!(!state.include_beta_updates());

    state.toggle_include_beta_updates();
    assert!(state.include_beta_updates());
    assert!(state.about_state().beta_included_in_updates);

    let saved = SessionStore::new(&session_path).load().expect("load saved");
    assert!(saved.include_beta_updates);

    let relaunched = ShellState::launch_in(
        ApplicationService::for_tests("updater-beta-relaunch"),
        &session_path,
    );
    assert!(relaunched.include_beta_updates());
    assert!(relaunched.about_state().beta_included_in_updates);

    let _ = std::fs::remove_file(&session_path);
}
