//! Slice 10 tests: per-app configuration, end to end through the real shell.
//!
//! The settings editor drives `ShellState::set_app_config` → `ApplicationService::set_config_data`
//! → `resolve_start_url` → the rail tab's URL → the session document, so every assertion here is
//! what the user did and what the shell did with it: the drawn editor, the moved tab URL, the
//! relaunch that shows the configured URL, and the hostile config that is refused with the
//! document untouched.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use appstore_service::write_commands::ApplicationConfigData;
use appstore_service::{ApplicationService, InstallOptions};
use rune::{Env, Key, KeyEvent, Mods, Runtime};
use session_store::{AppConfigEntry, SessionStore};

use crate::shell;
use crate::state::ShellState;
use crate::webview;
use crate::webview::fake::{Event, FakeWebview};

const WINDOW: (f32, f32) = (1200.0, 800.0);

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn tag() -> String {
    format!(
        "station-shell-config-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A shell launched from a fresh session file over an isolated private-manifest store.
fn launched(path: &std::path::Path) -> ShellState {
    ShellState::launch_in(ApplicationService::for_tests(&tag()), path)
}

fn session_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "station-shell-config-{name}-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn runtime(state: ShellState) -> Runtime<ShellState> {
    Runtime::new(
        state,
        shell::view,
        WINDOW,
        Env {
            system_dark: true,
            scale: 1.0,
        },
    )
}

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

/// Gmail's manifest id (google-account preset) and GitLab's (on-premise) in
/// `packages/app/manifests/definitions`.
const GMAIL_TILE: &str = "store-tile-14";
const SLACK_TILE: &str = "store-tile-21";

/// Installs Gmail through the store press path and returns its application id.
fn install_gmail(rt: &mut Runtime<ShellState>) -> String {
    press(rt, GMAIL_TILE);
    draw(rt);
    let id = rt.state.active().id.clone();
    assert_ne!(id, "appstore", "the install did not select the app");
    id
}

// ---- The editor in Settings -------------------------------------------------------------

#[test]
fn an_installed_preset_app_gets_a_configure_button_and_an_editor() {
    let mut rt = runtime(launched(&session_path("editor")));
    draw(&mut rt);
    let gmail = install_gmail(&mut rt);

    press(&mut rt, "rail-settings");
    draw(&mut rt);
    assert!(
        rt.find(&format!("settings-configure-{gmail}")).is_some(),
        "no Configure control for the app"
    );
    assert!(shows(&rt, "Configure"));

    press(&mut rt, &format!("settings-configure-{gmail}"));
    draw(&mut rt);
    assert!(
        shows(&rt, "Sign-in is not available in the native shell yet."),
        "a google-account app must say sign-in is unavailable, not show a stub field"
    );
    // No text field is offered for an identity the shell cannot obtain.
    let fields: Vec<_> = rt
        .elements()
        .into_iter()
        .filter(|element| {
            element
                .key
                .as_deref()
                .is_some_and(|key| key.starts_with("config-edit-"))
        })
        .collect();
    assert!(
        fields.is_empty(),
        "a google-account app must not collect an identity string the shell cannot use"
    );

    // Cancel returns to My apps.
    press(&mut rt, "config-cancel");
    draw(&mut rt);
    assert!(rt.find(&format!("settings-configure-{gmail}")).is_some());
}

#[test]
fn a_subdomain_app_edits_its_domain_and_the_tab_url_moves() {
    let log_host = {
        let (host, log) = FakeWebview::new();
        webview::install(Box::new(host));
        log
    };
    let path = session_path("on-premise");
    let mut rt = runtime(launched(&path));
    draw(&mut rt);

    press(&mut rt, SLACK_TILE);
    draw(&mut rt);
    let slack = rt.state.active().id.clone();
    let signed_out = rt.state.tab_url(&slack).unwrap().to_owned();
    assert!(
        signed_out.starts_with("https://"),
        "the unconfigured app opens a start URL: {signed_out}"
    );

    press(&mut rt, "rail-settings");
    draw(&mut rt);
    press(&mut rt, &format!("settings-configure-{slack}"));
    draw(&mut rt);
    // Slack's first preset is `subdomain`: the field is the domain, and the manifest's own
    // wording rides the placeholder.
    assert!(shows(&rt, "Domain"), "the subdomain field label is missing");
    assert!(
        rt.find(&format!("config-edit-{slack}")).is_some(),
        "no text field for the domain"
    );

    type_text(&mut rt, &format!("config-edit-{slack}"), "acme");
    press(&mut rt, &format!("config-save-{slack}"));
    draw(&mut rt);

    // The editor closed, the tab's URL is the configured one, and the page follows now.
    assert!(
        rt.find(&format!("settings-configure-{slack}")).is_some(),
        "the editor did not close"
    );
    assert_eq!(
        rt.state.tab_url(&slack),
        Some("https://acme.slack.com"),
        "the configured URL did not become the tab's URL (signed-out was {signed_out})"
    );
    assert!(
        log_host.events().contains(&Event::Load {
            tab: slack.clone(),
            url: "https://acme.slack.com".into(),
        }),
        "an open page was not navigated: {:?}",
        log_host.events()
    );

    // The document carries it (camelCase on disk).
    let saved = SessionStore::new(&path).load().unwrap();
    assert_eq!(
        saved
            .app_config
            .get(&slack)
            .and_then(|c| c.subdomain.as_deref()),
        Some("acme"),
        "the configuration did not reach the session document"
    );
}

#[test]
fn a_configured_app_relaunches_at_the_configured_url() {
    let path = session_path("relaunch");
    let mut state = launched(&path);
    assert!(state.install("station-manifest://123"));
    let gitlab = state.active().id.clone();

    assert!(state.open_config_editor(&gitlab));
    if let Some(draft) = state.config_draft_mut() {
        draft.edit.replace_text("https://gitlab.corp.acme.dev");
    }
    state.save_config_draft();
    assert_eq!(state.tab_url(&gitlab), Some("https://gitlab.corp.acme.dev"));

    // Relaunch from the same file: same tab, same configured URL.
    let restored = launched(&path);
    assert_eq!(
        restored.tab_url(&gitlab),
        Some("https://gitlab.corp.acme.dev"),
        "the relaunch did not show the configured URL"
    );
    assert_eq!(
        restored.config_of(&gitlab).custom_url.as_deref(),
        Some("https://gitlab.corp.acme.dev")
    );
}

#[test]
fn clear_reverts_to_the_manifest_start_url_and_persists() {
    let path = session_path("clear");
    let mut state = launched(&path);
    assert!(state.install("station-manifest://123"));
    let gitlab = state.active().id.clone();
    let signed_out = state.tab_url(&gitlab).unwrap().to_owned();

    state.open_config_editor(&gitlab);
    if let Some(draft) = state.config_draft_mut() {
        draft.edit.replace_text("https://gitlab.corp.acme.dev");
    }
    state.save_config_draft();

    // Clear through the editor path: config gone, tab reverted, document updated.
    assert!(state.open_config_editor(&gitlab));
    assert!(state.clear_config_draft());
    assert_eq!(state.tab_url(&gitlab), Some(signed_out.as_str()));
    assert!(state.config_of(&gitlab).is_empty());
    let saved = SessionStore::new(&path).load().unwrap();
    assert!(
        !saved.app_config.contains_key(&gitlab),
        "the cleared entry stayed in the document"
    );

    // A relaunch keeps the reverted URL.
    let restored = launched(&path);
    assert_eq!(restored.tab_url(&gitlab), Some(signed_out.as_str()));
}

#[test]
fn a_hostile_oversized_config_is_refused_and_the_document_is_untouched() {
    let path = session_path("hostile");
    let mut state = launched(&path);
    assert!(state.install("station-manifest://123"));
    let gitlab = state.active().id.clone();
    state.select("appstore");
    let before = std::fs::read(&path).unwrap();

    // A value past the document cap: the action is refused whole — nothing is written, and the
    // previous session stays byte-for-byte.
    let entry = AppConfigEntry {
        custom_url: Some("x".repeat(session_store::MAX_CONFIG_VALUE_CHARS + 1)),
        ..AppConfigEntry::default()
    };
    assert!(
        !state.set_app_config(&gitlab, entry),
        "a hostile config must not save"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a refused save changed the session file"
    );
    // The shell kept running: the next real action still writes.
    assert!(state.select("appstore"));
    let after = SessionStore::new(&path).load().unwrap();
    assert_eq!(after.installed.len(), 1);
    assert!(!after.app_config.contains_key(&gitlab));
}

#[test]
fn config_for_a_manifest_that_disappeared_does_not_crash() {
    let path = session_path("gone-manifest");
    let mut state = launched(&path);
    assert!(state.install("station-manifest://123"));
    let gitlab = state.active().id.clone();

    // The configuration saves; the manifest is then removed from the registry (a private
    // manifest deleted underneath the session is the same shape).
    assert!(state.open_config_editor(&gitlab));
    if let Some(draft) = state.config_draft_mut() {
        draft.edit.replace_text("https://gitlab.corp.acme.dev");
    }
    state.save_config_draft();
    state.private_store_remove_all_for_tests();

    // Nothing panics: the tab keeps its last URL, the session still saves, and a relaunch
    // restores the app with its configuration (the label falls back to the id).
    state.save();
    let restored = launched(&path);
    assert!(restored.tab_url(&gitlab).is_some());
    assert_eq!(
        restored.config_of(&gitlab).custom_url.as_deref(),
        Some("https://gitlab.corp.acme.dev")
    );
}

#[test]
fn a_plain_app_without_presets_offers_no_configure_control() {
    let mut rt = runtime(launched(&session_path("plain")));
    draw(&mut rt);
    press(&mut rt, "store-tile-157"); // Todoist: no presets.
    draw(&mut rt);
    let todoist = rt.state.active().id.clone();

    press(&mut rt, "rail-settings");
    draw(&mut rt);
    assert!(
        rt.find(&format!("settings-configure-{todoist}")).is_none(),
        "a plain app must not offer a configuration it cannot use"
    );
}

#[test]
fn an_install_carrying_config_opens_the_configured_url_immediately() {
    let path = session_path("install-config");
    let mut service = ApplicationService::for_tests(&tag());
    let options = InstallOptions {
        install_context: None,
        config_data: ApplicationConfigData {
            custom_url: Some("https://gitlab.corp.acme.dev".into()),
            ..ApplicationConfigData::default()
        },
    };
    let installed = service.install("station-manifest://123", &options).unwrap();
    let app_id = installed.application_id.clone();
    let state = ShellState::launch_in(service, &path);
    // The install's home tab URL is the configured one and the rail tab shows it.
    assert_eq!(
        state.tab_url(&app_id),
        Some("https://gitlab.corp.acme.dev"),
        "the install-time configuration did not reach the tab URL"
    );
}

// ---- Service-side guard the shell relies on --------------------------------------------

#[test]
fn set_config_data_reports_change_exactly_once() {
    let mut service = ApplicationService::for_tests(&tag());
    let app_id = service
        .install("station-manifest://123", &InstallOptions::default())
        .unwrap()
        .application_id;
    let config = AppConfigEntry {
        custom_url: Some("https://gitlab.corp.acme.dev".into()),
        ..AppConfigEntry::default()
    };
    assert!(service
        .set_config_data(&app_id, &crate::state::to_service_config_for_tests(&config))
        .unwrap());
    assert!(!service
        .set_config_data(&app_id, &crate::state::to_service_config_for_tests(&config))
        .unwrap());
    assert!(service.clear_config_data(&app_id).unwrap());
    assert!(!service.clear_config_data(&app_id).unwrap());
}
