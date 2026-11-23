# Preload / `bxApi` inventory — the P4.10 prerequisite

Status: read-only inventory, 2026-11-18. Scope: everything the Electron app injects into an app
page (a `<webview>` guest), what each injected member does, where it lands, and what a native port
must expose. Companion: `docs/full-rust-port-program.md` P4.10 (the "inventory first" gate),
`docs/rust-napi-bridge.md` (the bxApi→napi map written when Electron was expected to stay),
`rust/crates/station-shell/src/notifications/bridge.rs` (what the native shell already injects).

Every path below was opened; every `file:line` is from that read. Nothing here changes code.

**Surfaces found:** 1 page API (`window.bxApi`, 20 members), 13 live DOM/behaviour overrides
(plus 5 dead/disabled injections and one dev-only preload), and 7 per-site scripts mapped to 14 legacy service ids.

---

## 1. What is injected today, per surface

### 1.1 The page API — `window.bxApi` (20 members)

**Definition:** `packages/app/src/plugins/webview-preload.js:56-126`, exposed with
`contextBridge.exposeInMainWorld('bxApi', bxApi)` at `:128`. Loaded as part of the guest preload
chain — `packages/app/src/static/preload/preload.js:135` (`require('../../plugins/webview-preload')`).

**How it reaches main:** `BxAPI.perform` (`webview-preload.js:33-44`, after `appIsReady` gating on
`document.readyState`, `:46-53`) sends `bx-api-perform` to the **worker webContents** and resolves on
`bx-api-perform-response-${channel}` (`:5-17`). Subscriptions send `bx-api-subscribe` and listen on
`bx-api-subscribe-response-${channel}` (`:19-26`); unsubscribe is renderer-local only (`:28-30`).
The doc-time plan for this plumbing is `docs/rust-napi-bridge.md:29-38`.

All 20 are **class (a) page API** — callable by the page's own scripts.

| Namespace | Member (`webview-preload.js:line`) | Signature | What it does | Lands in (main) |
| --- | --- | --- | --- | --- |
| `notificationCenter` | `addSnoozeDurationInMsChangeListener` (`:58`) | `(fn(e, ms\|undefined)) => void` | subscribe to the shell's snooze duration | `GetSnoozeDuration` selector → `notification-center` state |
| | `sendNotification` (`:60`) | `(id, notification) => void` | raise a notification in the panel | IPC `new-notification` → `NotificationCenter` |
| | `closeNotification` (`:61`) | `(id) => void` | remove a notification | IPC `notification-close` |
| | `addNotificationClickListener` (`:63`) | `(fn(e, id)) => void` | receive clicks on your notifications | IPC `trigger-notification-click` |
| | `removeNotificationClickListener` (`:64`) | `(fn) => void` | stop receiving them | renderer-local `ipcRenderer.off` |
| `applications` | `install` (`:67`) | `(payload{manifestURL, context}) => Promise` | install an app from the page (store card action) | `InstallApplication` → `app-store/sagas` |
| | `uninstall` (`:72`) | `(applicationId) => Promise` | uninstall one app | `UninstallApplication` |
| | `uninstallByManifest` (`:77`) | `(manifestURL) => Promise` | uninstall every instance of a manifest | `UninstallApplications` |
| | `setConfigData` (`:82`) | `(applicationId, configData) => Promise` | write per-app config (configurator) | `SetApplicationConfigData` |
| | `search` (`:87`) | `(query) => Promise` | store search | `SearchApplication` |
| | `getMostPopularApps` (`:92`) | `() => Promise` | store home rows | `GetMostPopularApplication` |
| | `getAllCategories` (`:93`) | `() => Promise` | store sidebar | `GetAllCategories` |
| | `getApplicationsByCategory` (`:94`) | `() => Promise` | category rows | `GetApplicationsByCategory` |
| | `requestPrivate` (`:95`) | `(payload{name,themeColor,bxIconURL,startURL,scope}) => Promise` | request a private app | `RequestPrivateApplication` |
| | `getPrivateApps` (`:100`) | `() => Promise` | private apps list | `GetPrivateApplications` |
| `theme` | `addThemeColorsChangeListener` (`:107`) | `(fn(e, colors[])) => void` | subscribe to theme colours (store page gradient) | `GetThemeColors` selector |
| `identities` | `addIdentitiesChangeListener` (`:110`) | `(fn(e, identities[])) => void` | subscribe to the google identity list | `GetAllIdentities` selector |
| | `removeIdentitiesChangeListener` (`:111`) | `(fn) => void` | unsubscribe | renderer-local |
| | `requestLogin` (`:113`) | `(provider) => Promise` | start OAuth (configurator) | `RequestLogin` → `user-identities/sagas` |
| `manifest` | `getManifest` (`:120`) | `(manifestURL) => Promise` | fetch a manifest by URL | `GetManifestByURL` |

