//! The page→shell notification bridge: the Electron `bxApi.notificationCenter` IPC, rebuilt
//! over wry.
//!
//! In the Electron app a hosted page calls `window.bxApi.notificationCenter.sendNotification(id,
//! notification)` (`webview-preload.ts` → `ipcRenderer.send('new-notification', …)`) and
//! `closeNotification(id)` (`'notification-close'`). The native shell exposes exactly those two
//! members plus the click registry the preload also declared — `addNotificationClickListener` /
//! `removeNotificationClickListener` — and nothing else of `bxApi` is invented — through wry's
//! `with_ipc_handler` (`webview.rs::platform::attach` injects [`INIT_SCRIPT`] and hands messages
//! to [`receive`]), and delivers them to `ShellState` from the frame's poll
//! (`webview.rs::poll_at` → [`super::drain_page_messages`]).
//!
//! The same channel carries the page's own presentation (slice 20): the `browserx-*` meta
//! observer's `page-bxmetas-updated` payload as `kind: "meta"`, and `document.title` (Electron's
//! `page-title-updated` webview event, which had no IPC of its own) as `kind: "title"`. Both
//! reuse this module's queue, caps and sender check; `deliver_to` applies them to the tab. They
//! are presentation only: never written to the session, never a selection change.
//!
//! The sender is an arbitrary web page, so every message is treated like the routing queue
//! treats a new-window request: bounded lengths, bounded counts, dedupe by id/slot, and no panic
//! on any input. A message beyond a limit is refused, not truncated or queued.

use std::collections::HashMap;
use std::sync::Mutex;

use notification_center::{NotificationAction, NotificationCenter, NotificationProps};

use crate::state::ShellState;

