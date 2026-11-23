//! The monitor-DPI seam (slice 13): the window's real scale reaches the pages.
//!
//! The real read is `GetDpiForWindow` on the Rune parent HWND; in this desktop session WebView2
//! cannot start and no Rune window is created in tests, so the platform read stands in —
//! `webview::set_test_live_scale` forces what `GetDpiForWindow` would report ("the window is now
//! 1.5×") and the poll's detection, refusal and re-scaling are asserted against the recording
//! host. A real windowed monitor switch is therefore not observed here (the port doc's
//! Unverified section says so too).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use appstore_service::ApplicationService;

use crate::state::ShellState;
use crate::webview::fake::{Event, WebviewLog};
use crate::webview::{self, PaneRect};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A shell launched from `path` over an isolated private-manifest store.
fn launched(path: &Path) -> ShellState {
    let tag = format!("station-shell-dpi-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    ShellState::launch_in(ApplicationService::for_tests(&tag), path)
}

/// A session file path no other test uses, removed first.
fn session_path(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "station-shell-dpi-{tag}-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    path
}

/// A recording host installed as a window would be.
fn recording_host() -> WebviewLog {
    let (host, log) = crate::webview::fake::FakeWebview::new();
    webview::install(Box::new(host));
    log
}

/// Forces the platform's live DPI read for one test; the real read is back after the drop, so
/// no other test on this thread inherits the override.
struct LiveScale;

impl LiveScale {
    fn at(scale: f32) -> Self {
        webview::set_test_live_scale(Some(scale));
        Self
    }
}

impl Drop for LiveScale {
    fn drop(&mut self) {
        webview::set_test_live_scale(None);
    }
}

/// Installs an app and attaches its page at the build-time scale, like the first frame does.
fn attached_page(state: &mut ShellState, log: &WebviewLog) -> (String, PaneRect) {
    assert!(state.install("station-manifest://14"));
    let rect = PaneRect::new(0.0, 44.0, 1136.0, 756.0);
    webview::sync(state, Some(rect), 1.0);
    let tab = state.active().id.clone();
    assert!(
        log.events().contains(&Event::Attach {
            tab: tab.clone(),
            url: state.active().url.clone(),
            rect
        }),
        "the page never attached"
    );
    (tab, rect)
}

#[test]
fn a_dpi_change_reaches_the_host_and_re_bounds_the_page_once() {
    let log = recording_host();
    let mut state = launched(&session_path("dpi-change"));
    let (tab, rect) = attached_page(&mut state, &log);
    log.clear();

    {
        let _window = LiveScale::at(1.5);
        webview::poll_at(&mut state, 1.0);

        assert_eq!(
            log.events(),
            vec![
                Event::Scale { scale: 1.5 },
                Event::Bounds {
                    tab: tab.clone(),
                    rect
                },
            ],
            "the change was not learned once and turned into pixel-correct bounds"
        );

        // A steady frame at the same scale asks the host for nothing.
        log.clear();
        webview::poll_at(&mut state, 2.0);
    }
    assert!(log.events().is_empty(), "a steady frame bothered the host");
}

#[test]
fn implausible_scales_are_refused_and_the_engine_does_not_drift() {
    let log = recording_host();
    let mut state = launched(&session_path("dpi-absurd"));
    let (_tab, _rect) = attached_page(&mut state, &log);

    for absurd in [0.0, f32::NAN, 10.0] {
        let _window = LiveScale::at(absurd);
        log.clear();
        webview::poll_at(&mut state, 1.0);
        assert!(
            log.events().is_empty(),
            "a scale of {absurd} reached the host"
        );
    }

    // The engine never took an absurd value: the next sane change still applies once, from the
    // build-time 1.0 (the refusal is logged through the same throttled path as failures).
    let _window = LiveScale::at(1.5);
    log.clear();
    webview::poll_at(&mut state, 2.0);
    assert_eq!(
        log.events()
            .into_iter()
            .filter(|event| matches!(event, Event::Scale { scale: 1.5 }))
            .count(),
        1,
        "absurd scales were not refused whole"
    );
}

#[test]
fn a_dpi_change_while_the_page_is_hidden_still_reaches_the_page() {
    let log = recording_host();
    let mut state = launched(&session_path("dpi-hidden"));
    let (tab, rect) = attached_page(&mut state, &log);

    // Settings covers the pane, so the page is hidden — but its bounds are still the host's.
    // The window stays on the 1.75× monitor the whole time, so the override lives to the end:
    // a real read does the same for the window's lifetime, and `sync` must not fall back to
    // the frozen `Env::scale` while the window is still reporting 1.75×.
    state.open_settings();
    assert!(!state.shows_page(), "Settings did not take over the pane");
    log.clear();
    webview::sync(&state, Some(rect), 1.0);
    assert_eq!(
        log.events(),
        vec![Event::Visible {
            tab: tab.clone(),
            visible: false
        }]
    );

    let _window = LiveScale::at(1.75);
    log.clear();
    webview::poll_at(&mut state, 1.0);
    assert!(
        log.events().contains(&Event::Scale { scale: 1.75 }),
        "a hidden page's scale change was dropped"
    );
    assert!(
        log.events().contains(&Event::Bounds {
            tab: tab.clone(),
            rect
        }),
        "the hidden page was not re-bounded"
    );

    // Back to the app: the page returns, at the new scale, and the host is not re-told the
    // scale (it has it); only the visibility comes back.
    assert!(state.close_overlay());
    log.clear();
    webview::sync(&state, Some(rect), 1.0);
    assert_eq!(log.events(), vec![Event::Visible { tab, visible: true }]);
}