Two internal, non-exposed helpers: `BxAPI.perform` (`:33-44`) and `BxAPI.appIsReady` (`:46-53`).

### 1.2 DOM / behaviour overrides (13 live)

These change page-observable behaviour rather than adding an API. "Class" is (b) DOM/behaviour
override unless stated.

| # | Override | Where | What it does | Lands in |
| --- | --- | --- | --- | --- |
| 1 | `browserx-*` meta observer | `packages/app/src/static/preload/preload.js:10-84`, started `:86-102` | watches `<meta name="browserx-*">` in `document.head` (incl. insert/attr/remove) and sends the values | `sendToHost('page-bxmetas-updated', values)` (`:68`) → `packages/app/src/applications/Application.tsx:269-275` → `onBadgeUpdated(metas.badge)` |
| 2 | click capture → host | `packages/app/src/static/preload/preload.js:93-99` | reports every click (`sendToHost('page-click')`) | `Application.tsx:282-284` → `handlePageClick()` (`:300-316`) re-dispatches synthetic click/mousedown/mouseup on the `<webview>` element (Electron bug workaround) |
| 3 | `window.chrome` shims | `packages/app/src/static/preload/preload.js:110-118`, `:154-220` | `chrome = undefined` when absent; `{webstore: true}` flag; a stubbed `chrome.runtime` (`connect`/`sendMessage`/`sendNativeMessage` failing with "Receiving end does not exist") | none (page-local; GDrive/Qonto detection) |
| 4 | GDrive-offline markers | `packages/app/src/static/preload/preload.js:123-133` | sets `_docs_chrome_extension_exists/_features_version/_permissions` | none (page-local) |
| 5 | `window.print` | `packages/app/src/static/preload/preload.js:147-149` | replaces with `ipc.send('print')` | `packages/app/src/services/services/tab-webcontents/main.ts:377` |
| 6 | ECX preload | `packages/app/src/static/preload/preload.js:116-118` (`require('electron-chrome-extension/preload')`, off under `STATION_DISABLE_ECX`) | exposes Chrome-extension APIs to pages | `electron-chrome-extension` (ecx service) |
| 7 | `window.open` proxy | `packages/app/src/static/preload/preload.js:119` → `packages/app/src/static/preload/window-open/index.ts:1-35`, `packages/app/src/static/preload/window-open/window-setup.js:1-121` | replaces Electron's own `packages/app/src/static/preload/window-open/window-setup.js` so `window.open` goes through the guest-window proxy, with an override list for `app.mixmax.com/_oauth/google` | `BrowserWindowProxy` over `ELECTRON_GUEST_WINDOW_MANAGER_*` IPC (Electron-internal) |
| 8 | Cmd+T suppression | `packages/app/src/static/preload/preload.js:228-239` | capture-phase `keydown` stop for ⌘T (macOS tab-key parity) | none |
| 9 | Global cleanup | `packages/app/src/static/preload/preload.js:241-258` + `packages/app/src/static/preload/clean-global.js:1-19` | diffs `Object.keys(window)` before/after preloads and deletes everything not whitelisted; whitelist = `bxApi`, `chrome`, `_docs_chrome_extension_*` (`:247-253`) | none |
| 10 | `alert` override | `packages/app/src/dialogs/webview-preload.js:4-13` (recursive over iframes via `packages/app/src/utils/webview-override-helper.js:16-30`) | `alert(message, title)` → `ipc.sendSync('window-alert', …)` returning `undefined` | `packages/app/src/services/services/tab-webcontents/main.ts:136` |
| 11 | cursor override | `packages/app/src/ui/webview-preload.js:3-6` | `ui-setCursorIcon` → `html { cursor }` | sent by `packages/app/src/services/services/cursor/main.ts:7` |
| 12 | **`window.Notification` override** | `packages/app/src/static/preload/webview-inject.js:227-236` (`BxNotification` class `:77-225`) | replaces the page's Notification API (recursively into same-origin iframes, `:36-54`); every `new Notification(...)` becomes `bxApi.notificationCenter.sendNotification(id, {timestamp,title,body,icon})` (`:138-141`); `close()` forwards; click listener wired (`:144-163`); `permission='granted'`, `requestPermission` resolves granted (`:218-224`, `:228`) | `bxApi.notificationCenter` (§1.1) → panel |
| 13 | autologin | `packages/app/src/static/preload/autologin.js:1-132` | detects a login form, asks the shell for saved credentials (`:30-32`), fills and optionally submits (`:34-53`), shows a "remove saved login" banner on typing (`:80-86`) | `packages/app/src/services/services/tab-webcontents/main.ts:161,168` |