/// Injected into every page (`with_initialization_script`). Adds the one member pages may use —
/// `window.bxApi.notificationCenter` with the Electron preload's four functions — as members of
/// a frozen plain object (`Object.freeze(api)`), so a page cannot add, remove or reassign them.
/// Messages travel as JSON on wry's `window.ipc.postMessage`, which the Windows backend itself
/// defines.
///
/// The click listeners are the page side of `sagaNotificationClick`'s `triggerClick`: the shell
/// evaluates `window.__stationNotificationClick(<json id>)` (`webview::click_script`), which
/// invokes every registered callback with the notification id. `__stationNotificationClick` is
/// an internal, non-enumerable, non-writable hook — not a `bxApi` member. At most
/// [`MAX_CLICK_LISTENERS`] callbacks are registered; each is called in its own `try`/`catch` so
/// one throwing listener cannot stop the next.
///
/// It also replaces `window.Notification` with the port of Electron's page override
/// (`packages/app/src/static/preload/webview-inject.js:77-236`): every real page raises
/// notifications with `new Notification(title, options)`, and this class routes that into the
/// same `sendNotification` member above — no second channel, no new `kind`. The class follows the
/// TS contract (`title` required, defaults merged and made read-only, `notif/<nanoid>` id,
/// `permission`/`requestPermission` granted, `close()` → `closeNotification`) and adds the event
/// surface the TS renderer preload had (`onclick`/`onclose`/`onshow`/`onerror` plus
/// `addEventListener`/`removeEventListener`/`dispatchEvent`); clicks arrive through the same
/// click registry. Dropped from the TS (see the slice entry): the `blob:` icon XMLHttpRequest
/// (Telegram-only) and the recursive same-origin iframe override.
pub const INIT_SCRIPT: &str = r#"(() => {
  const MAX_CLICK_LISTENERS = 8;
  const clickListeners = [];
  const click = (id) => {
    for (const listener of clickListeners.slice()) {
      try { listener(id); } catch (e) {}
    }
  };
  Object.defineProperty(window, '__stationNotificationClick', {
    value: click, writable: false, configurable: false, enumerable: false,
  });
  const api = {
    sendNotification: (id, notification) => {
      try {
        window.ipc.postMessage(JSON.stringify({
          kind: 'send', id, notification,
        }));
      } catch (e) {}
    },
    closeNotification: (id) => {
      try {
        window.ipc.postMessage(JSON.stringify({ kind: 'close', id }));
      } catch (e) {}
    },
    addNotificationClickListener: (listener) => {
      if (typeof listener !== 'function' || clickListeners.length >= MAX_CLICK_LISTENERS) return;
      clickListeners.push(listener);
    },
    removeNotificationClickListener: (listener) => {
      const index = clickListeners.indexOf(listener);
      if (index !== -1) clickListeners.splice(index, 1);
    },
  };
  Object.defineProperty(window, 'bxApi', { value: Object.freeze({ notificationCenter: Object.freeze(api) }) });

  // The page's Notification API, overridden at initialization-script time (the page's first
  // line sees it, unlike Electron's dom-ready executeJavaScript).
  const NOTIFICATION_PERMISSION = 'granted';
  const NOTIFICATION_ALPHABET = 'useandom-26T198340PX75pxJACKVERYMINDBUSHWOLF_GQZbfghjklqvwyzrict';
  const notificationSuffix = () => {
    let suffix = '';
    let remaining = 21;
    while (remaining--) suffix += NOTIFICATION_ALPHABET[(Math.random() * 64) | 0];
    return suffix;
  };
  const notificationDefaults = (title) => ({
    actions: [], badge: '', body: '', data: null, dir: 'auto', lang: '', tag: '', icon: '',
    image: '', requireInteraction: false, silent: false, timestamp: Date.now(), title,
    vibrate: [],
  });

  class BxNotification {
    constructor(title, options = {}) {
      if (!title && title !== '') throw new Error('Title is required');
      this.id = 'notif/' + notificationSuffix();
      const properties = Object.assign({}, notificationDefaults(title), options || {});
      Object.defineProperty(this, '_handlers', { value: Object.create(null) });
      Object.defineProperty(this, '_listeners', { value: Object.create(null) });
      Object.keys(properties).forEach((key) => {
        Object.defineProperty(this, key, { value: properties[key], writable: false });
      });
      this._registerIPC();
      let icon = this.icon;
      if (typeof icon === 'string') {
        if (icon.startsWith('//')) icon = 'https:' + icon;
      } else {
        icon = undefined;
      }
      api.sendNotification(this.id, {
        timestamp: this.timestamp,
        title: this.title,
        body: this.body,
        icon: icon,
      });
      const notification = this;
      setTimeout(() => { notification._emit('show'); }, 0);
    }

    static requestPermission(deprecatedCallback) {
      const request = Promise.resolve(NOTIFICATION_PERMISSION);
      if (typeof deprecatedCallback === 'function') request.then(deprecatedCallback);
      return request;
    }

    get onclick() { return this._handlers.click || null; }
    set onclick(handler) { this._handlers.click = typeof handler === 'function' ? handler : null; }

    get onclose() { return this._handlers.close || null; }
    set onclose(handler) { this._handlers.close = typeof handler === 'function' ? handler : null; }

    get onshow() { return this._handlers.show || null; }
    set onshow(handler) { this._handlers.show = typeof handler === 'function' ? handler : null; }

    get onerror() { return this._handlers.error || null; }
    set onerror(handler) { this._handlers.error = typeof handler === 'function' ? handler : null; }

    addEventListener(type, listener) {
      if (typeof listener !== 'function') return;
      const bucket = this._listeners[type] || (this._listeners[type] = []);
      if (bucket.indexOf(listener) === -1) bucket.push(listener);
    }

    removeEventListener(type, listener) {
      const bucket = this._listeners[type];
      if (!bucket) return;
      const index = bucket.indexOf(listener);
      if (index !== -1) bucket.splice(index, 1);
    }

    dispatchEvent(event) {
      if (!event || typeof event.type !== 'string') return false;
      const handler = this._handlers[event.type];
      if (handler) {
        try { handler.call(this, event); } catch (e) {}
      }
      const bucket = this._listeners[event.type];
      if (bucket) {
        for (const listener of bucket.slice()) {
          try { listener.call(this, event); } catch (e) {}
        }
      }
      return true;
    }

    _emit(type) { this.dispatchEvent(new Event(type)); }

    _registerIPC() {
      const notification = this;
      this._clickListener = (id) => { notification._handleNotificationClick(id); };
      api.addNotificationClickListener(this._clickListener);
    }

    _unregisterIPC() {
      if (!this._clickListener) return;
      api.removeNotificationClickListener(this._clickListener);
      this._clickListener = null;
    }

    _handleNotificationClick(id) {
      if (this.id !== id) return;
      this._unregisterIPC();
      this._emit('click');
    }

    close() {
      try { api.closeNotification(this.id); } catch (e) {}
      this._unregisterIPC();
      this._emit('close');
    }
  }
  Object.defineProperty(BxNotification, 'permission', {
    value: NOTIFICATION_PERMISSION, writable: false, configurable: false, enumerable: true,
  });
  window.Notification = BxNotification;

  // Page title and badge, ported from `static/preload/preload.js:8-84` (the `browserx-*` meta
  // observer; its message is `page-bxmetas-updated`) and Electron's `page-title-updated` webview
  // event. Same wire discipline as the notifications above: one channel, a `kind`,
  // `JSON.stringify`, and the shell's caps.
  const postPageMeta = (payload) => {
    try { window.ipc.postMessage(JSON.stringify(payload)); } catch (e) {}
  };
  const META_SELECTOR = 'meta[name^=browserx-]';
  let sentMetas = null;
  let sentTitle = null;
  const currentMetas = () => {
    const values = {};
    const metas = document.querySelectorAll(META_SELECTOR);
    for (const meta of metas) {
      const key = meta.name.slice('browserx-'.length);
      if (key) values[key] = meta.content;
    }
    return values;
  };
  const emitMetas = () => {
    const values = currentMetas();
    const encoded = JSON.stringify(values);
    if (encoded === sentMetas) return;
    sentMetas = encoded;
    postPageMeta({ kind: 'meta', metas: values });
  };
  const emitTitle = () => {
    const title = document.title;
    if (title === sentTitle) return;
    sentTitle = title;
    postPageMeta({ kind: 'title', title: title });
  };
  const observePageMeta = () => {
    emitMetas();
    emitTitle();
    if (!document.head) return;
    new MutationObserver(() => {
      emitMetas();
      emitTitle();
    }).observe(document.head, {
      subtree: true, childList: true, attributes: true, characterData: true,
    });
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', observePageMeta, { once: true });
  } else {
    observePageMeta();
  }
})();"#;

