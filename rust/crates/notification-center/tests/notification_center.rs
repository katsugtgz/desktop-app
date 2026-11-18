//! Behavior specs ported from `packages/app/src/notification-center`
//! (`duck.ts`, `selectors.ts`, `sagas.ts`, `notifications/duck.ts`).
//!
//! Every expected value below is taken from the TypeScript source, not
//! recomputed by the implementation under test.

use notification_center::{
    ClickTarget, NewNotificationOptions, Notification, NotificationAction, NotificationCenter,
    NotificationGroup, NotificationProps, Snooze, INFINITE, SYNC_WITH_OS,
};

fn props(title: &str, timestamp: i64) -> NotificationProps {
    NotificationProps {
        title: title.into(),
        timestamp: Some(timestamp),
        body: Some(format!("{title} body")),
        icon: Some("icon.png".into()),
    }
}

fn new_notification(
    center: &mut NotificationCenter,
    id: &str,
    application_id: Option<&str>,
    tab_id: Option<&str>,
    title: &str,
    timestamp: i64,
) {
    center.apply(NotificationAction::NewNotification {
        application_id: application_id.map(str::to_owned),
        tab_id: tab_id.map(str::to_owned),
        notification_id: id.into(),
        props: props(title, timestamp),
        options: NewNotificationOptions::default(),
    });
}

#[test]
fn new_notification_appends_and_is_listed_in_order() {
    let mut center = NotificationCenter::new();
    new_notification(&mut center, "n1", Some("slack"), Some("t1"), "first", 1000);
    new_notification(&mut center, "n2", Some("slack"), Some("t2"), "second", 2000);

    let notifications = center.notifications();
    assert_eq!(notifications.len(), 2);
    assert_eq!(notifications[0].notification_id, "n1");
    assert_eq!(notifications[0].title, "first");
    assert_eq!(notifications[1].notification_id, "n2");
    assert_eq!(
        center.notification("n2").unwrap().body.as_deref(),
        Some("second body")
    );
    assert!(center.notification("missing").is_none());
}

#[test]
fn duplicate_notification_ids_replace_the_notification_keyed_by_id() {
    // notifications/duck.ts `ADD_NOTIFICATION`: `state.set(notificationId, ...)`
    let mut center = NotificationCenter::new();
    new_notification(&mut center, "n1", Some("slack"), Some("t1"), "first", 1000);
    new_notification(&mut center, "n1", Some("slack"), Some("t1"), "edited", 2000);

    assert_eq!(center.notifications().len(), 1);
    assert_eq!(center.notifications()[0].title, "edited");
    assert_eq!(center.notifications()[0].timestamp, Some(2000));
}

#[test]
fn remove_notification_removes_it_by_id() {
    // duck.ts `REMOVE_NOTIFICATION`
    let mut center = NotificationCenter::new();
    new_notification(&mut center, "n1", Some("slack"), None, "first", 1000);

    center.apply(NotificationAction::RemoveNotification {
        notification_id: "n1".into(),
    });
    assert!(center.notifications().is_empty());

    // Removing an unknown id is a no-op (`index === -1`).
    center.apply(NotificationAction::RemoveNotification {
        notification_id: "ghost".into(),
    });
    assert!(center.notifications().is_empty());
}

#[test]
fn mark_as_read_removes_the_notification_from_the_center() {
    // sagas.ts `sagaMarkAsRead`: removeNotificationFromNotificationCenter + removeNotification
    let mut center = NotificationCenter::new();
    new_notification(&mut center, "n1", Some("slack"), None, "first", 1000);

    center.apply(NotificationAction::MarkAsRead {
        notification_id: "n1".into(),
    });

    assert!(center.notifications().is_empty());
    assert_eq!(center.unread_count(), 0);
}

#[test]
fn mark_all_as_read_clears_every_notification() {
    let mut center = NotificationCenter::new();
    new_notification(&mut center, "n1", Some("slack"), None, "first", 1000);
    new_notification(&mut center, "n2", Some("mail"), None, "second", 2000);

    center.apply(NotificationAction::MarkAllAsRead);

    assert!(center.notifications().is_empty());
    assert_eq!(center.unread_count(), 0);
}

#[test]
fn notification_click_marks_the_notification_as_read() {
    // sagas.ts `sagaNotificationClick`: `put(markAsRead(notificationId))`
    let mut center = NotificationCenter::new();
    new_notification(&mut center, "n1", Some("slack"), None, "first", 1000);

    center.apply(NotificationAction::NotificationClick {
        notification_id: "n1".into(),
        interface_type: "notification_center".into(),
    });

    assert!(center.notifications().is_empty());
}