Notes on #12: it is **not** a preload. `packages/app/src/applications/Application.tsx:352-362` `handleDomReady`
executes it with `webView.view.executeJavaScript(...)` at `:353-354` — i.e. after the document is
ready, not before page scripts. Icon handling: `//`-prefixed URLs get `https:` (Gmail), `blob:http`
icons are fetched and inlined as data URLs (Telegram), other non-strings become `undefined`
(`:107-141`).

### 1.3 Dead, disabled, dev-only injections (do not port)

| Item | Evidence | State |
| --- | --- | --- |
| `redirect-url` listener | `packages/app/src/static/preload/preload.js:105-107` is the only occurrence anywhere in `src` | **dead channel** — no sender; would let main navigate the page |
| `notification-clicked` channel | received at `Application.tsx:277-279`; no sender in `src` | **dead channel** |
| `autofill` | `packages/app/src/static/preload/preload.js:141-144` — the `require` is commented out ("a hack that did not work properly … APP-760") | disabled; `packages/app/src/static/preload/autofill.js:1-51` stays unreferenced |
| `packages/app/src/notification-center/webview-preload.ts` for guest pages | `packages/app/src/static/preload/preload.js:136` commented out | disabled for pages; still imported by the **renderer** entries `packages/app/src/index.js:23` and `packages/app/src/index-sub.js:22` (`window.Notification = BxNotification` for Station's own UI) — dies with the renderer |
| `packages/app/src/webui/preload.js` | `packages/app/src/webui/preload.js:1-25`: only for `station:` pages; the configurator branch is entirely commented (`:20-23`), leaving `module.filename = handler.filePath` (`:15`) | no live behaviour |
| `packages/app/src/static/preload/dev-preload.js` | `packages/app/src/main.ts:192-198` registers it as a **session** preload when unpackaged and ECX enabled | dev-only (extensions debugging) |
| second copy under `packages/app/app/` | directory absent; `packages/app/src/webui/const.ts:1` still says "also edit `app/plugins/webview-preload.js` and `app/webui/preload.js`" | **stale comment**, no second copy |

### 1.4 Per-site injected scripts (7 files → 14 service ids)

Rows below name the file under that directory (all seven exist). **Registry:** `packages/app/src/applications/manifest-provider/const/index.ts:30-45`
(`JAVASCRIPT_INJECTIONS`). **Loader:** `packages/app/src/plugins/helpers.ts:6-22` — each file is
raw-loader'd, wrapped in `(function(){ … })()`, joined, memoized per legacy service id.
**Execution:** `Application.tsx:356-358` (`executeJavaScript` after `webview-inject.js`, same
`handleDomReady`).

| Script (`packages/app/src/plugins/injected-js/…`) | Service ids | What it does |
| --- | --- | --- |
| `gmailInjectedScript.js:1-40` | `gmail` | polls `.aim` every 2 s → unread count → `meta[browserx-badge]`; hides `a[href*="/SignOutOptions"]` (`:36-40`) |
| `slackInjectedScript.js:1-156` | `slack`, `station-support` | hides `#macssb1_banner`; finds Slack's redux store (`window.slackDebug[teamId].redux`, `:46-53`) or legacy `window.TS.model` (`:55-57`); subscribes token (`:107,138`) and unread-mention badge → `meta[browserx-badge]` (`:108-114,140-143`), 500 ms poll / 10 s timeout for the store |
| `whatsappInjectedScript.js:1-18` | `whatsapp` | wraps `Audio.prototype.play` so audio is muted while snoozed (except `blob:` calls); snooze state comes from `bxApi.notificationCenter.addSnoozeDurationInMsChangeListener` (`:16-18`) |
| `messengerInjectedScript.js:1-76` | `facebook-messenger` | badge from the document title `(n)` / `Messenger` (`:26-40`); mutes `<audio>` while snoozed, un-mutes after (`:44-76`) |
| `office365InjectedScript.js:1-23` | `outlook`, `office-365`, `outlook-pro` | polls the inbox counter (`span[title*=Inbox] + div > span`) every 2 s → `meta[browserx-badge]` |
| `removeGoogleAccountInjectedScript.js:1-11` | `gdrive-mu`, `gcalendar-mu`, `google-cloud`, `google-keep`, `meet` | hides the Google sign-out link |
| `asanaInjectedScript.js:1-72` | `asana` | forces the Google-auth popup into a redirect (`:2-9`); reloads after a 10 s disconnect banner (`:14-70`) |

---

## 2. Which page behaviours depend on them — and what breaks natively today

"Today" = the native shell as it stands (`rust/crates/station-shell/src/`, verified against source,
not the ADR).

| Behaviour | Depends on | Visible breakage in the native shell today |
| --- | --- | --- |
| Web-page notifications (`new Notification` from Gmail/Slack/WhatsApp/Teams…) reach the panel | #12 override + `bxApi.notificationCenter.sendNotification`/`addNotificationClickListener` | **Covered (slice 19):** the injected script installs the `Notification` override, so `new Notification(...)` reaches the panel and its `onclick` comes back through the click hook. |
| Unread badges on rail rows (Gmail, Slack, Outlook, Messenger, Asana) | site scripts writing `meta[browserx-badge]` + #1 observer + `page-bxmetas-updated` | **Covered for the channel (slice 20):** the injected `browserx-*` observer sends `kind:"meta"` with the metas object, and the rail draws the badge with `tab-model::Badge` semantics (count or dot). The **site scripts** that write the meta are still not injected (slice 2's data-table half), so a badge appears only when a page sets `meta[name=browserx-badge]` itself. |
| Tab titles from the page | Electron `did-update-title` → `onTitleUpdated` (`Application.tsx:370-372`); the dock shows it | **Covered (slice 20):** the injected observer sends `document.title` as `kind:"title"`; the top bar and the rail's accessible name draw it, with the manifest name as the fallback (clearing restores it). Never persisted. |
| New windows / `target=_blank` / `window.open` | #7 proxy in Electron; **native replacement exists** | **Covered** — `rust/crates/station-shell/src/webview.rs:892-895` (`with_new_window_req_handler` → `routing::enqueue`, allowlist `http/https/mailto`, per-frame + rolling caps `rust/crates/station-shell/src/routing.rs:39-75`). The guest-window proxy is not needed. |
| Notifications clickable back into the page | `bxApi.notificationCenter.addNotificationClickListener` + shell `click_script` | **Covered** for pages using the four members (`rust/crates/station-shell/src/webview.rs:495-498` injects `window.__stationNotificationClick`, paged listeners capped at 8), and for `new Notification()` instances since slice 19 (one click listener per instance → `onclick`). |
| Snooze mutes WhatsApp/Messenger audio; Slack snooze sync | site scripts + `addSnoozeDurationInMsChangeListener` | **Not covered** — no injected script, no snooze listener in the native script (`INIT_SCRIPT` exposes no such member). |
| Slack/Notion service runtimes (activity/history/search/resources, Slack search + snooze sync) | `getServiceRuntime` (`packages/app/src/plugins/api.ts:59-73` via `packages/app/src/plugins/sagas.ts`) loading `slack/main`, `notion/main`, `slite/main` (definitions `packages/app/manifests/definitions/21.json`, `…/129.json`, `…/27.json` `"main"`) | **Not covered** — no runtime host natively. Note the `renderer` runtimes are **dead even in Electron**: `getServiceRuntimeRenderer` returns immediately (`packages/app/src/plugins/api.ts:82-83`, "FIXME migrate this to use manifest"), so definitions' `"renderer"` values are never loaded. |
| `alert()` / confirm dialogs | #10 | WebView2 shows its own modal (Electron intercepted it to render a Station-styled dialog). Behaviour differs, nothing breaks. |
| Saved-credential autologin | #13 + `packages/app/src/services/services/tab-webcontents/main.ts:161` | **Not covered** — no credential store natively. |
| `window.print` | #5 | Page `window.print()` reaches WebView2's default print UI (or nothing) instead of the app's print sagas. |
| Chrome-extension APIs in pages (Gmail extensions, Mixmax, Clearbit…) | #6 + ECX | **Not covered** — no extensions (product decision, program P4.11). |
| Page detection of "Chrome" | #3 (+ `_docs_*` #4) | Reduced-compatibility pages may degrade (Qonto/GDrive notes in comments `packages/app/src/static/preload/preload.js:114,151-153`). Not measured. |
| Global leak cleanup | #9 | Not ported; `with_initialization_script` runs in an isolated world by design (page-visible globals are only what the script sets), so the Electron cleanup exists to hide Electron's own `require`/`process` leaks — a WebView2 page never sees those. Low priority. |
| Cmd+T parity | #8 | macOS-only; irrelevant on Windows. |

---

## 3. What the native shell already covers

Verified in `rust/crates/station-shell/src/`:

- **The four notification members only** — `rust/crates/station-shell/src/notifications/bridge.rs:36-70` (members `:47-68`) injects
  `window.bxApi = { notificationCenter: { sendNotification, closeNotification,
  addNotificationClickListener, removeNotificationClickListener } }` (frozen; **no**
  `addSnoozeDurationInMsChangeListener`); the click hook is the non-enumerable
  `window.__stationNotificationClick` (`:44-46`), listeners capped at 8 (`:52-58`, `:75-78`).
- **New windows and links** — `rust/crates/station-shell/src/webview.rs:892-895` deny-and-route; `rust/crates/station-shell/src/routing.rs:82-91` allows only
  `http`, `https`, `mailto` out to the OS browser; `MAX_QUEUED=64`, `MAX_EXTERNAL_PER_FRAME=5`,
  `MAX_EXTERNAL_PER_WINDOW=10` per `EXTERNAL_WINDOW_S` (`rust/crates/station-shell/src/routing.rs:39-57`).
- **Page → shell IPC** — exactly one handler per page: `with_ipc_handler` → 
  `crate::notifications::receive(&ipc_origin, &tabs_at_attach, body)` (`rust/crates/station-shell/src/webview.rs:885-890`), with
  the tab captured at attach (`:877-880`).
- **Page code execution by the shell** — only the click injection (`rust/crates/station-shell/src/webview.rs:495-498`). There is
  **no general `eval`** in the `WebviewHost` trait (only `click`, `rust/crates/station-shell/src/webview.rs:111`), so an SDK-style
  "execute code in a tab" surface does not exist natively.
- **Not covered:** page titles (top bar uses `tab.label`), badges (rail draws none), the
  `Notification` override, the meta observer, snooze listeners, autologin, print, chrome shims,
  extensions, alert dialogs.

---

## 4. Port plan — the smallest honest shape

### 4.1 Split the inventory along two axes

**Page-facing API** (what the port must expose as `window.bxApi…`): only three namespaces have a
consumer that survives the Electron tree:

| Member | Needed for | Decision |
| --- | --- | --- |
| `notificationCenter.{sendNotification, closeNotification, addNotificationClickListener, removeNotificationClickListener}` | the `Notification` override (#12) and site scripts (WhatsApp/Messenger snooze; Slack) | **keep** (already native) |
| `notificationCenter.addSnoozeDurationInMsChangeListener` | WhatsApp/Messenger audio muting | **add** (S; the shell knows the snooze state) |
| `theme.addThemeColorsChangeListener` | nothing but the store page (Rune-drawn now) and the configurator (not served) | **drop** (reason: its two consumers are both Electron-page surfaces) |
| `identities.*` | configurator only | **drop the page API**; the Settings sign-in row is the native surface (OAuth Option B). `requestLogin` from a *page* would be a phishing surface |
| `manifest.getManifest` | configurator only | **drop** (server-side fetch driven by a page URL) |
| `applications.*` (10) | store page + configurator only; the Rune shell draws the store and owns install/uninstall/config via `appstore-service` | **drop from the page API.** A hostile page could install/uninstall apps today; the native shell should not carry that surface forward |

**Integration** (how the API reaches the shell): the native shell has no napi and no Node. Pages get
`window.ipc.postMessage` (wry's IPC) and the injected script translates: one JSON `kind` per member,
one Rust parser per kind (`notifications::receive`'s shape). Do **not** port the
`bx-api-perform`/`bx-api-subscribe` round trip or the worker hop — `docs/rust-napi-bridge.md:29-38`
describes that plan for an Electron host; with Electron gone the shell is the responder.

### 4.2 Subset for the five most common apps

| App | Needs (in order) | Optional |
| --- | --- | --- |
| **Gmail** | #12 Notification override; `gmailInjectedScript` badge + observer; #3 `chrome.webstore` | `_docs_*` (GDrive only), print |
| **Slack** | #12 override; `slackInjectedScript` badge + observer; snooze listener | Slack service runtime (`slack/main`: activity/history/search/snooze-sync) — **product decision**, L; the API-token meta is dead (see below) |
| **WhatsApp** | #12 override; `whatsappInjectedScript` + snooze listener (audio muting) | — |
| **Messenger** | #12 override; `messengerInjectedScript` + snooze listener | — |
| **Notion** | #12 override only (no injected script in the registry) | Notion service runtime (`notion/main`) — same decision as Slack |

**Dead in the Electron tree already — port nothing:** the Slack API-token harvest. The site script
writes `meta[browserx-apiToken]` (`slackInjectedScript.js:16,40,74,138`) and
`Application.tsx:292-296` has `handleSlackApiTokenUpdate`, but **nothing calls it** and nothing
subscribes to `SlackSDKProvider`'s `api-token-request` (`packages/app/src/plugins/slack/SlackSDKProvider.ts:10-21`,
`SlackSDKConsumer.ts:18-24`) — no caller of `getAPITokens` exists.

**Drop with reasons:** `applications.*` (store is Rune-drawn; page-driven installs are an
unnecessary attack surface), `theme`/`identities`/`manifest` page members (configurator-only), #2
page-click (Electron `<webview>` bubbling workaround; the Rune shell does not receive page clicks and
hides pages behind panels instead), #5 print (no print surface natively; WebView2's own Ctrl+P
remains), #7 window-open proxy (native handler exists), #8 Cmd+T (macOS), #9 global cleanup (no
Electron leaks to hide), #13 autologin + `autofill` (no credential store natively; also a
phishing-shaped API: any page can present a "login form"), #10 alert override (WebView2's own modal
is acceptable), `packages/app/src/webui/preload.js` (no live behaviour), `packages/app/src/static/preload/dev-preload.js` (dev-only).

