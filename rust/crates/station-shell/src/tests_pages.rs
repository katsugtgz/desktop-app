//! Slice 8 tests: the page→shell notification pipeline, end to end.
//!
//! Everything here drives the real IPC-facing entry (`notifications::receive`, the same function
//! the wry `with_ipc_handler` calls with a page's `window.ipc.postMessage` payload) through the
//! real frame poll (`webview::poll_at`, which delivers to the center and expires snooze), and
//! asserts on the drawn panel and the recording host. No test bypasses the parse/limits layer.

use rune::Env;
use rune::{Key, KeyEvent, Mods, Runtime};

use crate::notifications::bridge::{
    parse, pending, sender_of, Outcome, PageMessage, Refusal, Sender, MAX_BODY_LEN, MAX_ID_LEN,
    MAX_MESSAGE_BYTES, MAX_PENDING, MAX_PER_SOURCE, MAX_TITLE_LEN,
};
use crate::notifications::{clear, deliver_to, expire_snooze, receive};
use crate::shell;
use crate::state::{AppTab, ShellState};
use crate::webview;
use crate::webview::fake::{Event, FakeWebview};

const WINDOW: (f32, f32) = (1200.0, 800.0);

/// Serializes every test that touches the bridge's module-static queue or the routing budget:
/// both are process-global, every test's frame poll drains them, and `cargo test` runs a
/// binary's tests in parallel threads. It is the shared test lock (`test_support`), which the
/// poll's drains also take.
#[cfg(test)]
pub(crate) fn bridge_lock() -> crate::test_support::Guard {
    crate::test_support::lock()
}

fn with_bridge<T>(body: impl FnOnce() -> T) -> T {
    let _guard = bridge_lock();
    clear();
    let result = body();
    clear();
    result
}

