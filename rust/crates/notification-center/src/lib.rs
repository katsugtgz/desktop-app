//! Notification-center state machine (port of
//! `packages/app/src/notification-center`).
//!
//! This crate ports the pure parts of the TypeScript notification center:
//! the reducers of `duck.ts`, the id-keyed notification map and its
//! `REMOVE`/`CLEAR` actions from `notifications/duck.ts`, the selectors from
//! `selectors.ts`, and the state transitions that `sagas.ts` performs
//! (`markAsRead`, `notificationClick`, snooze start/reset/expiry). It owns no
//! clock, no channels, and no globals: time enters either as `at_ms` on an
//! action or through [`NotificationCenter::advance_to`].
//!
//! Deliberately not ported: React components, Electron IPC/webviews, OS
//! notification rendering (`api.ts`), the provider/SDK calls, and the
//! saga orchestration that only forwards side effects (`showNotification`,
//! `snooze.triggerSet`, `clearNotifications`, `navigateToApplicationTab`).
//! `selectors.ts`'s `isVisible` is represented by
//! [`NotificationCenter::is_visible`].
//!
//! Fidelity notes:
//! - `APPEND_NOTIFICATION` pushes the id again without dedupe, but the id-keyed
//!   notification map (`ADD_NOTIFICATION`) replaces the notification object.
//!   A reused id therefore addresses one stored notification: the port keeps a
//!   single ordered entry whose payload is replaced (see
//!   [`NotificationCenter::apply`]).
//! - Snooze expiry mirrors `sagaSnooze`: after `durationInMs`, only
//!   `snoozeDuration` is reset; `snoozeStartedOn` is left untouched. Only an
//!   explicit reset clears both (the saga dispatches `resetSnoozeStartedOn`).
//! - `snoozeDuration` is kept as the raw TS string in [`Snooze::Unknown`] when
//!   the `ms` package would fail to parse it, matching the reducer which
//!   stores the string verbatim.
//!
//! There is no notification TTL in the TypeScript sources: notifications stay
//! in the center until removed or marked read. The only TTL is the snooze one.

mod duration;

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

pub use duration::parse_duration_ms;

/// `constants.ts` sentinel: snooze follows the OS "do not disturb" state.
pub const SYNC_WITH_OS: &str = "SYNC_WITH_OS";
/// `constants.ts` sentinel: snooze never ends by itself.
pub const INFINITE: &str = "INFINITE";

/// A stored notification (`notifications/duck.ts` `ADD_NOTIFICATION` payload).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub notification_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_contents_id: Option<u64>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub full: bool,
    #[serde(default)]
    pub silent: bool,
    /// Read by `getNotificationBadge` in the group selector; never set by
    /// `notifications/duck.ts`, hence normally `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    /// Read by the group selector (`notification.get('label')`), normally `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// `NotificationProps` from `notification-center/types.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationProps {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// `NewNotificationOptions` from `notification-center/duck.ts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewNotificationOptions {
    #[serde(default)]
    pub full: bool,
    #[serde(default)]
    pub silent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_contents_id: Option<u64>,
}

/// Stored `snoozeDuration` value (`SET_SNOOZE_DURATION`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Snooze {
    /// Parsed by the `ms` package: 21min, 4h, 250ms, ...
    Finite(u64),
    SyncWithOs,
    Infinite,
    /// The reducer stores the string verbatim even when `ms` cannot parse it;
    /// [`NotificationCenter::is_snoozed`] is false for these.
    Unknown(String),
}

impl Snooze {
    /// Parses the string exactly where `sagaSnooze`/`getSnoozeDurationInMs`
    /// branch on the two constants before calling `ms()`.
    fn from_duration_string(duration: &str) -> Self {
        match duration {
            SYNC_WITH_OS => Snooze::SyncWithOs,
            INFINITE => Snooze::Infinite,
            other => parse_duration_ms(other)
                .map_or_else(|| Snooze::Unknown(other.to_owned()), Snooze::Finite),
        }
    }
}

/// Where a notification click should be delivered
/// (`sagaNotificationClick`: webContentsId first, then the tab).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClickTarget {
    WebContents(u64),
    Tab(String),
}

/// One application group produced by `getFullNotificationsOrderedGrouped`:
/// newest first, consecutive notifications sharing an `applicationId`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationGroup {
    pub application_id: String,
    pub application_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    pub notifications: Vec<Notification>,
}

