//! Slice 21 tests: what a hosted page is created with — the Electron session's user agent, the
//! app's WebView2 profile (Electron `persist:<applicationId>`), and the bounded download log.
//!
//! Everything here goes through the same seam the window uses: `webview::sync` asks the host to
//! attach with a [`PageEnvironment`]. The recording fake keeps the UA/profile beside the event
//! stream (so existing attach assertions stay untouched), and the session tests read the real
//! bundled manifests (Gmail's `bx_use_default_session`, Youtube Music's
//! `bx_override_user_agent`) to prove the manifest wiring.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use appstore_service::write_commands::{ApplicationConfigData, InstalledApplication};
use appstore_service::{ApplicationService, TabRecord};
use session_store::{SessionState, SessionStore};

use crate::state::{AppTab, ShellState};
use crate::webview::fake::FakeWebview;
use crate::webview::{
    self, DownloadPhase, PageEnvironment, PageProfile, DEFAULT_USER_AGENT, MAX_DOWNLOAD_LINES,
    MAX_DOWNLOAD_TEXT_CHARS, MAX_USER_AGENT_CHARS,
};

const RECT: webview::PaneRect = webview::PaneRect::new(0.0, 44.0, 1136.0, 756.0);

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn tag(name: &str) -> String {
    format!(
        "station-shell-page-env-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn state_with(tab: AppTab, name: &str) -> ShellState {
    let mut state = ShellState::new(ApplicationService::for_tests(&tag(name)));
    let id = tab.id.clone();
    state.tabs_mut_for_tests().push(tab);
    state.select(&id);
    state
}

fn install_recording_host() -> webview::fake::WebviewLog {
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));
    log
}

fn session_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("{}.json", tag(name)));
    let _ = std::fs::remove_file(&path);
    path
}

/// A session with one installed bundled app and its home tab.
fn seeded_session(path: &Path, application_id: &str) {
    SessionStore::new(path)
        .save(&SessionState {
            active_application: Some(application_id.to_owned()),
            installed: vec![InstalledApplication {
                application_id: application_id.to_owned(),
                manifest_url: format!("station-manifest://{application_id}"),
                install_context: None,
                home_tab_url: Some("https://example.com/home".to_owned()),
                in_dock: true,
                config_data: ApplicationConfigData::default(),
            }],
            dock: vec![application_id.to_owned()],
            tabs: vec![TabRecord {
                tab_id: format!("tab-{application_id}"),
                application_id: application_id.to_owned(),
                url: "https://example.com/home".to_owned(),
                home: true,
            }],
            ..SessionState::default()
        })
        .expect("seed the session");
}

#[test]
fn password_saving_is_opt_in_and_applies_while_settings_are_open() {
    let path = session_path("password-autosave");
    let mut state = ShellState::launch_in(
        ApplicationService::for_tests(&tag("password-autosave")),
        &path,
    );
    let log = install_recording_host();
    state.open_settings();
    webview::sync(&state, None, 1.0);
    assert!(!log.password_autosave());

    state.toggle_password_autosave();
    webview::sync(&state, None, 1.0);
    assert!(log.password_autosave());
    assert!(SessionStore::new(&path).load().unwrap().password_autosave);

    state.toggle_password_autosave();
    webview::sync(&state, None, 1.0);
    assert!(!log.password_autosave());
    assert!(!SessionStore::new(&path).load().unwrap().password_autosave);
    std::fs::remove_file(path).unwrap();
}

// ---- User agent -------------------------------------------------------------------------

#[test]
fn the_default_user_agent_is_the_electron_session_string() {
    assert_eq!(
        DEFAULT_USER_AGENT,
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36",
        "session.ts's defaultUserAgent, verbatim"
    );
    assert!(DEFAULT_USER_AGENT.contains("Chrome/150.0.0.0"));
    assert!(
        DEFAULT_USER_AGENT.len() < MAX_USER_AGENT_CHARS,
        "the built-in UA fits its own cap"
    );
    assert!(
        !DEFAULT_USER_AGENT.contains("Station") && !DEFAULT_USER_AGENT.contains("station-shell"),
        "no product token or per-run value is mixed into the UA"
    );
}

