//! Headless Rune `Runtime` tests for the shell: the rail, the App Store drawn by Rune, and the
//! web view seam driven through a recording host.
//!
//! Everything goes through the real view: pointer presses on rail rows, store sections, tiles and
//! the search field, key events for typing, and the element tree for what the user can see. State
//! is built over an isolated private-manifest store, so no test touches the user's profile.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use appstore_service::{ApplicationService, InstallOptions};
use manifest_registry::NewPrivateApplication;
use notification_center::{
    NewNotificationOptions, NotificationAction, NotificationCenter, NotificationProps, Snooze,
};
use rune::tok;
use rune::{Env, Key, KeyEvent, Mods, Runtime};
use session_store::{SessionState, SessionStore, ThemePreference, WindowState};

use crate::routing::{self, Routed};
use crate::shell;
use crate::state::{AppTab, ShellState, TabKind};
use crate::webview::fake::{Event, FakeWebview, WebviewLog};
use crate::webview::{self, PaneRect};
use url_router::InstalledApp;

const WINDOW: (f32, f32) = (1200.0, 800.0);

static NEXT_STORE: AtomicUsize = AtomicUsize::new(0);

/// A shell over a fresh private-manifest store in the system temp dir.
fn shell_state() -> ShellState {
    let tag = format!(
        "station-shell-test-{}",
        NEXT_STORE.fetch_add(1, Ordering::Relaxed)
    );
    ShellState::new(ApplicationService::for_tests(&tag))
}

/// A session file path that no other test uses, removed first.
fn session_path(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "station-shell-session-{tag}-{}-{}.json",
        std::process::id(),
        NEXT_STORE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    path
}

/// A shell launched from `path` over an isolated private-manifest store.
fn launched(path: &Path) -> ShellState {
    let tag = format!(
        "station-shell-launch-{}",
        NEXT_STORE.fetch_add(1, Ordering::Relaxed)
    );
    ShellState::launch_in(ApplicationService::for_tests(&tag), path)
}

fn runtime(state: ShellState, dark: bool) -> Runtime<ShellState> {
    Runtime::new(
        state,
        shell::view,
        WINDOW,
        Env {
            system_dark: dark,
            scale: 1.0,
        },
    )
}

/// The app's frame loop: draw, then let the web view host catch up with what was presented.
fn draw(rt: &mut Runtime<ShellState>) {
    rt.frame(1.0 / 60.0);
    webview::after_frame(rt);
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

fn type_text(rt: &mut Runtime<ShellState>, key: &str, text: &str) {
    press(rt, key);
    rt.key(KeyEvent {
        key: Key::Text(text.to_owned()),
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
}

/// Installs a store-backed recording host for this test's thread, as a window would.
fn recording_host() -> WebviewLog {
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));
    log
}

fn attach(tab: &str, url: &str, rect: PaneRect) -> Event {
    Event::Attach {
        tab: tab.into(),
        url: url.into(),
        rect,
    }
}

fn visible(tab: &str, visible: bool) -> Event {
    Event::Visible {
        tab: tab.into(),
        visible,
    }
}

/// Every string the window shows now: element text and accessible labels of the elements that
/// are not on their way out (`exiting` elements stay listed for a frame or two after a list
/// changes, so they are not what the user sees).
fn shown(rt: &Runtime<ShellState>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|element| !element.exiting)
        .flat_map(|element| element.text.into_iter().chain(element.label))
        .collect()
}

fn shows(rt: &Runtime<ShellState>, text: &str) -> bool {
    shown(rt).iter().any(|shown| shown == text)
}

fn element_text(rt: &Runtime<ShellState>, key: &str) -> Option<String> {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .and_then(|e| e.text.or(e.label))
}

/// Gmail's manifest id in `packages/app/manifests/definitions`.
const GMAIL_TILE: &str = "store-tile-14";
/// Todoist's manifest id: a plain `https://` start URL, unlike Gmail's template.
const TODOIST_TILE: &str = "store-tile-157";

// ---- The App Store, drawn by Rune -------------------------------------------------------

#[test]
fn the_store_renders_its_sections_and_the_popular_apps() {
    let log = recording_host();
    let mut rt = runtime(shell_state(), true);
    draw(&mut rt);

    for text in [
        "Station App Store",
        "DISCOVER",
        "Most popular",
        "All apps",
        "My custom apps",
        "Most Popular",
        "Cream of the crop",
        "Runner ups",
        "Noteworthy",
        "Gmail",
        "Slack",
    ] {
        assert!(shows(&rt, text), "the store never shows {text:?}");
    }
    assert!(
        log.events().is_empty(),
        "the App Store is Rune-drawn: no page may be created for it"
    );
}

#[test]
fn searching_narrows_the_main_area_to_matching_apps() {
    let mut rt = runtime(shell_state(), true);
    draw(&mut rt);

    type_text(&mut rt, "store-search", "slack");

    assert_eq!(rt.state.query(), "slack", "the field did not take the text");
    assert!(shows(&rt, "Search results"));
    assert!(shows(&rt, "Slack"), "the match is missing");
    assert!(!shows(&rt, "Gmail"), "a non-match is still listed");
    assert!(
        !shows(&rt, "Cream of the crop"),
        "search results replaced the section"
    );
}

#[test]
fn switching_store_sections_changes_the_main_area() {
    let mut rt = runtime(shell_state(), true);
    draw(&mut rt);
    assert!(shows(&rt, "Cream of the crop"));

    press(&mut rt, "store-section-all-apps");
    draw(&mut rt);
    assert!(shows(&rt, "All Apps"));
    assert!(
        shows(&rt, "Communication & Collaboration"),
        "a category header is missing"
    );
    assert!(!shows(&rt, "Cream of the crop"));

    press(&mut rt, "store-section-my-custom-apps");
    draw(&mut rt);
    assert!(shows(&rt, "My custom apps"));
    assert!(shows(&rt, "No custom apps yet."));
}

#[test]
fn my_custom_apps_lists_private_manifests() {
    let mut store = ApplicationService::for_tests("station-shell-custom-apps");
    store
        .request_private_application(NewPrivateApplication {
            name: "Acme Intranet".to_owned(),
            theme_color: "#112233".to_owned(),
            icon_url: String::new(),
            start_url: "https://acme.test".to_owned(),
            scope: "https://acme.test".to_owned(),
        })
        .unwrap();
    let mut rt = runtime(ShellState::new(store), true);
    draw(&mut rt);

    press(&mut rt, "store-section-my-custom-apps");
    draw(&mut rt);

    assert!(shows(&rt, "Acme Intranet"));
    assert!(!shows(&rt, "No custom apps yet."));
}

// ---- Installing and removing from the store ---------------------------------------------

#[test]
fn installing_from_the_store_adds_a_rail_row_and_switches_to_the_app() {
    let log = recording_host();
    let mut rt = runtime(shell_state(), true);
    draw(&mut rt);
    assert_eq!(rt.state.tabs().len(), 1, "a fresh shell is the store alone");

    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);

    assert_eq!(rt.state.tabs().len(), 2);
    let gmail = rt.state.active().clone();
    assert_eq!(gmail.label, "Gmail");
    assert_eq!(gmail.kind, TabKind::Web);
    // Gmail is a multi-instance app (a `google-account` preset) with no identity yet: the install
    // opens the manifest's own formatted start URL (the signed-out account picker), not the
    // configurator the native shell does not draw.
    assert!(
        gmail
            .url
            .starts_with("https://accounts.google.com/AddSession?"),
        "unexpected home tab {:?}",
        gmail.url
    );
    assert_eq!(element_text(&rt, "topbar-title").as_deref(), Some("Gmail"));
    assert!(
        rt.find(&gmail.key()).is_some(),
        "the rail has no row for the new app"
    );

    let rect = webview::pane_rect(&rt).expect("the content element is laid out");
    assert_eq!(
        log.events(),
        vec![
            attach(&gmail.id, &gmail.url, rect),
            visible(&gmail.id, true)
        ],
        "installing switches to the app, and only its page is hosted"
    );
}

