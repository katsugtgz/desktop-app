//! Slice 20 tests: a page's own title and `browserx-badge` meta reach the chrome.
//!
//! The Electron preload observed `document.title` (the `<webview>`'s `page-title-updated`
//! event) and the `browserx-*` metas (`page-bxmetas-updated`); those are what this shell now
//! accepts on its one page channel. These tests drive the real entry (`notifications::receive`)
//! with the message shapes the injected script builds, then read the drawn top bar / rail and
//! the session document. No JavaScript engine runs here; the script's own source is asserted in
//! `notifications::tests_notification_override` and the script-side observer shapes below.

use appstore_service::ApplicationService;
use rune::{Env, Runtime};

use crate::notifications::bridge::{Outcome, Refusal, MAX_BADGE_CHARS, MAX_PAGE_TITLE_CHARS};
use crate::notifications::{clear, receive};
use crate::shell;
use crate::state::{AppTab, ShellState};
use crate::webview;
use crate::webview::fake::FakeWebview;

const WINDOW: (f32, f32) = (1200.0, 800.0);

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

/// A shell with one web tab (`gmail`) selected over an isolated private-manifest store.
fn gmail_state(tag: &str) -> ShellState {
    let mut state = ShellState::new(ApplicationService::for_tests(tag));
    state
        .tabs_mut_for_tests()
        .push(AppTab::web("gmail", "Gmail", "https://mail.google.com"));
    state.select("gmail");
    state
}

/// Draw, let the host catch up, run the poll (which delivers accepted page messages), and
/// invalidate when the poll says the view changed.
fn frame(rt: &mut Runtime<ShellState>, clock: f64) {
    rt.frame(1.0 / 60.0);
    webview::after_frame(rt);
    if webview::poll_at(&mut rt.state, clock) {
        rt.invalidate();
    }
}

/// The message the injected script builds for `document.title`.
fn title_message(title: &str) -> String {
    serde_json::json!({ "kind": "title", "title": title }).to_string()
}

/// The message the injected script builds for the `browserx-*` metas.
fn meta_message(badge: Option<&str>) -> String {
    let mut metas = serde_json::Map::new();
    if let Some(badge) = badge {
        metas.insert("badge".to_owned(), serde_json::Value::from(badge));
    }
    serde_json::json!({ "kind": "meta", "metas": metas }).to_string()
}

/// Feeds `message` from `origin` through the real bridge entry, then two frames so the poll
/// delivers it and the view draws it. Returns the bridge's outcome.
fn send(rt: &mut Runtime<ShellState>, origin: &str, message: &str) -> Outcome {
    let tabs = rt.state.tabs().to_vec();
    let outcome = receive(origin, &tabs, message);
    frame(rt, 1.0);
    rt.frame(1.0 / 60.0);
    outcome
}

fn text_of(rt: &Runtime<ShellState>, key: &str) -> Option<String> {
    rt.elements()
        .into_iter()
        .filter(|element| !element.exiting)
        .find(|element| element.key.as_deref() == Some(key))
        .and_then(|element| element.text)
}

fn label_of(rt: &Runtime<ShellState>, key: &str) -> Option<String> {
    rt.elements()
        .into_iter()
        .filter(|element| !element.exiting)
        .find(|element| element.key.as_deref() == Some(key))
        .and_then(|element| element.label)
}

// ---- Titles ---------------------------------------------------------------------------

#[test]
fn a_page_title_becomes_the_drawn_label() {
    let _serial = crate::tests_pages::bridge_lock();
    clear();
    let mut rt = runtime(gmail_state("page-title"));
    rt.frame(1.0 / 60.0);
    assert_eq!(text_of(&rt, "topbar-title").as_deref(), Some("Gmail"));
    assert_eq!(label_of(&rt, "tab-gmail").as_deref(), Some("Gmail"));

    let outcome = send(&mut rt, "gmail", &title_message("Inbox (3)"));
    assert_eq!(outcome, Outcome::Accepted);

    assert_eq!(
        text_of(&rt, "topbar-title").as_deref(),
        Some("Inbox (3)"),
        "the top bar did not take the page title"
    );
    assert_eq!(
        label_of(&rt, "tab-gmail").as_deref(),
        Some("Inbox (3)"),
        "the rail's accessible name did not take the page title"
    );
    assert_eq!(
        rt.state.active().id,
        "gmail",
        "a title update changed the selection"
    );
    assert_eq!(
        rt.state.active().label,
        "Gmail",
        "the manifest label was overwritten instead of overridden"
    );
}

