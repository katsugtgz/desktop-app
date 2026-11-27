//! Slice 32 tests: native Custom / Private App creation and deletion UI in App Store.

use appstore_service::ApplicationService;
use rune::{Env, Key, KeyEvent, Mods, Runtime, TextEdit};

use crate::shell;
use crate::state::{ShellState, StoreSection};
use crate::webview;
use crate::webview::fake::{Event, FakeWebview};

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

fn press(rt: &mut Runtime<ShellState>, key: &str) {
    let (x, y) = rt
        .find(key)
        .unwrap_or_else(|| panic!("{key} is not laid out"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
}

fn text_of(rt: &Runtime<ShellState>, key: &str) -> Option<String> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .find(|e| e.key.as_deref() == Some(key))
        .and_then(|e| e.text)
}

#[test]
fn custom_app_form_validates_hostile_and_missing_inputs_without_writing() {
    let mut rt = runtime(ShellState::new(ApplicationService::for_tests(
        "custom-app-validate",
    )));
    rt.frame(1.0 / 60.0);

    press(&mut rt, "store-section-my-custom-apps");
    assert_eq!(rt.state.section(), StoreSection::MyCustomApps);
    press(&mut rt, "store-custom-add");
    assert!(rt.state.custom_app_draft().is_some());
    assert!(rt.find("store-custom-form").is_some());

    // Empty name refused.
    press(&mut rt, "store-custom-submit");
    assert!(text_of(&rt, "store-custom-error")
        .unwrap_or_default()
        .contains("name"));
    assert!(rt.state.custom().is_empty());

    // Hostile javascript: URL refused.
    {
        let draft = rt.state.custom_app_draft_mut().unwrap();
        draft.name = TextEdit::new("Evil App");
        draft.start_url = TextEdit::new("javascript:alert(1)");
    }
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    press(&mut rt, "store-custom-submit");
    assert!(text_of(&rt, "store-custom-error")
        .unwrap_or_default()
        .contains("Start URL"));
    assert!(rt.state.custom().is_empty());

    // Invalid hex color refused.
    {
        let draft = rt.state.custom_app_draft_mut().unwrap();
        draft.start_url = TextEdit::new("https://intranet.acme.test/app");
        draft.theme_color = TextEdit::new("not-a-color");
    }
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    press(&mut rt, "store-custom-submit");
    assert!(text_of(&rt, "store-custom-error")
        .unwrap_or_default()
        .contains("hex"));
    assert!(rt.state.custom().is_empty());

    // Escape closes the form cleanly without leaving the store.
    let mut tx = rune::Tx::default();
    assert!(shell::on_key(
        &mut rt.state,
        &KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        },
        &mut tx
    ));
    assert!(rt.state.custom_app_draft().is_none());
}

#[test]
fn creating_and_deleting_a_custom_app_persists_installs_and_closes_its_webview() {
    let _serial = crate::test_support::lock();
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));

    let tag = format!("custom-app-lifecycle-{}", std::process::id());
    let mut rt = runtime(ShellState::new(ApplicationService::for_tests(&tag)));
    rt.frame(1.0 / 60.0);

    press(&mut rt, "store-section-my-custom-apps");
    press(&mut rt, "store-custom-add");
    {
        let draft = rt.state.custom_app_draft_mut().unwrap();
        draft.name = TextEdit::new("Acme Wiki");
        draft.start_url = TextEdit::new("https://wiki.acme.test:8443/home");
        // Leave scope blank to verify automatic origin derivation.
        draft.scope = TextEdit::new("");
        draft.icon_url = TextEdit::new("https://wiki.acme.test/icon.png");
        draft.theme_color = TextEdit::new("#10B981");
    }
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    press(&mut rt, "store-custom-submit");

    // Draft closed, manifest persisted, installed & selected in rail.
    assert!(rt.state.custom_app_draft().is_none());
    assert_eq!(rt.state.custom().len(), 1);
    let created = rt.state.custom()[0].clone();
    assert_eq!(created.name, "Acme Wiki");
    assert_eq!(created.theme_color, "#10B981");
    assert_eq!(rt.state.active().label, "Acme Wiki");
    assert_eq!(rt.state.active().url, "https://wiki.acme.test:8443/home");

    // Verify routing scope was derived from the start URL origin (with port).
    let app_id = rt.state.active().id.clone();
    let routing_app = rt
        .state
        .routing_apps()
        .into_iter()
        .find(|a| a.application_id == app_id)
        .expect("installed custom app in routing table");
    assert_eq!(
        routing_app.scope.as_deref(),
        Some("https://wiki.acme.test:8443")
    );

    // Attach its web view so we can prove deletion detaches it.
    webview::after_frame(&rt);
    assert!(log
        .events()
        .iter()
        .any(|e| matches!(e, Event::Attach { tab, .. } if tab == &app_id)));

    // Return to App Store -> My custom apps and delete the custom app.
    log.clear();
    rt.state.open_app_store();
    rt.state.show_section(StoreSection::MyCustomApps);
    rt.invalidate();
    rt.frame(1.0 / 60.0);

    let delete_key = format!("store-custom-delete-{}", created.id);
    assert!(rt.find(&delete_key).is_some(), "missing {delete_key}");
    press(&mut rt, &delete_key);

    assert!(rt.state.custom().is_empty());
    assert_eq!(rt.state.tabs().len(), 1, "only App Store remains");
    assert!(log
        .events()
        .iter()
        .any(|e| matches!(e, Event::Detach { tab } if tab == &app_id)));
    assert!(rt.find("store-custom-empty").is_some());
}