### 4.3 Slices

1. **`Notification` override** (S) — §6. **Executed (slice 19).**
2. **Badge path** (M): port #1's meta observer + the site scripts for Gmail/Outlook/Messenger/Slack
   as *data* (a Rust-side table of script bodies, injected per manifest), add `page-bxmetas-updated`,
   draw the badge on the rail row (`rust/crates/station-shell/src/rail.rs` + `tab-model::Badge`).
   **Partly executed (slice 20):** the meta observer, the `page-bxmetas-updated` payload (as
   `kind:"meta"`), the `document.title` path (as `kind:"title"`), and the rail badge are done; the
   per-site script data table is the remaining half.
3. **Snooze listeners** (S): add `addSnoozeDurationInMsChangeListener` to `INIT_SCRIPT`, emit on
   snooze changes (shell → page), then port WhatsApp/Messenger scripts.
4. **Chrome shims** (S): `chrome.webstore`, the `runtime` stub; `_docs_*` only if GDrive is kept.
5. **Per-site script host** (M): a manifest-driven table instead of the TS registry, one injected
   string per page, capped size, no `executeJavaScript` on arbitrary input.

---

## 5. Security rules the port must keep

The native bridge is already the strictest part of the shell; anything added must keep its shape.

**Current guards (`rust/crates/station-shell/src/notifications/bridge.rs`):**

