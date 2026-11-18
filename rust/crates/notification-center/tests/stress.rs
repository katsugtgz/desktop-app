//! Adversarial stress tests for `notification-center`.
//!
//! Bounds: wall-clock assertions are explicit and generous enough for slow
//! CI while still failing on quadratic insert/remove pathologies.

use std::time::{Duration, Instant};

use notification_center::{
    ClickTarget, NewNotificationOptions, NotificationAction, NotificationCenter, NotificationProps,
    Snooze, SYNC_WITH_OS,
};

fn new_notification(center: &mut NotificationCenter, id: &str, app: &str, at_ms: i64) {
    center.apply(NotificationAction::NewNotification {
        application_id: Some(app.into()),
        tab_id: None,
        notification_id: id.into(),
        props: NotificationProps {
            title: format!("title {id}"),
            timestamp: Some(at_ms),
            body: None,
            icon: None,
        },
        options: NewNotificationOptions::default(),
    });
}

/// 100k new notifications then 100k removals must stay far below quadratic:
/// the stored collection is id-addressed, so both paths are expected to be
/// sub-linear per action.
#[test]
fn hundred_k_notifications_add_and_remove_stay_bounded() {
    const N: u64 = 100_000;
    let mut center = NotificationCenter::new();

    let started = Instant::now();
    for i in 0..N {
        new_notification(&mut center, &format!("n{i}"), "app", i as i64);
    }
    let added = started.elapsed();

    assert_eq!(center.unread_count() as u64, N);
    // Append order is preserved: oldest first.
    assert_eq!(center.notifications()[0].notification_id, "n0");
    assert_eq!(
        center.notifications().last().unwrap().notification_id,
        format!("n{}", N - 1)
    );

    let removal_started = Instant::now();
    for i in 0..N {
        center.apply(NotificationAction::RemoveNotification {
            notification_id: format!("n{i}"),
        });
    }
    let removed = removal_started.elapsed();

    assert_eq!(center.unread_count(), 0);
    assert!(center.notifications().is_empty());
    assert!(
        added < Duration::from_secs(3),
        "100k inserts took {added:?}; quadratic scan regression"
    );
    assert!(
        removed < Duration::from_secs(3),
        "100k removals took {removed:?}"
    );
}

/// Reused ids address one stored notification: replace in place, keep
/// position, never duplicate.
#[test]
fn duplicate_ids_replace_in_place_and_keep_order() {
    let mut center = NotificationCenter::new();
    for round in 0..1_000u64 {
        new_notification(&mut center, "same", "app", round as i64);
        new_notification(&mut center, &format!("other{round}"), "app", round as i64);
    }
    assert_eq!(
        center.unread_count(),
        1_001,
        "reused id collapses to one entry"
    );
    let notifications = center.notifications();
    let positions: Vec<usize> = notifications
        .iter()
        .enumerate()
        .filter(|(_, n)| n.notification_id == "same")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(positions, [0], "reused id must keep exactly one position");
    assert_eq!(notifications[0].timestamp, Some(999));
}

/// Unknown ids and repeated applications of every action are no-ops that
/// never panic and never change counts.
#[test]
fn unknown_ids_and_repeated_actions_are_stable_noops() {
    let mut center = NotificationCenter::new();
    new_notification(&mut center, "n1", "app", 1);

    for _ in 0..10_000 {
        center.apply(NotificationAction::RemoveNotification {
            notification_id: "missing".into(),
        });
        center.apply(NotificationAction::MarkAsRead {
            notification_id: "missing".into(),
        });
        center.apply(NotificationAction::NotificationClick {
            notification_id: "missing".into(),
            interface_type: "test".into(),
        });
        center.apply(NotificationAction::SetVisibility { visible: true });
    }
    assert_eq!(center.unread_count(), 1);
    assert!(center.is_visible());

    // Even number of toggles returns to the start state.
    for _ in 0..10_000 {
        center.apply(NotificationAction::ToggleVisibility);
    }
    assert!(center.is_visible());

    assert_eq!(center.click_target("missing"), None);
    assert_eq!(center.click_target("n1"), None); // no webContentsId/tabId
}

/// Extreme timestamps and durations: saturation instead of overflow.
#[test]
fn extreme_timestamps_and_durations_saturate_without_panics() {
    let mut center = NotificationCenter::new();

    center.apply(NotificationAction::SetSnoozeDuration {
        duration: "1m".into(),
        at_ms: i64::MAX,
    });
    assert_eq!(center.snooze_started_on(), Some(i64::MAX));
    assert_eq!(center.snooze_end_at_ms(), Some(i64::MAX));
    assert!(!center.advance_to(i64::MIN));
    assert!(!center.advance_to(i64::MAX - 60_000 + 1));
    assert!(center.advance_to(i64::MAX));

    center.apply(NotificationAction::SetSnoozeDuration {
        duration: "1m".into(),
        at_ms: i64::MIN,
    });
    assert_eq!(center.snooze_end_at_ms(), Some(i64::MIN + 60_000));
    assert!(center.advance_to(i64::MIN + 60_000));

    // Zero and "negative-looking" durations.
    center.apply(NotificationAction::SetSnoozeDuration {
        duration: "0s".into(),
        at_ms: 0,
    });
    assert_eq!(center.snooze(), Some(Snooze::Finite(0)));
    assert!(!center.is_snoozed());
    assert!(center.advance_to(0), "zero duration expires immediately");

    center.apply(NotificationAction::SetSnoozeDuration {
        duration: "-5m".into(),
        at_ms: 0,
    });
    assert!(matches!(center.snooze(), Some(Snooze::Unknown(_))));
    assert!(!center.is_snoozed());

    // A duration that does not fit i64 can never expire.
    center.set_snooze_duration_ms(Some(u64::MAX));
    assert_eq!(center.snooze_duration_ms(), Some(u64::MAX));
    assert_eq!(center.snooze_end_at_ms(), None);
    assert!(!center.advance_to(i64::MAX));

    center.apply(NotificationAction::SetSnoozeStartedOn {
        timestamp: i64::MIN,
    });
    center.apply(NotificationAction::ResetSnoozeStartedOn);
    assert_eq!(center.snooze_started_on(), None);
}

