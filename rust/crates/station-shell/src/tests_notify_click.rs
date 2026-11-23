//! Slice 15 tests: a notification click reaches the page that raised it.
//!
//! The page side is the injected `INIT_SCRIPT` (its four frozen `bxApi.notificationCenter`
//! members and the internal dispatch hook); the shell side is `ShellState::click_notification`
//! (mark read + route) followed by `notifications::click` → `webview::click` → the
//! [`WebviewHost::click`](crate::webview::WebviewHost) seam. The headless tests drive the real
//! panel press through Rune and observe the recording host. Pages do render in this desktop
//! session (`docs/webview2-probe.md`: a seeded Gmail launch produced the WebView2 process tree
//! and a visible render widget titled "Sign in - Google Accounts"), but no GUI run was attempted
//! in this turn, so the wry host's `WebView::evaluate_script` call was **not exercised** live:
//! the remaining gap is that observation, not the runtime.

use notification_center::{NewNotificationOptions, NotificationAction, NotificationProps};
use rune::Env;
use rune::Runtime;

use crate::notifications::bridge::{INIT_SCRIPT, MAX_CLICK_LISTENERS};
use crate::shell;
use crate::state::{AppTab, ShellState};
use crate::webview;
use crate::webview::fake::{Event, FakeWebview};

const WINDOW: (f32, f32) = (1200.0, 800.0);

/// A shell with Gmail and Slack (web tabs) and the store; no session file.
fn state_with_pages() -> ShellState {
    let mut state = ShellState::new(appstore_service::ApplicationService::for_tests(
        "notify-click",
    ));
    state
        .tabs_mut_for_tests()
        .push(AppTab::web("gmail", "Gmail", "https://mail.google.com"));
    state
        .tabs_mut_for_tests()
        .push(AppTab::web("slack", "Slack", "https://app.slack.com"));
    state
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

/// The app's frame: draw, catch the host up, run the poll.
fn frame(rt: &mut Runtime<ShellState>, clock: f64) {
    rt.frame(1.0 / 60.0);
    webview::after_frame(rt);
    webview::poll_at(&mut rt.state, clock);
    rt.frame(1.0 / 60.0);
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

/// One stored notification as the bridge delivers it: attributed to `tab`.
fn add_notification(state: &mut ShellState, notification_id: &str, tab: &str, title: &str) {
    state
        .notifications_mut()
        .apply(NotificationAction::NewNotification {
            application_id: Some(tab.to_owned()),
            tab_id: Some(tab.to_owned()),
            notification_id: notification_id.to_owned(),
            props: NotificationProps {
                title: title.to_owned(),
                timestamp: Some(1_700_000_000_000),
                body: Some("body".to_owned()),
                icon: None,
            },
            options: NewNotificationOptions::default(),
        });
}

fn between<'a>(script: &'a str, start: &str, end: &str) -> &'a str {
    let from = script.find(start).expect("start marker") + start.len();
    let to = script[from..].find(end).expect("end marker") + from;
    &script[from..to]
}

fn clicks(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Click { tab, script } => Some((tab.clone(), script.clone())),
            _ => None,
        })
        .collect()
}

// ---- The injected page side ----------------------------------------------------------------

#[test]
fn the_init_script_exposes_exactly_the_four_notification_members() {
    // bxapi.d.ts: sendNotification, closeNotification, addNotificationClickListener,
    // removeNotificationClickListener — and nothing else of bxApi.
    let api = between(INIT_SCRIPT, "const api = {", "\n  };");
    for member in [
        "sendNotification",
        "closeNotification",
        "addNotificationClickListener",
        "removeNotificationClickListener",
    ] {
        assert!(api.contains(&format!("{member}: (")), "missing {member}");
    }
    assert_eq!(
        api.matches(": (").count(),
        4,
        "an extra bxApi member was invented: {api}"
    );
    assert!(INIT_SCRIPT.contains("Object.freeze"));
}