| Guard | Where | Value |
| --- | --- | --- |
| Sender identity | `rust/crates/station-shell/src/notifications/bridge.rs:103` (`Sender::App{tab_id, application_id}` / `Unknown`), decided in `receive` (`:196-202`) | only an installed app's tab; unknown tab ids and the Rune-drawn store tab are refused |
| Origin capture | `rust/crates/station-shell/src/webview.rs:877-880` (`ipc_origin` = tab id at attach; `tabs_at_attach` snapshot) | a message can never name another tab |
| Field caps | `rust/crates/station-shell/src/notifications/bridge.rs:79-88` | id 256, title 512, body 4096, icon URL 2048, message 32 KiB |
| Counts | `rust/crates/station-shell/src/notifications/bridge.rs:91-95` | 64 live per source, 256 total, 128 pending between frames |
| Click listeners | `rust/crates/station-shell/src/notifications/bridge.rs:76`, page-side `:60-63` | 8 per page; each listener `try/catch`ed |
| No shell execution | whole module | page strings are compared/parsed, never evaluated; click delivery is a fixed template with a JSON-encoded id (`rust/crates/station-shell/src/webview.rs:495-498`) |
| Frozen surface | `rust/crates/station-shell/src/notifications/bridge.rs:69` | `Object.freeze` on `bxApi` and its namespace |