#[test]
fn clearing_the_title_restores_the_manifest_label() {
    let _serial = crate::tests_pages::bridge_lock();
    clear();
    let mut rt = runtime(gmail_state("page-title-clear"));
    rt.frame(1.0 / 60.0);

    send(&mut rt, "gmail", &title_message("Inbox"));
    assert_eq!(text_of(&rt, "topbar-title").as_deref(), Some("Inbox"));

    // Whitespace trims to empty, which the bridge treats as the page clearing its title.
    let outcome = send(&mut rt, "gmail", &title_message("   "));
    assert_eq!(outcome, Outcome::Accepted);
    assert_eq!(
        text_of(&rt, "topbar-title").as_deref(),
        Some("Gmail"),
        "the manifest label did not come back"
    );
    assert_eq!(label_of(&rt, "tab-gmail").as_deref(), Some("Gmail"));
    assert_eq!(rt.state.active().page_title, None);
}

#[test]
fn oversized_or_control_titles_are_refused_and_keep_the_current_label() {
    let _serial = crate::tests_pages::bridge_lock();
    clear();
    let mut rt = runtime(gmail_state("page-title-hostile"));
    rt.frame(1.0 / 60.0);
    send(&mut rt, "gmail", &title_message("Inbox"));

    let oversized = "a".repeat(MAX_PAGE_TITLE_CHARS + 1);
    assert_eq!(
        send(&mut rt, "gmail", &title_message(&oversized)),
        Outcome::Refused(Refusal::BadText),
        "an oversized title was not refused"
    );
    assert_eq!(text_of(&rt, "topbar-title").as_deref(), Some("Inbox"));

    assert_eq!(
        send(&mut rt, "gmail", &title_message("Inbox\u{7}bell")),
        Outcome::Refused(Refusal::BadText),
        "a control character was not refused"
    );
    assert_eq!(
        text_of(&rt, "topbar-title").as_deref(),
        Some("Inbox"),
        "a refused title changed the label"
    );
}

// ---- Badges ---------------------------------------------------------------------------

#[test]
fn a_badge_count_draws_and_a_text_badge_draws_a_dot() {
    let _serial = crate::tests_pages::bridge_lock();
    clear();
    let mut rt = runtime(gmail_state("page-badge"));
    rt.frame(1.0 / 60.0);
    assert!(
        rt.find("tab-badge-gmail").is_none(),
        "a fresh tab drew a badge"
    );

    let outcome = send(&mut rt, "gmail", &meta_message(Some("3")));
    assert_eq!(outcome, Outcome::Accepted);
    assert!(
        rt.find("tab-badge-gmail").is_some(),
        "the rail row has no badge"
    );
    assert_eq!(
        text_of(&rt, "tab-badge-count-gmail").as_deref(),
        Some("3"),
        "a numeric badge did not draw its count"
    );

    // The TS site scripts use `•` for a dot (Slack); text badges render as a dot, not a word.
    send(&mut rt, "gmail", &meta_message(Some("•")));
    assert!(
        rt.find("tab-badge-gmail").is_some(),
        "the dot badge disappeared"
    );
    assert_eq!(
        text_of(&rt, "tab-badge-count-gmail"),
        None,
        "a text badge drew a count"
    );

    // Past 999 the count is capped for drawing (the stored value stays the page's).
    send(&mut rt, "gmail", &meta_message(Some("1500")));
    assert_eq!(
        text_of(&rt, "tab-badge-count-gmail").as_deref(),
        Some("999+")
    );

    // An unrelated `browserx-*` meta is not a badge: the missing `badge` key clears it.
    send(
        &mut rt,
        "gmail",
        &serde_json::json!({ "kind": "meta", "metas": { "other": "x" } }).to_string(),
    );
    assert!(
        rt.find("tab-badge-gmail").is_none(),
        "an unknown meta left the badge in place"
    );

    // A missing or empty badge meta clears it.
    send(&mut rt, "gmail", &meta_message(Some("2")));
    assert!(rt.find("tab-badge-gmail").is_some());
    send(&mut rt, "gmail", &meta_message(None));
    assert!(
        rt.find("tab-badge-gmail").is_none(),
        "a cleared badge still draws"
    );
    assert_eq!(rt.state.active().badge, None);
}

#[test]
fn an_oversized_badge_is_refused_and_does_not_reach_the_rail() {
    let _serial = crate::tests_pages::bridge_lock();
    clear();
    let mut rt = runtime(gmail_state("page-badge-hostile"));
    rt.frame(1.0 / 60.0);

    let oversized = "a".repeat(MAX_BADGE_CHARS + 1);
    assert_eq!(
        send(&mut rt, "gmail", &meta_message(Some(&oversized))),
        Outcome::Refused(Refusal::BadText)
    );
    assert!(rt.find("tab-badge-gmail").is_none());
    assert_eq!(
        send(&mut rt, "gmail", &meta_message(Some("3\u{7}"))),
        Outcome::Refused(Refusal::BadText)
    );
    assert!(rt.find("tab-badge-gmail").is_none());
}