#[test]
fn a_page_is_attached_with_the_session_user_agent_and_its_app_profile() {
    let log = install_recording_host();
    let state = state_with(
        AppTab::web("app", "App", "https://example.com/"),
        "attach-default",
    );

    webview::sync(&state, Some(RECT), 1.0);

    let environments = log.environments();
    assert_eq!(environments.len(), 1, "the page was not attached once");
    let environment = &environments[0];
    assert_eq!(environment.tab, "app");
    assert_eq!(environment.user_agent, DEFAULT_USER_AGENT);
    assert_eq!(
        environment.profile,
        PageProfile::App("app".to_owned()),
        "an app that is not bx_use_default_session gets its own profile"
    );
    assert_eq!(log.attached(), vec!["app".to_owned()]);
}

#[test]
fn a_manifest_user_agent_overrides_the_session_one_and_is_bounded() {
    let mut tab = AppTab::web("app", "App", "https://example.com/");
    tab.user_agent = Some("Custom/1.0".to_owned());
    assert_eq!(PageEnvironment::for_tab(&tab).user_agent, "Custom/1.0");

    let mut long = "A".repeat(MAX_USER_AGENT_CHARS + 100);
    long.push('\n');
    tab.user_agent = Some(long);
    let user_agent = PageEnvironment::for_tab(&tab).user_agent;
    assert_eq!(user_agent.chars().count(), MAX_USER_AGENT_CHARS);
    assert!(!user_agent.contains('\n'), "control characters are dropped");

    tab.user_agent = Some(" \t\r\n ".to_owned());
    assert_eq!(
        PageEnvironment::for_tab(&tab).user_agent,
        DEFAULT_USER_AGENT,
        "an empty/blank manifest UA falls back to the session UA"
    );
}

// ---- Per-app profile --------------------------------------------------------------------

#[test]
fn default_session_apps_share_the_default_profile() {
    let mut tab = AppTab::web("app", "App", "https://example.com/");
    assert_eq!(
        PageEnvironment::for_tab(&tab).profile,
        PageProfile::App("app".to_owned())
    );
    tab.use_default_session = true;
    assert_eq!(PageEnvironment::for_tab(&tab).profile, PageProfile::Shared);
}

#[test]
fn the_recorded_profile_follows_bx_use_default_session() {
    let log = install_recording_host();
    let mut tab = AppTab::web("shared", "Shared", "https://example.com/");
    tab.use_default_session = true;
    let state = state_with(tab, "attach-shared");

    webview::sync(&state, Some(RECT), 1.0);

    assert_eq!(log.environments()[0].profile, PageProfile::Shared);
}

#[test]
fn profile_directories_cannot_escape_the_data_root() {
    assert_eq!(PageProfile::Shared.directory_name(), "default");
    assert_eq!(
        PageProfile::App("gmail_14-x".to_owned()).directory_name(),
        "app-gmail_14-x"
    );

    let evil = PageProfile::App("../../etc".to_owned()).directory_name();
    assert!(evil.starts_with("app-"));
    assert!(
        !evil.contains('/') && !evil.contains('\\') && !evil.contains(".."),
        "an application id cannot choose a path: {evil}"
    );

    let overlong = "a".repeat(65);
    let hashed = PageProfile::App(overlong.clone()).directory_name();
    assert!(hashed.starts_with("app-"));
    assert_eq!(hashed.len(), "app-".len() + 16, "a 64-bit stable prefix");
    assert_eq!(hashed, PageProfile::App(overlong).directory_name());
    assert_ne!(hashed, PageProfile::App("a".repeat(66)).directory_name());

    let root = Path::new(r"C:\data");
    assert_eq!(
        webview::profile_dir(root, &PageProfile::Shared),
        root.join("default")
    );
}