/// Actions of `notification-center/duck.ts` plus the `NOTIFICATION_CLICK` and
/// `NEW_NOTIFICATION` saga entry points that mutate pure state.
///
/// The `via` field of the snooze actions is dropped: it only tells the saga
/// which UI surface triggered the change and is never stored in state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationAction {
    /// `NEW_NOTIFICATION`: `addNotification` + `appendNotification` +
    /// `showNotification` (the two last steps have no pure state effect).
    NewNotification {
        application_id: Option<String>,
        tab_id: Option<String>,
        notification_id: String,
        props: NotificationProps,
        options: NewNotificationOptions,
    },
    /// `REMOVE_NOTIFICATION`.
    RemoveNotification { notification_id: String },
    /// `REMOVE_ALL_NOTIFICATIONS`.
    RemoveAllNotifications,
    /// `MARK_AS_READ`: removes the notification from the center and from the
    /// id-keyed notification map (`sagaMarkAsRead`).
    MarkAsRead { notification_id: String },
    /// `MARK_ALL_AS_READ`: both reducers' clear actions.
    MarkAllAsRead,
    /// `NOTIFICATION_CLICK`: the saga marks the notification as read first.
    NotificationClick {
        notification_id: String,
        /// Only used by the saga to pick the click transport; not stored.
        interface_type: String,
    },
    /// `TOGGLE_VISIBILITY`.
    ToggleVisibility,
    /// `SET_VISIBILITY`.
    SetVisibility { visible: bool },
    /// `SET_SNOOZE_DURATION`; `at_ms` is the saga's `Date.now()` for
    /// `setSnoozeStartedOn`.
    SetSnoozeDuration { duration: String, at_ms: i64 },
    /// `RESET_SNOOZE_DURATION` (the saga also resets the start timestamp).
    ResetSnoozeDuration,
    /// `SET_SNOOZE_STARTED_ON`.
    SetSnoozeStartedOn { timestamp: i64 },
    /// `RESET_SNOOZE_STARTED_ON`.
    ResetSnoozeStartedOn,
}

/// The notification center state: ordered notifications, visibility and snooze.
///
/// Notifications live in a sequence-keyed map plus an id→seq index, so
/// 100k-scale add/remove sequences stay logarithmic per action instead of
/// quadratic; ascending seq is exactly the append order the reducers keep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationCenter {
    by_seq: BTreeMap<u64, Notification>,
    seq_of: HashMap<String, u64>,
    next_seq: u64,
    is_visible: bool,
    snooze: Option<Snooze>,
    snooze_started_on: Option<i64>,
}