/// The click callbacks one page may register (`addNotificationClickListener`); a ninth is
/// refused. Mirrors the `MAX_CLICK_LISTENERS` literal inside [`INIT_SCRIPT`] (the cap itself is
/// enforced in the page); the constant is the Rust-side anchor the tests check against.
#[allow(dead_code)]
pub const MAX_CLICK_LISTENERS: usize = 8;

/// The longest `id` a page may give a notification.
pub const MAX_ID_LEN: usize = 256;
/// The longest `title` a page may send.
pub const MAX_TITLE_LEN: usize = 512;
/// The longest `body` a page may send.
pub const MAX_BODY_LEN: usize = 4096;
/// The longest `icon` URL a page may send.
pub const MAX_ICON_LEN: usize = 2048;
/// The longest page title (`document.title`) a tab's label takes, in chars. A real title is
/// tens of characters; 256 is generous and bounded.
pub const MAX_PAGE_TITLE_CHARS: usize = 256;
/// The longest `browserx-badge` value, in chars. The TS values are counts or `"•"`.
pub const MAX_BADGE_CHARS: usize = 32;
/// The longest single IPC message, in bytes (a 512 pt title plus a 4 KiB body as JSON escape
/// the worst case sixfold; 32 KiB keeps arbitrary heads of hoarded text out of the frame).
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024;

/// The notifications one page may keep live, and the shell-wide total.
pub const MAX_PER_SOURCE: usize = 64;
pub const MAX_TOTAL: usize = 256;

/// The most messages held between frames: a flood drops the newest ones beyond this.
pub const MAX_PENDING: usize = 128;

/// What the tab that sent a message is, decided in one place from the live tabs.
///
/// Only an installed app's web tab may speak: the App Store's tab is Rune-drawn and never gets
/// a page, and a tab id we do not know is not ours (it may name an app that left the rail).
/// `tab_id` keys the per-source cap, and the application id is the group the panel shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sender {
    /// An installed application's tab: `(tab_id, application_id)`.
    App {
        tab_id: String,
        application_id: String,
    },
    /// A sender that is not an installed app's tab; its messages are refused.
    Unknown,
}