#[test]
fn the_bundled_manifests_reach_the_tab_environment() {
    // Gmail: `bx_use_default_session` → the default profile, no UA override.
    let gmail_path = session_path("gmail");
    seeded_session(&gmail_path, "14");
    let gmail = ShellState::launch_in(ApplicationService::for_tests(&tag("gmail")), &gmail_path);
    let gmail_tab = gmail
        .tabs()
        .iter()
        .find(|tab| tab.id == "14")
        .expect("Gmail");
    assert_eq!(gmail_tab.user_agent, None);
    assert!(gmail_tab.use_default_session);
    assert_eq!(
        PageEnvironment::for_tab(gmail_tab).profile,
        PageProfile::Shared
    );
    assert_eq!(
        PageEnvironment::for_tab(gmail_tab).user_agent,
        DEFAULT_USER_AGENT
    );

    // Youtube Music: `bx_override_user_agent` and no default session → its own profile.
    let music_path = session_path("music");
    seeded_session(&music_path, "14830");
    let music = ShellState::launch_in(ApplicationService::for_tests(&tag("music")), &music_path);
    let music_tab = music
        .tabs()
        .iter()
        .find(|tab| tab.id == "14830")
        .expect("Music");
    assert_eq!(
        music_tab.user_agent.as_deref(),
        Some(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_13_6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/79.0.3945.79 Safari/537.36 Edge/80.0.361.62"
        )
    );
    assert!(!music_tab.use_default_session);
    assert_eq!(
        PageEnvironment::for_tab(music_tab).profile,
        PageProfile::App("14830".to_owned())
    );
    assert_eq!(
        PageEnvironment::for_tab(music_tab).user_agent,
        music_tab.user_agent.clone().unwrap()
    );

    let _ = std::fs::remove_file(&gmail_path);
    let _ = std::fs::remove_file(&music_path);
}

// ---- Downloads --------------------------------------------------------------------------

#[test]
fn download_log_lines_are_bounded_and_control_free() {
    let long_url = format!("https://example.com/{}", "x".repeat(1000));
    let started = webview::download_line(
        DownloadPhase::Started,
        &format!("{long_url}\nnext line"),
        None,
    );
    assert!(started.starts_with("station-shell: download started: "));
    assert!(!started.contains('\n'), "a URL cannot forge a second line");
    assert_eq!(
        started.chars().count(),
        "station-shell: download started: ".chars().count() + MAX_DOWNLOAD_TEXT_CHARS
    );

    let completed = webview::download_line(
        DownloadPhase::Completed { success: true },
        "https://example.com/file.zip",
        Some(Path::new(r"C:\Users\me\Downloads\file.zip")),
    );
    assert!(completed.contains("download complete"));
    assert!(completed.contains("file.zip"), "the saved path is named");

    assert_eq!(
        webview::download_line(
            DownloadPhase::Completed { success: false },
            "https://example.com/file.zip",
            None
        ),
        "station-shell: download failed: https://example.com/file.zip"
    );
    assert_eq!(MAX_DOWNLOAD_LINES, 32, "the per-run log cap is pinned");
}

/// The page environment is built once per attach from the tab, never from page input: this is the
/// compile-time shape (fields are private to the crate's construction path). The assertion keeps
/// a future edit from adding a page-settable setter unnoticed.
#[test]
fn the_environment_is_only_derived_from_the_tab() {
    let tab = AppTab::web("app", "App", "https://example.com/");
    let before = PageEnvironment::for_tab(&tab);
    let tab_with_page_fields = AppTab {
        page_title: Some("a page title".to_owned()),
        ..tab
    };
    assert_eq!(
        PageEnvironment::for_tab(&tab_with_page_fields),
        before,
        "page-supplied data (title/badge) never reaches the UA or profile"
    );
}