impl NotificationCenter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one action to the state, reproducing the TS reducers and the
    /// pure state effects of their sagas.
    pub fn apply(&mut self, action: NotificationAction) {
        match action {
            NotificationAction::NewNotification {
                application_id,
                tab_id,
                notification_id,
                props,
                options,
            } => self.insert(Notification {
                notification_id,
                application_id,
                tab_id,
                web_contents_id: options.web_contents_id,
                title: props.title,
                timestamp: props.timestamp,
                body: props.body,
                icon: props.icon,
                full: options.full,
                silent: options.silent,
                badge: None,
                label: None,
            }),
            NotificationAction::RemoveNotification { notification_id }
            | NotificationAction::MarkAsRead { notification_id }
            | NotificationAction::NotificationClick {
                notification_id, ..
            } => {
                self.remove(&notification_id);
            }
            NotificationAction::RemoveAllNotifications | NotificationAction::MarkAllAsRead => {
                self.by_seq.clear();
                self.seq_of.clear();
            }
            NotificationAction::ToggleVisibility => self.is_visible = !self.is_visible,
            NotificationAction::SetVisibility { visible } => self.is_visible = visible,
            NotificationAction::SetSnoozeDuration { duration, at_ms } => {
                self.snooze_started_on = Some(at_ms);
                self.snooze = Some(Snooze::from_duration_string(&duration));
            }
            NotificationAction::ResetSnoozeDuration => {
                self.snooze = None;
                self.snooze_started_on = None;
            }
            NotificationAction::SetSnoozeStartedOn { timestamp } => {
                self.snooze_started_on = Some(timestamp);
            }
            NotificationAction::ResetSnoozeStartedOn => self.snooze_started_on = None,
        }
    }

    /// `getFullNotifications` / `getFullNotificationsOrdered` after the
    /// reverse: stored notifications, oldest first (append order).
    pub fn notifications(&self) -> Vec<Notification> {
        self.by_seq.values().cloned().collect()
    }

    pub fn notification(&self, notification_id: &str) -> Option<&Notification> {
        self.by_seq.get(self.seq_of.get(notification_id)?)
    }

    /// Dock icon badge: `getNotifications(state).size`.
    pub fn unread_count(&self) -> usize {
        self.by_seq.len()
    }

    /// `getNotifications(state).size > 0 && !snoozeDuration`
    /// (`NotificationCenterDockIcon.tsx`).
    pub fn dock_icon_shows_badge(&self) -> bool {
        !self.by_seq.is_empty() && self.snooze.is_none()
    }

    /// `selectors.ts` `isVisible` (defaults to false).
    pub fn is_visible(&self) -> bool {
        self.is_visible
    }

    /// `getSnoozeDurationInMs`: only finite durations have a millisecond value;
    /// the `SYNC_WITH_OS` and `INFINITE` sentinels do not.
    pub fn snooze_duration_ms(&self) -> Option<u64> {
        match self.snooze {
            Some(Snooze::Finite(ms)) => Some(ms),
            _ => None,
        }
    }

    /// The stored `snoozeDuration`, sentinels and unparseable strings included.
    pub fn snooze(&self) -> Option<Snooze> {
        self.snooze.clone()
    }

    /// `SET_SNOOZE_DURATION` without a saga-observed start time (does not touch
    /// `snoozeStartedOn`).
    pub fn set_snooze_duration_ms(&mut self, ms: Option<u64>) {
        self.snooze = ms.map(Snooze::Finite);
    }

    /// `selectors.ts` `getSnoozeState`: `ms > 0`, or one of the two sentinels.
    pub fn is_snoozed(&self) -> bool {
        match self.snooze {
            Some(Snooze::Finite(ms)) => ms > 0,
            Some(Snooze::SyncWithOs | Snooze::Infinite) => true,
            _ => false,
        }
    }

    /// `getSnoozeStartedOn`.
    pub fn snooze_started_on(&self) -> Option<i64> {
        self.snooze_started_on
    }

    /// `moment(snoozeStartedOn).add(snoozeDurationInMs, 'ms')`
    /// (`NotificationCenterHeader.tsx`); `None` for sentinels and unparseable
    /// durations.
    pub fn snooze_end_at_ms(&self) -> Option<i64> {
        let started_on = self.snooze_started_on?;
        let ms = i64::try_from(self.snooze_duration_ms()?).ok()?;
        Some(started_on.saturating_add(ms))
    }

    /// Drives the `sagaSnooze` `delay(durationInMs)` + `resetSnoozeDuration`:
    /// when a finite snooze has elapsed, its duration is cleared (the start
    /// timestamp is left behind, exactly like the reducer). Returns whether the
    /// snooze expired.
    pub fn advance_to(&mut self, now_ms: i64) -> bool {
        let Some(Snooze::Finite(ms)) = self.snooze.clone() else {
            return false;
        };
        let Some(started_on) = self.snooze_started_on else {
            return false;
        };
        let Ok(ms) = i64::try_from(ms) else {
            return false;
        };
        if started_on.saturating_add(ms) > now_ms {
            return false;
        }
        self.snooze = None;
        true
    }

    /// `getFullNotificationsOrderedGrouped`: newest first, a new group whenever
    /// the `applicationId` changes, defaulting to `station`/`Station`.
    pub fn groups(&self) -> Vec<NotificationGroup> {
        let mut groups: Vec<NotificationGroup> = Vec::new();
        for notification in self.by_seq.values().rev() {
            let application_id = notification
                .application_id
                .clone()
                .unwrap_or_else(|| "station".to_owned());
            match groups.last_mut() {
                Some(group) if group.application_id == application_id => {
                    group.notifications.push(notification.clone());
                }
                _ => groups.push(NotificationGroup {
                    application_id,
                    application_name: "Station".to_owned(),
                    icon: notification.icon.clone(),
                    label: notification.label.clone(),
                    badge: notification.badge.clone(),
                    notifications: vec![notification.clone()],
                }),
            }
        }
        groups
    }

    /// `sagaNotificationClick` target resolution: a truthy `webContentsId`
    /// wins, otherwise a truthy `tabId`; `None` stops the click.
    pub fn click_target(&self, notification_id: &str) -> Option<ClickTarget> {
        let notification = self.notification(notification_id)?;
        if let Some(web_contents_id) = notification.web_contents_id.filter(|id| *id != 0) {
            return Some(ClickTarget::WebContents(web_contents_id));
        }
        notification
            .tab_id
            .as_deref()
            .filter(|tab_id| !tab_id.is_empty())
            .map(|tab_id| ClickTarget::Tab(tab_id.to_owned()))
    }

    /// `ADD_NOTIFICATION` (`state.set(id, payload)`) + `APPEND_NOTIFICATION`.
    /// A reused id keeps its position and gets the new payload.
    fn insert(&mut self, notification: Notification) {
        match self.seq_of.get(&notification.notification_id) {
            Some(&seq) => self.by_seq.insert(seq, notification),
            None => {
                let seq = self.next_seq;
                self.next_seq += 1;
                self.seq_of
                    .insert(notification.notification_id.clone(), seq);
                self.by_seq.insert(seq, notification)
            }
        };
    }

    /// `REMOVE_NOTIFICATION`: removes the matching id, no-op otherwise.
    fn remove(&mut self, notification_id: &str) {
        if let Some(seq) = self.seq_of.remove(notification_id) {
            self.by_seq.remove(&seq);
        }
    }
}