#[test]
fn a_second_press_on_an_installed_tile_uninstalls_it() {
    let log = recording_host();
    let mut rt = runtime(shell_state(), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail_id = rt.state.active().id.clone();

    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    assert!(shows(&rt, "Installed"), "the tile is not marked installed");

    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);

    assert_eq!(rt.state.tabs().len(), 1, "the rail row is gone");
    assert_eq!(rt.state.active().id, "appstore");
    assert!(!shows(&rt, "Installed"), "the marker outlived the install");
    assert!(
        log.events().contains(&Event::Detach { tab: gmail_id }),
        "the app's page was left behind"
    );
}

// ---- The web view seam -------------------------------------------------------------------

#[test]
fn the_store_tab_never_gets_a_page_while_web_tabs_do() {
    let log = recording_host();
    let mut rt = runtime(shell_state(), true);
    draw(&mut rt);
    draw(&mut rt);
    assert!(log.events().is_empty(), "the store created a page");

    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail_id = rt.state.active().id.clone();
    assert_eq!(log.attached(), vec![gmail_id.clone()]);

    // Back to the store: the page hides, and nothing new is created.
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    assert_eq!(
        log.events().last(),
        Some(&visible(&gmail_id, false)),
        "the app's page stayed visible over the store"
    );
    assert_eq!(log.attached(), vec![gmail_id.clone()]);

    // Back to the app: the same page shows again.
    let gmail_key = rt.state.tabs()[1].key();
    press(&mut rt, &gmail_key);
    draw(&mut rt);
    assert_eq!(
        log.events().last(),
        Some(&visible(&gmail_id, true)),
        "the app's page did not come back"
    );
    assert_eq!(log.attached().len(), 1, "the page was created twice");
}