**Must be added for the port:**

- **The `Notification` override must clamp what the page can send** — a page-supplied `id` bounded
  (it can collide with another page's ids: ids are namespaced per tab today, keep that), `body` is
  drawn as Rune text (no HTML/markdown), `icon` stays a URL passed through the existing icon
  fetcher (http/https only, 1 MiB, 10 s, 8192 px — `rust/crates/station-shell/src/icons.rs` caps per `rust/README.md`).
- **Never expose `applications.*`** (§4.1) — the single biggest reduction: today any page can
  `bxApi.applications.install({manifestURL})`/`uninstall(id)`.
- **Any future `manifest.getManifest`-like fetch** must be a host-allowlisted server fetch with the
  appstore-service caps (`PrivateStore` 8 MiB/10k, manifest fetch limits), never a page-chosen
  arbitrary host (SSRF).
- **Snooze/theme subscriptions** need a per-page subscriber cap (the click-listener cap is the
  precedent) and a single shell→page fan-out (one `webview.eval`-equivalent per subscriber, not per
  frame).
- **Site scripts run in the page's world by necessity** (they must touch the page's DOM and
  `Notification`) — so they must be **static, reviewed strings shipped in the binary**, selected by
  manifest id, never built from page input (this replaces `raw-loader`'s build-time bundle with a
  `&'static str` table).
