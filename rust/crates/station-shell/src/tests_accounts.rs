//! Slice 17 tests: accounts exist as data.
//!
//! The identity model lives in `session-store` (stored, validated, additive to format 1; the
//! store's own tests cover the refusals). This module drives the shell: Settings lists what the
//! document holds, the `google-account` Configure editor offers a stored identity through the
//! existing `set_app_config` path, the tab URL is re-resolved like any other configuration, and
//! the choice survives a relaunch. Nothing here signs in: this slice stores no tokens and has no
//! button that would pretend to.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use appstore_service::ApplicationService;
use rune::{Env, Runtime};
use session_store::{Identity, IdentityProfile, SessionState, SessionStore};

use crate::shell;
use crate::state::ShellState;
use crate::webview;
use crate::webview::fake::FakeWebview;

const WINDOW: (f32, f32) = (1200.0, 800.0);

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn tag() -> String {
    format!(
        "station-shell-accounts-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A session file path no other test or process uses, removed first.
fn session_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "station-shell-accounts-{name}-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    path
}

/// A shell launched from `path` over an isolated private-manifest store.
fn launched(path: &std::path::Path) -> ShellState {
    ShellState::launch_in(ApplicationService::for_tests(&tag()), path)
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

fn keys_with_prefix(rt: &Runtime<ShellState>, prefix: &str) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter_map(|element| element.key)
        .filter(|key| key.starts_with(prefix))
        .collect()
}

/// One stored identity in the TS shape (`identityId: "google-<sub>"`), no tokens.
fn identity(id: &str, name: &str, email: &str) -> Identity {
    Identity {
        identity_id: id.to_owned(),
        provider: "google".to_owned(),
        user_id: format!("user-{id}"),
        profile_data: IdentityProfile {
            display_name: name.to_owned(),
            first_name: name.split_whitespace().next().unwrap_or("").to_owned(),
            last_name: name.split_whitespace().last().unwrap_or("").to_owned(),
            email: email.to_owned(),
            image_url: format!("https://cdn.example/{id}.png"),
        },
    }
}

/// Writes a session document holding `identities` (what a later sign-in slice will save).
fn write_accounts(path: &std::path::Path, identities: &[Identity]) {
    let document = SessionState {
        identities: identities
            .iter()
            .map(|identity| (identity.identity_id.clone(), identity.clone()))
            .collect::<BTreeMap<_, _>>(),
        ..SessionState::default()
    };
    SessionStore::new(path).save(&document).expect("save");
}

/// Gmail's manifest id (google-account preset) in `packages/app/manifests/definitions`.
const GMAIL_TILE: &str = "store-tile-14";

fn install_gmail(rt: &mut Runtime<ShellState>) -> String {
    press(rt, GMAIL_TILE);
    draw(rt);
    let id = rt.state.active().id.clone();
    assert_ne!(id, "appstore", "the install did not select the app");
    id
}

// ---- Settings -------------------------------------------------------------------------

#[test]
fn settings_lists_the_stored_accounts() {
    let path = session_path("list");
    write_accounts(
        &path,
        &[
            identity("google-ada", "Ada Lovelace", "ada@example.com"),
            identity("google-grace", "Grace Hopper", "grace@example.com"),
        ],
    );
    let mut rt = runtime(launched(&path));
    draw(&mut rt);
    press(&mut rt, "rail-settings");
    draw(&mut rt);

    assert!(rt.find("settings-accounts").is_some(), "no Accounts card");
    for (id, name, email) in [
        ("google-ada", "Ada Lovelace", "ada@example.com"),
        ("google-grace", "Grace Hopper", "grace@example.com"),
    ] {
        assert!(
            rt.find(&format!("settings-account-{id}")).is_some(),
            "account {id} is not listed"
        );
        assert_eq!(
            rt.elements()
                .into_iter()
                .find(|element| element.key.as_deref()
                    == Some(&format!("settings-account-name-{id}")))
                .and_then(|element| element.text),
            Some(name.to_owned()),
            "the display name is not shown"
        );
        assert_eq!(
            rt.elements()
                .into_iter()
                .find(|element| element.key.as_deref()
                    == Some(&format!("settings-account-email-{id}")))
                .and_then(|element| element.text),
            Some(email.to_owned()),
            "the email is not shown"
        );
    }
    // The identities were not invented by the card: the shell's stored set is what it drew.
    assert_eq!(rt.state.identities().len(), 2);
}

#[test]
fn the_empty_accounts_card_is_honest_about_sign_in() {
    let mut rt = runtime(launched(&session_path("empty")));
    draw(&mut rt);
    press(&mut rt, "rail-settings");
    draw(&mut rt);

    assert!(rt.find("settings-accounts").is_some(), "no Accounts card");
    assert!(shows(&rt, "No accounts yet."), "no empty-state sentence");
    assert!(
        shows(&rt, "Sign-in is not available in the native shell yet."),
        "the empty card does not say sign-in is absent"
    );
    assert!(
        rt.find("settings-accounts-signin").is_some(),
        "the honest note has no key"
    );
    assert!(
        keys_with_prefix(&rt, "settings-account-").is_empty(),
        "an empty store drew an account row"
    );
    // No sign-in control: nothing in this slice can create an identity.
    let buttons: Vec<String> = rt
        .elements()
        .into_iter()
        .filter(|element| element.role == "button")
        .filter_map(|element| element.label)
        .collect();
    assert!(
        !buttons.iter().any(|label| label.contains("Sign in")),
        "a sign-in button exists before sign-in: {buttons:?}"
    );
}

// ---- The Configure editor -------------------------------------------------------------

#[test]
fn the_chooser_writes_the_identity_and_the_tab_follows_the_existing_path() {
    let log = {
        let (host, log) = FakeWebview::new();
        webview::install(Box::new(host));
        log
    };
    let path = session_path("chooser");
    write_accounts(
        &path,
        &[identity("google-ada", "Ada Lovelace", "ada@example.com")],
    );
    let mut rt = runtime(launched(&path));
    draw(&mut rt);
    let gmail = install_gmail(&mut rt);
    let before = rt.state.tab_url(&gmail).map(str::to_owned);
    log.clear();

    press(&mut rt, "rail-settings");
    draw(&mut rt);
    press(&mut rt, &format!("settings-configure-{gmail}"));
    draw(&mut rt);

    assert!(
        shows(&rt, "Choose an account"),
        "the editor does not offer the stored account"
    );
    assert!(
        rt.find("config-account-google-ada").is_some(),
        "the stored identity is not choosable"
    );
    // The chooser is the whole field: a Save for the (unused) draft text would revert the
    // choice, so it is absent.
    assert!(
        rt.find(&format!("config-save-{gmail}")).is_none(),
        "a Save control survived beside the chooser"
    );
    assert!(
        rt.find(&format!("config-edit-{gmail}")).is_none(),
        "a text field survived beside the chooser"
    );

    press(&mut rt, "config-account-google-ada");
    draw(&mut rt);

    assert_eq!(
        rt.state.config_of(&gmail).identity_id.as_deref(),
        Some("google-ada"),
        "the chooser did not write the identity through set_app_config"
    );
    // Gmail's template uses `userIdentity.profileData.email`, which the shell cannot render
    // (TS leaves it empty too — docs/oauth-config-options.md), so the re-resolved URL is the
    // same string. The path is what must hold: the tab is re-resolved and an open page is
    // navigated only when the URL moved.
    assert_eq!(
        rt.state.tab_url(&gmail),
        before.as_deref(),
        "the tab URL is not the re-resolved configuration"
    );
    assert!(
        !log.events()
            .iter()
            .any(|event| matches!(event, crate::webview::fake::Event::Load { .. })),
        "an unchanged URL navigated the page: {:?}",
        log.events()
    );

    // The document carries both the pointer and the account.
    let saved = SessionStore::new(&path).load().expect("load");
    assert_eq!(
        saved
            .app_config
            .get(&gmail)
            .and_then(|config| config.identity_id.as_deref()),
        Some("google-ada"),
        "the choice did not reach appConfig"
    );
    assert!(
        saved.identities.contains_key("google-ada"),
        "the account list was lost when the choice was saved"
    );

    // A relaunch restores both; the tab still opens at the configured URL.
    let restored = launched(&path);
    assert_eq!(
        restored.config_of(&gmail).identity_id.as_deref(),
        Some("google-ada"),
        "the relaunch lost the configured identity"
    );
    assert_eq!(
        restored.identities().len(),
        1,
        "the relaunch lost the account"
    );
    assert_eq!(
        restored.tab_url(&gmail),
        Some(before.expect("signed-out URL").as_str()),
        "the relaunch did not re-resolve the configured app"
    );
}

#[test]
fn a_google_account_app_without_identities_keeps_the_honest_note() {
    let mut rt = runtime(launched(&session_path("no-identity")));
    draw(&mut rt);
    let gmail = install_gmail(&mut rt);

    press(&mut rt, "rail-settings");
    draw(&mut rt);
    press(&mut rt, &format!("settings-configure-{gmail}"));
    draw(&mut rt);

    assert!(
        shows(&rt, "Sign-in is not available in the native shell yet."),
        "a google-account app must say sign-in is unavailable"
    );
    assert!(
        rt.find("config-identity-empty-hint").is_some(),
        "the empty editor gives no next step"
    );
    assert!(
        shows(&rt, "Add an account once sign-in lands."),
        "the hint does not say how an account would appear"
    );
    assert!(
        keys_with_prefix(&rt, "config-account-").is_empty(),
        "an empty store offered a chooser"
    );
    assert!(
        rt.find(&format!("config-edit-{gmail}")).is_none(),
        "the editor collects an identity string the shell cannot obtain"
    );
    assert!(
        rt.find(&format!("config-save-{gmail}")).is_none(),
        "a Save control exists with nothing to write"
    );
}

// ---- The document ---------------------------------------------------------------------

#[test]
fn accounts_ride_the_session_document_through_an_unrelated_save() {
    let path = session_path("unrelated-save");
    write_accounts(
        &path,
        &[identity("google-ada", "Ada Lovelace", "ada@example.com")],
    );
    let mut state = launched(&path);
    assert_eq!(state.identities().len(), 1);

    // A user action that writes the session (the appearance toggle) must carry the accounts.
    state.set_theme(session_store::ThemePreference::Dark);

    let saved = SessionStore::new(&path).load().expect("load");
    assert_eq!(saved.theme, session_store::ThemePreference::Dark);
    let ada = saved.identities.get("google-ada").expect("account kept");
    assert_eq!(ada.profile_data.display_name, "Ada Lovelace");
    assert_eq!(ada.profile_data.email, "ada@example.com");
    assert_eq!(ada.provider, "google");
}