#[test]
fn the_ninth_click_listener_registration_is_refused() {
    assert_eq!(MAX_CLICK_LISTENERS, 8);
    assert!(INIT_SCRIPT.contains(&format!(
        "const MAX_CLICK_LISTENERS = {MAX_CLICK_LISTENERS};"
    )));
    // The registry check runs before the push, so the ninth listener is never kept.
    assert!(INIT_SCRIPT.contains(
        "if (typeof listener !== 'function' || clickListeners.length >= MAX_CLICK_LISTENERS) return;"
    ));
    assert!(INIT_SCRIPT.contains("clickListeners.push(listener);"));
}

#[test]
fn a_removed_click_listener_is_no_longer_invoked() {
    // The dispatch iterates a copy of the registry; removal splices the registry itself. A
    // removed callback is therefore absent from later dispatches (and one removed during a
    // dispatch keeps that dispatch's snapshot, like Node's EventEmitter).
    assert!(INIT_SCRIPT.contains("for (const listener of clickListeners.slice())"));
    assert!(INIT_SCRIPT.contains("const index = clickListeners.indexOf(listener);"));
    assert!(INIT_SCRIPT.contains("if (index !== -1) clickListeners.splice(index, 1);"));
}

#[test]
fn a_throwing_click_listener_does_not_stop_the_next() {
    // Each callback has its own try/catch inside the loop, so a throw is swallowed and the
    // remaining listeners still run.
    assert!(INIT_SCRIPT.contains("try { listener(id); } catch (e) {}"));
    let dispatch = between(INIT_SCRIPT, "const click = (id) => {", "};");
    assert_eq!(
        dispatch.matches("try {").count(),
        1,
        "the dispatch body should wrap exactly the listener call"
    );
}

#[test]
fn a_page_with_no_click_listener_is_dispatched_to_safely() {
    // An empty registry iterates zero times; the only throwing operation (the callback) is
    // guarded. Nothing else the delivery hook does can throw on a page with no listener.
    let dispatch = between(INIT_SCRIPT, "const click = (id) => {", "};");
    assert!(dispatch.contains("clickListeners.slice()"));
    assert!(!dispatch.contains("throw"));
    assert!(!dispatch.contains("JSON.parse"));
}

// ---- Shell → page delivery ----------------------------------------------------------------

#[test]
fn a_press_on_a_gmail_notification_delivers_exactly_one_click_to_gmail() {
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));
    let mut rt = runtime(state_with_pages());

    // Both pages exist as far as the host is concerned: Gmail shown, Slack attached then
    // backgrounded.
    rt.state.select("gmail");
    frame(&mut rt, 1.0);
    rt.state.select("slack");
    frame(&mut rt, 2.0 / 60.0);
    rt.state.select("gmail");
    frame(&mut rt, 3.0 / 60.0);
    assert_eq!(log.live_pages().len(), 2, "both pages should be attached");

    add_notification(&mut rt.state, "notif/1", "gmail", "Mail arrived");
    frame(&mut rt, 4.0 / 60.0);
    press(&mut rt, "rail-notifications");
    frame(&mut rt, 5.0 / 60.0);

    log.clear();
    press(&mut rt, "notification-notif/1");
    frame(&mut rt, 6.0 / 60.0);

    let delivered = clicks(&log.events());
    assert_eq!(
        delivered.len(),
        1,
        "one click message per press: {delivered:?}"
    );
    assert_eq!(delivered[0].0, "gmail", "the click went to the wrong tab");
    assert_eq!(
        delivered[0].1,
        format!(
            "window.__stationNotificationClick({});",
            serde_json::to_string("notif/1").unwrap()
        )
    );
    // The saga's order still holds: read first, then the sender's tab focused.
    assert_eq!(rt.state.notifications().unread_count(), 0);
    assert_eq!(rt.state.active().id, "gmail");
}