#[test]
fn switching_between_installed_apps_hides_and_shows_without_reattaching() {
    let log = recording_host();
    let mut rt = runtime(shell_state(), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    press(&mut rt, TODOIST_TILE);
    draw(&mut rt);

    assert_eq!(rt.state.tabs().len(), 3);
    let todoist = rt.state.active().clone();
    assert_eq!(
        todoist.url, "https://app.todoist.com/auth/login",
        "a plain manifest's home tab is its start_url"
    );
    let gmail = rt.state.tabs()[1].clone();
    log.clear();

    let gmail_key = gmail.key();
    press(&mut rt, &gmail_key);
    draw(&mut rt);

    assert_eq!(
        log.events(),
        vec![visible(&todoist.id, false), visible(&gmail.id, true)],
        "switching apps re-created a page"
    );
    assert!(log.attached().is_empty());
}

#[test]
fn a_resize_reaches_the_host_as_new_bounds() {
    let log = recording_host();
    let mut rt = runtime(shell_state(), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail_id = rt.state.active().id.clone();
    let before = webview::pane_rect(&rt).expect("the content element is laid out");
    log.clear();

    rt.set_viewport((1000.0, 700.0), 1.0);
    draw(&mut rt);
    let after = webview::pane_rect(&rt).expect("the content element is laid out");

    assert_ne!(after, before, "the pane did not move with the window");
    assert_eq!(
        log.events(),
        vec![Event::Bounds {
            tab: gmail_id,
            rect: after,
        }]
    );

    log.clear();
    draw(&mut rt);
    assert!(
        log.events().is_empty(),
        "an unchanged pane asks for nothing"
    );
}

#[test]
fn closing_the_window_detaches_every_page() {
    let log = recording_host();
    let mut rt = runtime(shell_state(), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    press(&mut rt, TODOIST_TILE);
    draw(&mut rt);
    log.clear();

    webview::close_all();

    let mut detached: Vec<String> = log
        .events()
        .into_iter()
        .map(|event| match event {
            Event::Detach { tab } => tab,
            other => panic!("unexpected event on close: {other:?}"),
        })
        .collect();
    let mut expected = vec![rt.state.tabs()[1].id.clone(), rt.state.tabs()[2].id.clone()];
    detached.sort();
    expected.sort();
    assert_eq!(detached, expected);
}

/// How many attach attempts the fake host refused.
fn failed_attempts(log: &WebviewLog) -> usize {
    log.events()
        .into_iter()
        .filter(|event| matches!(event, Event::AttachFailed { .. }))
        .count()
}

#[test]
fn a_failed_page_retries_with_backoff_and_recovers() {
    let (host, log) = FakeWebview::failing_times(2);
    webview::install(Box::new(host));
    let mut rt = runtime(launched(&session_path("retry-recovers")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt); // attempt 1 fails

    assert_eq!(failed_attempts(&log), 1);
    webview::poll_at(&mut rt.state, 0.0);
    assert!(rt.state.page_error().is_some(), "a failed page is silent");

    // Nothing within the 1 s window, even if frames keep coming.
    draw(&mut rt);
    assert_eq!(failed_attempts(&log), 1);

    webview::poll_at(&mut rt.state, 1.0);
    draw(&mut rt); // attempt 2 fails
    assert_eq!(failed_attempts(&log), 2);

    webview::poll_at(&mut rt.state, 6.0);
    draw(&mut rt); // attempt 3 succeeds
    assert_eq!(failed_attempts(&log), 2);
    assert_eq!(log.attached(), vec![rt.state.active().id.clone()]);

    webview::poll_at(&mut rt.state, 6.0);
    assert!(
        rt.state.page_error().is_none(),
        "the error outlived the recovered page"
    );
}

#[test]
fn a_permanently_broken_page_stops_retrying_and_the_retry_control_restarts_it() {
    let (host, log) = FakeWebview::refusing();
    webview::install(Box::new(host));
    let mut rt = runtime(launched(&session_path("retry-exhausted")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);

    // Every due retry, and then far past the last delay.
    for clock in [0.0, 1.0, 6.0, 36.0, 1_000.0, 10_000.0] {
        webview::poll_at(&mut rt.state, clock);
        draw(&mut rt);
    }

    assert_eq!(
        failed_attempts(&log),
        webview::MAX_ATTEMPTS as usize,
        "the shell kept retrying a broken host"
    );
    assert!(rt.state.page_error().is_some(), "nothing told the user");

    // The Retry control: the failure is forgotten and one fresh attempt happens.
    let before = failed_attempts(&log);
    let gmail = rt.state.active().id.clone();
    webview::retry(&gmail);
    rt.state.clear_page_error();
    draw(&mut rt);
    assert_eq!(failed_attempts(&log), before + 1);
}

#[test]
fn switching_to_a_failed_tab_retries_without_waiting_out_the_backoff() {
    let (host, log) = FakeWebview::refusing();
    webview::install(Box::new(host));
    let mut rt = runtime(launched(&session_path("retry-switch")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    assert_eq!(failed_attempts(&log), 1);
    assert!(
        rt.state.save_failures() == 0,
        "the session is unrelated to this test"
    );

    // Back to the failed tab within the 1 s window: a user switch is its own reason to try.
    let gmail = rt.state.tabs()[1].id.clone();
    assert!(rt.state.select(&gmail));
    draw(&mut rt);

    assert_eq!(
        failed_attempts(&log),
        2,
        "re-selecting the failed tab did not retry"
    );
}

#[test]
fn retargeting_a_failed_page_forgets_its_failure() {
    let (host, log) = FakeWebview::refusing();
    webview::install(Box::new(host));
    let mut rt = runtime(launched(&session_path("retry-retarget")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    assert_eq!(failed_attempts(&log), 1);

    let gmail = rt.state.active().id.clone();
    rt.state
        .retarget(&gmail, "https://mail.google.com/mail/u/0/#other");
    draw(&mut rt);

    assert_eq!(
        failed_attempts(&log),
        2,
        "a retargeted page did not get a fresh attempt"
    );
}

#[test]
fn six_tabs_keep_at_most_five_pages_and_recreate_the_evicted_one() {
    let log = recording_host();
    let mut rt = runtime(launched(&session_path("page-cap")), true);
    draw(&mut rt);

    let mut tabs: Vec<String> = Vec::new();
    for id in ["14", "157", "21", "18", "16", "388"] {
        assert!(rt.state.install(&format!("station-manifest://{id}")));
        tabs.push(rt.state.active().id.clone());
        draw(&mut rt);
    }

    let live = log.live_pages();
    assert_eq!(live.len(), webview::MAX_PAGES, "the page cap did not hold");
    assert!(live.contains(&tabs[5]), "the active page was evicted");
    assert!(
        !live.contains(&tabs[0]),
        "the least-recently-shown page was not the one closed"
    );

    // Re-selecting the evicted tab recreates its page.
    log.clear();
    assert!(rt.state.select(&tabs[0]));
    draw(&mut rt);
    assert_eq!(log.attached(), vec![tabs[0].clone()]);
}

#[test]
fn a_scale_change_reaches_the_host_and_re_bounds_every_page() {
    let log = recording_host();
    let mut rt = runtime(launched(&session_path("scale")), true);
    draw(&mut rt);
    rt.state.install("station-manifest://14");
    draw(&mut rt);
    let gmail = rt.state.active().id.clone();
    let rect = webview::pane_rect(&rt).expect("the pane is laid out");
    log.clear();

    rt.env.scale = 2.0;
    rt.frame(0.0);
    webview::after_frame(&rt);

    assert!(
        log.events().contains(&Event::Scale { scale: 2.0 }),
        "the host never learned the new scale"
    );
    assert!(
        log.events().contains(&Event::Bounds {
            tab: gmail.clone(),
            rect,
        }),
        "the page was not re-bounded at the new scale"
    );
}

#[test]
fn a_missing_or_non_finite_pane_hides_the_page() {
    let log = recording_host();
    let mut rt = runtime(launched(&session_path("no-pane")), true);
    draw(&mut rt);
    rt.state.install("station-manifest://14");
    draw(&mut rt);
    let gmail = rt.state.active().id.clone();
    assert_eq!(log.events().last(), Some(&visible(&gmail, true)));

    webview::sync(&rt.state, None, 1.0);
    assert_eq!(
        log.events().last(),
        Some(&visible(&gmail, false)),
        "a page floated with no pane to sit on"
    );

    draw(&mut rt);
    assert_eq!(log.events().last(), Some(&visible(&gmail, true)));

    let nan = PaneRect::new(f32::NAN, 0.0, 100.0, 100.0);
    webview::sync(&rt.state, Some(nan), 1.0);
    assert_eq!(
        log.events().last(),
        Some(&visible(&gmail, false)),
        "a NaN pane left the page on screen"
    );

    let negative = PaneRect::new(-40.0, -8.0, 100.0, 50.0);
    assert_eq!(
        negative.clamped(),
        Some(PaneRect::new(0.0, 0.0, 100.0, 50.0))
    );
    assert_eq!(nan.clamped(), None);
}

#[test]
fn closing_is_idempotent_and_nothing_attaches_afterwards() {
    let log = recording_host();
    let mut rt = runtime(launched(&session_path("close")), true);
    draw(&mut rt);
    rt.state.install("station-manifest://14");
    draw(&mut rt);
    assert_eq!(log.live_pages().len(), 1);

    webview::close_all();
    let after_close = log.events().len();
    assert!(log.live_pages().is_empty());

    webview::close_all();
    assert_eq!(log.events().len(), after_close, "a second close did work");

    draw(&mut rt);
    assert_eq!(
        log.events().len(),
        after_close,
        "a page attached after close"
    );

    // A fresh host (a new window) works again.
    let log = recording_host();
    let mut rt = runtime(launched(&session_path("close-reopen")), true);
    draw(&mut rt);
    rt.state.install("station-manifest://14");
    draw(&mut rt);
    assert_eq!(log.live_pages().len(), 1);
}

#[test]
fn a_selection_is_written_immediately_and_the_last_one_wins() {
    let path = session_path("selection-writes");
    let mut rt = runtime(launched(&path), true);
    draw(&mut rt);
    assert!(rt.state.install("station-manifest://14"));
    assert!(rt.state.install("station-manifest://157"));
    let first = rt.state.tabs()[1].id.clone();
    let second = rt.state.tabs()[2].id.clone();

    let active_in_file = || -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(&path).expect("session")).expect("json")
    };

    // No flush tick, no frame: the write is immediate, so a hard kill cannot lose it.
    assert!(rt.state.select(&first));
    assert_eq!(
        active_in_file()["activeApplication"],
        serde_json::json!(first)
    );
    assert!(rt.state.select(&second));
    assert_eq!(
        active_in_file()["activeApplication"],
        serde_json::json!(second),
        "the last selection was not on disk"
    );
}

#[test]
fn reselecting_the_active_tab_does_not_write_again() {
    let dir = std::env::temp_dir().join(format!(
        "station-shell-reselect-dir-{}-{}",
        std::process::id(),
        NEXT_STORE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.json");
    let mut rt = runtime(launched(&path), true);
    draw(&mut rt);
    assert!(rt.state.install("station-manifest://14")); // writes at once
    let gmail = rt.state.active().id.clone();
    let other = rt.state.tabs()[0].id.clone();

    // Make every later write fail on both Windows (file readonly blocks rename) and Unix
    // (directory readonly blocks temp file creation): an attempted write becomes a `save_failures` count.
    let mut file_perms = std::fs::metadata(&path).unwrap().permissions();
    file_perms.set_readonly(true);
    std::fs::set_permissions(&path, file_perms).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    }

    assert!(rt.state.select(&gmail));
    assert!(rt.state.select(&gmail));
    assert_eq!(
        rt.state.save_failures(),
        0,
        "re-selecting the active tab wrote the session"
    );

    assert!(rt.state.select(&other));
    assert_eq!(
        rt.state.save_failures(),
        1,
        "a real selection did not write at once"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn the_launch_size_is_capped_below_the_renderer_ceiling() {
    use crate::session::{safe_window_size, MAX_DEVICE_PX, MAX_LAUNCH_POINTS};

    // The launch cap is the scale-free guard: even a hostile session opens at 2048 pt.
    assert_eq!(
        safe_window_size(16_384.0, 1e30, 1.0),
        (MAX_LAUNCH_POINTS, MAX_LAUNCH_POINTS),
        "the launch cap did not hold"
    );
    assert_eq!(
        safe_window_size(f32::NAN, f32::INFINITY, 1.0),
        (1200.0, 800.0),
        "a non-finite size did not fall back"
    );
    assert_eq!(safe_window_size(2048.0, 2048.0, 2.0), (2048.0, 2048.0));
    assert_eq!(safe_window_size(2048.0, 2048.0, 4.0), (2048.0, 2048.0));
    // 2048 pt at 8× would be 16 384 px (1 GiB of readback): the scale-aware cap shrinks it.
    let (width, height) = safe_window_size(2048.0, 2048.0, 8.0);
    assert_eq!((width, height), (1024.0, 1024.0));
    assert!(width * 8.0 <= MAX_DEVICE_PX && height * 8.0 <= MAX_DEVICE_PX);
}

#[test]
fn a_windowed_restore_keeps_large_displays_and_shrinks_what_will_not_fit() {
    use crate::session::{restored_size, MAX_DEVICE_PX};

    // A legitimate large display comes back at the size the user left it.
    assert_eq!(restored_size(3840.0, 2160.0, 1.0), (3840.0, 2160.0));
    assert_eq!(restored_size(5120.0, 1440.0, 1.0), (5120.0, 1440.0));
    let (width, height) = restored_size(3840.0, 2160.0, 2.0);
    assert_eq!((width, height), (3840.0, 2160.0));
    assert!(width * 2.0 <= MAX_DEVICE_PX && height * 2.0 <= MAX_DEVICE_PX);
    // A size the display cannot give the renderer is shrunk, never refused.
    assert_eq!(restored_size(8192.0, 8192.0, 2.0), (4096.0, 4096.0));
    assert_eq!(restored_size(16_384.0, 16_384.0, 1.0), (8192.0, 8192.0));
    assert_eq!(restored_size(5120.0, 1440.0, 2.0), (4096.0, 1440.0));
    assert_eq!(restored_size(8192.0, 8192.0, 4.0), (2048.0, 2048.0));
    // Degenerate values fall back to the shell's defaults, floored to the session minimum.
    assert_eq!(restored_size(f32::NAN, f32::INFINITY, 1.0), (1200.0, 800.0));
    assert_eq!(restored_size(-10.0, 0.0, 1.0), (1200.0, 800.0));
    assert_eq!(restored_size(10.0, 10.0, 1.0), (320.0, 240.0));
    for (width, height, scale) in [
        (3840.0, 2160.0, 1.0),
        (8192.0, 8192.0, 2.0),
        (16_384.0, 16_384.0, 8.0),
    ] {
        let (width, height) = restored_size(width, height, scale);
        assert!(
            width * scale <= MAX_DEVICE_PX && height * scale <= MAX_DEVICE_PX,
            "{width}x{height} pt at {scale}x exceeds the ceiling"
        );
    }
}

#[test]
fn a_session_with_a_hostile_window_size_snaps_without_panicking() {
    let path = session_path("hostile-window");
    std::fs::write(
        &path,
        br##"{"version":1,"window":{"x":null,"y":null,"width":16384.0,"height":1e30,"maximized":false},
             "theme":"system","activeApplication":null,"railOrder":[],"installed":[],"dock":[],"tabs":[]}"##,
    )
    .expect("write");

    let window = crate::session::window_state(&path);
    assert_eq!(window.width, crate::session::MAX_WINDOW_POINTS);
    assert_eq!(window.height, crate::session::MAX_WINDOW_POINTS);

    // What `main` does with it: a sane viewport, then a frame that a real window would present.
    let (width, height) = crate::session::safe_window_size(window.width, window.height, 1.0);
    let mut rt = Runtime::new(
        launched(&path),
        shell::view,
        (width, height),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    assert!(
        rt.presented("shell").is_some(),
        "the shell produced no frame for a hostile session"
    );
}

#[test]
fn a_failing_save_keeps_the_shell_running_and_retries_on_the_next_action() {
    let dir = std::env::temp_dir().join(format!(
        "station-shell-savefail-dir-{}-{}",
        std::process::id(),
        NEXT_STORE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.json");
    let mut rt = runtime(launched(&path), true);
    draw(&mut rt);
    rt.state.install("station-manifest://14");
    // Make the destination unwritable on both Windows and Unix: the next write fails like a full
    // or read-only disk.
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    }

    assert!(rt.state.install("station-manifest://157"));
    assert_eq!(rt.state.save_failures(), 1, "the failure was not recorded");

    // The shell keeps working: a selection still succeeds and asks the store again.
    let first = rt.state.tabs()[1].id.clone();
    assert!(rt.state.select(&first));
    assert!(
        rt.state.save_failures() >= 2,
        "the next user action did not retry the write"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn launch_survives_an_unavailable_private_store() {
    // `APPDATA` below a file: the config dir cannot be created, so `ApplicationService::new`
    // fails. The shell must open with an in-memory store instead of panicking.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = std::env::temp_dir().join(format!("station-shell-nostore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let blocker = root.join("blocked");
    std::fs::write(&blocker, b"not a directory").unwrap();
    // `PrivateStore::new` reads APPDATA; the session path is pointed at a temp file so launch
    // reads no real session either.
    let previous = std::env::var_os("APPDATA");
    let previous_session = std::env::var_os(crate::session::PATH_ENV);
    std::env::set_var("APPDATA", blocker.join("nested"));
    let session = session_path("no-store");
    std::env::set_var(crate::session::PATH_ENV, &session);

    let mut state = ShellState::launch();

    match previous {
        Some(value) => std::env::set_var("APPDATA", value),
        None => std::env::remove_var("APPDATA"),
    }
    match previous_session {
        Some(value) => std::env::set_var(crate::session::PATH_ENV, value),
        None => std::env::remove_var(crate::session::PATH_ENV),
    }

    assert_eq!(state.tabs().len(), 1, "the shell did not open");
    assert_eq!(state.active().id, "appstore");
    // The in-memory store still accepts an install for this session.
    assert!(state.install("station-manifest://14"));
    assert_eq!(state.tabs().len(), 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn points_convert_to_device_pixels_at_the_window_scale() {
    let pane = PaneRect::new(10.0, 44.0, 100.5, 50.0);
    let pixels = pane.pixels(1.5);
    assert_eq!((pixels.x, pixels.y), (15, 66));
    assert_eq!((pixels.w, pixels.h), (151, 75));

    let collapsed = PaneRect::new(0.0, 0.0, 0.1, 0.0).pixels(1.0);
    assert_eq!(
        (collapsed.w, collapsed.h),
        (1, 1),
        "never a zero-sized child"
    );
}

// ---- The rail and the window -------------------------------------------------------------

#[test]
fn the_rail_starts_with_the_store_alone_and_stays_unique() {
    let mut rt = runtime(shell_state(), true);
    draw(&mut rt);

    let ids: Vec<&str> = rt.state.tabs().iter().map(|tab| tab.id.as_str()).collect();
    assert_eq!(ids, vec!["appstore"]);

    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    press(&mut rt, TODOIST_TILE);
    draw(&mut rt);

    let mut ids: Vec<String> = rt.state.tabs().iter().map(AppTab::key).collect();
    let count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), count, "the rail repeats a key");
    assert_eq!(count, 3);
}

#[test]
fn the_window_paints_from_theme_tokens_in_dark_and_light() {
    for dark in [true, false] {
        let mut rt = runtime(shell_state(), dark);
        rt.frame(0.0);
        let shell = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("shell"))
            .expect("the shell root is laid out");
        assert_eq!(
            shell.fill,
            tok::theme(dark).bg,
            "shell background is not the theme's"
        );
        let rail = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("rail"))
            .expect("the rail is laid out");
        assert_eq!(
            rail.fill,
            tok::theme(dark).card,
            "rail is not the theme's card colour"
        );
    }
}

// ---- The session ---------------------------------------------------------------------

#[test]
fn installing_persists_the_rail_row_and_the_selection() {
    let path = session_path("install");
    let mut rt = runtime(launched(&path), true);
    draw(&mut rt);

    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail = rt.state.active().clone();

    let restored = launched(&path);
    assert_eq!(restored.tabs().len(), 2, "the rail row did not come back");
    assert_eq!(restored.tabs()[1].id, gmail.id);
    assert_eq!(restored.tabs()[1].label, "Gmail");
    assert_eq!(restored.tabs()[1].url, gmail.url);
    assert_eq!(
        restored.active().id,
        gmail.id,
        "the selection did not come back"
    );
}

#[test]
fn uninstalling_leaves_only_the_store_on_relaunch() {
    let path = session_path("uninstall");
    let mut rt = runtime(launched(&path), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    press(&mut rt, "tab-appstore");
    draw(&mut rt);

    press(&mut rt, GMAIL_TILE); // installed: the tile removes it
    draw(&mut rt);
    assert_eq!(rt.state.tabs().len(), 1);

    let restored = launched(&path);
    assert_eq!(restored.tabs().len(), 1);
    assert_eq!(restored.active().id, "appstore");
}

#[test]
fn an_unknown_active_application_falls_back_to_the_store() {
    let mut service = ApplicationService::for_tests("session-active");
    service
        .install("station-manifest://14", &InstallOptions::default())
        .unwrap();
    let installed: Vec<_> = service.installed_applications().values().cloned().collect();
    let application_id = installed[0].application_id.clone();

    let path = session_path("active");
    let write = |active: &str| {
        SessionStore::new(&path)
            .save(&SessionState {
                active_application: Some(active.to_owned()),
                installed: installed.clone(),
                dock: service.dock().to_vec(),
                tabs: service.tabs().to_vec(),
                ..SessionState::default()
            })
            .unwrap();
    };

    write(&application_id);
    let restored = launched(&path);
    assert_eq!(restored.active().id, application_id, "known selection lost");

    write("an-application-that-is-gone");
    let restored = launched(&path);
    assert_eq!(
        restored.active().id,
        "appstore",
        "a session naming a missing app must not strand the shell"
    );
}

#[test]
fn a_missing_session_starts_at_the_store_and_writes_nothing() {
    let path = session_path("missing");
    let state = launched(&path);

    assert_eq!(state.tabs().len(), 1);
    assert_eq!(state.active().id, "appstore");
    assert!(!path.exists(), "launch created a session file");
}

#[test]
fn a_corrupt_session_warns_starts_fresh_and_is_left_alone() {
    let path = session_path("corrupt");
    let corrupt = b"{ this is not a session".to_vec();
    std::fs::write(&path, &corrupt).unwrap();

    let mut state = launched(&path);
    assert_eq!(state.tabs().len(), 1);
    assert_eq!(state.active().id, "appstore");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        corrupt,
        "launch overwrote a session it could not read"
    );

    // The user acts (an install, not a no-op re-selection of the tab already showing): the file
    // is replaced with a valid session.
    assert!(state.install("station-manifest://14"));
    assert!(
        SessionStore::new(&path).load().is_ok(),
        "a user action did not replace the unreadable session"
    );
}

#[test]
fn an_unsupported_session_version_is_refused_and_kept() {
    let path = session_path("future");
    let future = br#"{"version":99,"installed":[],"dock":[],"tabs":[]}"#.to_vec();
    std::fs::write(&path, &future).unwrap();

    let state = launched(&path);

    assert_eq!(state.tabs().len(), 1);
    assert_eq!(state.active().id, "appstore");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        future,
        "a newer session was replaced"
    );
}

#[test]
fn a_session_whose_dock_names_an_unknown_app_starts_fresh() {
    let path = session_path("bad-dock");
    SessionStore::new(&path)
        .save(&SessionState {
            dock: vec!["an-application-that-was-never-installed".to_owned()],
            ..SessionState::default()
        })
        .unwrap();
    let saved = std::fs::read(&path).unwrap();

    let state = launched(&path);

    assert_eq!(state.tabs().len(), 1);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        saved,
        "the session was rewritten"
    );
}

#[test]
fn the_window_placement_survives_a_session_round_trip() {
    let path = session_path("window");
    let window = WindowState {
        x: Some(120),
        y: Some(64),
        width: 1000.0,
        height: 700.0,
        maximized: true,
    };
    SessionStore::new(&path)
        .save(&SessionState {
            window: window.clone(),
            ..SessionState::default()
        })
        .unwrap();

    let mut state = launched(&path);
    assert!(state.select("appstore"), "no selection to persist");

    let reread = SessionStore::new(&path).load().unwrap();
    assert_eq!(reread.window, window, "placement was lost on save");
}

// ---- Settings -------------------------------------------------------------------------

#[test]
fn settings_opens_over_the_app_and_hides_its_page() {
    let log = recording_host();
    let mut rt = runtime(launched(&session_path("settings-open")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail_id = rt.state.active().id.clone();
    assert_eq!(log.events().last(), Some(&visible(&gmail_id, true)));

    press(&mut rt, "rail-settings");
    draw(&mut rt);

    assert_eq!(
        log.events().last(),
        Some(&visible(&gmail_id, false)),
        "the page stayed on screen under Settings"
    );
    assert!(shows(&rt, "Settings"), "the screen did not open");
    assert!(shows(&rt, "General"));
    assert!(shows(&rt, "My apps"));
    assert_eq!(
        element_text(&rt, "settings-version").as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert!(rt.find("settings-session-path").is_some());

    press(&mut rt, "settings-back");
    draw(&mut rt);
    assert_eq!(
        log.events().last(),
        Some(&visible(&gmail_id, true)),
        "the page did not come back"
    );
    assert_eq!(log.attached(), vec![gmail_id], "the page was re-created");
}

#[test]
fn settings_uninstalls_an_app_and_persists_it() {
    let path = session_path("settings-uninstall");
    let log = recording_host();
    let mut rt = runtime(launched(&path), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail_id = rt.state.active().id.clone();

    press(&mut rt, "rail-settings");
    draw(&mut rt);
    assert!(rt.find(&format!("settings-open-{gmail_id}")).is_some());
    assert!(rt.find(&format!("settings-uninstall-{gmail_id}")).is_some());

    press(&mut rt, &format!("settings-uninstall-{gmail_id}"));
    draw(&mut rt);

    assert_eq!(rt.state.tabs().len(), 1, "the rail row survived");
    assert!(
        log.events().contains(&Event::Detach {
            tab: gmail_id.clone()
        }),
        "the app's page was left behind"
    );
    assert_eq!(
        rt.state.surface(),
        crate::state::Surface::Settings,
        "uninstalling closed Settings"
    );
    assert!(shows(&rt, "No apps installed yet."));

    press(&mut rt, "settings-back");
    draw(&mut rt);
    assert_eq!(rt.state.surface(), crate::state::Surface::Apps);

    let restored = launched(&path);
    assert_eq!(restored.tabs().len(), 1, "the uninstall was not persisted");
}

#[test]
fn settings_open_switches_to_the_app() {
    let mut rt = runtime(launched(&session_path("settings-open-app")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail_id = rt.state.active().id.clone();
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    press(&mut rt, TODOIST_TILE);
    draw(&mut rt);
    assert_ne!(rt.state.active().id, gmail_id);

    press(&mut rt, "rail-settings");
    draw(&mut rt);
    press(&mut rt, &format!("settings-open-{gmail_id}"));
    draw(&mut rt);

    assert_eq!(rt.state.surface(), crate::state::Surface::Apps);
    assert_eq!(rt.state.active().id, gmail_id);
}

#[test]
fn the_appearance_preference_drives_the_theme_and_is_remembered() {
    let path = session_path("appearance");
    // The desktop says light; the user asks for dark.
    let mut rt = runtime(launched(&path), false);
    draw(&mut rt);
    assert_eq!(shell_fill(&rt), tok::theme(false).bg);

    press(&mut rt, "rail-settings");
    draw(&mut rt);
    press(&mut rt, "settings-theme-dark");
    draw(&mut rt);

    assert_eq!(rt.state.theme(), ThemePreference::Dark);
    // The window background is a paint track: let its spring land before reading the colour.
    rt.settle_all();
    rt.frame(1.0 / 60.0);
    assert_eq!(
        shell_fill(&rt),
        tok::theme(true).bg,
        "the choice did not repaint"
    );

    let restored = launched(&path);
    assert_eq!(
        restored.theme(),
        ThemePreference::Dark,
        "the appearance was not persisted"
    );
}

fn shell_fill(rt: &Runtime<ShellState>) -> rune::Color {
    rt.elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some("shell"))
        .expect("the shell root is laid out")
        .fill
}

#[test]
fn escape_leaves_settings_and_the_notification_panel() {
    let mut rt = runtime(launched(&session_path("escape")), true);
    rt.on_key = Some(shell::on_key);
    draw(&mut rt);
    press(&mut rt, "rail-settings");
    draw(&mut rt);
    assert_eq!(rt.state.surface(), crate::state::Surface::Settings);

    rt.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.surface(), crate::state::Surface::Apps);

    press(&mut rt, "rail-notifications");
    draw(&mut rt);
    assert_eq!(rt.state.surface(), crate::state::Surface::Notifications);
    rt.key(KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.surface(), crate::state::Surface::Apps);
}

// ---- The notification panel -----------------------------------------------------------

/// One stored notification, as the center takes it.
struct Sample {
    id: &'static str,
    application: Option<&'static str>,
    title: &'static str,
    body: Option<&'static str>,
    timestamp: i64,
}

/// A center with two groups: two "slack" notifications, then one without an application.
fn prepopulated_center() -> NotificationCenter {
    let mut center = NotificationCenter::new();
    for sample in [
        Sample {
            id: "n1",
            application: Some("slack"),
            title: "New message",
            body: Some("Ada: ship it"),
            timestamp: 1_000_000,
        },
        Sample {
            id: "n2",
            application: Some("slack"),
            title: "Mentioned you",
            body: Some("in #general"),
            timestamp: 1_000_100,
        },
        Sample {
            id: "n3",
            application: None,
            title: "Station update",
            body: None,
            timestamp: 1_000_200,
        },
    ] {
        center.apply(NotificationAction::NewNotification {
            application_id: sample.application.map(str::to_owned),
            tab_id: None,
            notification_id: sample.id.to_owned(),
            props: NotificationProps {
                title: sample.title.to_owned(),
                timestamp: Some(sample.timestamp),
                body: sample.body.map(str::to_owned),
                icon: None,
            },
            options: NewNotificationOptions::default(),
        });
    }
    center
}

fn panel_runtime(tag: &str) -> Runtime<ShellState> {
    runtime(
        launched(&session_path(tag)).with_notifications(prepopulated_center()),
        true,
    )
}

#[test]
fn the_notification_panel_shows_its_groups_and_rows() {
    let mut rt = panel_runtime("panel-rows");
    draw(&mut rt);

    press(&mut rt, "rail-notifications");
    draw(&mut rt);

    assert!(rt.find("notifications").is_some(), "the panel did not open");
    assert!(shows(&rt, "Notifications"));
    assert!(shows(&rt, "3 unread"));
    for title in ["New message", "Mentioned you", "Station update"] {
        assert!(shows(&rt, title), "row {title:?} is missing");
    }
    assert!(shows(&rt, "Ada: ship it"), "the body is missing");
    assert!(rt.find("notification-n1").is_some());
    assert!(!shows(&rt, "You're all caught up."));

    press(&mut rt, "rail-notifications");
    draw(&mut rt);
    assert!(
        rt.find("notifications").is_none(),
        "the panel did not close"
    );
}

#[test]
fn mark_all_read_clears_the_rows() {
    let mut rt = panel_runtime("panel-mark-all");
    draw(&mut rt);
    press(&mut rt, "rail-notifications");
    draw(&mut rt);

    press(&mut rt, "notifications-mark-all");
    draw(&mut rt);

    assert!(!shows(&rt, "New message"), "a row survived mark-all");
    assert!(shows(&rt, "You're all caught up."));
    assert!(shows(&rt, "0 unread"));
    assert_eq!(rt.state.notifications().unread_count(), 0);
}

#[test]
fn choosing_a_snooze_updates_the_center() {
    let mut rt = panel_runtime("panel-snooze");
    draw(&mut rt);
    press(&mut rt, "rail-notifications");
    draw(&mut rt);

    press(&mut rt, "notifications-snooze-15m");
    draw(&mut rt);
    assert_eq!(
        rt.state.notifications().snooze_duration_ms(),
        Some(15 * 60_000)
    );

    press(&mut rt, "notifications-snooze-1h");
    draw(&mut rt);
    assert_eq!(
        rt.state.notifications().snooze_duration_ms(),
        Some(60 * 60_000)
    );

    press(&mut rt, "notifications-snooze-sync");
    draw(&mut rt);
    assert_eq!(rt.state.notifications().snooze(), Some(Snooze::SyncWithOs));
    assert!(rt.state.notifications().is_snoozed());

    press(&mut rt, "notifications-snooze-off");
    draw(&mut rt);
    assert_eq!(rt.state.notifications().snooze(), None);
    assert_eq!(rt.state.notifications().snooze_duration_ms(), None);
}

#[test]
fn the_panel_hides_the_page_and_closing_brings_it_back() {
    let log = recording_host();
    let mut rt = runtime(launched(&session_path("panel-page")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail_id = rt.state.active().id.clone();

    press(&mut rt, "rail-notifications");
    draw(&mut rt);
    assert_eq!(
        log.events().last(),
        Some(&visible(&gmail_id, false)),
        "the page stayed on screen beside the panel"
    );

    press(&mut rt, "rail-notifications");
    draw(&mut rt);
    assert_eq!(
        log.events().last(),
        Some(&visible(&gmail_id, true)),
        "the page did not come back"
    );
    assert_eq!(log.attached(), vec![gmail_id], "the page was re-created");
}

// ---- Routing a page's new window ------------------------------------------------------

/// The request queue and the rolling browser budget are process-global, and **every** test's
/// frame poll drains them: routing tests take the shared test lock (which the poll's own drains
/// also take) and clear the queue/budget, so a flood in one test cannot be routed, opened or
/// budgeted by another.
pub(crate) fn routing_lock() -> crate::test_support::Guard {
    let guard = crate::test_support::lock();
    let _ = routing::take();
    routing::reset_external_budget();
    guard
}

/// An installed application in `url-router`'s shape, for the pure decision cases.
fn routing_app(id: &str, scope: &str) -> InstalledApp {
    InstalledApp {
        application_id: id.to_owned(),
        manifest_url: format!("station-manifest://{id}"),
        scope: Some(scope.to_owned()),
        extended_scopes: Vec::new(),
        custom_urls: Vec::new(),
        start_url: None,
        single_page: false,
    }
}

#[test]
fn routing_a_scoped_link_lands_in_that_app_and_not_the_browser() {
    let apps = [routing_app("gmail", "https://mail.google.com")];
    let tabs = [url_router::OpenTab {
        tab_id: "gmail-tab".to_owned(),
        application_id: "gmail".to_owned(),
        url: "https://mail.google.com/mail/u/0/".to_owned(),
        is_home: true,
    }];

    // From another tab: a URL in Gmail's scope makes Gmail's tab load it.
    let routed = routing::decide(
        "https://mail.google.com/mail/u/0/#inbox/abc",
        &apps,
        &tabs,
        Some("other-tab"),
    );
    assert_eq!(
        routed,
        Routed::FocusAndLoad {
            tab_id: "gmail-tab".to_owned(),
            url: "https://mail.google.com/mail/u/0/#inbox/abc".to_owned(),
        }
    );
}

#[test]
fn routing_unknown_links_and_unknown_tabs_stays_put() {
    let apps: [InstalledApp; 0] = [];
    let tabs: [url_router::OpenTab; 0] = [];

    assert_eq!(
        routing::decide("https://example.com/page", &apps, &tabs, None),
        Routed::External
    );
    // A tab id that is not ours cannot be focused; the request is dropped, not sent to a browser
    // that would open a second window for a page we already own.
    let tabs = [url_router::OpenTab {
        tab_id: "gmail-tab".to_owned(),
        application_id: "gmail".to_owned(),
        url: "https://mail.google.com/".to_owned(),
        is_home: true,
    }];
    assert_eq!(
        routing::decide("https://elsewhere.test/x", &apps, &tabs, Some("gone-tab")),
        Routed::External
    );
}

#[test]
fn routing_a_link_that_matches_an_open_tab_only_focuses_it() {
    let apps = [
        routing_app("gmail", "https://mail.google.com"),
        routing_app("todoist", "https://*.todoist.com"),
    ];
    let tabs = [
        url_router::OpenTab {
            tab_id: "gmail-tab".to_owned(),
            application_id: "gmail".to_owned(),
            url: "https://mail.google.com/".to_owned(),
            is_home: true,
        },
        url_router::OpenTab {
            tab_id: "todoist-tab".to_owned(),
            application_id: "todoist".to_owned(),
            url: "https://app.todoist.com/auth/login".to_owned(),
            is_home: true,
        },
    ];

    assert_eq!(
        routing::decide(
            "https://app.todoist.com/auth/login",
            &apps,
            &tabs,
            Some("gmail-tab")
        ),
        Routed::Focus {
            tab_id: "todoist-tab".to_owned()
        }
    );
}

#[test]
fn a_scoped_new_window_loads_the_app_tab_and_selects_it() {
    let _routing = routing_lock();
    let log = recording_host();
    let mut rt = runtime(launched(&session_path("route-in-scope")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail = rt.state.active().clone();
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    assert_ne!(rt.state.active().id, gmail.id);
    log.clear();

    let url = "https://mail.google.com/mail/u/0/#inbox/routed";
    routing::enqueue(&gmail.id, url);
    let changed = webview::route_requests(&mut rt.state, 0.0);

    assert!(changed, "the routed tab was not selected");
    assert_eq!(rt.state.active().id, gmail.id);
    assert_eq!(rt.state.active().url, url, "the page was not re-pointed");
    assert_eq!(
        log.events(),
        vec![Event::Load {
            tab: gmail.id.clone(),
            url: url.to_owned(),
        }],
        "an open page is navigated, not re-created"
    );
    assert!(log.attached().is_empty(), "a routed link created a webview");

    draw(&mut rt);
    assert_eq!(
        log.events().last(),
        Some(&Event::Visible {
            tab: gmail.id.clone(),
            visible: true
        })
    );
}

#[test]
fn an_unknown_link_goes_to_the_browser_and_changes_nothing() {
    let _routing = routing_lock();
    let log = recording_host();
    let recorded = routing::record_external();
    let mut rt = runtime(launched(&session_path("route-external")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let active = rt.state.active().id.clone();
    log.clear();

    let url = "https://example.com/some/page";
    routing::enqueue(&active, url);
    assert!(
        !webview::route_requests(&mut rt.state, 0.0),
        "a selection changed"
    );

    assert_eq!(routing::recorded_external(&recorded), vec![url.to_owned()]);
    assert!(
        log.events().is_empty(),
        "an external link touched a webview"
    );
    assert_eq!(rt.state.active().id, active);
}

#[test]
fn a_link_to_an_unopened_app_opens_it_at_that_url() {
    let _routing = routing_lock();
    let log = recording_host();
    let mut rt = runtime(launched(&session_path("route-unopened")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail_id = rt.state.active().id.clone();
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    press(&mut rt, TODOIST_TILE);
    draw(&mut rt);
    let todoist = rt.state.active().clone();
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    log.clear();

    // Todoist's page is attached already (it was selected once), so this is a load; the point is
    // that a scoped link reaches the app rather than the browser.
    let url = "https://app.todoist.com/app/today";
    routing::enqueue(&gmail_id, url);
    assert!(webview::route_requests(&mut rt.state, 0.0));

    assert_eq!(rt.state.active().id, todoist.id);
    assert_eq!(
        log.events(),
        vec![Event::Load {
            tab: todoist.id.clone(),
            url: url.to_owned(),
        }]
    );
    assert_eq!(
        routing::recorded_external(&routing::record_external()),
        Vec::<String>::new()
    );
}

#[test]
fn a_routed_page_that_is_not_open_attaches_at_the_routed_url() {
    let _routing = routing_lock();
    let path = session_path("route-attach");
    // Phase 1 (no host): install two apps and leave Gmail selected.
    let mut rt = runtime(launched(&path), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let gmail_id = rt.state.tabs()[1].id.clone();
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    press(&mut rt, TODOIST_TILE);
    draw(&mut rt);
    let todoist_id = rt.state.active().id.clone();
    let gmail_key = rt.state.tabs()[1].key();
    press(&mut rt, &gmail_key);
    draw(&mut rt);
    drop(rt);

    // Phase 2: a new process — fresh host, so Todoist's page has never existed.
    let log = recording_host();
    let mut rt = runtime(launched(&path), true);
    draw(&mut rt);
    assert_eq!(
        rt.state.active().id,
        gmail_id,
        "the session lost the selection"
    );
    log.clear();

    let url = "https://app.todoist.com/app/upcoming";
    routing::enqueue(&todoist_id, url);
    assert!(webview::route_requests(&mut rt.state, 0.0));
    assert_eq!(rt.state.active().id, todoist_id);
    assert!(
        log.attached().is_empty(),
        "the page was attached before the frame"
    );

    draw(&mut rt);
    assert_eq!(
        log.attached(),
        vec![todoist_id.clone()],
        "nothing was attached for the routed tab"
    );
    let attached = log
        .events()
        .into_iter()
        .find(|event| matches!(event, Event::Attach { .. }))
        .expect("no attach event");
    assert_eq!(
        attached,
        Event::Attach {
            tab: todoist_id.clone(),
            url: url.to_owned(),
            rect: webview::pane_rect(&rt).expect("the pane is laid out"),
        },
        "the page attached at its home URL, not the routed one"
    );
}

// ---- Hostile pages: bounded queues, safe browser hand-off -----------------------------

#[test]
fn only_web_schemes_may_reach_the_browser() {
    for allowed in [
        "https://example.com/",
        "http://example.com/path?q=1",
        "mailto:someone@example.com",
    ] {
        assert!(
            routing::is_external_scheme(allowed),
            "{allowed} was refused"
        );
    }
    for refused in [
        "file:///C:/Windows/System32/calc.exe",
        "javascript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "vbscript:msgbox(1)",
        "ms-settings:privacy",
        "C:\\Windows\\System32\\calc.exe",
        "",
        "not a url",
    ] {
        assert!(
            !routing::is_external_scheme(refused),
            "{refused} would reach the OS"
        );
    }
}

#[test]
fn a_refused_url_never_reaches_the_opener() {
    let _routing = routing_lock();
    let recorded = routing::record_external();

    routing::open_external("file:///C:/Windows/System32/calc.exe");
    routing::open_external("javascript:alert(1)");
    routing::open_external("data:text/html,<script>alert(1)</script>");

    assert!(
        routing::recorded_external(&recorded).is_empty(),
        "a non-web URL reached the OS opener"
    );
}

#[test]
fn shell_metacharacters_are_data_not_syntax() {
    let recorded = routing::record_external();
    // The Windows path is ShellExecuteW (one string, no interpreter); on every platform this must
    // at least reach the opener unchanged and only because it is https.
    let url = "https://example.com/?a=1&calc.exe||%22quoted%22";

    routing::open_external(url);

    assert_eq!(routing::recorded_external(&recorded), vec![url.to_owned()]);
}

#[test]
fn a_popup_flood_is_queued_within_a_bound_and_deduped() {
    let _routing = routing_lock();

    // The same request a thousand times: one queued copy.
    for _ in 0..1_000 {
        routing::enqueue("gmail", "https://example.com/same");
    }
    assert_eq!(routing::take().len(), 1);

    // A flood of distinct requests: the queue stops at its cap.
    for index in 0..1_000 {
        routing::enqueue("gmail", &format!("https://example.com/{index}"));
    }
    let queued = routing::take();
    assert_eq!(queued.len(), routing::MAX_QUEUED);
    assert_eq!(queued[0].url, "https://example.com/0");
}

#[test]
fn a_popup_flood_opens_at_most_a_few_browsers_per_frame() {
    let _routing = routing_lock();
    let recorded = routing::record_external();
    let mut rt = runtime(launched(&session_path("popup-flood")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let origin = rt.state.active().id.clone();

    for index in 0..1_000 {
        routing::enqueue(&origin, &format!("https://example.com/flood/{index}"));
    }
    webview::route_requests(&mut rt.state, 0.0);

    let opened = routing::recorded_external(&recorded);
    assert_eq!(
        opened.len(),
        routing::MAX_EXTERNAL_PER_FRAME,
        "one frame opened {} browsers",
        opened.len()
    );

    // The rest of that frame's flood is dropped, not deferred: a hostile page cannot trickle
    // browsers out frame after frame.
    webview::route_requests(&mut rt.state, 0.0);
    assert_eq!(
        routing::recorded_external(&recorded).len(),
        routing::MAX_EXTERNAL_PER_FRAME
    );
    // Duplicate URLs are opened once per frame even from different origins.
    let _ = routing::take();
    routing::enqueue(&origin, "https://example.com/dup");
    routing::enqueue("another-tab", "https://example.com/dup");
    webview::route_requests(&mut rt.state, 0.0);
    let after = routing::recorded_external(&recorded);
    assert_eq!(
        after
            .iter()
            .filter(|url| *url == "https://example.com/dup")
            .count(),
        1
    );
}

#[test]
fn external_opens_have_a_rolling_minute_budget() {
    // The rolling budget is process-global and another test in this binary spends it
    // (`the_external_budget_is_global_across_origin_tabs`); serialize the spenders so neither
    // finds the budget drained by a concurrent run of the other. Same pattern as `tests_pages`.
    let _bridge = crate::tests_pages::bridge_lock();
    let _routing = routing_lock();
    let recorded = routing::record_external();
    let mut rt = runtime(launched(&session_path("external-budget")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);
    let origin = rt.state.active().id.clone();

    // 60 frames of 5 fresh URLs each: the per-frame cap alone would open 300 browsers.
    for frame in 0..60 {
        let now = frame as f64 / 60.0;
        for index in 0..5 {
            routing::enqueue(&origin, &format!("https://example.com/f{frame}/{index}"));
        }
        webview::route_requests(&mut rt.state, now);
    }
    assert_eq!(
        routing::recorded_external(&recorded).len(),
        routing::MAX_EXTERNAL_PER_WINDOW,
        "the rolling budget did not bound the launches"
    );

    // Still inside the window: more frames and more URLs open nothing.
    for frame in 60..120 {
        let now = frame as f64 / 60.0;
        routing::enqueue(&origin, &format!("https://example.com/late/{frame}"));
        webview::route_requests(&mut rt.state, now);
    }
    assert_eq!(
        routing::recorded_external(&recorded).len(),
        routing::MAX_EXTERNAL_PER_WINDOW,
        "a browser was launched while the budget was exhausted"
    );

    // Past the window, launches resume.
    routing::enqueue(&origin, "https://example.com/after-the-window");
    webview::route_requests(&mut rt.state, 61.0);
    assert_eq!(
        routing::recorded_external(&recorded).len(),
        routing::MAX_EXTERNAL_PER_WINDOW + 1,
        "the budget never refilled"
    );
}

#[test]
fn the_external_budget_is_global_across_origin_tabs() {
    let _bridge = crate::tests_pages::bridge_lock();
    let _routing = routing_lock();
    let recorded = routing::record_external();
    let mut rt = runtime(launched(&session_path("external-budget-global")), true);
    draw(&mut rt);
    press(&mut rt, GMAIL_TILE);
    draw(&mut rt);

    // With 10 in any rolling window, two tabs share it. (A frame opens at most 5, so the budget
    // is spent over two frames — the same clock, no refill.)
    for batch in 0..(routing::MAX_EXTERNAL_PER_WINDOW / routing::MAX_EXTERNAL_PER_FRAME) {
        for index in 0..routing::MAX_EXTERNAL_PER_FRAME {
            routing::enqueue("tab-a", &format!("https://example.com/a/{batch}/{}", index));
        }
        webview::route_requests(&mut rt.state, 0.0);
    }
    assert_eq!(
        routing::recorded_external(&recorded).len(),
        routing::MAX_EXTERNAL_PER_WINDOW
    );

    routing::enqueue("tab-b", "https://example.com/b");
    webview::route_requests(&mut rt.state, 0.5);
    assert_eq!(
        routing::recorded_external(&recorded).len(),
        routing::MAX_EXTERNAL_PER_WINDOW,
        "another tab got a fresh budget"
    );
}

#[test]
fn a_poisoned_request_queue_does_not_panic() {
    let _routing = routing_lock();
    routing::poison_queue();

    // The queue is recovered from the poison, not abandoned: the request is accepted and
    // delivered, and nothing panics on the way.
    assert!(routing::enqueue(
        "gmail",
        "https://example.com/after-poison"
    ));
    assert_eq!(routing::take().len(), 1);
    assert!(routing::enqueue("gmail", "https://example.com/again"));
    assert_eq!(routing::take().len(), 1);
}
