//! Slice 27 tests: in-tab search (`Ctrl+F` / `Cmd+F`), content-rect shrinking, IPC match
//! counts, Escape clearing, and safe JSON-escaped script dispatch.

use appstore_service::ApplicationService;
use rune::{Env, Key, KeyEvent, Mods, Runtime, Tx};

use crate::find::FIND_BAR_HEIGHT;
use crate::notifications::bridge::{parse, Outcome, PageMessage, Refusal, Sender};
use crate::notifications::{clear, receive};
use crate::shell;
use crate::state::{AppTab, ShellState};
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

fn gmail_state(tag: &str) -> ShellState {
    let mut state = ShellState::new(ApplicationService::for_tests(tag));
    state
        .tabs_mut_for_tests()
        .push(AppTab::web("gmail", "Gmail", "https://mail.google.com"));
    state.select("gmail");
    state
}

fn frame(rt: &mut Runtime<ShellState>, clock: f64) {
    rt.frame(1.0 / 60.0);
    webview::after_frame(rt);
    if webview::poll_at(&mut rt.state, clock) {
        rt.invalidate();
    }
}

fn rect_of(rt: &Runtime<ShellState>, key: &str) -> (f32, f32, f32, f32) {
    let r = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no element `{key}`"))
        .rect;
    (r[0], r[1], r[2], r[3])
}

fn text_of(rt: &Runtime<ShellState>, key: &str) -> Option<String> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .find(|e| e.key.as_deref() == Some(key))
        .and_then(|e| e.text)
}

fn press(rt: &mut Runtime<ShellState>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element `{key}`"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    webview::after_frame(rt);
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
fn ctrl_f_opens_find_bar_shrinks_content_receives_counts_and_escape_clears() {
    let _serial = crate::tests_pages::bridge_lock();
    clear();
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));

    let mut rt = runtime(gmail_state("find-flow"));
    frame(&mut rt, 1.0);
    let base_rect = rect_of(&rt, "content");
    assert!(rt.find("find-bar").is_none());

    // Cmd/Ctrl+F opens the find bar cleanly above content (no WebView2 overlap).
    assert!(send_key(&mut rt, Key::Command('f')));
    rt.frame(1.0);
    frame(&mut rt, 1.1);
    assert!(rt.state.find().open);
    assert!(rt.find("find-bar").is_some());
    let shrunk_rect = rect_of(&rt, "content");
    assert!(
        (shrunk_rect.1 - (base_rect.1 + FIND_BAR_HEIGHT)).abs() < 1.0,
        "find bar must push content down by {FIND_BAR_HEIGHT} pt: base={base_rect:?} shrunk={shrunk_rect:?}"
    );
    assert!(
        (shrunk_rect.3 - (base_rect.3 - FIND_BAR_HEIGHT)).abs() < 1.0,
        "find bar must shrink content height by {FIND_BAR_HEIGHT} pt"
    );

    // Typing a query and dispatching Prev/Next evaluates JSON-escaped __stationFind.
    log.clear();
    rt.state.find_edit_mut().text = "a\"b(c+d)".to_owned();
    rt.state.dispatch_find(true);
    press(&mut rt, "find-prev");
    let evals: Vec<String> = log
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Eval { script, .. } => Some(script),
            _ => None,
        })
        .collect();
    assert_eq!(evals.len(), 2);
    assert_eq!(evals[0], "window.__stationFind(\"a\\\"b(c+d)\", true);");
    assert_eq!(evals[1], "window.__stationFind(\"a\\\"b(c+d)\", false);");

    // Page replies over IPC with `{ kind: "find", current: 2, total: 5 }` -> bar shows `2 / 5`.
    let tabs = rt.state.tabs().to_vec();
    assert_eq!(
        receive("gmail", &tabs, r#"{"kind":"find","current":2,"total":5}"#),
        Outcome::Accepted
    );
    frame(&mut rt, 1.2);
    frame(&mut rt, 1.3);
    assert_eq!(text_of(&rt, "find-count").as_deref(), Some("2 / 5"));

    // Zero matches renders "No matches".
    assert_eq!(
        receive("gmail", &tabs, r#"{"kind":"find","current":0,"total":0}"#),
        Outcome::Accepted
    );
    frame(&mut rt, 1.4);
    frame(&mut rt, 1.5);
    assert_eq!(text_of(&rt, "find-count").as_deref(), Some("No matches"));

    // Escape closes the find bar and evaluates `__stationFindClear()`.
    log.clear();
    assert!(send_key(&mut rt, Key::Escape));
    rt.frame(1.0);
    frame(&mut rt, 1.6);
    assert!(!rt.state.find().open);
    assert!(rt.find("find-bar").is_none());
    let clear_evals: Vec<String> = log
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Eval { script, .. } => Some(script),
            _ => None,
        })
        .collect();
    assert_eq!(clear_evals, vec!["window.__stationFindClear();"]);
    let restored_rect = rect_of(&rt, "content");
    assert!((restored_rect.1 - base_rect.1).abs() < 1.0);
}

#[test]
fn find_ipc_refuses_inconsistent_or_oversized_counts() {
    let sender = Sender::App {
        tab_id: "gmail".to_owned(),
        application_id: "gmail".to_owned(),
    };
    assert_eq!(
        parse(sender.clone(), r#"{"kind":"find","current":3,"total":2}"#),
        Err(Refusal::TooLarge)
    );
    assert_eq!(
        parse(
            sender.clone(),
            r#"{"kind":"find","current":1,"total":200000}"#
        ),
        Err(Refusal::TooLarge)
    );
    assert_eq!(
        parse(sender, r#"{"kind":"find","current":2,"total":5}"#),
        Ok(PageMessage::Find {
            tab_id: "gmail".to_owned(),
            current: 2,
            total: 5,
        })
    );
}