#[test]
fn unread_count_tracks_the_ordered_notification_list() {
    // NotificationCenterDockIcon.tsx: `badge: getNotifications(state).size`
    let mut center = NotificationCenter::new();
    assert_eq!(center.unread_count(), 0);

    new_notification(&mut center, "n1", Some("slack"), None, "first", 1000);
    new_notification(&mut center, "n2", Some("mail"), None, "second", 2000);
    assert_eq!(center.unread_count(), 2);

    center.apply(NotificationAction::MarkAsRead {
        notification_id: "n1".into(),
    });
    assert_eq!(center.unread_count(), 1);
}

#[test]
fn dock_icon_badge_is_hidden_while_snoozed() {
    // NotificationCenterDockIcon.tsx: `(badge > 0) && !Boolean(snoozeDuration)`
    let mut center = NotificationCenter::new();
    assert!(!center.dock_icon_shows_badge());

    new_notification(&mut center, "n1", Some("slack"), None, "first", 1000);
    assert!(center.dock_icon_shows_badge());

    center.apply(NotificationAction::SetSnoozeDuration {
        duration: "21min".into(),
        at_ms: 1_000_000,
    });
    assert!(center.is_snoozed());
    assert!(!center.dock_icon_shows_badge());
}

#[test]
fn snooze_duration_parses_ms_units_used_by_the_panel() {
    // selectors.ts `getSnoozeDurationInMs` delegates to the `ms` package.
    // Values below are the panel's durations (NotiticationCenterSnoozePanel.tsx)
    // and its hint labels (20 minutes / 1 hour / 2 hours / 4 hours).
    let cases: &[(&str, u64)] = &[
        ("21min", 21 * 60 * 1000),
        ("61min", 61 * 60 * 1000),
        ("121min", 121 * 60 * 1000),
        ("241min", 241 * 60 * 1000),
        ("30m", 30 * 60 * 1000),
        ("4h", 4 * 60 * 60 * 1000),
        ("1.5h", 5_400_000),
        ("90s", 90_000),
        ("250ms", 250),
        ("500", 500),
    ];

    for (duration, expected_ms) in cases {
        let mut center = NotificationCenter::new();
        center.apply(NotificationAction::SetSnoozeDuration {
            duration: (*duration).into(),
            at_ms: 1_000_000,
        });
        assert_eq!(
            center.snooze_duration_ms(),
            Some(*expected_ms),
            "duration {duration}"
        );
        assert_eq!(center.snooze(), Some(Snooze::Finite(*expected_ms)));
    }
}

#[test]
fn sync_with_os_and_infinite_snooze_have_no_finite_duration_but_do_snooze() {
    // selectors.ts `getSnoozeDurationInMs` returns the constants unchanged,
    // `getSnoozeState` treats them as snoozed.
    let mut center = NotificationCenter::new();
    center.apply(NotificationAction::SetSnoozeDuration {
        duration: SYNC_WITH_OS.into(),
        at_ms: 1,
    });
    assert_eq!(center.snooze_duration_ms(), None);
    assert_eq!(center.snooze(), Some(Snooze::SyncWithOs));
    assert!(center.is_snoozed());
    assert_eq!(center.snooze_started_on(), Some(1));

    let mut center = NotificationCenter::new();
    center.apply(NotificationAction::SetSnoozeDuration {
        duration: INFINITE.into(),
        at_ms: 2,
    });
    assert_eq!(center.snooze(), Some(Snooze::Infinite));
    assert!(center.is_snoozed());
    assert_eq!(center.snooze_end_at_ms(), None);
}

#[test]
fn snooze_state_is_false_when_the_duration_is_zero_or_unparseable() {
    // selectors.ts `getSnoozeState`: `if (!currentSnoozeDurationInMs) return false;`
    let mut center = NotificationCenter::new();
    center.apply(NotificationAction::SetSnoozeDuration {
        duration: "0min".into(),
        at_ms: 1,
    });
    assert_eq!(center.snooze_duration_ms(), Some(0));
    assert!(!center.is_snoozed());

    center.apply(NotificationAction::SetSnoozeDuration {
        duration: "soon".into(),
        at_ms: 1,
    });
    assert_eq!(center.snooze_duration_ms(), None);
    assert!(!center.is_snoozed());
    // The raw string is still stored, like the reducer's `snoozeDuration`.
    assert_eq!(center.snooze(), Some(Snooze::Unknown("soon".into())));
    assert!(!center.dock_icon_shows_badge());
}

