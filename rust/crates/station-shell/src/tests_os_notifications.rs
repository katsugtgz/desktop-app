//! Slice 29 tests: `Snooze::SyncWithOs` OS DND polling (throttled + page fan-out) and
//! OS desktop notification delivery (snooze suppression + dedupe).

use std::cell::{Cell, RefCell};

use appstore_service::ApplicationService;

use crate::notifications::os::{self, OsDndReader, OsNotifier};
use crate::notifications::{
    clear, deliver_to_with_notifier, poll_os_dnd_with, receive, reset_dnd_poll_throttle,
};
use crate::state::{AppTab, ShellState};
use crate::webview::fake::{Event, FakeWebview};
use crate::webview::{self, PaneRect};

const RECT: PaneRect = PaneRect::new(0.0, 44.0, 1136.0, 756.0);

struct FakeDnd {
    active: Cell<bool>,
    reads: Cell<usize>,
}

impl OsDndReader for FakeDnd {
    fn is_dnd_active(&self) -> bool {
        self.reads.set(self.reads.get() + 1);
        self.active.get()
    }
}

struct FakeNotifier(RefCell<Vec<(String, String, Option<String>)>>);

impl OsNotifier for FakeNotifier {
    fn notify(&self, app_name: &str, title: &str, body: Option<&str>) -> bool {
        self.0.borrow_mut().push((
            app_name.to_owned(),
            title.to_owned(),
            body.map(str::to_owned),
        ));
        true
    }
}

#[test]
fn sync_with_os_polls_dnd_on_throttle_and_fans_out_to_attached_pages() {
    let _serial = crate::test_support::lock();
    reset_dnd_poll_throttle();
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));

    let mut state = ShellState::new(ApplicationService::for_tests("os-dnd-sync"));
    state.tabs_mut_for_tests().push(AppTab::web(
        "whatsapp",
        "WhatsApp",
        "https://web.whatsapp.com",
    ));
    state.select("whatsapp");
    webview::sync(&state, Some(RECT), 1.0);
    log.clear();

    // Select SyncWithOs with OS DND off -> effective snooze stays None.
    let dnd = FakeDnd {
        active: Cell::new(false),
        reads: Cell::new(0),
    };
    state.snooze_notifications(Some(notification_center::SYNC_WITH_OS));
    assert!(!poll_os_dnd_with(&mut state, 1.0, &dnd));
    assert_eq!(dnd.reads.get(), 1);
    assert_eq!(state.effective_snooze_duration_ms(), None);

    // Within 5s throttle window -> reader is NOT polled again.
    dnd.active.set(true);
    assert!(!poll_os_dnd_with(&mut state, 3.0, &dnd));
    assert_eq!(dnd.reads.get(), 1);
    assert_eq!(state.effective_snooze_duration_ms(), None);

    // After 5s throttle window -> reader runs, flips DND on, fans out to attached page!
    assert!(poll_os_dnd_with(&mut state, 7.0, &dnd));
    assert_eq!(dnd.reads.get(), 2);
    assert_eq!(
        state.effective_snooze_duration_ms(),
        Some(u64::from(u32::MAX))
    );
    webview::sync_snooze(state.effective_snooze_duration_ms());
    assert_eq!(
        log.events(),
        vec![Event::Eval {
            tab: "whatsapp".to_owned(),
            script: format!("window.__stationSnoozeUpdate({});", u64::from(u32::MAX)),
        }]
    );

    // OS DND turns back off after another 5s -> attached page receives `null`!
    log.clear();
    dnd.active.set(false);
    assert!(poll_os_dnd_with(&mut state, 13.0, &dnd));
    assert_eq!(state.effective_snooze_duration_ms(), None);
    webview::sync_snooze(state.effective_snooze_duration_ms());
    assert_eq!(
        log.events(),
        vec![Event::Eval {
            tab: "whatsapp".to_owned(),
            script: "window.__stationSnoozeUpdate(null);".to_owned(),
        }]
    );
    reset_dnd_poll_throttle();
}

#[test]
fn new_page_notification_dispatches_os_balloon_unless_snoozed_or_duplicate() {
    let _serial = crate::test_support::lock();
    clear();
    os::reset_rate_limit();
    reset_dnd_poll_throttle();

    let mut state = ShellState::new(ApplicationService::for_tests("os-notify-dispatch"));
    state
        .tabs_mut_for_tests()
        .push(AppTab::web("gmail", "Gmail", "https://mail.google.com"));
    state.select("gmail");
    let tabs = state.tabs().to_vec();
    let notifier = FakeNotifier(RefCell::new(Vec::new()));

    // Un-snoozed new notification -> dispatched with app label "Gmail".
    assert!(receive(
        "gmail",
        &tabs,
        r#"{"kind":"send","id":"n1","notification":{"title":"Invoice","body":"Paid"}}"#
    )
    .accepted());
    assert!(deliver_to_with_notifier(&mut state, 10.0, &notifier));
    assert_eq!(
        *notifier.0.borrow(),
        vec![(
            "Gmail".to_owned(),
            "Invoice".to_owned(),
            Some("Paid".to_owned())
        )]
    );

    // Replacing existing notification id does NOT spam a duplicate OS balloon.
    assert!(receive(
        "gmail",
        &tabs,
        r#"{"kind":"send","id":"n1","notification":{"title":"Invoice updated"}}"#
    )
    .accepted());
    assert!(deliver_to_with_notifier(&mut state, 11.0, &notifier));
    assert_eq!(notifier.0.borrow().len(), 1);

    // While SyncWithOs DND is active, new notifications enter center but OS balloon is suppressed.
    state.snooze_notifications(Some(notification_center::SYNC_WITH_OS));
    let dnd_on = FakeDnd {
        active: Cell::new(true),
        reads: Cell::new(0),
    };
    assert!(poll_os_dnd_with(&mut state, 20.0, &dnd_on));
    assert!(receive(
        "gmail",
        &tabs,
        r#"{"kind":"send","id":"n2","notification":{"title":"Quiet mail"}}"#
    )
    .accepted());
    assert!(deliver_to_with_notifier(&mut state, 21.0, &notifier));
    assert_eq!(state.notifications().unread_count(), 2);
    assert_eq!(
        notifier.0.borrow().len(),
        1,
        "OS balloon must be suppressed while OS DND is active"
    );

    os::reset_rate_limit();
    reset_dnd_poll_throttle();
}