/// Which tab `origin` is, over the rail's tabs. Public for the frame-side callers and tests.
pub fn sender_of(origin: &str, tabs: &[crate::state::AppTab]) -> Sender {
    match tabs.iter().find(|tab| tab.id == origin) {
        Some(tab) if tab.kind == crate::state::TabKind::Web => Sender::App {
            tab_id: tab.id.clone(),
            application_id: tab.id.clone(),
        },
        _ => Sender::Unknown,
    }
}

/// A validated, attributed message from a page: already bounded, ready for the frame.
///
/// Notifications go to the center; [`PageMessage::Title`] and [`PageMessage::Meta`] are the
/// page's own presentation (slice 20), applied to its tab and never persisted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageMessage {
    /// `sendNotification(id, notification)`: a new or replacing notification from one app.
    Send {
        tab_id: String,
        application_id: String,
        notification_id: String,
        props: NotificationProps,
    },
    /// `closeNotification(id)`.
    Close {
        tab_id: String,
        notification_id: String,
    },
    /// `document.title` (Electron's `page-title-updated`), trimmed; empty clears the override.
    Title { tab_id: String, title: String },
    /// The `browserx-*` metas (`page-bxmetas-updated`); only `badge` is consumed, and `None`
    /// means the meta is absent or empty (clear).
    Meta {
        tab_id: String,
        badge: Option<String>,
    },
}

/// The wire format: the Electron preload's `ipcRenderer.send` payloads plus the `kind` tag that
/// picks the channel (`'new-notification'` / `'notification-close'`), and (slice 20) the page's
/// own presentation: `title` (`document.title`, Electron's `page-title-updated` event) and
/// `meta` (`browserx-*` metas, TS `page-bxmetas-updated`).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireMessage {
    kind: String,
    /// Notification id; absent for `title`/`meta` (a send/close without it is malformed).
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    notification: Option<WireNotification>,
    /// `kind: "title"`: the page's `document.title`.
    #[serde(default)]
    title: Option<String>,
    /// `kind: "meta"`: every `browserx-*` meta value, keyed by the name without the prefix.
    #[serde(default)]
    metas: Option<HashMap<String, String>>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireNotification {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

/// The outcome of one message: accepted into the queue, or refused with the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    /// Refused inputs, counted in order of check.
    Refused(Refusal),
}