#[test]
fn snooze_expiry_clears_the_duration_after_the_ttl() {
    // sagas.ts `sagaSnooze`: `delay(durationInMs)` then `resetSnoozeDuration(via)`.
    // 21min = 1_260_000 ms, started at 1_000_000 -> ends at 2_260_000.
    let mut center = NotificationCenter::new();
    center.apply(NotificationAction::SetSnoozeDuration {
        duration: "21min".into(),
        at_ms: 1_000_000,
    });
    assert_eq!(center.snooze_end_at_ms(), Some(2_260_000));

    assert!(!center.advance_to(2_259_999));
    assert!(center.is_snoozed());

    assert!(center.advance_to(2_260_000));
    assert_eq!(center.snooze(), None);
    // The saga only dispatches RESET_SNOOZE_DURATION, whose reducer removes
    // `snoozeDuration`; `snoozeStartedOn` is left behind (duck.ts).
    assert_eq!(center.snooze_started_on(), Some(1_000_000));
}

#[test]
fn reset_snooze_clears_duration_and_started_on() {
    // sagas.ts `sagaResetSnooze` dispatches resetSnoozeStartedOn; the reducer
    // removes both fields across RESET_SNOOZE_DURATION + RESET_SNOOZE_STARTED_ON.
    let mut center = NotificationCenter::new();
    center.apply(NotificationAction::SetSnoozeDuration {
        duration: "61min".into(),
        at_ms: 10,
    });
    center.apply(NotificationAction::ResetSnoozeDuration);

    assert_eq!(center.snooze(), None);
    assert_eq!(center.snooze_started_on(), None);
    assert!(!center.is_snoozed());
}

#[test]
fn set_and_reset_snooze_started_on_follow_the_reducer() {
    let mut center = NotificationCenter::new();
    center.apply(NotificationAction::SetSnoozeStartedOn { timestamp: 1234 });
    assert_eq!(center.snooze_started_on(), Some(1234));

    center.apply(NotificationAction::ResetSnoozeStartedOn);
    assert_eq!(center.snooze_started_on(), None);
}

#[test]
fn set_snooze_duration_ms_is_observable() {
    let mut center = NotificationCenter::new();
    center.set_snooze_duration_ms(Some(5_400_000));
    assert_eq!(center.snooze_duration_ms(), Some(5_400_000));
    assert_eq!(center.snooze(), Some(Snooze::Finite(5_400_000)));
    assert!(center.is_snoozed());

    center.set_snooze_duration_ms(None);
    assert_eq!(center.snooze_duration_ms(), None);
    assert_eq!(center.snooze(), None);
}

#[test]
fn visibility_defaults_to_false_and_toggles() {
    // duck.ts SET_VISIBILITY / TOGGLE_VISIBILITY; selectors.ts `isVisible`.
    let mut center = NotificationCenter::new();
    assert!(!center.is_visible());

    center.apply(NotificationAction::ToggleVisibility);
    assert!(center.is_visible());

    center.apply(NotificationAction::SetVisibility { visible: false });
    assert!(!center.is_visible());
}

#[test]
fn groups_reverse_the_list_and_merge_consecutive_application_ids() {
    // selectors.ts `getFullNotificationsOrderedGrouped`: reverse the ordered
    // list, then start a new group whenever `applicationId` changes.
    let mut center = NotificationCenter::new();
    new_notification(&mut center, "n1", Some("slack"), None, "first", 1000);
    new_notification(&mut center, "n2", Some("slack"), None, "second", 2000);
    new_notification(&mut center, "n3", Some("mail"), None, "third", 3000);

    let groups = center.groups();
    assert_eq!(groups.len(), 2);

    // Newest first: n3 (mail), then n2 + n1 (slack).
    assert_eq!(groups[0].application_id, "mail");
    assert_eq!(groups[0].notifications.len(), 1);
    assert_eq!(groups[0].notifications[0].notification_id, "n3");

    assert_eq!(groups[1].application_id, "slack");
    assert_eq!(groups[1].application_name, "Station");
    assert_eq!(
        groups[1]
            .notifications
            .iter()
            .map(|n| n.notification_id.as_str())
            .collect::<Vec<_>>(),
        vec!["n2", "n1"]
    );
    assert_eq!(groups[1].icon.as_deref(), Some("icon.png"));
}