- **No new OS side effects from page input**: the only page-triggered OS action today is the
  routed URL (allowlisted schemes + caps). Keep it that way; the `Notification` override adds no
  new side effect (the panel is in-process).

---

## 6. First slice (recommended): `window.Notification` → the panel

**Status: executed (slice 19)** — the override is part of `INIT_SCRIPT` in
`rust/crates/station-shell/src/notifications/bridge.rs`; the slice entry in
`docs/rust-rune-port.md` records the two deliberate drops (the `blob:` icon path and the recursive
iframe override) and the tests.

**Why this one:** it is the smallest slice that flips a *real, visible* behaviour (Gmail/Slack/
WhatsApp notifications appear in the Rune panel instead of nothing), it needs **no new Rust state**
(the panel, click routing, snooze caps all exist), and it converts the shell's four already-injected
members from "unused" into "used by every page that raises a notification".

**Shape (one file + tests):**

- `rust/crates/station-shell/src/notifications/bridge.rs` — extend `INIT_SCRIPT` with the port of
  `webview-inject.js:77-236`, minus the dead parts:
  - `class BxNotification` with the same constructor contract (`title` required, options merged,
    `id = 'notif/' + random`, `timestamp`), `close()`, `permission = 'granted'`,
    `requestPermission`, click listener registration through the existing click hook;
  - `window.Notification = BxNotification` at initialization-script time (this is *better* than
    Electron's `dom-ready` `executeJavaScript`: the page's own code sees the override from its first
    line);
  - icon normalisation (`//` → `https:`, non-string → `undefined`); **omit** the `blob:` XHR path for
    the slice (Telegram only; add later with a size cap);
  - recursive same-origin iframe override **omitted** in slice 1 (iframe policy is a decision: it
    lets a page raise notifications from embedded content).