impl Outcome {
    /// True when the message was queued (tests read it as a bool).
    #[cfg(test)]
    pub fn accepted(self) -> bool {
        matches!(self, Outcome::Accepted)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not valid JSON, or not a JSON object.
    Malformed,
    /// Valid shape, but over a limit.
    TooLarge,
    /// Not from an installed app's web tab.
    UnknownSender,
    /// Valid shape, but page text the tab's label/badge will not take: over its char cap, or
    /// containing a control character. The current label/badge is left as it was.
    BadText,
}

/// One accepted message from the IPC thread, waiting for the next frame's poll.
static PENDING: Mutex<Vec<PageMessage>> = Mutex::new(Vec::new());

/// The IPC-facing entry (`webview.rs::platform`): one `window.ipc.postMessage` payload from the
/// page behind `origin`. Never panics; the count it returns is for a log line.
pub fn receive(origin: &str, tabs: &[crate::state::AppTab], message: &str) -> Outcome {
    let sender = sender_of(origin, tabs);
    match parse(sender, message) {
        Ok(parsed) => enqueue(parsed),
        Err(refusal) => Outcome::Refused(refusal),
    }
}

/// Validates one message against its sender. Pure, so the hostile-input tests need no locks.
pub fn parse(sender: Sender, message: &str) -> Result<PageMessage, Refusal> {
    let Sender::App {
        tab_id,
        application_id,
    } = sender
    else {
        return Err(Refusal::UnknownSender);
    };
    if message.len() > MAX_MESSAGE_BYTES {
        return Err(Refusal::TooLarge);
    }
    let Ok(wire) = serde_json::from_str::<serde_json::Value>(message) else {
        return Err(Refusal::Malformed);
    };
    if !wire.is_object() {
        return Err(Refusal::Malformed);
    }
    // Unknown members stay unknown: the string form is parsed once, into this shape.
    let Ok(wire) = serde_json::from_value::<WireMessage>(wire) else {
        return Err(Refusal::Malformed);
    };
    match wire.kind.as_str() {
        "close" => Ok(PageMessage::Close {
            tab_id,
            notification_id: validated_id(wire.id.as_deref().ok_or(Refusal::Malformed)?)?,
        }),
        "send" => {
            let notification_id = validated_id(wire.id.as_deref().ok_or(Refusal::Malformed)?)?;
            let notification = wire.notification.unwrap_or(WireNotification {
                title: None,
                body: None,
                icon: None,
                timestamp: None,
            });
            let title = match notification.title {
                Some(title) if title.len() <= MAX_TITLE_LEN => title,
                Some(_) => return Err(Refusal::TooLarge),
                None => String::new(),
            };
            let body = match notification.body {
                Some(body) if body.len() <= MAX_BODY_LEN => body,
                Some(_) => return Err(Refusal::TooLarge),
                None => String::new(),
            };
            let icon = match notification.icon {
                Some(icon) if icon.len() <= MAX_ICON_LEN => icon,
                Some(_) => return Err(Refusal::TooLarge),
                None => String::new(),
            };
            Ok(PageMessage::Send {
                tab_id: tab_id.clone(),
                application_id,
                notification_id,
                props: NotificationProps {
                    title,
                    timestamp: notification.timestamp,
                    body: (!body.is_empty()).then_some(body),
                    icon: (!icon.is_empty()).then_some(icon),
                },
            })
        }
        "title" => {
            // Trim first: whitespace is not a label. Empty then means "clear", which restores
            // the manifest label (the shell's fallback).
            let title = page_text(&wire.title.unwrap_or_default(), MAX_PAGE_TITLE_CHARS)?;
            Ok(PageMessage::Title { tab_id, title })
        }
        "meta" => {
            // The TS preload sends every `browserx-*` meta; only `badge` is consumed here.
            // A missing or empty meta clears the badge.
            let badge = wire
                .metas
                .unwrap_or_default()
                .get("badge")
                .map(|badge| page_text(badge, MAX_BADGE_CHARS))
                .transpose()?
                .filter(|badge| !badge.is_empty());
            Ok(PageMessage::Meta { tab_id, badge })
        }
        _ => Err(Refusal::Malformed),
    }
}

/// A notification id: present and within [`MAX_ID_LEN`] (the notification kinds only).
fn validated_id(id: &str) -> Result<String, Refusal> {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return Err(Refusal::TooLarge);
    }
    Ok(id.to_owned())
}

/// Trims page text and refuses what a tab label will not take: over `max_chars`, or containing
/// a control character. Control characters are refused, never stripped — a page must not
/// smuggle formatting into a label the shell draws.
fn page_text(value: &str, max_chars: usize) -> Result<String, Refusal> {
    let trimmed = value.trim();
    if trimmed.chars().count() > max_chars || trimmed.chars().any(char::is_control) {
        return Err(Refusal::BadText);
    }
    Ok(trimmed.to_owned())
}

/// Queues one validated message: bounded hold, deduped, live-count-checked.
fn enqueue(message: PageMessage) -> Outcome {
    let mut pending = match PENDING.lock() {
        Ok(pending) => pending,
        Err(poisoned) => poisoned.into_inner(),
    };
    // One slot per source: a replacement notification (same id), or the latest title/badge,
    // replaces what that tab already queued instead of growing the queue.
    if let Some(slot) = pending
        .iter_mut()
        .find(|queued| same_slot(queued, &message))
    {
        *slot = message;
        return Outcome::Accepted;
    }
    if pending.len() >= MAX_PENDING {
        return Outcome::Refused(Refusal::TooLarge);
    }
    pending.push(message);
    Outcome::Accepted
}

/// Whether two queued messages occupy the same slot: a notification id (send/close replace each
/// other by id, as before) or the single title/badge slot per tab.
fn same_slot(queued: &PageMessage, next: &PageMessage) -> bool {
    match (queued, next) {
        (
            PageMessage::Send {
                tab_id,
                notification_id,
                ..
            }
            | PageMessage::Close {
                tab_id,
                notification_id,
                ..
            },
            PageMessage::Send {
                tab_id: other_tab,
                notification_id: other_id,
                ..
            }
            | PageMessage::Close {
                tab_id: other_tab,
                notification_id: other_id,
                ..
            },
        ) => tab_id == other_tab && notification_id == other_id,
        (PageMessage::Title { tab_id, .. }, PageMessage::Title { tab_id: other, .. })
        | (PageMessage::Meta { tab_id, .. }, PageMessage::Meta { tab_id: other, .. }) => {
            tab_id == other
        }
        _ => false,
    }
}

