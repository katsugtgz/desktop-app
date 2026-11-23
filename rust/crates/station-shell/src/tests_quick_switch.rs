//! Slice 28 tests: Bang Quick Switcher (`Ctrl+K` / `Ctrl+T`), pinned favorites persistence,
//! `activity::frecency` ranking, keyboard navigation, and catalog quick-install.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use appstore_service::ApplicationService;
use rune::{Env, Key, KeyEvent, Mods, Runtime, Tx};

use crate::quick_switch::{entries_for, QuickSwitchAction};
use crate::shell;
use crate::state::{ShellState, Surface};
use crate::webview;
use crate::webview::fake::FakeWebview;

const WINDOW: (f32, f32) = (1200.0, 800.0);

fn temp_session(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "station-quick-switch-{tag}-{}-{nanos}.json",
        std::process::id()
    ))
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

fn send_key(rt: &mut Runtime<ShellState>, key: Key) -> bool {
    let mut tx = Tx::default();
    let handled = shell::on_key(
        &mut rt.state,
        &KeyEvent {
            key,
            mods: Mods::default(),
        },
        &mut tx,
    );
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    webview::after_frame(rt);
    handled
}

#[test]
fn frecency_ranks_frequently_selected_tab_first_and_keyboard_switches_it() {
    let (host, _log) = FakeWebview::new();
    webview::install(Box::new(host));

    let mut state = ShellState::new(ApplicationService::for_tests("qs-frecency"));
    assert!(state.install("station-manifest://14")); // Gmail
    let gmail_id = state.active().id.clone();
    assert!(state.install("station-manifest://21")); // Slack
    let slack_id = state.active().id.clone();

    // Repeatedly switch to Gmail so its frecency score clearly exceeds Slack's.
    for _ in 0..5 {
        assert!(state.select(&gmail_id));
        assert!(state.select("appstore"));
    }
    assert!(state.select(&gmail_id));

    let mut rt = runtime(state);
    rt.frame(1.0 / 60.0);

    // Cmd/Ctrl+K opens Quick Switcher.
    assert!(send_key(&mut rt, Key::Command('k')));
    assert_eq!(rt.state.surface(), Surface::QuickSwitch);
    assert!(rt.find("quick-switch-card").is_some());

    // First App entry is Gmail (highest frecency), second is Slack.
    let entries = entries_for(&rt.state);
    let app_tabs: Vec<String> = entries
        .iter()
        .filter_map(|e| match &e.action {
            QuickSwitchAction::SelectTab { tab_id } => Some(tab_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(app_tabs, vec![gmail_id.clone(), slack_id.clone()]);

    // Down arrow moves selection from 0 (Gmail) to 1 (Slack); Enter activates Slack & closes palette.
    assert!(send_key(&mut rt, Key::Down));
    assert_eq!(rt.state.quick_switch().selected, 1);
    assert!(send_key(&mut rt, Key::Enter));
    assert_eq!(rt.state.surface(), Surface::Apps);
    assert_eq!(rt.state.active().id, slack_id);
}

#[test]
fn pinned_favorite_persists_across_relaunch_and_cleans_up_on_uninstall() {
    let path = temp_session("favorites-persist");
    let mut state = ShellState::launch_in(ApplicationService::for_tests("qs-fav-1"), &path);
    assert!(state.install("station-manifest://14")); // Gmail
    let gmail_id = state.active().id.clone();

    // Pin Gmail as a favorite
    assert!(!state.is_favorite(&gmail_id));
    assert!(state.toggle_favorite_for_tab(&gmail_id));
    assert!(state.is_favorite(&gmail_id));
    assert_eq!(state.favorites().len(), 1);

    // Relaunch from disk -> favorite survives and appears first in Quick Switcher!
    let mut relaunched = ShellState::launch_in(ApplicationService::for_tests("qs-fav-2"), &path);
    assert!(relaunched.is_favorite(&gmail_id));
    let entries = entries_for(&relaunched);
    assert!(
        matches!(
            entries.first().map(|e| &e.action),
            Some(QuickSwitchAction::OpenFavorite { application_id, .. }) if application_id == &gmail_id
        ),
        "pinned favorite should appear first in Quick Switcher: {entries:?}"
    );

    // Uninstalling the app automatically drops its favorites and saves cleanly.
    assert!(relaunched.uninstall(&gmail_id));
    assert!(relaunched.favorites().is_empty());

    let after_uninstall = ShellState::launch_in(ApplicationService::for_tests("qs-fav-3"), &path);
    assert!(after_uninstall.favorites().is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn quick_switcher_search_filters_and_offers_app_store_quick_install() {
    let (host, _log) = FakeWebview::new();
    webview::install(Box::new(host));

    let state = ShellState::new(ApplicationService::for_tests("qs-install"));
    let mut rt = runtime(state);
    rt.frame(1.0 / 60.0);

    // Ctrl+T also opens Quick Switcher
    assert!(send_key(&mut rt, Key::Command('t')));
    assert_eq!(rt.state.surface(), Surface::QuickSwitch);

    rt.state.quick_switch_edit_mut().text = "Gmail".to_owned();
    rt.state.reset_quick_switch_selection();
    let entries = entries_for(&rt.state);
    assert!(
        entries
            .iter()
            .any(|e| matches!(e.action, QuickSwitchAction::InstallApp { .. })),
        "searching an uninstalled app should offer quick-install: {entries:?}"
    );

    // Pressing Enter installs Gmail and switches straight to it!
    assert!(send_key(&mut rt, Key::Enter));
    assert_eq!(rt.state.surface(), Surface::Apps);
    assert_ne!(rt.state.active().id, "appstore");
}