#[test]
fn a_press_delivers_to_the_source_tab_not_the_active_one_and_routes_to_the_source() {
    // Both pages attached, Gmail active, the notification's source is Slack. The click must go
    // to Slack only; the existing routing then selects Slack (the source), so that is asserted
    // too rather than "Gmail stays".
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));
    let mut rt = runtime(state_with_pages());
    rt.state.select("gmail");
    frame(&mut rt, 1.0);
    rt.state.select("slack");
    frame(&mut rt, 2.0 / 60.0);
    rt.state.select("gmail");
    frame(&mut rt, 3.0 / 60.0);
    assert_eq!(
        rt.state.active().id,
        "gmail",
        "Gmail must be the active tab"
    );
    assert_eq!(log.live_pages().len(), 2, "both pages should be attached");

    add_notification(&mut rt.state, "notif/slack-1", "slack", "Slack ping");
    frame(&mut rt, 4.0 / 60.0);
    press(&mut rt, "rail-notifications");
    frame(&mut rt, 5.0 / 60.0);

    log.clear();
    press(&mut rt, "notification-notif/slack-1");
    frame(&mut rt, 6.0 / 60.0);

    let delivered = clicks(&log.events());
    assert_eq!(
        delivered.len(),
        1,
        "one click message per press: {delivered:?}"
    );
    assert_eq!(
        delivered[0].0, "slack",
        "the click went to the active tab instead of the source"
    );
    assert_eq!(
        delivered[0].1,
        format!(
            "window.__stationNotificationClick({});",
            serde_json::to_string("notif/slack-1").unwrap()
        )
    );
    assert!(
        !delivered.iter().any(|(tab, _)| tab == "gmail"),
        "the active (Gmail) page received a click it did not raise"
    );
    // Existing routing (`ShellState::click_notification` → `select`) focuses the source tab.
    assert_eq!(rt.state.active().id, "slack");
    assert_eq!(rt.state.notifications().unread_count(), 0);
}

#[test]
fn a_press_with_no_attached_page_marks_read_and_routes_without_delivering() {
    // Gmail never became the active tab, so its page was never attached.
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));
    let mut rt = runtime(state_with_pages());
    frame(&mut rt, 1.0);
    assert!(!log.live_pages().iter().any(|tab| tab == "gmail"));

    add_notification(&mut rt.state, "notif/1", "gmail", "Mail arrived");
    frame(&mut rt, 2.0 / 60.0);
    press(&mut rt, "rail-notifications");
    frame(&mut rt, 3.0 / 60.0);

    log.clear();
    press(&mut rt, "notification-notif/1");
    frame(&mut rt, 4.0 / 60.0);

    assert!(
        clicks(&log.events()).is_empty(),
        "a click was delivered to a page that does not exist"
    );
    assert_eq!(
        rt.state.notifications().unread_count(),
        0,
        "mark-read still ran"
    );
    assert_eq!(rt.state.active().id, "gmail", "routing still ran");
}

#[test]
fn an_absent_page_delivers_nothing_and_does_not_panic() {
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));
    webview::click("ghost", "notif/1");
    assert!(log.events().is_empty());
}

#[test]
fn a_hostile_id_is_delivered_as_a_json_string_literal() {
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));
    let mut rt = runtime(state_with_pages());
    rt.state.select("gmail");
    frame(&mut rt, 1.0);
    log.clear();

    // Quotes, an apostrophe, a script-tag opener and a newline: all page-controlled.
    let hostile = "a\"'<script>\n";
    webview::click("gmail", hostile);

    let delivered = clicks(&log.events());
    assert_eq!(delivered.len(), 1, "one message for one press");
    assert_eq!(delivered[0].0, "gmail");
    assert_eq!(
        delivered[0].1,
        r#"window.__stationNotificationClick("a\"'<script>\n");"#
    );
    // The raw newline is escaped: the whole message stays one JS expression line.
    assert!(!delivered[0].1.contains('\n'));
    assert!(delivered[0].1.contains(r#"\n"#));
}
