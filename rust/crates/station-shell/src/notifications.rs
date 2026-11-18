//! The notification panel: 320 pt on the right of the content area.
//!
//! It is Rune-drawn beside the app's page, not over it: the page is a native child window, so
//! `ShellState::toggle_notifications` narrows the page area (and `webview::sync` hides the page)
//! while the panel shows. The rail stays Rune's and stays interactive.
//!
//! Nothing produces notifications yet — there is no OS source and no web view bridge — so the
//! panel renders whatever the center holds, which today is what a test put there. The state
//! machine itself (`notification-center`) is the ported one.

use crate::shell::Paint;
use crate::state::ShellState;
use notification_center::{NotificationCenter, NotificationGroup, Snooze};
use rune::tok::{radius, space, type_};
use rune::{col, el, row, text, El, ScrollSpec};

/// The panel's width, in points.
pub const PANEL_WIDTH: f32 = 320.0;

/// One snooze choice: the label the footer shows and the duration string the center parses
/// (`None` clears it).
const SNOOZE_CHOICES: [(&str, &str, Option<&str>); 5] = [
    ("off", "Off", None),
    ("15m", "15 min", Some("15min")),
    ("1h", "1 hour", Some("1h")),
    (
        "sync",
        "Sync with OS",
        Some(notification_center::SYNC_WITH_OS),
    ),
    ("infinite", "Infinite", Some(notification_center::INFINITE)),
];

pub fn panel(state: &ShellState, p: Paint) -> El {
    let center = state.notifications();
    let mut list = col()
        .key("notifications-list")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .px(space::S2)
        .gap(space::S2)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    if center.unread_count() == 0 {
        list = list.child(
            text("You're all caught up.")
                .key("notifications-empty")
                .font(type_::BODY)
                .color(p.t.text_secondary)
                .mt(space::S2),
        );
    } else {
        for group in center.groups() {
            list = list.child(group_view(&group, p));
        }
    }

    col()
        .key("notifications")
        .w(PANEL_WIDTH)
        .h_full()
        .min_h(0.0)
        .fill(p.t.card)
        .border_sides([0.0, 0.0, 0.0, 1.0], p.t.separator)
        .child(header(state, p))
        .child(list)
        .child(footer(state, p))
}

fn header(state: &ShellState, p: Paint) -> El {
    let center = state.notifications();
    let mut title = row()
        .key("notifications-header")
        .w_full()
        .h(44.0)
        .px(space::S3)
        .gap(space::S2)
        .center_y()
        .justify_between()
        .child(
            text("Notifications")
                .key("notifications-title")
                .font(type_::TITLE)
                .color(p.t.text),
        )
        .child(
            text(format!("{} unread", center.unread_count()))
                .key("notifications-count")
                .font(type_::LABEL)
                .color(p.t.text_secondary),
        );
    if center.unread_count() > 0 {
        title = title.child(
            el().key("notifications-mark-all")
                .role("button")
                .label("Mark all read")
                .focusable(true)
                .h(24.0)
                .px(space::S2)
                .center()
                .radius(radius::SM)
                .fill(p.t.control_track)
                .hover(p.t.hover)
                .press(p.t.press)
                .on_press(|state: &mut ShellState| state.mark_all_notifications_read())
                .child(
                    text("Mark all read")
                        .font(type_::LABEL)
                        .color(p.t.text)
                        .a11y_hidden(),
                ),
        );
    }
    title
}