#[test]
fn notifications_without_an_application_group_under_station() {
    // selectors.ts: `notification.get('applicationId', 'station')`,
    // `notification.get('applicationName', 'Station')`.
    let mut center = NotificationCenter::new();
    new_notification(&mut center, "n1", None, None, "first", 1000);

    let groups = center.groups();
    assert_eq!(
        groups,
        vec![NotificationGroup {
            application_id: "station".into(),
            application_name: "Station".into(),
            icon: Some("icon.png".into()),
            label: None,
            badge: None,
            notifications: vec![center.notifications()[0].clone()],
        }]
    );
}

#[test]
fn groups_keep_groups_apart_when_an_application_reappears() {
    let mut center = NotificationCenter::new();
    new_notification(&mut center, "n1", Some("slack"), None, "first", 1000);
    new_notification(&mut center, "n2", Some("mail"), None, "second", 2000);
    new_notification(&mut center, "n3", Some("slack"), None, "third", 3000);

    let groups = center.groups();
    let ids: Vec<&str> = groups.iter().map(|g| g.application_id.as_str()).collect();
    assert_eq!(ids, vec!["slack", "mail", "slack"]);
    assert_eq!(groups[0].notifications.len(), 1);
}

#[test]
fn empty_center_has_no_groups() {
    let center = NotificationCenter::new();
    assert!(center.groups().is_empty());
    assert_eq!(center.unread_count(), 0);
}

#[test]
fn click_target_prefers_web_contents_then_falls_back_to_the_tab() {
    // sagas.ts `sagaNotificationClick`: webContentsId wins, otherwise the tab id.
    let mut center = NotificationCenter::new();
    center.apply(NotificationAction::NewNotification {
        application_id: Some("slack".into()),
        tab_id: Some("tab-1".into()),
        notification_id: "n1".into(),
        props: props("first", 1000),
        options: NewNotificationOptions {
            full: false,
            silent: false,
            web_contents_id: Some(42),
        },
    });
    center.apply(NotificationAction::NewNotification {
        application_id: Some("slack".into()),
        tab_id: Some("tab-2".into()),
        notification_id: "n2".into(),
        props: props("second", 2000),
        options: NewNotificationOptions::default(),
    });
    center.apply(NotificationAction::NewNotification {
        application_id: Some("slack".into()),
        tab_id: None,
        notification_id: "n3".into(),
        props: props("third", 3000),
        options: NewNotificationOptions::default(),
    });

    assert_eq!(
        center.click_target("n1"),
        Some(ClickTarget::WebContents(42))
    );
    assert_eq!(
        center.click_target("n2"),
        Some(ClickTarget::Tab("tab-2".into()))
    );
    assert_eq!(center.click_target("n3"), None);
    assert_eq!(center.click_target("ghost"), None);

    // `if (!webContentsId)` in the saga: 0 is falsy, so the tab wins.
    center.apply(NotificationAction::NewNotification {
        application_id: Some("slack".into()),
        tab_id: Some("tab-4".into()),
        notification_id: "n4".into(),
        props: props("fourth", 4000),
        options: NewNotificationOptions {
            full: false,
            silent: false,
            web_contents_id: Some(0),
        },
    });
    assert_eq!(
        center.click_target("n4"),
        Some(ClickTarget::Tab("tab-4".into()))
    );
}

#[test]
fn new_notification_options_are_stored_on_the_notification() {
    let mut center = NotificationCenter::new();
    center.apply(NotificationAction::NewNotification {
        application_id: Some("slack".into()),
        tab_id: Some("tab-1".into()),
        notification_id: "n1".into(),
        props: props("first", 1000),
        options: NewNotificationOptions {
            full: true,
            silent: true,
            web_contents_id: Some(7),
        },
    });

    let notification: &Notification = center.notification("n1").unwrap();
    assert!(notification.full);
    assert!(notification.silent);
    assert_eq!(notification.web_contents_id, Some(7));
    assert_eq!(notification.tab_id.as_deref(), Some("tab-1"));
}

#[test]
fn notification_serializes_with_the_typescript_field_names() {
    let mut center = NotificationCenter::new();
    new_notification(
        &mut center,
        "n1",
        Some("slack"),
        Some("tab-1"),
        "first",
        1000,
    );

    let json = serde_json::to_value(&center.notifications()[0]).unwrap();
    assert_eq!(json["notificationId"], "n1");
    assert_eq!(json["applicationId"], "slack");
    assert_eq!(json["tabId"], "tab-1");
    assert_eq!(json["title"], "first");
    assert_eq!(json["timestamp"], 1000);
    assert_eq!(json["full"], false);

    let back: Notification = serde_json::from_value(json).unwrap();
    assert_eq!(back, center.notifications()[0]);
}
