//! Static, reviewed per-site injected scripts (port of
//! `packages/app/src/plugins/injected-js/*` and `JAVASCRIPT_INJECTIONS` in
//! `packages/app/src/applications/manifest-provider/const/index.ts`).
//!
//! Each script is selected by the manifest's exact `bx_legacy_service_id`,
//! never constructed from page input, and runs inside a self-contained IIFE
//! after `DOMContentLoaded` (or immediately if the document is already ready)
//! with `try`/`catch` isolation so a site DOM change never breaks the hosted
//! page or the core bridge.
//!
//! Badge scripts write `meta[name=browserx-badge]`, which `INIT_SCRIPT`'s
//! `MutationObserver` picks up and sends to the shell over the single bounded
//! IPC channel (`kind: "meta"`). Snooze-aware scripts (WhatsApp, Messenger)
//! subscribe through `window.bxApi.notificationCenter.addSnoozeDurationInMsChangeListener`.

use crate::notifications::INIT_SCRIPT;

/// Service-id → static script mapping matching Electron's `JAVASCRIPT_INJECTIONS`.
pub fn for_service(legacy_service_id: &str) -> Option<&'static str> {
    match legacy_service_id {
        "gmail" => Some(GMAIL_SCRIPT),
        "slack" | "station-support" => Some(SLACK_SCRIPT),
        "whatsapp" => Some(WHATSAPP_SCRIPT),
        "facebook-messenger" => Some(MESSENGER_SCRIPT),
        "outlook" | "office-365" | "outlook-pro" => Some(OFFICE365_SCRIPT),
        "gdrive-mu" | "gcalendar-mu" | "google-cloud" | "google-keep" | "meet" => {
            Some(REMOVE_GOOGLE_ACCOUNT_SCRIPT)
        }
        "asana" => Some(ASANA_SCRIPT),
        _ => None,
    }
}

/// All supported legacy service ids (for tests and inventory checks).
#[cfg(test)]
pub const SUPPORTED_SERVICES: [&str; 14] = [
    "asana",
    "gmail",
    "slack",
    "station-support",
    "gdrive-mu",
    "gcalendar-mu",
    "google-cloud",
    "google-keep",
    "meet",
    "outlook",
    "office-365",
    "outlook-pro",
    "facebook-messenger",
    "whatsapp",
];

/// Composes the core `INIT_SCRIPT` with the app's optional per-site script.
pub fn combined_init_script(legacy_service_id: Option<&str>) -> String {
    match legacy_service_id.and_then(for_service) {
        Some(site) => format!("{INIT_SCRIPT}\n{site}"),
        None => INIT_SCRIPT.to_owned(),
    }
}

/// Gmail: polls `.aim` unread count into `meta[name=browserx-badge]` and hides
/// Google's sign-out link. Null-safe when `.aim` is absent (signed-out page).
pub const GMAIL_SCRIPT: &str = r#"(() => {
  const updateBadge = (badge) => {
    try {
      if (!document.head) return;
      let meta = document.querySelector('meta[name=browserx-badge]');
      if (!meta) {
        meta = document.createElement('meta');
        meta.name = 'browserx-badge';
        meta.content = '';
        document.head.appendChild(meta);
      }
      const next = String(badge);
      if (meta.content !== next) meta.content = next;
    } catch (e) {}
  };
  const poll = () => {
    try {
      const aimEl = document.getElementsByClassName('aim')[0];
      if (!aimEl || !aimEl.textContent) return;
      const parts = aimEl.textContent.split(':');
      const badge = parseInt(parts[parts.length - 1].replace(/[^0-9]/g, ''), 10);
      updateBadge(isNaN(badge) ? '' : badge);
    } catch (e) {}
  };
  const start = () => {
    try {
      const style = document.createElement('style');
      style.textContent = 'a[href*="/SignOutOptions"] { visibility: hidden !important; }';
      (document.head || document.documentElement || document.body).appendChild(style);
    } catch (e) {}
    poll();
    setInterval(poll, 2000);
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start, { once: true });
  } else {
    start();
  }
})();"#;