- Tests: `rust/crates/station-shell/src/tests.rs` / an existing `tests_notify_*.rs` pattern — assert
  the script text defines `Notification`, installs exactly one listener per instance, and that
  `sendNotification` still routes through the capped parser (no new IPC `kind`).
- **Human verification:** windowed probe — install an app whose `start_url` is a local page calling
  `new Notification('hello')`; expect a panel row grouped under that app; click it → the page gets
  the click. (Headless tests cannot execute the page JS; this is the honest split.)

**Verification commands:** `cd rust && cargo fmt --all --check && cargo test -p station-shell &&
cargo clippy -p station-shell --all-targets -- -D warnings`; then the windowed probe above.

**Second slice**, once this lands: the badge meta observer + Gmail/Outlook/Messenger/Slack site
scripts + rail badge (M) — the next real behaviour, and the one the five-app matrix needs most.
**Status: the observer/title/badge half executed (slice 20);** the per-site scripts remain.

---

## 7. UNVERIFIED

- **Whether Electron's `dom-ready` execution of `webview-inject.js` misses early notifications**
  (a page that raises one before `dom-ready`). The timing is visible in source
  (`Application.tsx:352-364`); the consequence was not measured.
- **Whether WebView2 shows its own notification for `new Notification()` when no permission prompt
  exists** (i.e. what the user sees *today* in the native shell). Not observed; the probe only
  covered page rendering (`docs/webview2-probe.md`).
- **Whether `with_initialization_script` runs before *all* page scripts** including inline `<script>`
  in `<head>`. Standard behaviour for WebView2's `AddScriptToExecuteOnDocumentCreated`, but not
  exercised in this repo.
- **Whether any page relies on `chrome.runtime`/`chrome.webstore` beyond the two comments' claims**
  (GDrive, Qonto). Only the source comments assert it.
- **Which pages actually call `bxApi.applications.*`** (only the store/configurator, by reading;
  no runtime instrumentation).
- **The real-world need for `alert` interception, `print`, and iframe notification override** in the
  five apps — inferred from source, not observed in a native run.
- **`page-click` value to the Rune shell**: the Electron workaround exists for React click-outside
  inside the *Electron* renderer; the Rune shell's panels are separate surfaces, so it is assumed
  unnecessary. Not measured.
- **Slack service-runtime behaviour** (`slack/main`: activity/history/search) — read at a glance
  (`packages/app/manifests/runtime/slack/main.ts:14-35`); its consumers (`station-sdk` traits) have
  no native transport today, so the port's scope is unmeasured.