#[test]
fn a_title_or_badge_from_an_unknown_tab_is_refused() {
    let _serial = crate::tests_pages::bridge_lock();
    clear();
    let mut rt = runtime(gmail_state("page-meta-sender"));
    rt.frame(1.0 / 60.0);

    assert_eq!(
        send(&mut rt, "ghost", &title_message("Not yours")),
        Outcome::Refused(Refusal::UnknownSender)
    );
    assert_eq!(
        send(&mut rt, "ghost", &meta_message(Some("9"))),
        Outcome::Refused(Refusal::UnknownSender)
    );
    // The Rune-drawn App Store tab never has a page, so it cannot speak either.
    assert_eq!(
        send(&mut rt, "appstore", &title_message("Not yours")),
        Outcome::Refused(Refusal::UnknownSender)
    );
    assert_eq!(text_of(&rt, "topbar-title").as_deref(), Some("Gmail"));
    assert!(rt.find("tab-badge-gmail").is_none());
}

// ---- Bounds ---------------------------------------------------------------------------

#[test]
fn a_title_or_badge_update_causes_no_host_event() {
    let _serial = crate::tests_pages::bridge_lock();
    clear();
    let log = {
        let (host, log) = FakeWebview::new();
        webview::install(Box::new(host));
        log
    };
    let mut rt = runtime(gmail_state("page-meta-host"));
    frame(&mut rt, 1.0);
    rt.frame(1.0 / 60.0);
    assert!(
        log.events()
            .iter()
            .any(|event| matches!(event, crate::webview::fake::Event::Attach { .. })),
        "the page never attached; the test proves nothing"
    );
    log.clear();

    send(&mut rt, "gmail", &title_message("Inbox"));
    send(&mut rt, "gmail", &meta_message(Some("2")));
    assert!(
        log.events().is_empty(),
        "a title/badge update touched the page: {:?}",
        log.events()
    );
}

#[test]
fn the_session_document_keeps_neither_title_nor_badge() {
    let _serial = crate::tests_pages::bridge_lock();
    clear();
    let path = std::env::temp_dir().join(format!(
        "station-shell-page-meta-{}-{}.json",
        std::process::id(),
        "session"
    ));
    let _ = std::fs::remove_file(&path);
    let mut state =
        ShellState::launch_in(ApplicationService::for_tests("page-meta-session"), &path);
    assert!(state.install("station-manifest://14"));
    let gmail = state.active().id.clone();

    // A page says its title and badge; then a user action writes the session.
    assert!(
        state.set_page_title(&gmail, "Inbox (3)"),
        "the title did not apply"
    );
    assert!(state.set_page_badge(&gmail, Some("3")));
    state.set_theme(session_store::ThemePreference::Dark);

    let document = std::fs::read_to_string(&path).expect("the session was written");
    for absent in ["Inbox (3)", "pageTitle", "\"badge\"", "browserx-"] {
        assert!(
            !document.contains(absent),
            "the session document kept {absent}: {document}"
        );
    }
    // The tab record is still the install's: URL and home flag, no page presentation.
    assert!(document.contains("station-manifest://14") || document.contains("accounts.google.com"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_page_title_survives_a_rebuild_but_not_the_document() {
    // `rebuild_tabs` runs on install/uninstall/config/reorder; the page's own label is keyed by
    // tab id and must not be lost by an unrelated rebuild.
    let _binding = crate::tests_pages::bridge_lock();
    let mut state = ShellState::new(ApplicationService::for_tests("page-meta-rebuild"));
    assert!(state.install("station-manifest://14"));
    let gmail = state.active().id.clone();
    assert!(state.set_page_title(&gmail, "Inbox (3)"));
    assert!(state.set_page_badge(&gmail, Some("3")));

    // Installing another app rebuilds the rail from the service; the page's presentation rides
    // along, keyed by tab id.
    assert!(state.install("station-manifest://157"));
    let tab = state
        .tabs()
        .iter()
        .find(|tab| tab.id == gmail)
        .expect("gmail is still installed");
    assert_eq!(tab.display_label(), "Inbox (3)");
    assert!(tab.badge.is_some());
}

#[test]
fn the_injected_observer_builds_the_messages_this_shell_parses() {
    // No JavaScript engine runs here; this pins the script-side half (the observer's selector,
    // the `browserx-` key strip, both `kind`s and their change dedupe) against the wire shapes
    // the tests above drive.
    let script = crate::notifications::INIT_SCRIPT;
    assert!(script.contains("const META_SELECTOR = 'meta[name^=browserx-]';"));
    assert!(script.contains("meta.name.slice('browserx-'.length)"));
    assert!(script.contains("postPageMeta({ kind: 'meta', metas: values });"));
    assert!(script.contains("postPageMeta({ kind: 'title', title: title });"));
    assert!(script.contains("const title = document.title;"));
    assert!(script.contains("new MutationObserver"));
    assert!(
        script.contains("subtree: true, childList: true, attributes: true, characterData: true,")
    );
    assert!(script.contains("if (encoded === sentMetas) return;"));
    assert!(script.contains("if (title === sentTitle) return;"));
    assert!(script.contains(
        "document.addEventListener('DOMContentLoaded', observePageMeta, { once: true });"
    ));
}