/// Malformed snooze strings are stored verbatim as `Snooze::Unknown` and
/// never parsed into panics, including strings far over the 100-char `ms`
/// limit.
#[test]
fn malformed_and_huge_snooze_strings_stay_unknown() {
    let mut center = NotificationCenter::new();
    let huge = "9".repeat(1_000_000);

    let cases = [
        "",
        "   ",
        "-1m",
        "1e999",
        "1m30s",
        "soon",
        "5м", // Cyrillic м, not a unit
        huge.as_str(),
    ];
    for duration in cases {
        center.apply(NotificationAction::SetSnoozeDuration {
            duration: duration.into(),
            at_ms: 0,
        });
        let snooze = center.snooze().expect("duration stored");
        assert!(
            matches!(snooze, Snooze::Unknown(_)),
            "{:?} parsed as {snooze:?}",
            duration.get(..16).unwrap_or(duration)
        );
        assert!(!center.is_snoozed());
        assert_eq!(center.snooze_duration_ms(), None);
        assert_eq!(center.snooze_end_at_ms(), None);
        assert!(!center.advance_to(i64::MAX));
    }

    // The sentinels parse ahead of `ms`.
    center.apply(NotificationAction::SetSnoozeDuration {
        duration: SYNC_WITH_OS.into(),
        at_ms: 0,
    });
    assert_eq!(center.snooze(), Some(Snooze::SyncWithOs));
    assert!(center.is_snoozed());
    center.apply(NotificationAction::SetSnoozeDuration {
        duration: "INFINITE".into(),
        at_ms: 0,
    });
    assert_eq!(center.snooze(), Some(Snooze::Infinite));
    assert!(!center.advance_to(i64::MAX), "sentinels never expire");
}

/// `groups` over 100k notifications stays linear and keeps newest-first
/// order with per-application runs.
#[test]
fn groups_over_100k_notifications_stay_bounded() {
    const N: u64 = 100_000;
    let mut center = NotificationCenter::new();
    for i in 0..N {
        let app = if i % 2 == 0 { "a" } else { "b" };
        new_notification(&mut center, &format!("n{i}"), app, i as i64);
    }

    let started = Instant::now();
    let groups = center.groups();
    let elapsed = started.elapsed();

    // Newest first, alternating applications: every notification is its own run.
    assert_eq!(groups.len() as u64, N);
    assert_eq!(
        groups[0].notifications[0].notification_id,
        format!("n{}", N - 1)
    );
    assert_eq!(groups[0].application_id, "b");
    assert_eq!(groups[N as usize - 1].application_id, "a");
    for group in &groups {
        assert_eq!(group.notifications.len(), 1);
    }
    assert!(
        elapsed < Duration::from_secs(3),
        "grouping 100k took {elapsed:?}"
    );

    // One application: one group, newest first inside.
    let mut single = NotificationCenter::new();
    for i in 0..N {
        new_notification(&mut single, &format!("n{i}"), "app", i as i64);
    }
    let groups = single.groups();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].notifications.len() as u64, N);
    assert_eq!(
        groups[0].notifications[0].notification_id,
        format!("n{}", N - 1)
    );
    assert_eq!(
        groups[0].notifications.last().unwrap().notification_id,
        "n0"
    );
}

/// Huge titles/bodies and webContents/tab click-target edges.
#[test]
fn huge_strings_and_click_targets_do_not_panic() {
    let mut center = NotificationCenter::new();
    let huge = "x".repeat(1_000_000);

    center.apply(NotificationAction::NewNotification {
        application_id: Some("app".into()),
        tab_id: Some("tab".into()),
        notification_id: "huge".into(),
        props: NotificationProps {
            title: huge.clone(),
            timestamp: Some(0),
            body: Some(huge.clone()),
            icon: Some(huge),
        },
        options: NewNotificationOptions {
            web_contents_id: Some(0), // falsy in TS: ignored
            ..NewNotificationOptions::default()
        },
    });
    assert_eq!(center.unread_count(), 1);
    assert_eq!(
        center.click_target("huge"),
        Some(ClickTarget::Tab("tab".into()))
    );

    center.apply(NotificationAction::NewNotification {
        application_id: None,
        tab_id: Some(String::new()),
        notification_id: "wc".into(),
        props: NotificationProps {
            title: "t".into(),
            timestamp: None,
            body: None,
            icon: None,
        },
        options: NewNotificationOptions {
            web_contents_id: Some(u64::MAX),
            ..NewNotificationOptions::default()
        },
    });
    assert_eq!(
        center.click_target("wc"),
        Some(ClickTarget::WebContents(u64::MAX))
    );

    // Clicking removes: repeated clicks on the same id are no-ops after the first.
    for _ in 0..1_000 {
        center.apply(NotificationAction::NotificationClick {
            notification_id: "huge".into(),
            interface_type: "test".into(),
        });
    }
    assert_eq!(center.unread_count(), 1);
}