fn group_view(group: &NotificationGroup, p: Paint) -> El {
    let mut rows = col()
        .key(format!("notifications-group-{}", group.application_id))
        .w_full()
        .gap(space::S1);
    rows = rows.child(
        text(group.application_name.clone())
            .font(type_::SECTION)
            .color(p.t.text_secondary)
            .ml(space::S2)
            .a11y_hidden(),
    );
    for notification in &group.notifications {
        let mut notification_row = col()
            .key(format!("notification-{}", notification.notification_id))
            .w_full()
            .gap(2.0)
            .padding(space::S2)
            .radius(radius::SM)
            .fill(p.t.card)
            .child(
                text(notification.title.clone())
                    .key(format!(
                        "notification-title-{}",
                        notification.notification_id
                    ))
                    .font(type_::BODY)
                    .color(p.t.text)
                    .truncate(),
            );
        if let Some(body) = &notification.body {
            notification_row = notification_row.child(
                text(body.clone())
                    .key(format!(
                        "notification-body-{}",
                        notification.notification_id
                    ))
                    .font(type_::LABEL)
                    .color(p.t.text_secondary)
                    .truncate(),
            );
        }
        if let Some(timestamp) = notification.timestamp {
            notification_row = notification_row.child(
                text(age(timestamp))
                    .key(format!(
                        "notification-time-{}",
                        notification.notification_id
                    ))
                    .font(type_::MONO_CAPTION)
                    .color(p.t.text_secondary),
            );
        }
        rows = rows.child(notification_row);
    }
    rows
}

fn footer(state: &ShellState, p: Paint) -> El {
    let center = state.notifications();
    // Two rows of choices: five 24 pt pills and their labels do not fit one 320 pt line.
    let mut short = row().gap(space::S1);
    let mut long = row().gap(space::S1);
    for (index, (key, label, duration)) in SNOOZE_CHOICES.into_iter().enumerate() {
        let choice = snooze_button(key, label, duration, center, p);
        if index < 3 {
            short = short.child(choice);
        } else {
            long = long.child(choice);
        }
    }
    col()
        .key("notifications-footer")
        .w_full()
        .py(space::S2)
        .px(space::S2)
        .gap(space::S1)
        .border_sides([1.0, 0.0, 0.0, 0.0], p.t.separator)
        .child(
            text("Snooze")
                .font(type_::SECTION)
                .color(p.t.text_secondary)
                .a11y_hidden(),
        )
        .child(short)
        .child(long)
}

fn snooze_button(
    key: &str,
    label: &str,
    duration: Option<&'static str>,
    center: &NotificationCenter,
    p: Paint,
) -> El {
    let selected = snooze_selected(key, duration, center);
    el().key(format!("notifications-snooze-{key}"))
        .role("radio")
        .label(format!("Snooze {label}"))
        .checked(selected)
        .focusable(true)
        .h(24.0)
        .px(space::S2)
        .center()
        .radius(radius::SM)
        .fill(if selected {
            p.t.control_track
        } else {
            p.t.card
        })
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press(move |state: &mut ShellState| state.snooze_notifications(duration))
        .child(
            text(label)
                .font(type_::LABEL)
                .color(if selected {
                    p.t.text
                } else {
                    p.t.text_secondary
                })
                .a11y_hidden(),
        )
}

/// Whether a footer choice is the center's current snooze.
fn snooze_selected(key: &str, duration: Option<&'static str>, center: &NotificationCenter) -> bool {
    match (duration, center.snooze()) {
        (None, snooze) => snooze.is_none(),
        (Some(_), Some(Snooze::SyncWithOs)) => key == "sync",
        (Some(_), Some(Snooze::Infinite)) => key == "infinite",
        (Some(duration), Some(Snooze::Finite(ms))) => choice_ms(duration) == Some(ms),
        (Some(_), _) => false,
    }
}

/// The milliseconds a footer choice's duration string stands for (`"15min"`, `"1h"`).
fn choice_ms(duration: &str) -> Option<u64> {
    if let Some(minutes) = duration.strip_suffix("min") {
        return minutes.parse::<u64>().ok().map(|minutes| minutes * 60_000);
    }
    duration
        .strip_suffix('h')
        .and_then(|hours| hours.parse::<u64>().ok())
        .map(|hours| hours * 3_600_000)
}

/// How old a stored timestamp is, in the shell's own clock: "just now", "5m", "3h", "2d".
fn age(timestamp_ms: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(timestamp_ms);
    let seconds = (now_ms - timestamp_ms).max(0) / 1000;
    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}