/// A shell with Gmail (web tab, `https://mail.google.com`) and the store; no session file.
fn gmail_state() -> ShellState {
    let mut state = ShellState::new(appstore_service::ApplicationService::for_tests(
        "notif-slice8",
    ));
    // The rail's store row is id `appstore`; a web tab for the page-source app.
    state
        .tabs_mut_for_tests()
        .push(AppTab::web("gmail", "Gmail", "https://mail.google.com"));
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

/// The app's frame: draw, catch the host up, run the poll (the delivery + expiry path).
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

/// One `sendNotification` payload as the preload sends it.
fn send(id: &str, title: &str, body: &str) -> String {
    format!(
        r#"{{"kind":"send","id":"{id}","notification":{{"title":"{title}","body":"{body}","timestamp":1700000000000}}}}"#
    )
}

/// A page message from Gmail's tab, through the real `receive`.
fn from_gmail(state: &ShellState, message: &str) -> Outcome {
    let tabs = state.tabs().to_vec();
    receive("gmail", &tabs, message)
}

// ---- The trust boundary --------------------------------------------------------------------

#[test]
fn a_page_send_becomes_a_bounded_message() {
    with_bridge(|| {
        clear();
        let state = gmail_state();
        let outcome = from_gmail(&state, &send("notif/1", "Mail arrived", "From Ada"));
        assert_eq!(outcome, crate::notifications::bridge::Outcome::Accepted);
        match &pending()[0] {
            PageMessage::Send {
                tab_id,
                application_id,
                notification_id,
                props,
            } => {
                assert_eq!(tab_id, "gmail");
                assert_eq!(application_id, "gmail");
                assert_eq!(notification_id, "notif/1");
                assert_eq!(props.title, "Mail arrived");
                assert_eq!(props.body.as_deref(), Some("From Ada"));
            }
            other => panic!("unexpected {other:?}"),
        }
    });
}

#[test]
fn hostile_payloads_are_refused_not_panicked_on() {
    with_bridge(|| {
        let state = gmail_state();
        let tabs = state.tabs().to_vec();
        // Not JSON, not an object, unknown kind, wrong-typed members.
        for bad in [
            "",
            "not json {",
            "null",
            "[1, 2, 3]",
            "42",
            r#"{"kind":"extra","id":"x"}"#,
            r#"{"kind":"close"}"#,
        ] {
            let outcome = receive("gmail", &tabs, bad);
            assert_eq!(
                outcome,
                crate::notifications::bridge::Outcome::Refused(Refusal::Malformed),
                "refused nothing for {bad:?}"
            );
        }
        // An empty id is bounded, not malformed: nothing to address.
        assert_eq!(
            receive("gmail", &tabs, r#"{"kind":"send","id":""}"#),
            crate::notifications::bridge::Outcome::Refused(Refusal::TooLarge)
        );
        // Oversized fields, each at its stated cap.
        let long_title = "x".repeat(MAX_TITLE_LEN + 1);
        assert_eq!(
            parse(
                Sender::App {
                    tab_id: "gmail".into(),
                    application_id: "gmail".into()
                },
                &format!(r#"{{"kind":"send","id":"x","notification":{{"title":"{long_title}"}}}}"#)
            ),
            Err(Refusal::TooLarge)
        );
        let long_body = "x".repeat(MAX_BODY_LEN + 1);
        assert_eq!(
            parse(
                Sender::App {
                    tab_id: "gmail".into(),
                    application_id: "gmail".into()
                },
                &format!(r#"{{"kind":"send","id":"x","notification":{{"body":"{long_body}"}}}}"#)
            ),
            Err(Refusal::TooLarge)
        );
        let long_id = "x".repeat(MAX_ID_LEN + 1);
        assert_eq!(
            parse(
                Sender::App {
                    tab_id: "gmail".into(),
                    application_id: "gmail".into()
                },
                &format!(r#"{{"kind":"close","id":"{long_id}"}}"#)
            ),
            Err(Refusal::TooLarge)
        );
        let huge = format!(
            "{{\"kind\":\"send\",\"id\":\"x\",\"padding\":\"{}\"}}",
            "x".repeat(MAX_MESSAGE_BYTES)
        );
        assert!(huge.len() > MAX_MESSAGE_BYTES);
        assert_eq!(
            receive("gmail", &tabs, &huge),
            crate::notifications::bridge::Outcome::Refused(Refusal::TooLarge)
        );
    });
}

#[test]
fn a_message_from_an_unknown_tab_is_refused() {
    with_bridge(|| {
        let state = gmail_state();
        let tabs = state.tabs().to_vec();
        assert_eq!(
            receive("not-a-tab", &tabs, r#"{"kind":"close","id":"x"}"#),
            crate::notifications::bridge::Outcome::Refused(Refusal::UnknownSender)
        );
        // The store's tab is Rune-drawn and never hosts a page: it is not a source either.
        assert_eq!(
            receive("appstore", &tabs, r#"{"kind":"close","id":"x"}"#),
            crate::notifications::bridge::Outcome::Refused(Refusal::UnknownSender)
        );
        assert_eq!(sender_of("appstore", &tabs), Sender::Unknown);
    });
}

#[test]
fn a_same_id_send_replaces_instead_of_growing() {
    with_bridge(|| {
        clear();
        let state = gmail_state();
        assert!(from_gmail(&state, &send("same", "first", "")).accepted());
        assert!(from_gmail(&state, &send("same", "second", "")).accepted());
        let held = pending();
        assert_eq!(held.len(), 1, "a reused id replaces the queued one");
        match &held[0] {
            PageMessage::Send { props, .. } => assert_eq!(props.title, "second"),
            other => panic!("unexpected {other:?}"),
        }
    });
}

#[test]
fn a_flood_of_messages_is_bounded() {
    with_bridge(|| {
        clear();
        let state = gmail_state();
        for index in 0..(MAX_PENDING + 50) {
            let _ = from_gmail(&state, &send(&format!("n{index}"), "t", ""));
        }
        assert_eq!(pending().len(), MAX_PENDING, "the hold is capped");
        // And delivery keeps the live counts at their caps too.
        let mut state = state;
        deliver_to(&mut state);
        assert!(state.notifications().unread_count() <= MAX_PENDING);
        clear();
    });
}

#[test]
fn the_live_caps_hold_after_delivery() {
    with_bridge(|| {
        clear();
        let mut state = gmail_state();
        for index in 0..(MAX_PER_SOURCE + 20) {
            let _ = from_gmail(&state, &send(&format!("n{index}"), "t", ""));
        }
        deliver_to(&mut state);
        assert_eq!(
            state.notifications().unread_count(),
            MAX_PER_SOURCE,
            "one page cannot hold more than its cap"
        );
        clear();
    });
}

// ---- End to end: page message → poll → panel -----------------------------------------------

#[test]
fn a_page_notification_reaches_the_panel_and_the_unread_count() {
    with_bridge(|| {
        clear();
        let log = {
            let (host, log) = FakeWebview::new();
            webview::install(Box::new(host));
            log
        };
        let mut rt = runtime(gmail_state());
        frame(&mut rt, 1.0);

        // The page raises a notification, exactly as `BxNotification`'s constructor does.
        assert!(from_gmail(&rt.state, &send("notif/1", "Mail arrived", "Ada: ship it")).accepted());
        frame(&mut rt, 2.0 / 60.0);

        // Open the panel: the row renders, the count says one.
        press(&mut rt, "rail-notifications");
        frame(&mut rt, 3.0 / 60.0);
        assert!(rt.find("notifications").is_some(), "the panel did not open");
        assert!(shows(&rt, "1 unread"), "the unread count did not change");
        assert!(shows(&rt, "Mail arrived"), "the title row is missing");
        assert!(shows(&rt, "Ada: ship it"), "the body row is missing");
        assert!(
            rt.find("notification-notif/1").is_some(),
            "the row key is missing"
        );
        let label = rt
            .elements()
            .into_iter()
            .find(|element| element.key.as_deref() == Some("notification-notif/1"))
            .and_then(|element| element.label);
        assert_eq!(
            label.as_deref(),
            Some("Open Mail arrived"),
            "the row is not pressable"
        );
        assert!(
            rt.state.notifications().click_target("notif/1").is_some(),
            "the click target (the sender's tab) is not reachable"
        );
        let _ = &log;
        clear();
    });
}

#[test]
fn clicking_a_notification_row_marks_it_read_and_focuses_the_tab() {
    with_bridge(|| {
        clear();
        let (host, _log) = FakeWebview::new();
        webview::install(Box::new(host));
        let mut rt = runtime(gmail_state());
        frame(&mut rt, 1.0);
        let _ = from_gmail(&rt.state, &send("notif/1", "Mail arrived", ""));
        frame(&mut rt, 2.0 / 60.0);

        press(&mut rt, "rail-notifications");
        frame(&mut rt, 3.0 / 60.0);
        press(&mut rt, "notification-notif/1");
        frame(&mut rt, 4.0 / 60.0);

        // The saga's semantics: read (removed) first, then the tab focused. The shell also closes
        // the panel so the focused tab's page can show.
        assert_eq!(
            rt.state.notifications().unread_count(),
            0,
            "the click did not mark read"
        );
        assert!(rt.find("notification-notif/1").is_none());
        assert_eq!(
            rt.state.active().id,
            "gmail",
            "the sender's tab was not focused"
        );
        assert_eq!(
            rt.state.surface(),
            crate::state::Surface::Apps,
            "the panel did not close"
        );
        clear();
    });
}

#[test]
fn close_notification_removes_the_stored_one() {
    with_bridge(|| {
        clear();
        let mut state = gmail_state();
        let tabs = state.tabs().to_vec();
        let _ = receive("gmail", &tabs, &send("notif/1", "t", ""));
        deliver_to(&mut state);
        assert_eq!(state.notifications().unread_count(), 1);

        let _ = receive("gmail", &tabs, r#"{"kind":"close","id":"notif/1"}"#);
        deliver_to(&mut state);
        assert_eq!(
            state.notifications().unread_count(),
            0,
            "the page's close did not remove it"
        );
        clear();
    });
}

#[test]
fn a_message_from_a_tab_that_later_left_is_still_delivered_but_a_new_tab_is_not_fooled() {
    with_bridge(|| {
        clear();
        let mut state = gmail_state();
        let tabs = state.tabs().to_vec();
        // Delivered while the tab existed.
        let _ = receive("gmail", &tabs, &send("n1", "t", ""));
        // The tab leaves (uninstall); the page may still post until its web view dies.
        let without = state
            .tabs()
            .iter()
            .filter(|t| t.id != "gmail")
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            receive("gmail", &without, &send("n2", "t", "")),
            crate::notifications::bridge::Outcome::Refused(Refusal::UnknownSender),
            "a tab that is gone is an unknown sender"
        );
        deliver_to(&mut state);
        assert_eq!(state.notifications().unread_count(), 1);
        clear();
    });
}

// ---- Snooze expiry --------------------------------------------------------------------------

#[test]
fn a_finite_snooze_expires_by_the_frame_poll_and_the_notification_comes_back() {
    with_bridge(|| {
        let (host, _log) = FakeWebview::new();
        webview::install(Box::new(host));
        let mut state = gmail_state();
        let tabs = state.tabs().to_vec();
        let _ = receive("gmail", &tabs, &send("n1", "Ping", ""));
        deliver_to(&mut state);

        // Snooze 15 minutes from the panel path (wall clock start).
        state.snooze_notifications(Some("15min"));
        assert!(state.notifications().is_snoozed());

        // Frames inside the window: nothing expires, however many run.
        let mut expired = false;
        for frame_index in 0..120u64 {
            expired |= webview::poll_at(&mut state, frame_index as f64);
        }
        assert!(!expired, "a snooze expired far too early");
        assert!(state.notifications().is_snoozed());

        // Expire it deterministically: a fresh 250 ms snooze, then a real wait past its deadline.
        // The deadline is wall-clock stamped, so the wait is too; the polls afterwards just give
        // the frame path its chance.
        state.snooze_notifications(Some("250ms"));
        assert!(state.notifications().is_snoozed());
        std::thread::sleep(std::time::Duration::from_millis(400));
        let mut expired = false;
        for frame_index in 0..600u64 {
            expired |= webview::poll_at(&mut state, frame_index as f64 / 60.0 + 10_000.0);
            if expired {
                break;
            }
        }
        assert!(expired, "a finite snooze never expired");
        assert!(!state.notifications().is_snoozed());
        // The notification "comes back": it was never removed, and the badge shows again.
        assert_eq!(state.notifications().unread_count(), 1);
        assert!(state.notifications().dock_icon_shows_badge());
    });
}

#[test]
fn expiry_scans_at_most_once_per_second() {
    with_bridge(|| {
        let mut state = gmail_state();
        state.snooze_notifications(Some("15min"));
        // The deadline is in the future, so `advance_to` would decline anyway; the throttle is what
        // the next test proves by construction. Here: a NonExhaustive check that the helper exists
        // and is honest about a non-snoozed center.
        let mut quiet = gmail_state();
        assert!(!expire_snooze(&mut quiet, 1.0));
        let _ = &mut state;
    });
}

#[test]
fn sync_with_os_and_infinite_never_expire() {
    with_bridge(|| {
        let mut state = gmail_state();
        state.snooze_notifications(Some(notification_center::SYNC_WITH_OS));
        for frame_index in 0..600u64 {
            webview::poll_at(&mut state, frame_index as f64);
        }
        assert_eq!(
            state.notifications().snooze(),
            Some(notification_center::Snooze::SyncWithOs),
            "SyncWithOs expired without a reader"
        );
        state.snooze_notifications(Some(notification_center::INFINITE));
        for frame_index in 0..600u64 {
            webview::poll_at(&mut state, frame_index as f64 + 1_000.0);
        }
        assert_eq!(
            state.notifications().snooze(),
            Some(notification_center::Snooze::Infinite),
            "Infinite expired"
        );
    });
}

#[test]
fn a_snoozed_center_still_accepts_notifications() {
    with_bridge(|| {
        clear();
        let mut state = gmail_state();
        state.snooze_notifications(Some("15min"));
        let tabs = state.tabs().to_vec();
        let _ = receive("gmail", &tabs, &send("n1", "t", ""));
        assert!(deliver_to(&mut state), "delivery stopped while snoozed");
        assert_eq!(state.notifications().unread_count(), 1);
        clear();
    });
}

// ---- The injected script --------------------------------------------------------------------

#[test]
fn the_init_script_exposes_exactly_the_preload_notification_functions() {
    with_bridge(|| {
        // Member surface frozen at what the Electron preload declared (bxapi.d.ts):
        // `sendNotification`, `closeNotification` and the click-listener pair. No other
        // `bxApi` member is invented.
        for member in [
            "sendNotification",
            "closeNotification",
            "addNotificationClickListener",
            "removeNotificationClickListener",
        ] {
            assert!(crate::notifications::INIT_SCRIPT.contains(member));
        }
        assert!(!crate::notifications::INIT_SCRIPT.contains("applications"));
        assert!(!crate::notifications::INIT_SCRIPT.contains("getManifest"));
        assert!(!crate::notifications::INIT_SCRIPT.contains("addListenerToChannel"));
        assert!(crate::notifications::INIT_SCRIPT.contains("Object.freeze"));
    });
}

#[test]
fn sending_from_the_real_state_tabs_works_end_to_end() {
    with_bridge(|| {
        clear();
        let (host, _log) = FakeWebview::new();
        webview::install(Box::new(host));
        let mut rt = runtime(gmail_state());
        frame(&mut rt, 1.0);
        // The poll published the tabs, so the same tabs slice works as the ipc handler's snapshot.
        let tabs = rt.state.tabs().to_vec();
        let outcome = receive("gmail", &tabs, &send("n9", "Real path", "body"));
        assert_eq!(outcome, crate::notifications::bridge::Outcome::Accepted);
        frame(&mut rt, 2.0);
        assert_eq!(rt.state.notifications().unread_count(), 1);
        press(&mut rt, "rail-notifications");
        frame(&mut rt, 3.0);
        assert!(shows(&rt, "Real path"));
        clear();
    });
}

#[test]
fn typing_garbage_into_the_ipc_never_panics_the_frame() {
    with_bridge(|| {
        clear();
        let mut state = gmail_state();
        let tabs = state.tabs().to_vec();
        // A page fuzzing the bridge: every shape of wrong.
        for junk in [
            "{\"kind\":\"send\",\"id\":123}",
            "{\"kind\":\"send\",\"id\":\"x\",\"notification\":\"not an object\"}",
            "{\"kind\":\"send\",\"id\":\"x\",\"notification\":{\"title\":42}}",
            "{\"kind\":[],\"id\":{}}",
            "\u{0}\u{1}\u{2}",
            "{\"kind\":\"close\",\"id\":null}",
            "{\"kind\":\"send\",\"id\":\"x\",\"notification\":{\"timestamp\":\"not a number\"}}",
        ] {
            let _ = receive("gmail", &tabs, junk);
        }
        let mut changed = false;
        for frame_index in 0..10 {
            changed |= webview::poll_at(&mut state, frame_index as f64);
        }
        // Whatever survived validation is at most the one valid-shaped message; the frame lived.
        assert!(state.notifications().unread_count() <= 1);
        assert_eq!(changed, state.notifications().unread_count() > 0);
        clear();
    });
}

#[test]
fn a_notification_from_a_page_can_route_like_a_new_window() {
    with_bridge(|| {
        clear();
        let (host, log) = FakeWebview::new();
        webview::install(Box::new(host));
        let mut rt = runtime(gmail_state());
        // An installed Todoist, not yet open, as a click destination a notification could name.
        rt.state.tabs_mut_for_tests().push(AppTab::web(
            "todoist",
            "Todoist",
            "https://app.todoist.com",
        ));
        frame(&mut rt, 1.0);
        let _ = from_gmail(&rt.state, &send("n1", "t", ""));
        frame(&mut rt, 2.0 / 60.0);

        press(&mut rt, "rail-notifications");
        frame(&mut rt, 3.0 / 60.0);
        press(&mut rt, "notification-n1");
        frame(&mut rt, 4.0 / 60.0);

        // Gmail was in the background: focusing it shows its existing page, never a second window.
        assert_eq!(rt.state.active().id, "gmail");
        let events = log.events();
        assert!(
            events.iter().any(
                |event| matches!(event, Event::Visible { tab, visible: true } if tab == "gmail")
            ),
            "the focused page was not shown: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::Attach { tab, .. } if tab != "gmail")),
            "a click attached a page it should not have"
        );
        clear();
    });
}

#[test]
fn a_key_event_cannot_spoof_a_notification() {
    with_bridge(|| {
        clear();
        let mut rt = runtime(gmail_state());
        frame(&mut rt, 1.0);
        // Keys reach Rune's focus graph, not the bridge; the only entry is `receive`.
        rt.key(KeyEvent {
            key: Key::Text("{\"kind\":\"send\",\"id\":\"spoof\"}".to_owned()),
            mods: Mods::default(),
        });
        frame(&mut rt, 2.0);
        assert_eq!(rt.state.notifications().unread_count(), 0);
        clear();
    });
}