/// How many live notifications each source has, from the center's stored ones.
fn per_source(center: &NotificationCenter) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for notification in center.notifications() {
        let source = match (&notification.application_id, &notification.tab_id) {
            (Some(id), _) | (None, Some(id)) => id.clone(),
            (None, None) => "station".to_owned(),
        };
        *counts.entry(source).or_default() += 1;
    }
    counts
}

/// Drains the accepted messages into `state`: notifications into the center, page titles/badges
/// onto their tabs (slice 20). Called by the frame's poll, on the main thread. Returns whether
/// the view must rebuild. A source at its cap has its newest notifications refused until it
/// marks them read; a full center refuses everything new.
pub fn deliver_to(state: &mut ShellState) -> bool {
    // The bridge's pending queue is process state and every test's frame poll drains it:
    // serialized so a test that received a message delivers it itself, not through another
    // test's poll.
    #[cfg(test)]
    let _serial = crate::test_support::lock();
    let messages = drain();
    if messages.is_empty() {
        return false;
    }
    let mut changed = false;
    let mut counts = per_source(state.notifications());
    let mut total = state.notifications().unread_count();
    for message in messages {
        match message {
            PageMessage::Send {
                tab_id,
                application_id,
                notification_id,
                props,
            } => {
                // Dedupe by id: the TS reducer's ADD_NOTIFICATION replaces the payload, so a
                // reused id keeps its position in the list (the insert is keyed on it).
                let existing = state
                    .notifications()
                    .notification(&notification_id)
                    .is_some();
                let source_count = counts.get(tab_id.as_str()).copied().unwrap_or(0);
                if !existing && (total >= MAX_TOTAL || source_count >= MAX_PER_SOURCE) {
                    continue;
                }
                if !existing {
                    total += 1;
                    *counts.entry(tab_id.clone()).or_default() += 1;
                }
                state
                    .notifications_mut()
                    .apply(NotificationAction::NewNotification {
                        application_id: Some(application_id),
                        tab_id: Some(tab_id),
                        notification_id,
                        props,
                        options: notification_center::NewNotificationOptions::default(),
                    });
                changed = true;
            }
            PageMessage::Close {
                tab_id: _,
                notification_id,
            } => {
                changed |= state.close_notification(&notification_id);
            }
            PageMessage::Title { tab_id, title } => {
                // The page's own label (slice 20): never persisted, never a selection change.
                changed |= state.set_page_title(&tab_id, &title);
            }
            PageMessage::Meta { tab_id, badge } => {
                changed |= state.set_page_badge(&tab_id, badge.as_deref());
            }
        }
    }
    changed
}

/// Takes the held messages (the frame's poll). A poisoned lock still yields them.
fn drain() -> Vec<PageMessage> {
    let mut pending = match PENDING.lock() {
        Ok(pending) => pending,
        Err(poisoned) => poisoned.into_inner(),
    };
    std::mem::take(&mut *pending)
}

/// Forgets every held message (tests).
#[cfg(test)]
pub fn clear() {
    match PENDING.lock() {
        Ok(mut pending) => pending.clear(),
        Err(poisoned) => poisoned.into_inner().clear(),
    }
}

/// The held messages, in arrival order (tests observe what the frame will deliver).
#[cfg(test)]
pub fn pending() -> Vec<PageMessage> {
    let mut held = match PENDING.lock() {
        Ok(pending) => pending.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    held.truncate(MAX_PENDING);
    held
}

#[cfg(test)]
mod tests {
    use super::*;

    // The parse tests live in the shell's `tests_pages.rs`: they share the module-static
    // PENDING queue through `receive`, and `cargo test` runs a binary's tests in parallel
    // threads. `parse` needs no queue, so these only pin the sender gate.
    fn app(id: &str) -> Sender {
        Sender::App {
            tab_id: id.to_owned(),
            application_id: id.to_owned(),
        }
    }

    #[test]
    fn an_unknown_sender_is_refused_whatever_the_message() {
        assert_eq!(
            parse(Sender::Unknown, r#"{"kind":"close","id":"x"}"#),
            Err(Refusal::UnknownSender)
        );
        let _ = app("unused");
    }
}