/// Slack / Station Support: hides `#macssb1_banner` and forwards unread mention
/// counts from Slack's redux store (or legacy `TS.model`) to `browserx-badge`.
/// Deliberately omits the legacy `browserx-apiToken` export (unused without SDK).
pub const SLACK_SCRIPT: &str = r#"(() => {
  const INTERVAL_MS = 500;
  const TIMEOUT_MS = 10000;
  const WITH_BADGE = '\u2022';
  const WITHOUT_BADGE = '';
  const updateBadge = (value) => {
    try {
      if (!document.head) return;
      let meta = document.querySelector('meta[name=browserx-badge]');
      if (!meta) {
        meta = document.createElement('meta');
        meta.name = 'browserx-badge';
        meta.content = '';
        document.head.appendChild(meta);
      }
      if (meta.content !== value) meta.content = value;
    } catch (e) {}
  };
  const getSlackStore = () => {
    const teamId = window.location.pathname.split('/')[2];
    if (!teamId) return null;
    const team = window.slackDebug && window.slackDebug[teamId];
    return team && team.redux ? team.redux : null;
  };
  const getLegacyModel = () => (window.TS && window.TS.model ? window.TS.model : null);
  const getBadge = (state) => {
    try {
      const counts = state && state.unreadCounts && state.unreadCounts.countsPerChannel;
      if (!counts) return WITHOUT_BADGE;
      const mentions = Object.values(counts)
        .map((c) => (c && c.unreadHighlightCnt) || 0)
        .reduce((acc, n) => acc + n, 0);
      return mentions ? WITH_BADGE : WITHOUT_BADGE;
    } catch (e) {
      return WITHOUT_BADGE;
    }
  };
  const waitValue = (getter) => new Promise((resolve) => {
    const immediate = getter();
    if (immediate) { resolve(immediate); return; }
    let elapsed = 0;
    const id = setInterval(() => {
      const value = getter();
      elapsed += INTERVAL_MS;
      if (value || elapsed >= TIMEOUT_MS) {
        clearInterval(id);
        resolve(value || null);
      }
    }, INTERVAL_MS);
  });
  const start = async () => {
    try {
      const style = document.createElement('style');
      style.textContent = '#macssb1_banner { display: none !important; }';
      (document.head || document.documentElement || document.body).appendChild(style);
    } catch (e) {}
    try {
      const storeOrModel = await Promise.race([
        waitValue(getSlackStore),
        waitValue(getLegacyModel),
      ]);
      if (storeOrModel && typeof storeOrModel.getState === 'function') {
        let current;
        const onChange = () => {
          const next = getBadge(storeOrModel.getState());
          if (next !== current) {
            current = next;
            updateBadge(next);
          }
        };
        if (typeof storeOrModel.subscribeImmediate === 'function') {
          storeOrModel.subscribeImmediate(onChange);
        } else if (typeof storeOrModel.subscribe === 'function') {
          onChange();
          storeOrModel.subscribe(onChange);
        }
      } else if (storeOrModel) {
        const pollModel = () => {
          updateBadge(storeOrModel.all_unread_highlights_cnt ? WITH_BADGE : WITHOUT_BADGE);
        };
        pollModel();
        setInterval(pollModel, INTERVAL_MS);
      }
    } catch (e) {}
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => { start(); }, { once: true });
  } else {
    start();
  }
})();"#;

/// WhatsApp: mutes `Audio.prototype.play` while Station notifications are snoozed,
/// bypassing `blob:http` voice notes.
pub const WHATSAPP_SCRIPT: &str = r#"(() => {
  let snoozed = false;
  const originalPlay = Audio.prototype.play;
  const shouldBypassSnooze = (audio) => Boolean(audio && typeof audio.src === 'string' && audio.src.startsWith('blob:http'));
  Audio.prototype.play = function () {
    if (!snoozed || shouldBypassSnooze(this)) {
      return originalPlay.call(this);
    }
    return Promise.resolve();
  };
  if (window.bxApi && window.bxApi.notificationCenter && typeof window.bxApi.notificationCenter.addSnoozeDurationInMsChangeListener === 'function') {
    window.bxApi.notificationCenter.addSnoozeDurationInMsChangeListener((_, duration) => {
      snoozed = Boolean(duration);
    });
  }
})();"#;

/// Facebook Messenger: maps `(n)` / `Messenger` document titles into `browserx-badge`
/// and mutes `<audio>` elements while snoozed outside active video calls.
pub const MESSENGER_SCRIPT: &str = r#"(() => {
  const updateBadge = (badge) => {
    try {
      if (!document.head) return;
      let meta = document.querySelector('meta[name=browserx-badge]');
      if (!meta) {
        meta = document.createElement('meta');
        meta.name = 'browserx-badge';
        meta.content = '';
        document.head.appendChild(meta);
      }
      const next = String(badge);
      if (meta.content !== next) meta.content = next;
    } catch (e) {}
  };
  const checkTitle = (title) => {
    if (!title) return;
    const matchCount = title.match(/^\((\d+)\)/);
    if (matchCount) {
      updateBadge(matchCount[1]);
      return;
    }
    if (/^Messenger$/.test(title)) {
      updateBadge('');
    }
  };
  const setAudioVolume = (element, vol) => { try { element.volume = vol; } catch (e) {} };
  const turnOffAudioVolumeObserver = (target) => {
    if (!target) return () => {};
    const observer = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        for (const node of Array.from(mutation.addedNodes)) {
          if (node && node.tagName === 'AUDIO') setAudioVolume(node, 0);
        }
      }
    });
    Array.from(target.querySelectorAll('audio')).forEach((el) => setAudioVolume(el, 0));
    observer.observe(target, { attributes: true, childList: true });
    return () => observer.disconnect();
  };
  let disconnectObserver = null;
  const start = () => {
    checkTitle(document.title);
    const titleEl = document.getElementsByTagName('title')[0];
    if (titleEl) {
      new MutationObserver(() => checkTitle(document.title)).observe(titleEl, {
        subtree: true, characterData: true, childList: true,
      });
    }
    if (window.bxApi && window.bxApi.notificationCenter && typeof window.bxApi.notificationCenter.addSnoozeDurationInMsChangeListener === 'function') {
      window.bxApi.notificationCenter.addSnoozeDurationInMsChangeListener((_, duration) => {
        if (duration && !document.location.pathname.includes('/videocall/incall/')) {
          if (disconnectObserver) disconnectObserver();
          disconnectObserver = turnOffAudioVolumeObserver(document.body);
        } else {
          if (disconnectObserver) {
            disconnectObserver();
            disconnectObserver = null;
          }
          document.querySelectorAll('audio').forEach((el) => setAudioVolume(el, 1));
        }
      });
    }
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start, { once: true });
  } else {
    start();
  }
})();"#;

/// Outlook / Office 365 / Outlook Pro: polls the Inbox counter into `browserx-badge`.
pub const OFFICE365_SCRIPT: &str = r#"(() => {
  const updateBadge = (badge) => {
    try {
      if (!document.head) return;
      let meta = document.querySelector('meta[name=browserx-badge]');
      if (!meta) {
        meta = document.createElement('meta');
        meta.name = 'browserx-badge';
        meta.content = '';
        document.head.appendChild(meta);
      }
      const next = String(badge);
      if (meta.content !== next) meta.content = next;
    } catch (e) {}
  };
  const poll = () => {
    try {
      const counter = document.querySelector("span[title*='Inbox'] + div > span");
      if (!counter) return;
      const inboxText = counter.textContent;
      if (!inboxText) { updateBadge(''); return; }
      const badge = parseInt(inboxText, 10);
      updateBadge(isNaN(badge) ? '' : badge);
    } catch (e) {}
  };
  const start = () => {
    poll();
    setInterval(poll, 2000);
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start, { once: true });
  } else {
    start();
  }
})();"#;

/// Google workspace apps (Drive, Calendar, Cloud, Keep, Meet): hides the sign-out link.
pub const REMOVE_GOOGLE_ACCOUNT_SCRIPT: &str = r#"(() => {
  const apply = () => {
    try {
      const style = document.createElement('style');
      style.textContent = 'a[href*="/SignOutOptions"] { visibility: hidden !important; }';
      (document.head || document.documentElement || document.body).appendChild(style);
    } catch (e) {}
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', apply, { once: true });
  } else {
    apply();
  }
})();"#;

/// Asana: forces Google auth button to redirect instead of popup, and reloads if
/// stuck on a disconnect banner while online for 10 seconds.
pub const ASANA_SCRIPT: &str = r#"(() => {
  const DISCONNECTED_CLASS = 'DisconnectBar DisconnectBarManager-bar';
  const replacePopupWithRedirect = () => {
    try {
      const btn = document.getElementById('google_auth_button');
      if (btn) {
        const str = btn.getAttribute('onclick');
        if (str) btn.setAttribute('onclick', str.replace('false', 'true'));
      }
    } catch (e) {}
  };
  const reloadIfStuck = () => setTimeout(() => {
    try {
      if (document.querySelector('.DisconnectBar.DisconnectBarManager-bar')) {
        window.location.reload();
      }
    } catch (e) {}
  }, 10000);
  const start = () => {
    replacePopupWithRedirect();
    const observer = new MutationObserver((mutations) => {
      if (!document.querySelector('.DisconnectBarManager')) return;
      setTimeout(() => {
        const found = mutations.some((m) =>
          Array.from(m.addedNodes).some((n) => n && n.className === DISCONNECTED_CLASS)
        );
        if (found) reloadIfStuck();
      }, 500);
    });
    const opts = { childList: true, subtree: true };
    if (navigator.onLine) observer.observe(document, opts);
    window.addEventListener('online', () => {
      reloadIfStuck();
      observer.observe(document, opts);
    });
    window.addEventListener('offline', () => observer.disconnect());
  };
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start, { once: true });
  } else {
    start();
  }
})();"#;
