# Station's Rune shell — ADR

Status: accepted for the `station-shell` foundation slice (2026-11-18).
Scope: `rust/crates/station-shell` and the workspace's Rune dependency. Nothing in this document
claims the Electron shell is gone; it records how the native shell replaces it, one slice at a
time, per `docs/rust-rewrite-plan.md`.

## Context

`station-shell` was a `println!` stub. Station's shell is a left rail of apps, a top bar naming the
active app, and a content area per app. The Electron shell renders that chrome in HTML and puts
each app in a `<webview>` tag. The native port has to keep that shape while the UI layer moves to
Rune (pinned `10a39584cc1ff739477be52e333a742a024ac956`).

## Decisions

### D1 — Rune is the shell UI, pinned by rev

`rune = { git = "https://github.com/anomalyco/rune", rev = "10a39584cc1ff739477be52e333a742a024ac956" }`
in `rust/crates/station-shell/Cargo.toml`.

Reasons: it renders its own GPU UI (no embedded browser for chrome), it exposes a headless
`Runtime` that drives a real view through pointer input and an element tree, and it puts colour,
type, spacing and motion behind tokens rather than in the view. Those two properties are why this
slice can be tested at all — `cargo test -p station-shell` needs no window (D3).

### D2 — Rune's five patched crates are vendored into `rust/vendor`

Rune's `[patch.crates-io]` (`Cargo.toml` of the pinned rev) does not apply to a workspace that
*consumes* it: Cargo reads patches only from the dependent workspace root. Its own note is explicit
— "apps that build Rune patch it too". Resolution fails without `parley_engine`'s patched
`line-dictionaries` feature, and Windows compilation needs `wgpu-hal`'s
`dx12::Instance::expose_adapter` / `Surface::set_present_rects` and `winit`'s
`platform::windows::warm_keyboard_layout`.

So `rust/vendor/{winit,wgpu-hal,parley_engine,accesskit_windows,accesskit_macos}` are verbatim
copies of Rune's `vendor/` at the pinned rev (crate sources plus the `.patch` deltas beside them),
and `rust/Cargo.toml` carries the matching `[patch.crates-io]` and `exclude` list. 5.7 MB of
third-party source is the cost of a reproducible build on the pinned rev; the alternative
(patching by machine-local path into Cargo's git checkout) is not shippable.

`tiny-skia-path` needs no action: Rune depends on it by relative path inside its own checkout.

### D3 — Module seams: pure state, thin views, one theme resolution point

| Module | Interface | Depth / why it is a module | Locality |
| --- | --- | --- | --- |
| `state::ShellState` | `seeded()`, `new(ApplicationService)`, `tabs()`, `active()`, `select(id)`, `install(url)`, `uninstall(id)`, `activate_tile(&MinimalApplication)`, `section()/show_section`, `search()/search_mut()/query()/refresh_search()/results()`, `popular()/categories()/custom()` | Owns the installed-application service and the store's cached lists (`PopularApps`, categories, private manifests), so views never query. Not `Clone`: `ApplicationService` is the authority on what is installed, and copies would drift. Tests build one over `ApplicationService::for_tests`, so nothing touches the user profile. | All reads/writes of "which app" and "which store list" are here; a tile, a rail row and the search field each call one method. |
| `rail::rail(&ShellState, Paint) -> El` | one function; `RAIL_WIDTH` | Adapter from state to rows: each row knows its key, its accessible name, and its press. Draws the two bottom items and the "+" affordance without knowing the pane exists. | The write for a row lives in that row's press closure (`ShellState::select`), not in a router. |
| `placeholder::top_bar/content(&ShellState, Paint) -> El` | two functions | The top bar, and the placeholder a web tab's page covers. `content` — the element whose presented box the web view is placed on — is the root of whichever pane body runs (this placeholder or the store), so one key means one pane geometry. | Pane layout in one file; the web view borrows `content`'s box instead of the pane measuring itself. |
| `appstore::view(&ShellState, Paint) -> El` | one function plus private builders | The Rune-drawn App Store: a 220 pt sidebar (name, search field, DISCOVER sections) beside the selected list (three buckets, categories, private apps, or search results). Tiles are a 4-column grid of letter avatars tinted from `theme_color`. Keys (`store-section-*`, `store-search`, `store-tile-<manifest id>`, `store-group-*`) are the whole test surface. | One place reads the store's cached lists; the only host call in the module is `webview::close_tab` after a successful uninstall. |
| `shell::Paint { t: Theme, dark }` | `Paint::of(dark)` | The token bundle every view below the composition root takes. `tok::Theme` keeps only colours (no `dark` flag), while the accent and the accent's foreground need it, so the appearance travels with its tokens. | One struct instead of threading `(Theme, bool)` through every function. |
| `shell::view(&ShellState, &mut Cx<ShellState>) -> El` | Rune's `AppSpec::view` shape | The composition root and the *only* place appearance is resolved: `tok::theme(cx.system_dark())`. Every child takes `Paint`, so no module can hardcode a palette. | One call site to change if appearance becomes a setting. |
| `shell::view_themed(&ShellState, dark: bool) -> El` | pure | The seam the tests (and any future snapshot tooling) use when there is no `Cx`. Deliberately a one-line alias of the real view, not a parallel implementation. | — |
| `webview::WebviewHost` + `PaneRect` | `attach(tab, url, rect)`, `set_bounds(tab, rect)`, `set_visible(tab, on)`, `detach(tab)`; `PaneRect::pixels(scale)` | Trait over the native child web view so the shell's layout never depends on wry. `sync(&ShellState, PaneRect)` turns state + geometry into the minimum set of host calls (a steady window asks for nothing), and keeps the attach/bounds/visibility bookkeeping; `close_all()` drops the host. The `fake` module (cfg(test)) records every call, so the seam is tested without a window. | Owns every non-`Send` platform handle (thread-local), keeping `ShellState` pure. |

Leverage: `El` construction is pure and cheap, so the whole shell is a function of `ShellState`;
that is what makes `Runtime` in a test the same code path as `rune::run` in the window.

### D4 — App content is an OS child webview over the Rune window, not a Rune element

Rune has no element that hosts a foreign native window. `NativeSurface` (`crates/rune/src/native_surface/mod.rs`)
accepts GPU frames — CPU byte frames, an IOSurface-backed pixel buffer, or a D3D12 shared texture
(`native_surface/windows.rs`) — not an `HWND`. Its own research is explicit that native views are
limited to underlay/overlay slots, not interleaved compositing
(`research/11-platform-shells.md`:15,368).

What Rune *does* expose is the native window handle:

- `App::on_window(FnMut(&WindowHandle))` — `crates/rune/src/app.rs:525`; `WindowSpec::on_created`
  (`crates/rune/src/window/spec.rs:232,299`) — both run after native creation, on the event thread.
- `WindowHandle::with_raw_handle(f)` — `crates/rune/src/window/handle.rs:178` — borrows winit's
  `raw_window_handle::WindowHandle` for the callback.
- `pub use winit::raw_window_handle` — `crates/rune/src/lib.rs:390`, so the handle types the host
  needs come from `rune`, not a second `raw-window-handle` dependency.

wry 0.57.2 accepts exactly that: `WebViewBuilder::build_as_child(&parent)` takes
`impl HasWindowHandle` and documents child webviews on Windows, macOS and Linux/X11
(`wry/src/lib.rs`:76–99, 1551–1576). On Windows it reads the parent's
`RawWindowHandle::Win32` and creates a container `HWND` child of it
(`wry/src/webview2/mod.rs`:102–113). `bounds` applies to child webviews and
`WebView::set_bounds` moves them later (`wry/src/lib.rs`:777, 2266).

**Integration point (implemented, slice 3):** `webview::window_created` is `App::on_window`: it
reads the raw handle with `WindowHandle::with_raw_handle(|h| h.as_raw())` and stores it for the
thread. On the first drawn frame `webview::after_frame` builds `WryWebview` with that handle and
`Runtime::env.scale`, then `sync` calls `build_as_child` on a thin `HasWindowHandle` adapter around
the `RawWindowHandle` (`WindowHandle::borrow_raw` is `unsafe` — rwh 0.6.2 `src/borrowed.rs`:247 — so
the adapter's safety argument is "a live Rune window, used only inside wry's call"). Wry is 0.57.2
(WebView2 `webview2-com` 0.39.1). The adapter is the only place a raw handle exists; everything
above it deals in `PaneRect` points.

The pane's rectangle comes from Rune, not from constants: `pane_rect` reads the presented box of
the `content` element (`Runtime::presented`, no per-frame `Vec`), and `PaneRect::pixels(scale)`
converts to device pixels. Observed on this machine (scale 1.0): the Rune window's client is
1200×800; the WebView2 container `WRY_WEBVIEW` was created as a child of the Rune `HWND` at
window-relative (64, 44) sized 1136×756 — exactly `RAIL_WIDTH` inward and `TOP_BAR_HEIGHT` down from
the content corner, with the rail and top bar left to Rune.

Z-order is the constraint that makes this a *child-window* design rather than a Rune element:
Rune's DirectComposition target is created below the window's child windows
(`crates/rune/src/platform/win32/dcomp_handover.rs`:29–35, `CreateTargetForHwnd(hwnd, false)`), so a
child `HWND` composites above everything Rune draws. Rune can therefore not paint over the webview;
Rune-drawn popovers/dialogs that must cover app content have to be separate always-on-top windows
(Rune already has overlay windows) or be drawn *outside* the webview's rect.

No patch to Rune is required for this on Windows. The missing convenience is only `HasWindowHandle`
on `WindowHandle`/a `Runtime` method returning a borrowed parent handle per window — an
upstream-compatible addition (`impl HasWindowHandle for WindowHandle` is impossible because the
handle is bound to a closure's lifetime; the shippable upstream API is
`WindowHandle::with_raw_handle`, which exists today).

### D5 — State stays testable: no webview handles in `ShellState`

`wry::WebView` is neither `Send` nor `Clone`, and `ShellState` must stay `Clone + PartialEq` to be
constructed in headless tests. So the registry lives outside it: a single-threaded store
(`thread_local!` `ENGINE`) holding the host, the per-tab bounds, the visible tab and the tabs whose
attach failed, all behind the `WebviewHost` trait (D3). Tests install the recording fake; the window
gets the wry adapter on its first frame. `close_all` detaches every page and drops the host, so a
frame still in flight cannot attach to a window that is gone. This is the same reason the shell's
views take `&ShellState` and not a handle to anything native.

`ShellState` therefore still has no knowledge of web views: the frame hook diffs the *active tab*
each drawn frame, so a selection change made by a rail press needs no callback into the host.

### D6 — The App Store is a Rune view; only HTTP(S) apps get a page

`ShellState` now owns `appstore_service::ApplicationService` plus the store's cached lists, and the
rail is the App Store followed by installed applications in install order. A tab is Rune-drawn when
`TabKind::Store` **or** its URL is `station://appstore` (`AppTab::renders_in_rune`), and
`webview::sync` never attaches or shows a page for one; a page that was visible is hidden. That is
the whole rule — the host has no list of "special" URLs.

Things the implementation taught, worth keeping:

- **Installed Gmail lands on `station://multi-instance-configurator/…`**, not `mail.google.com`:
  Gmail is a `google-account` preset app and the install has no identity/config data, so
  `start_url_for_install` returns the configurator. A plain manifest (Todoist) lands on its literal
  `start_url`. Nothing in this shell serves the configurator yet, so Gmail's page is blank — the
  store and rail are still correct. Recorded in the test rather than papered over.
- **Removed elements stay in `Runtime::elements()` for a few frames** while they exit (Rune keeps
  them listed on purpose). Tests that read "what the user sees" must skip `ElementInfo::exiting`;
  without that, a switched section still "shows" the previous section's header and an uninstalled
  tile still "shows" its Installed marker.
- **A text field needs `El::label`**, not only a placeholder: Rune's debug build warns "a focusable
  text field has no name a screen reader can say" and the store's search box had none until
  `.label("Search an app")` was added.
- `tok::Theme` carries no `dark` flag (`Paint` does), `ScrollSpec` has no `Default`
  (`ScrollSpec { stick_bottom: false }`), and `ScrollSpec`'s viewport needs `.min_h(0.0)` on every
  flex column between it and the bounded pane.
- **Avatar tint** is the manifest's `theme_color` through `rune::color::hex`, else
  `tok::accent(id-derived index, dark)`; the letter's ink is picked by the tint's luminance from the
  appearance's own foregrounds. No literal palette anywhere in the store.
- **A tile press is the only store action that touches the host**: `activate_tile` returns
  `TileAction::Uninstalled { application_id }` and the closure calls `webview::close_tab`, keeping
  "state holds no host" (D5) true while still guaranteeing uninstall closes the page.
- **Pressing an installed tile uninstalls it.** Station's own store would open the app; this slice
  needs a reachable uninstall (the settings surface is slice 6) and a tile that does nothing would
  be a stub. Slice 6 flips it to "open" and moves removal to Settings.

### D7 — The session is a file the shell only writes after the user acts

`session-store`'s JSON document (`version: 1`) is the on-disk truth: `window`, `activeApplication`,
`railOrder`, `installed`, `dock`, `tabs`. `ShellState::launch` loads it over the user's private
manifest store, replays it with `ApplicationService::restore`, rebuilds the rail and restores the
active application; a missing file is simply a fresh shell. `railOrder` is the ordering authority
when it names the docked set exactly (D11); any other shape is refused and the rail falls back to
install order.

- **Path.** `STATION_SESSION_PATH` when set (tests and smoke runs); otherwise
  `<dirs::config_dir()>/station/session.json`. `dirs` is the cross-platform config dir —
  `%APPDATA%` on Windows, `$XDG_CONFIG_HOME` or `~/.config` on Linux, `Application Support` on
  macOS — the same convention `manifest-registry` hand-rolls for its private manifests. The
  session and the private manifests stay separate files: one is replaced wholesale, the other is
  the user's data.
- **Whole or nothing.** `ApplicationService::restore(installed, dock, tabs)` validates before it
  mutates: unique application ids, unique dock entries, dock ⊆ installed, unique tab ids, tab
  application ⊆ installed. `RestoreError` names the offending id. A refused session leaves the
  service exactly as it was, and the shell starts with the App Store alone.
- **Never overwrite unread.** A parse error, an unsupported `version` or a refused replay prints
  one stderr line and blocks saving (`save_blocked`): the file is left byte-for-byte as it was
  until the user acts — the next `select`/`install`/`uninstall` clears the block and writes a
  valid session. `on_quit` and `on_close` do not clear it.
- **Atomicity.** Every write is `session-store`'s: a sibling temp file, `sync_all`, then a rename
  over the document. A failed save leaves the previous session in place, prints where it failed,
  and the shell keeps running.
- **When.** Immediately on `install`, `uninstall` and any selection change (a rail click, the "+"
  affordance, a store tile), plus on `on_close` and `on_quit`.
- **Window placement.** The size opens in `AppSpec` (points), `App::maximized` takes the flag, and
  `on_window` applies the stored position with `WindowHandle::set_position`. Saving reads the live
  window's placement (`WindowHandle::placement`) and, while maximized, stores the frame the window
  restores to (`WindowPlacement::restore`) with `maximized` set — a maximized content size belongs
  to the display it was measured on. **Finding:** on the last-window-close path Rune reports no
  placement in `on_quit` (the window's host is already gone), but does in `on_close`; so a save
  captures the live placement when it can and otherwise keeps the last captured one. `on_quit`
  still saves, because `AppHandle::quit` closes windows *without* `on_close`.
- **Not persisted** (deliberately, this slice): the store's section and search text, scroll
  offsets, each tab's navigation history, snooze, and the pages themselves — a web view is
  recreated from its URL on the next activation. Private manifests are already on disk
  (`manifest-registry`), so they are not repeated in the session.

### D8 — Settings and notifications replace the page area; they cannot cover it

Rune draws chrome, but an app's page is a native child window above everything Rune paints
(D4/D6). So neither Settings nor the notification panel is a modal: `ShellState.surface`
(`Apps`/`Settings`/`Notifications`) decides what the content area shows, and `shows_page()` is the
single predicate `webview::sync` follows — when it is false the page is **hidden**
(`set_visible(false)`), not detached, so closing the surface shows it again with no re-attach and
no reload.

- **Settings** fills the content area: a Back button plus title, a General card (Appearance
  segments, `CARGO_PKG_VERSION`, the session path — or "Not saving" when the shell has no file) and
  a My apps card (Open, Uninstall). Escape closes it through `AppSpec::on_key` (keys no control
  took); the back button and Open/Uninstall act through state. Uninstall stays in Settings — one
  visit can remove several — and calls `webview::close_tab` after `uninstall` succeeds.
- **Appearance** is `session_store::ThemePreference` (`System`/`Dark`/`Light`, default `System`)
  stored in the session's `theme` field. The format stays **v1**: the field is additive with
  `#[serde(default)]`, so a pre-existing document opens as `System` and an older build ignores it —
  no migration for a field that has a correct default. `shell::view` maps it to `Paint`
  (`System` → `cx.system_dark()`), which is why the choice repaints the whole window.
- **The notification panel** is a 320 pt column on the right of the content area (a row, not an
  absolute overlay: the page is hidden anyway, and the store keeps its own scroll). It shows the
  unread count, Mark all read (only while there are rows), the center's groups with each row's
  title, body and age ("just now"/"5m"/"3h"/"2d"), an empty state, and the snooze footer — Off,
  15 min, 1 hour, Sync with OS, Infinite, two rows because five pills do not fit 320 pt. Snooze
  goes through the center's own actions with the shell's wall clock (`SystemTime`); the center
  owns no clock. Neither the panel's open state nor the notifications are persisted.
- **Slice 8 gave the center a page source**: `webview`'s ipc handler queues validated
  `bxApi.notificationCenter` messages and the poll delivers them (`notifications::bridge`), so a
  running shell's panel fills from its pages. Still no OS source; the remaining limits are the
  slice-8 entries in the Robustness table.
- **Preset apps now open a page**: `ApplicationService::resolve_start_url(manifest_url,
  application_id, config)` exposes the install-time decision, and `start_url_for_install` no
  longer falls straight to `station://multi-instance-configurator/` when a preset app has no
  identity/config — it returns the manifest's own formatted start URL (Gmail →
  `https://accounts.google.com/AddSession?…`, the signed-out account picker). This is a deliberate
  divergence from the TS saga, recorded here: the native shell does not draw the configurator yet.
  A raw config the presets do not use (`resolve_start_url` takes the caller's config verbatim)
  still yields the configurator, which is the path OAuth will drive.
- **Windowed evidence** — measured (`docs/surface-hide-probe.md`, two launches of the release
  binary with a seeded Gmail session, presses posted to the real window): the rail's App Store row
  at client `(32,32)` flipped `IsWindowVisible(WRY_WEBVIEW)` from true to **false** with the
  process alive and the same HWND/rect; the Gmail row at `(32,80)` flipped it **true** again with
  the same `WRY_WEBVIEW` HWND and the same `Chrome_WidgetWin_1` HWND/title ("Sign in - Google
  Accounts"). Settings at `(32,720)` hid it and `VK_ESCAPE` restored it; the notification moon at
  `(32,768)` hid it, Escape restored it, and a second press restored it by toggle. One
  `WRY_WEBVIEW` HWND per run throughout — no second page or browser tree — and Gmail's page
  navigates to `accounts.google.com` rather than blank. Not pixels (no screenshot), and
  DOM/history/scroll survival is not observable from handles.

### D9 — A page's new window is routed, never a second window

WebView2 raises `NewWindowRequested` for `window.open` and `target=_blank`. The shell answers with
`NewWindowResponse::Deny` (wry 0.57's `with_new_window_req_handler`) and queues `{ origin_tab, url }`
in `routing`; the handler also calls Rune's `Waker` (captured in `AppSpec::setup`), because the
click happened inside the page's own window and Rune would otherwise stay idle. `AppSpec::poll`
drains the queue (`webview::poll` → `route_requests`) — poll is the one hook that gets `&mut
ShellState`, which a routed request needs to select a tab.

`routing::decide(url, apps, tabs, origin_tab)` is the whole decision, in `url-router`'s shape, with
no platform types: `Focus`/`FocusAndLoad`/`Load`/`External`/`Ignore`.

- `FocusTab`/`Reload` → show that app's tab.
- `PushAndFocusTab`, and `OpenInTab` with a tab → show that tab **and** load the URL in its page
  (`WebView::load_url`); the tab's own URL is retargeted first, so a page that is not open yet
  attaches at the routed URL on the next frame (its old home URL is not flashed).
- `NewTab`/`NewInstance` → the shell has one tab per app, so the app's existing tab navigates and
  comes forward.
- `InstallAndOpen` (the app is not installed) and anything unmatched → the OS default browser
  (`ShellExecuteW` on Windows — one string, no command interpreter — `open`, `xdg-open`); never a
  second WebView2, and only `http`/`https`/`mailto` reach it. The shell does not install from a
  link.

A tab id that is not ours focuses nothing (`Ignore`) rather than opening a browser for a URL a page
we own already has.

### D10 — Per-app configuration is the service's stored config, edited in Settings

`ApplicationService` gains `set_config_data` / `clear_config_data` (port of `SET_CONFIG_DATA` +
`setIdentityIfMultiInstanceApplication`): the incoming fields merge over the stored ones, then the
preset gate keeps only what the manifest's first matching preset consumes (google-account →
`identityId`, subdomain → `subdomain`, on-premise → `customURL`, in that order). The stored config
in `InstalledApplication.config_data` is the one source of truth; the session's `appConfig` map is
its on-disk projection (`ShellState::session_state` derives it, `launch` replays it through
`restore`).

- **Field set.** Exactly the three `ApplicationConfigData` fields (`identityId`, `subdomain`,
  `customURL` — `packages/app/src/applications/duck.ts`). Nothing invented: the configurator's
  other inputs (the identity chooser, the normal flow's "use the original app") are behaviors, not
  fields.
- **Format stays v1.** `appConfig` is additive with `#[serde(default)]` — the `theme` precedent.
  Absent entry = "not configured" (a preset app opens its signed-out start URL, the slice-6
  divergence). An entry with every field empty would be the TS normal flow
  (`setConfigData(applicationId, {})`); the shell cannot produce one (its Clear removes the entry),
  but the document accepts and preserves one a hand-written or future document carries.
- **Validation, refused not truncated.** `sanitize` refuses (a) a value past **2 048** chars or
  whitespace-only, (b) a key that names no installed application — each naming the offending app
  id in `StoreError::BadConfig`. No count cap is needed: entries are keyed by installed
  applications, already capped at 512. `save` validates too, so a state this build would refuse to
  load is never written. `ShellState::set_app_config` checks the cap *before* trimming, so a
  hostile value from the editor is refused whole and nothing changes — in memory or on disk.
- **Applied to the URL.** `rebuild_tabs` resolves every rail tab's URL through
  `resolve_start_url(manifest, id, config)` — the same decision install makes — so a saved config
  moves the tab on the next frame and a relaunch reopens the configured URL. A tab whose page is
  attached is navigated now (`webview::load_if_attached`, the `retarget` path); one that is not
  attaches there. A manifest that disappeared keeps the record's home tab URL (and its id as the
  label); nothing crashes.
- **Settings editor.** My apps gains a Configure control per preset app (presets come from
  `get_presets`; an app without presets offers nothing). The editor shows the one field the
  manifest's presets require — Domain (`subdomain`, placeholder from the manifest's own wording)
  or Server URL (`customURL`) — prefilled, with Save, Clear and Cancel. A google-account preset is
  shown honestly: "Sign-in is not available in the native shell yet." — no field, because the shell
  cannot obtain an identity (no OAuth), and a string field would collect a value nothing can use.
  This is a deliberate, recorded divergence: TS drives `bxApi.identities.requestLogin('google')`
  and stores the identity id.
- **When written.** Save and Clear are user actions: the session is written immediately (D7), and
  Clear is idempotent (a second one reports nothing to clear and writes nothing).

### D11 — The rail order is the user's, reordered from Settings' My apps card

`railOrder` is the ordering authority: the rail draws the App Store followed by the docked
applications in `rail_order` when it names the docked set exactly; `dock` stays membership only.
Install appends to the user's order, uninstall removes the entry (`reconcile_rail_order` is the
one membership writer), and any other shape falls back to install order — healed on the next
write, since a document whose order does not name the dock exactly is refused whole by
`session-store` (unknown id, duplicate, over cap).

- **Why Settings, not the rail.** The first cut put Alt+Up / Alt+Down on the rail row itself.
  Rune gives a plain rail row no keyboard focus, so the row's `on_key` never fires — the
  affordance was untestable and would not have worked in the window either. Reordering moved to
  two small pressable controls per My apps row (`settings-move-up-<id>` /
  `settings-move-down-<id>`), which are ordinary buttons: a press calls
  `ShellState::move_rail_row` and the session is written at once (the D7 persist path every
  Settings control shares). A control is absent — never present-but-disabled — where the move
  cannot happen: the first movable app has no up, the last no down, and the App Store row is
  always first and never movable. Each control's accessible name says what it does and to whom
  ("Move Gmail up"). Rail rows keep press-to-select; reordering never fights selection.

## Robustness

Limits and policies added by the hardening pass (2026-11-18), each covered by a test.

| Concern | Limit / policy |
| --- | --- |
| Page notification messages (`notifications::bridge`) | A page may send at most **128** messages between frames (`MAX_PENDING`); beyond it new ones are refused, never deferred. One IPC message is at most **32 KiB** (`MAX_MESSAGE_BYTES`); a `title` **512** chars, a `body` **4 096**, an `icon` URL **2 048**, an id **256**. Non-JSON, non-object, unknown-`kind` or wrongly-typed messages are refused (`Malformed`), never truncated and never a panic. Only an installed app's web tab may send (`Sender::Unknown` refuses the store's Rune-drawn tab and any id that is not a rail tab). |
| Live notifications (`notifications::bridge::deliver_to`) | **64** live notifications per source app (`MAX_PER_SOURCE`), **256** total (`MAX_TOTAL`). A source at its cap has its new notifications dropped until it closes some or the user marks them read. A resend of an id the page already used replaces the payload (the TS `ADD_NOTIFICATION` semantics), never grows the count. |
| Page click delivery (`webview::click`, `WebviewHost::click`) | One message per row press, to the sender's tab only. The id is page-controlled (≤ **256** chars from the bridge) and is delivered as a **JSON string literal** built with `serde_json::to_string` inside `window.__stationNotificationClick(<literal>)` — quotes, backslashes, newlines and `</script>` cannot break out; a serialization failure refuses the message. An unattached page, a missing host or a failed evaluation is dropped silently (no retry, no second message). A page may register at most **8** click listeners (`MAX_CLICK_LISTENERS`); a ninth is refused, a removed one is no longer called, and each call is wrapped in `try`/`catch` so a throwing listener cannot stop the next. |
| Snooze expiry (`notifications::expire_snooze`) | The frame poll scans at most once per **1 s** of frame clock (`THROTTLE_S`); the deadline is wall-clock stamped (`SetSnoozeDuration`) plus the duration. `Snooze::SyncWithOs` has **no reader** (no OS do-not-disturb state is polled) and `Snooze::Infinite` never ends: neither expires and neither is claimed to. |
| New-window request queue (`routing`) | **64** requests; a duplicate `(origin, url)` already queued is dropped; a full queue refuses new ones. A page's `window.open` loop cannot grow memory. |
| Browser hand-off, rolling budget (`routing`) | **10** browser launches in **any rolling 60 s**, globally across tabs and independent of the frame rate (`MAX_EXTERNAL_PER_WINDOW` / `EXTERNAL_WINDOW_S`; timestamps pruned lazily). A page looping `window.open` across frames cannot exceed it; refused requests are dropped, never queued. |
| Browser hand-off per frame (`routing`) | **5** external opens per frame, each distinct URL once, as an inner guard on the rolling budget. Beyond the cap a frame's requests are dropped, not deferred. |
| External schemes (`routing`) | **http, https, mailto** only. `file:`, `javascript:`, `data:`, app schemes and bare paths are refused (logged) and never reach the OS. On Windows the URL goes through `ShellExecuteW` — one string, no shell — so `&`, `|`, quotes are data (the old `cmd /C start` gave a page-controlled URL to a command interpreter). |
| Launch deep link (`deep_link`) | Read **once**, in `AppSpec::setup`, from `args_os` (never `args`, which panics on invalid UTF-8). A candidate carries the `station:` scheme as a case-insensitive prefix — what the WHATWG parser itself accepts, it lowercases the scheme, so `STATION://` is the same URL; the **host is compared exactly**, because an opaque host keeps its case (`station://AppStore` is a different URL). At most **2 048** chars (`MAX_ARG_CHARS`); control characters refused; parsing is `url_router::parse_url`, nothing about the URL is judged by hand. Served: `station://appstore` and `station://appstore/` only — the store tab through the same `open_app_store` path as a rail press. Any other host, a deeper path, a query or a fragment: one stderr line, state untouched. The **first** candidate wins; later candidates are never examined. No second window, no OS browser, no install from a link. |
| Single-instance transport (`instance`) | One request is at most **4 096** bytes (`MAX_REQUEST_BYTES`); a longer one is abandoned before parsing. Each connection gets a **2 s** read/write timeout (`IO_TIMEOUT_S`), both sides. The token is **32** lowercase hex chars (`TOKEN_CHARS`), fresh per run, compared constant-length (`token_matches`); the file itself is refused above **4 096** bytes (`MAX_FILE_BYTES`), and a wrong shape (port 0, a token that is not the generated shape) is refused, never truncated. Only `station:`-prefixed URLs pass the listener (`NotStation` otherwise) — the rest of the decision is `deep_link`'s, shared with argv. At most **8** accepted requests per run (`MAX_ACCEPTED_PER_RUN`) and **10** in any rolling **60 s** (`MAX_ACCEPTED_PER_WINDOW` / `ACCEPT_WINDOW_S`), links and focus pings together; refused requests are dropped, never deferred. The listener binds **127.0.0.1** only; a stale `instance.json` is ignored when the connect fails. |
| Single-instance focus (`webview::focus_window`) | The primary brings its window forward once per poll that drained at least one accepted request (link or focus ping). `SetForegroundWindow` is best-effort behind the documented restore/attach workaround; a refusal is one stderr line and never fatal — the link is already acted on. |
| Icon fetches (`icons`) | Only **http/https** URLs are fetched (`is_fetchable`; `file:`, `data:` and everything else keep the letter avatar). One icon body is at most **1 MiB** (`MAX_ICON_BYTES`: refused when `Content-Length` says so, and the read is capped at cap+1 so a lying header cannot dodge it), with a **10 s** overall timeout (`FETCH_TIMEOUT_S`); a decoded icon is at most **8192 px a side** (`MAX_ICON_SIDE`, the same limit rune's decoder enforces), at most **1 500** files on disk (`MAX_CACHED_ICONS`, pruned oldest-modified-first), and at most **128** fetches queued at once (`MAX_INFLIGHT`; beyond it the request is dropped, never deferred). Each URL is asked for **once per run** — a failure never retries and logs one stderr line. Every failure (refused scheme, network error, oversized body, undecodable or hostile image) leaves the handle empty and the view draws the letter avatar; nothing panics on remote data. Tests never touch the network: the fetch sits behind the `Fetcher` trait, exactly like `WebviewHost`/`FakeWebview`. |
| Notification row icons (`notifications::panel`, `ShellState::prefetch_icons`) | At most **32** distinct notification icon URLs are asked for per frame (`MAX_NOTIFICATION_ICON_URLS`), only while the panel is the surface and only for rows the panel actually draws (it draws none while nothing is unread). The first 32 in row order (newest first), deduplicated; rows past it keep the letter avatar without an ask, and a later frame asks nothing new (the icon store's once-per-run set). `file:`, `data:` and every other non-http(s) URL spend no slot and keep the letter avatar, as do broken/refused/undecodable ones (the `icons` fallback). The ask runs before the store's list prefetch so the drop-not-defer `MAX_INFLIGHT` queue cannot spend the visible panel's asks on off-screen tiles. |
| Live pages (`webview`) | **5** attached pages. Attaching a sixth closes the least-recently-*shown* page that is not the active one; its bounds, failure and recency go with it, and re-selecting recreates it. |
| Attach retries (`webview`) | 4 attempts (the first plus three), delayed **1 s, 5 s, 30 s**. Switching to the failed tab or leaving/returning from Settings or the notification panel retries at once. A retargeted URL is a new page and clears the failure. After the fourth failure the shell waits for the user. `webview::retry` (the pane's **Retry**) forgets the failure and starts over. Recurring failure logs are one line per message per **10 s**. |
| Failed page, made visible | `ShellState::page_error` (`{ application, detail }`) is set from the frame/poll path, and the pane shows **"Could not load <app>"**, the host's error and a Retry control. It is cleared when the page attaches, on retry, and when the tab is closed. |
| Session document | Refused unread above **16 MiB**. Collections are refused (not truncated) past **512** installed, **512** dock, **2048** tabs, **512** rail entries — a refusal keeps the file's referential integrity. Per-app configuration (`appConfig`): a value over **2 048** chars or whitespace-only is refused, and a key naming no installed application is refused — each with the offending app id (`StoreError::BadConfig`); no entry-count cap is needed (entries are keyed by installed applications, already capped). Both `load` and `save` validate. Window size: non-finite or ≤ 0 falls back to 1200×800, otherwise clamped to **320×240 … 8192×8192 points** — the *stored* bound is scale-unaware so a 3840-pt 4K window or a 5120×1440 ultrawide is kept as it was. |
| Config editor values (`ShellState::set_app_config`) | The **2 048**-char cap is checked on the value as given, before trimming: a hostile over-cap value is refused whole — nothing changes in memory, and the document is left byte-for-byte. The editor itself trims whitespace and bounds the draft. |
| Page environment (`PageEnvironment`) | UA: the exact session string, or a manifest `bx_override_user_agent` capped at **512** chars with control characters dropped; a blank value falls back. Profile: `default` for `bx_use_default_session`, else `app-<id>` for a non-empty ≤64-char `[A-Za-z0-9_-]` id, otherwise `app-<sha256 prefix>` — an id is never a path. Both are chosen when the web view is built; no page message reaches them. |
| Downloads (`webview` handlers + slice 22 `downloads`) | Every download is offered to slice 22's `downloads::accept`: the page-suggested name is reduced to one component, sanitized and capped at **120** chars, placed under the OS Downloads folder (never a page path), conflicts get ` (1)`… suffixes without overwriting, and at most **4** run at once. This slice's log is at most **32** lines per run (`MAX_DOWNLOAD_LINES`), then one "stopped" line; a URL or saved path is capped at **256** chars (`MAX_DOWNLOAD_TEXT_CHARS`) and control characters are dropped, so neither can forge a line. No UI or counter. |
| Tray (`tray`) | Windows only (`rune::Tray` reports `TrayError::Unsupported` elsewhere, and the shell starts none). One tray window/icon per process; the menu is exactly two enabled items (Open, Exit). A tray that cannot be created is one stderr line and the shell keeps running. The icon decodes from the embedded ICO through `ico` (side at most **256** px, `MAX_ICON_SIDE`); a missing size uses the nearest entry at least as large, else the largest. |
| Launch at login (`autolaunch`) | One value: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Station`, `REG_SZ`, data the executable path in quotes with no arguments. A command longer than **4 096** UTF-16 units (`MAX_COMMAND_CHARS`) is refused, never truncated; reads use that buffer and treat a longer stored value as present. Disabling removes the value; a missing key or value is success. Windows only — elsewhere the item is `Unsupported` and the Settings row is absent. A failed write keeps the last known toggle state and shows the reason; tests use a fake item and never write the registry. |
| Session `startAtLogin` | An additive v1 boolean (`#[serde(default)]`, camelCase, default `false`): a pre-existing document opens unchanged and an older build ignores it, so the format version stays 1. The registry is the runtime truth; the field is what a headless or non-Windows run still round-trips. |
| Window launch and restore | **Launch**: the display scale is unknown before the window exists (and `--snap` defaults to 2×), so the window opens at most **2048 pt** (`MAX_LAUNCH_POINTS`; 4096 px at 2×, 8192 px at 4×). **Restore** (`on_window`, real scale known): `restored_size` keeps the session's points up to **8192** and shrinks only what `points × scale` cannot fit under the **8192 device-px** ceiling (the 256 MiB snapshot-readback limit, `8192² × 4 B`): 3840 pt at 1× or 2× stays 3840, 8192 pt at 2× becomes 4096, 16 384 pt at 1× becomes 8192. So a large display comes back at its size and a hostile or over-scaled one shrinks instead of failing. |
| Session writes | Every user action that changes durable state — a selection, an install, an uninstall, an appearance change — is written **immediately** through `session-store`'s temp-file-and-rename, so a hard kill cannot lose the last action (an earlier 250 ms coalescing window was removed for exactly that reason). Close and quit write too. A failed write is reported at most once per **10 s**, the next user action retries it, and the shell keeps running. |
| Pane geometry | A missing pane or a non-finite rectangle hides the visible page instead of placing it at NaN; negative origins clamp to 0 and sides to 16384 points. |
| Display scale | The window's real DPI — `GetDpiForWindow` on the Rune parent HWND, read by the frame poll each frame (`webview::adopt_live_scale`) — is the authority, because `Runtime::env.scale` is frozen at the windowed app's build-time 1.0. A change beyond ±0.001 calls the host's `set_scale` and re-applies every attached page's bounds (points are converted with the new scale inside the host). Only scales in **0.5–4.0×** (`SANE_SCALE`) are accepted; a read outside — or non-finite — is refused and logged at most once per 10 s (`log_throttled`), and `sync` consults the same read so the two hooks cannot fight over the engine's scale. `Env::scale` stays the fallback where no live read exists (headless tests, other platforms). |
| Waker and clock | The wake handle is a thread-local because the only writer is this process's web-view thread, and `AppSpec::poll` runs on **every** frame, so the wake is a latency hint, not a delivery guarantee: a request queued while the loop is idle is still routed by the next frame (a wake makes that frame happen sooner). |
| Degenerate geometry | The shell clamps everything it computes or loads (session sizes, pane rects). A *presented* box that is degenerate for another reason (a view that lays out a NaN size of its own) is Rune's to guard — the renderer does not reject one, and its wgpu path can panic on it; the shell's own paths cannot produce one. |
| Shutdown | `close_all` is idempotent and sets a closed flag: nothing attaches or re-attaches after it until a new host is installed. A poisoned request queue is recovered (`into_inner`), and the waker uses `try_borrow`, so no lock or borrow panic takes a frame down. |
| Private manifests | `PrivateStore::save_new_application` / `delete_manifest` return `io::Result` and roll back on failure (no more `expect`); `ApplicationService::request_private_application` / `delete_private_manifest` map that to `ApplicationError::PrivateStoreWrite`. Writes go through the same **temp file + `sync_all` + rename** pattern as the session (the old file stays on failure, the temp is cleaned up), reads are capped at **8 MiB** and **10 000 records** (a larger file is warned about and treated as empty, like corrupt JSON), and the id counter saturates instead of overflowing on a stored `u64::MAX`. When the config dir cannot be opened, `ShellState::launch` warns and falls back to an **in-memory** store so the window still opens. |
| Test isolation (`cargo test`) | Ten consecutive `cargo test --workspace` runs (three of them alongside a second `station-shell` test binary in another process) were **492 passed / 0 failed** each; the intermittent station-shell failure observed earlier was **not reproduced**. The audit still found and closed real cross-test windows: `routing`'s queue and rolling browser budget and the notification bridge's pending queue are process state that **every** test's frame poll drains, so a test that enqueued could have its request routed (or its external URL actually opened and budgeted) by another test's poll. One reentrant process-state lock (`test_support`) now owns both, taken by `webview::route_requests` and `notifications::bridge::deliver_to` (cfg(test)) and by `tests::routing_lock` / `tests_pages::bridge_lock` for a whole test; `instance` keeps its `TEST_LOCK` + `freeze`/`own_freeze` protocol, icons keep a per-test `IconStore` (plus `ORDER` for the worker tests), and no test starts the listener or binds a port. Temp paths are process-unique: session files (`station-shell-{session,config,dpi,rail-order}-*`) and `ApplicationService::for_tests` (which deletes its directory) now carry `pid` plus a per-process counter, like the icon caches and the manifest/session-store tests. No production limit changed. |
| Page title and badge (`notifications::bridge`, `kind:"title"`/`kind:"meta"`) | Page text is trimmed and **refused, never truncated**, when over **256** chars (title) or **32** chars (badge), or when it contains a control character (`Refusal::BadText`); an empty/whitespace title clears to the manifest label, and a missing or empty `badge` meta clears the badge (a non-string meta payload is malformed and refused). Messages share the page channel's **32 KiB** message cap and **128**-message hold, and the latest title/badge per tab replaces the queued one (one slot each), so a mutating page cannot grow the queue. Sender identity is the existing check: only an installed app's web tab; the Rune-drawn store tab and unknown ids are refused. Both values live in memory only — the session document stores neither (no `pageTitle`/`badge` key), so a page cannot grow the document. |
| Stored identities (`session-store`) | An additive `identities` object in the v1 document, keyed by `identityId`: at most **32** (`MAX_IDENTITIES`), and each of its eight strings (id, provider, user id, display/first/last name, email, image URL) at most **256** chars (`MAX_IDENTITY_FIELD_CHARS`). Validated on **load and save**: a key that disagrees with the entry's `identityId`, an oversized field, more than 32 entries, or a JSON object that repeats a key (a key-only scan catches it before `serde` collapses the duplicate) refuses the document whole and names the offending id (`StoreError::BadIdentity`; the count uses `TooManyEntries`) — the account list is never truncated. The shape carries **no tokens**; an older document opens with no accounts and an older build ignores the field (format stays 1). |

Tests (97 in `station-shell` — 60 in `tests`, 19 in `tests_pages`, 17 in `tests_icons`
including the gated raster proof — 16 in `session-store`, 50 in `appstore-service`): the
hostile-page set
(`only_web_schemes_may_reach_the_browser`, `a_refused_url_never_reaches_the_opener`,
`shell_metacharacters_are_data_not_syntax`, `a_popup_flood_is_queued_within_a_bound_and_deduped`,
`a_popup_flood_opens_at_most_a_few_browsers_per_frame`, `external_opens_have_a_rolling_minute_budget`,
`the_external_budget_is_global_across_origin_tabs`, `a_poisoned_request_queue_does_not_panic`),
the page-lifecycle set (`a_failed_page_retries_with_backoff_and_recovers`,
`a_permanently_broken_page_stops_retrying_and_the_retry_control_restarts_it`,
`switching_to_a_failed_tab_retries_without_waiting_out_the_backoff`,
`retargeting_a_failed_page_forgets_its_failure`, `six_tabs_keep_at_most_five_pages_and_recreate_the_evicted_one`,
`a_scale_change_reaches_the_host_and_re_bounds_every_page`, `a_missing_or_non_finite_pane_hides_the_page`,
`closing_is_idempotent_and_nothing_attaches_afterwards`), the session set
(`a_selection_is_written_immediately_and_the_last_one_wins`,
`a_failing_save_keeps_the_shell_running_and_retries_on_the_next_action`,
`launch_survives_an_unavailable_private_store`, and in `session-store`
`a_non_finite_window_size_falls_back_to_the_default`, `a_hostile_window_size_is_clamped`,
`an_oversized_document_is_refused_without_being_parsed`, `a_document_with_too_many_entries_is_refused`),
and the store set (`a_failed_private_store_write_is_an_error_not_a_panic`,
`a_failed_private_delete_is_an_error_and_keeps_the_record`, `an_in_memory_service_accepts_writes_without_a_disk`,
and in `manifest-registry` `a_save_and_delete_round_trip_through_the_atomic_write`,
`a_failed_write_keeps_the_old_file_and_leaves_no_temp`, `an_oversized_private_file_is_ignored`,
`a_file_with_too_many_records_is_ignored`, `the_highest_possible_id_does_not_overflow_on_save`).

## Remaining gaps

1. Notifications have a page source (slices 8/19): `bxApi.notificationCenter.sendNotification` /
   `closeNotification` reach the center, and the injected script now replaces
   `window.Notification`, so an ordinary page's `new Notification(title, options)` lands in the
   same panel — the four members existed since slice 8, but no real page calls them directly.
   Clicks route through `click_target` + `url-router` back to the page's `onclick`/listeners
   (slice 15), and finite snooze expires. Still missing: an **OS notification source** (no OS
   toast bridge; the panel is Rune/in-process only) and the **OS do-not-disturb reader** behind
   `Snooze::SyncWithOs` (stored, exempt from expiry, never claimed to work). Related:
   `InstallAndOpen` routing does not install (the store's install path exists, but a link cannot
   trigger it yet).
2. Snooze expiry is driven (slice 8: the frame poll calls the ported expiry path through
   `NotificationCenter` at most once per second), but `SYNC_WITH_OS` still has no system-state
   reader (gap 1).
3. Settings covers appearance, version, session path, app removal and the user's rail order
   (slice 11: move up / move down per My apps row, D11), **and per-app configuration for what
   the shell can collect (slice 10: subdomain / custom URL, validated, persisted, applied to the
   tab URL)**. Accounts now exist as data (slice 17): the session stores identities (no tokens),
   Settings lists them, and a `google-account` app's editor chooses a stored identity through
   `appConfig.identityId`. Still missing: **sign-in itself** — nothing creates an identity yet, so
   the empty state says so and no control pretends otherwise — and an identity does not sign the
   app's page in (the brief's B6: Gmail's template renders `Email=` empty). The OAuth slice's
   callback cannot be `station://` (custom schemes need a period); it is loopback, per
   `docs/oauth-config-options.md`.
4. A hosted page's own state (scroll, history, form drafts) is lost when the shell closes; only
   the URL is restored.
5. **Done in the shell (slice 13):** the monitor-DPI change is detected each frame from the window's real DPI (`GetDpiForWindow`, 0.5–4.0× accepted) and re-scales every page. What remains unverified is the *windowed observation* itself (WebView2 cannot start in this desktop session), not the seam — see the Unverified DPI note.
6. **Done for the shell's own surfaces (slices 9 and 16):** rail rows and store tiles draw the
   manifest's real icon, and notification rows draw their own `icon` URL (the center's stored
   URL, at most **32** distinct per frame — see the Robustness table); all fetched once per run,
   disk-cached, with the letter avatar as the honest fallback. Still icon-less: the *hosted
   pages* themselves (a page's favicon is the page's business) and the multi-instance
   configurator surface (nothing serves it yet).
7. **Done for the shell (slice 14):** a launched `station://` link is honoured (slice 12), a
   second launch hands its link to the running shell and exits without a window, and the
   primary acts on it (selects the store, or names-and-ignores a URL it does not serve) and
   brings its window forward. What the lock does **not** do: it is per-Windows-session only
   (`Local\…`; other platforms have no lock, matching the web view's documented `Unsupported`),
   it does not handle Windows Store protocol activation (`Windows.Protocol` contracts hand the
   URL to an already-running packaged app their own way — the packaging story for the Store is
   `docs/rust-packaging-plan.md` §7's, not this lock's), and non-URL `--flag`-style arguments a
   second launch carries are not forwarded (only the first `station:` candidate is). And only
   `station://appstore` is served (D6) — the rest of the `station://` WebUI set, the
   multi-instance configurator among it (gap 6), is named on stderr and dropped, on both
   arrival paths alike.

## Slice sequence for the rest of the port

1. **Done (slice 1)** — Rune app, rail/pane, tokens across dark/light, headless tests.
2. **Done (slice 2)** — `webview.rs`: `WebviewHost` + `PaneRect` + recording fake, the
   content-rect → bounds feed, tested headlessly.
3. **Done (slice 3)** — wry adapter on Windows: one WebView2 child per tab, created on first
   activation, hidden on switch, bounds from the `content` element, dropped on close/quit. Manual
   acceptance this slice: creation, parenting and geometry observed (below); a real page was not
   loaded because `station://appstore` is not served yet (slice 4), and Gmail was not exercised in
   the window.
4. **Done (slice 4)** — the App Store as native Rune UI: `state` owns `ApplicationService` and the
   store's cached lists, the rail is the store plus installed apps, and a tab whose URL is
   `station://appstore` is drawn by Rune and never hosted. Tiles install (and switch) or uninstall;
   search runs over `search_applications`; sections read `get_most_popular_apps`,
   `get_all_categories`/`get_applications_by_category` and `private_manifests()`.
5. **Done (slice 5)** — the session persists: `session-store` document, `ApplicationService::restore`
   with all-or-nothing validation, `ShellState::launch` restoring rail + selection, saves on
   install/uninstall/selection/close/quit, and window placement restored and captured (D7).
6. **Done (slice 6)** — Settings (appearance/version/session path, My apps with Open and Uninstall),
   the notification panel (groups, mark-all, snooze), both hiding the hosted page instead of
   covering it; and preset apps unblocked (`resolve_start_url`, D8).
7. **Done (slice 7)** — new windows routed through `url-router`: the WebView2 handler denies and
   queues, Rune's `Waker` reaches the next frame, `poll` drains and `routing::decide` picks app tab
   vs OS browser; plus the release build and `rust/README.md` (D9).
8. **Done (slice 8)** — the notification page source: wry's `with_ipc_handler` plus an injected
   init script exposing only `window.bxApi.notificationCenter.sendNotification` /
   `closeNotification` (the Electron preload's surface, nothing else of `bxApi`); hostile-input
   validation and caps (the "Robustness" table); delivery into the ported center under the
   sender's application; pressable rows routing through `click_target` + the existing
   `url-router` decision (external URLs still through the allowlist + budget path); snooze
   expiry from the frame poll at most once per second; never a second OS window (D9).
   *(Corrected by slice 19: those members were only callable by a page that used them directly.
   The init script now also replaces `window.Notification`, which is how real pages raise
   notifications.)*
9. **Partly done (slice 13)** — the monitor-DPI half of lifecycle hardening: the frame poll
   reads the window's real DPI (`GetDpiForWindow` on the stored parent HWND, the same raw
   handle `window_created` kept) every frame and, on a sane change (accepted **0.5–4.0×**,
   otherwise refused and throttled-logged), calls the host's `set_scale` and re-applies every
   attached page's bounds — hidden pages included, so a change while Settings or the
   notification panel is open still lands. `sync` consults the same live read, so the poll and
   the frame's `sync` cannot fight over the engine's scale; `Runtime::env.scale` (frozen at
   build time in the windowed app — Rune never updates it) is only the headless fallback. The
   seam is proven headlessly through a test override of the read (`tests_dpi`); the windowed
   observation was not possible (Unverified, DPI). Still open in this slice: one webview per
   tab under memory pressure, crash/reload of a page, focus hand-off.
   Verified (this slice): `cargo test -p station-shell` **133** (130 + 3 in the new
   `tests_dpi` module), `cargo test --workspace` **449 pass / 0 fail**, clippy
   (`--workspace --all-targets --exclude station-bridge -- -D warnings`) exits 0,
   `cargo fmt --all --check` clean, `cargo build --release -p station-shell` ok.
10. **Done (slice 9)** — real app icons and window identity: the manifest's `icons[0].src` URL
    reaches `MinimalApplication.icon_url` for bundled manifests too (the TS build embeds the same
    source as a data URL; here it stays remote); `icons::IconStore` fetches each URL once per run
    on one worker thread behind the `Fetcher` trait (`HttpFetcher` in the window, canned bytes in
    tests), caches the bytes on disk (`<config dir>/station/icons/<sha256(url)>.bin`, temp+rename,
    LRU-pruned at 1 500 files), decodes with the same `image` limits rune uses (8192 px a side),
    and lands the pixels in a `rune::Pixels` the view draws synchronously — the frame never waits
    on the network; a `waker()` after each job brings the next frame. Rail rows and store tiles
    draw the icon over the letter avatar (`icons::avatar` is the one builder for both); a missing,
    refused (non-http), failed, oversized, or undecodable icon keeps the letter avatar. The window
    carries Station's icon (title bar + taskbar) from `WM_SETICON` in `webview::window_created` —
    the one place a raw handle exists — with HICONs made from
    `packages/app/build/icon.ico`'s own 32 px and 64 px `RT_ICON` entries (`platform_icon.rs`).
    Verified (this slice): `cargo test --workspace` **394 pass / 0 fail** (slice 8: 374; +17 in
    the new `tests_icons` module and +2 in `manifest-registry`'s own tests, +1 in the bridge's
    sender gate from the parallel bridge slice), `cargo test -p station-shell` **97**,
    `cargo test -p manifest-registry` **50** (6+20+12+5+7),
    `cargo clippy --workspace --all-targets --exclude station-bridge -- -D warnings` exits 0,
    `cargo fmt --all --check` clean. Headless raster proof (`tests_icons::
    `a_decoded_icon_and_a_letter_fallback_rasterize_side_by_side`, gated on `STATION_ICONS_PROOF`,
    rune's own `Gpu::headless` + `Offscreen` readback): the decoded canned PNG fills the clipped
    circular avatar on the "ready" row; the failed row shows the letter avatar (red tint, "G" on
    its own foreground) — viewed, then deleted. `MAX_INFLIGHT`'s drop path is proven through the
    public seam (`the_inflight_cap_drops_requests_instead_of_queueing_without_end`): one worker
    parked inside a gated fetcher, 200 more asks — at most 1 running + 128 queued were ever
    handed to the fetcher, and re-asking a dropped URL does not re-queue it (one attempt per
    run). The windowed icon is now observed (2026-11-18, `docs/window-icon-probe.md`): a seeded
    launch's top window answers `WM_GETICON` with a 32×32 `ICON_SMALL` and a 64×64 `ICON_BIG`
    (both confirmed through `GetIconInfo`), while the class-icon fallback is null; pixel
    appearance and taskbar/DWM rendering specifics are still not observed.
11. **Done (slice 10)** — per-app configuration, owned by the native shell (D10): the session's
    additive `appConfig` map (`AppConfigEntry`, the three `ApplicationConfigData` fields, format
    stays v1), validated in `session-store` (`BadConfig` names the app; refused, never truncated;
    `save` validates too); `ApplicationService::set_config_data` / `clear_config_data` (the
    `SET_CONFIG_DATA` merge + preset gate); Settings' My apps gains Configure per preset app with
    a one-field editor (Domain / Server URL, Save, Clear, Cancel) and the honest "Sign-in is not
    available in the native shell yet." for google-account presets; `rebuild_tabs` resolves every
    tab URL through `resolve_start_url` so save/clear move the tab on the next frame (an attached
    page is navigated via `webview::load_if_attached`) and a relaunch reopens the configured URL.
    Rail reordering is deliberately not here.
12. **Done (slice 11)** — the rail order is the user's (D11): `session-store` validates
    `railOrder` on load and save — the authority only when it names the docked set exactly;
    unknown ids, duplicates and over-cap orders are refused whole (`BadRailOrder` /
    `TooManyEntries`), and the rail falls back to install order, healed on the next write.
    `ShellState::move_rail_row` moves one step (store row never moves, edges refused) and
    persists at once; reordering lives in Settings' My apps card — a `settings-move-up-<id>` /
    `settings-move-down-<id>` pressable pair per row, absent at the edges and on the store row,
    labelled "Move <app> up/down" — because Rune gives a rail row no keyboard focus, so the
    first cut's Alt+Up / Alt+Down `on_key` path on the rail could never fire and was deleted.
    Verified (this slice): `cargo test -p session-store` **29**, `cargo test -p station-shell`
    **116**, `cargo test --workspace` **432 pass / 0 fail** (first run 432/0; a later run showed
    one station-shell failure once that never reproduced across two further full runs), clippy
    (`--workspace --all-targets --exclude station-bridge -- -D warnings`) exits 0, `cargo fmt --all --check`
    clean, `cargo build --release -p station-shell` ok; a `settings` `--snap` capture showed the
    move controls in place, then was deleted.
13. **Done (slice 12)** — a launched `station://` URL is honoured, once: `deep_link::open(state,
    args)` runs in `AppSpec::setup` over `args_os`, takes the **first** `station:`-prefixed
    argument, and serves exactly `station://appstore` / `station://appstore/` — the store tab
    selected through the same `open_app_store` path as a rail press (D6). Any other `station:`
    URL (another host, a deeper path, a query or fragment, `station://AppStore`'s preserved host
    case) is one stderr line naming the URL and no state change; bounds before parsing: the
    **2 048**-char cap, control characters, invalid UTF-8 (refused, never a panic — `args_os`,
    not `args`), and what the WHATWG parser refuses outright (`station://app store`); parsing is
    `url_router::parse_url`, reused, nothing hand-rolled. A deep link never opens a second
    window or the OS browser, and never installs. Honest limits: a second launch opens its own
    window (no single-instance lock, Remaining gaps 7), and nothing else of the `station://`
    WebUI set is served.
    Verified (this slice): `cargo test -p station-shell` **130** (116 + 14 in the new
    `tests_deep_link` module), `cargo test --workspace` **446 pass / 0 fail**, clippy
    (`--workspace --all-targets --exclude station-bridge -- -D warnings`) exits 0,
    `cargo fmt --all --check` clean, `cargo build --release -p station-shell` ok.
14. **Done (slice 14)** — one shell instance, and a live deep link reaches it: every launch
    takes `instance::handoff_if_secondary` **before Rune is asked for anything** — a named
    mutex in the logon-session namespace (`Local\station-shell-singleton`,
    created-or-exists, the documented Win32 check). The primary keeps the handle for its
    lifetime; a secondary sees `ERROR_ALREADY_EXISTS`, sends its first `station:` argument (or
    the focus-only ping `token\n\n` when its launch carried none) to the primary and **exits
    the process** — no state, no window, no listener. A mutex call that fails outright logs
    once and runs sole: the shell must open even when the lock cannot.
    **Transport — loopback TCP, and why not `WM_COPYDATA`:** the primary binds an ephemeral
    port on **127.0.0.1** and writes `<config dir>/station/instance.json` (`{ "port", "token" }`,
    temp + `sync_all` + rename, deleted first in `shutdown`, and a stale file is ignored when
    the connect fails). `WM_COPYDATA` via subclassing was rejected because it is unverifiable
    in this tree: the subclass lives on the window proc, and this session cannot open the
    windowed path (WebView2 cannot start here), so the notification semantics could never be
    observed. TCP keeps the whole request path (framing, token gate, caps, queue) in
    plain-library code that the headless tests exercise directly — `handle_request` is the
    listener's whole decision minus I/O — and the only window-bound part left is the focus
    call, which is a courtesy, not the transport. Threat model: the listener is reachable only
    from the same machine (loopback bind); the per-run **32-hex-char** token keeps other local
    processes and a leftover file from injecting links, compared constant-length; the frame
    cannot be flooded (caps below); and the request body is never trusted past
    `deep_link`'s argv-grade validation on the UI thread. The token is a gate against local
    processes, not a secret from the user's own account — nothing here defends against
    code running as the user.
    **Acting on the URL:** the listener thread only validates and queues; `webview::poll_at`
    drains (`instance::drain`), applies the **one** decision (`deep_link::act`, shared with
    argv — the refactor that made the two paths identical by construction; `first_candidate`
    is the forwarding half), and returns "state changed" so Rune rebuilds; any accepted
    request also brings the window forward (`webview::focus_window` →
    `SetForegroundWindow` behind the restore/`AttachThreadInput` workaround, best-effort,
    logged once on refusal, never fatal). The waker is the latency hint, the poll the
    guarantee — the same shape as the routed new-window queue (D9). Cross-platform: no lock,
    the documented `Unsupported` shape — no half-lock.
    Verified (this slice): `cargo fmt --all --check` clean; `cargo test -p station-shell`
    **156** (133 + 23 in the new `tests_instance` module); `cargo test --workspace` **472
    pass / 0 fail**, run twice, both runs 472/0; `cargo clippy --workspace --all-targets
    --exclude station-bridge -- -D warnings` exits 0; `cargo build --release -p station-shell`
    ok (29,301,760 bytes; a `--snap` release run renders 1 headless frame). **Windowed
    two-process smoke against the release binary** (scratch `STATION_SESSION_PATH`, so no user
    profile touched): the primary wrote `instance.json` within one 200 ms poll (port present,
    token 32 lowercase-hex); a secondary launched with `station://appstore` exited **0** in
    **266 ms** with empty stderr, left exactly one station-shell process (the primary, still
    alive); `station://settings` exited 0 in 272 ms and the primary printed exactly one line —
    `station-shell: deep link not served (only station://appstore is): station://settings` — and
    a focus-only launch (no arguments) exited 0 in 289 ms with nothing spurious on the primary's
    stderr; a `WM_CLOSE` closed the primary and `instance.json` was gone, and a third launch
    then became the primary (a fresh `instance.json`, new port); final count of station-shell
    processes: 0. Still not verifiable in this tree: the focus *observation* itself (whether
    Windows granted the foreground switch — `SetForegroundWindow` was called and its failure
    would have logged; no log line appeared, but the smoke cannot see which window has focus),
    and the flood/gate behaviour is headless-tested only (a scripted flood against a live
    primary was not run).
15. **Done (slice 15)** — a notification click reaches the page that raised it: the injected
    script now exposes the preload's four `bxApi.notificationCenter` members — the existing
    `sendNotification`/`closeNotification` plus `addNotificationClickListener` /
    `removeNotificationClickListener` — over a page-side registry capped at **8** listeners (a
    ninth is refused), with each callback called in its own `try`/`catch` so a throwing one does
    not stop the next and a removed one is no longer called. A row press still marks the
    notification read and routes the sender's tab, then the view action asks the host
    (`webview::click` → new `WebviewHost::click(tab_id, notification_id)`) to evaluate
    `window.__stationNotificationClick(<json id>)` on that page; the wry host uses
    `WebView::evaluate_script` (wry 0.57.2), the recording fake logs the exact script. The id is
    page-controlled, so it is a **JSON string literal** from `serde_json::to_string` — quotes,
    backslashes, newlines and `</script>` cannot break out — and a serialization failure refuses
    the message. An unattached page (or a missing host) drops the click silently while mark-read
    and routing still run; `ShellState` stays handle-free (D5). One message per press, no retry.
    Verified (this slice): `cargo fmt --all --check` clean; `cargo test -p station-shell` **165**
    (156 + 9 in the new `tests_notify_click` module); `cargo test --workspace` **481 pass / 0
    fail** (472 + 9); `cargo clippy --workspace --all-targets --exclude station-bridge -- -D
    warnings` exits 0; `cargo build --release -p station-shell` ok. The click path is proven
    headlessly (panel press → `evaluate_script` seam → recorded script); pages do render in this
    desktop session (`docs/webview2-probe.md`), but no GUI run was attempted in this turn, so the
    wry `evaluate_script` call was not exercised against a live page: no page-side listener
    actually ran and no real `Notification.onclick` fired. *(Slice 19 wired the same click path to
    `window.Notification` instances: the override registers one click listener per instance and
    dispatches `onclick`/`addEventListener('click')` handlers when the shell's hook calls back.)*
16. **Done (slice 16)** — notification rows draw their `icon`: the URL a page put in
    `NotificationProps.icon` (bridge-capped at 2 048 chars, stored on `Notification.icon`) is now
    the row's ~20 pt avatar, before the title/body/age. Decoded pixels draw when they land; until
    then, and for every refused/broken/undecodable URL, the sender app's letter avatar draws
    (`icons::avatar`, the same builder and tint rule as the rail and the store: manifest name and
    `theme_color` when the panel can obtain them, else the id with the id-derived accent; a gone
    app or manifest is the id). Asking reuses the one `IconStore` (`prefetch_icons` /
    `drain_icons`, no second path or cache): each frame the panel is the surface, its actually
    drawn rows contribute icon URLs, deduplicated and capped at **32** distinct
    (`MAX_NOTIFICATION_ICON_URLS`, Robustness), and the ask runs before the store's off-screen
    lists so `MAX_INFLIGHT`'s drop-not-defer queue cannot starve the visible panel. A closed
    panel — and an opened one with nothing unread, which draws no rows — asks for nothing. Late
    bytes need no extra rebuild: the drain already reports "changed" and that redraws the next
    frame. Verified (this slice): `cargo fmt --all --check` clean; `cargo test -p station-shell`
    **176** (7 in the new `tests_notify_icons` module); `cargo test --workspace` **492 pass / 0
    fail** (481 at slice 15; the 7 here, plus 4 from the platform-icon work already in this
    tree); `cargo clippy --workspace --all-targets --exclude station-bridge -- -D warnings`
    exits 0; `cargo build --release -p station-shell` ok. Drawn-headlessly only: the panel is
    proven through Rune's element tree (the icon element's key, the letter disappearing when the
    pixels land, the tint), not by a windowed screenshot.
17. **Done (slice 17)** — accounts exist as data (the brief's B1 in
    `docs/oauth-config-options.md`; no network, no sign-in button, no tokens). `session-store`
    gained an additive `identities` object in the v1 document, keyed by `identityId` and shaped
    exactly like TS `StationUserIdentity` (`packages/app/src/user-identities/types.ts`):
    `provider`, `userId`, and `profileData{displayName,firstName,lastName,email,imageURL}` —
    with no `accessToken`/`refreshToken`. Validation runs on **load and save** in the existing
    refuse-never-truncate style: at most **32** identities, every field at most **256** chars, a
    key that disagrees with the entry's `identityId` refused by id, a repeated JSON key refused
    (a key-only scan before serde collapses it), all as `StoreError::BadIdentity` /
    `TooManyEntries` with the document untouched (Robustness). The shell loads the set at launch
    and writes it back unchanged on every save; Settings shows an **Accounts** card (display name
    + email, or "No accounts yet." plus the honest sign-in note), and a `google-account` app's
    Configure editor now offers the stored accounts as a chooser that writes
    `appConfig.identityId` through the existing `set_app_config` path — the tab is re-resolved and
    an open page follows via `webview::load_if_attached` if the URL moved. With no identity the
    editor keeps "Sign-in is not available in the native shell yet." and adds "Add an account once
    sign-in lands."; there is no Save (the choice applies at once) and no sign-in control.
    **What this does not do:** create an identity (that is the next OAuth slice) and sign an app's
    page in — Gmail's `start_url_tpl` renders `userIdentity.profileData.email`, which both TS and
    this port leave empty, so choosing an account leaves that URL string unchanged (the brief's
    A6/B6); the write and re-resolution path is nevertheless the existing configured-app one.
    `station://` cannot be Google's redirect either (custom schemes need a period), so the later
    callback is loopback, per `docs/oauth-config-options.md`. Verified (this slice):
    `cargo fmt --all --check` clean; `cargo test -p session-store` **37** (29 + 8 identity tests);
    `cargo test -p station-shell` **181** (176 + 5 in the new `tests_accounts` module);
    `cargo test --workspace` **505 pass / 0 fail** (492 + 8 + 5); `cargo clippy --workspace
    --all-targets --exclude station-bridge -- -D warnings` exits 0; `cargo build --release -p
    station-shell` ok. Proven headlessly (session files, Settings' element tree, a fake host);
    no windowed run and no live sign-in.
18. **Done (slice 19)** — `new Notification(title, options)` from an ordinary page reaches the
    panel. Real pages never call `bxApi.notificationCenter` directly (preload inventory §1.2 #12 /
    §6): Electron made their `new Notification(...)` work by overriding the page's
    `window.Notification` in `webview-inject.js`. The injected init script now carries the port: a
    `BxNotification` class installed as `window.Notification` at document-creation time (so the
    page's first line sees it, unlike Electron's dom-ready inject), with the TS contract — `title`
    required (`Error('Title is required')`), the defaults object merged (actions, badge, body,
    data, dir, lang, tag, icon, image, requireInteraction, silent, timestamp, title, vibrate) and
    defined read-only, id `notif/<21-char nanoid>` from the same `urlAlphabet`
    (`webview-inject.js:87`), static `permission = 'granted'` and `requestPermission()` resolving
    (and then-ing) it, `close()` sending `kind:"close"`, and clicks through the existing registry
    (`addNotificationClickListener` → `__stationNotificationClick`) to `onclick` and
    `addEventListener('click')`; `onclose`/`onshow`/`onerror` are real handler surfaces (close and
    show dispatch locally; error exists but no shell report dispatches it). Constructing sends
    exactly `{timestamp,title,body,icon}` to the existing `sendNotification` — no second channel,
    no new `kind` — and every cap is unchanged (32 KiB message; 512/4096/2048 title/body/icon; 256
    id; 128 pending; 64 per source / 256 total; 8 click listeners). Hostile strings stay data: the
    wire is `JSON.stringify`, never concatenated code, and the id on the click path is a JSON
    string literal from `serde_json`. Dropped from the TS, with the reason: the `blob:` icon XHR
    (Telegram-only, needs a page-side fetch + inline with its own size cap) and the recursive
    same-origin iframe override (a policy decision about embedded content, not a mechanical port);
    `renotify`/`actions`/`image`/`vibrate` stay object properties only — the TS sent none of them
    to the shell either. Verified (this slice): `cargo fmt --all --check` clean; `cargo test -p
    station-shell` **196** (7 in the new `notifications::tests_notification_override` module, plus
    the tray session's 8); `cargo test --workspace` **520 pass / 0 fail** (505 + 7 + 8); `cargo
    clippy --workspace --all-targets --exclude station-bridge -- -D warnings` exits 0;
    `cargo build --release -p station-shell` ok. **String-level only:** no JavaScript engine runs
    in the tests (the script's reviewed source and the real Rust parser are tested; the hostile
    title's JSON is built with `serde_json` and fed through `bridge::parse`). Only a windowed page
    can prove the constructor runs: load a page calling `new Notification('hello')` and watch the
    panel row and the page's `onclick`.
19. **Done (slice 18, landed after slice 19 in this tree)** — the tray icon and launch at login,
    the two smallest Electron-only desktop behaviours; both are Windows-first. **Tray:**
    `tray.rs` uses the pinned Rune `Tray` — `Shell_NotifyIcon` on a message-only window that owns
    its thread and message loop, `TaskbarCreated` recovery, `TrackPopupMenu` — rather than a
    second hand-written Win32 loop. Its `Send + Sync` handler owns `AppHandle<ShellState>`, so a
    primary click runs `AppHandle::focus()` (the slice-14 focus path, restoring a minimized
    window) and Exit runs `AppHandle::quit()` (the one `on_quit`: save the session, close the
    pages, shut the listener down); there is no second shutdown path. The menu is Electron's:
    **Open**, **Exit** — no settings item (the TS tray had none) and no notification list (it had
    none either). Icon bytes are the embedded `rust/assets/station.ico`, decoded to straight RGBA
    through the new shared `ico` module (the directory parser moved out of `platform_icon.rs`,
    which still hands the ICO's raw `RT_ICON` entries to `CreateIconFromResourceEx`); the tray
    side follows `SM_CXSMICON`, and the entry rule is exact → smallest at least as large →
    largest. Electron listened for `double-click`; Rune's tray reports one primary click as
    `Activate`, so one click brings the window forward — the same action, one click earlier,
    recorded rather than hidden. **Launch at login:** `autolaunch.rs` reads/writes exactly what
    the Electron service's `auto-launch@5.0.6` did — `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Station`,
    `REG_SZ`, data `"<exe path>"` with no arguments (the Squirrel `update.exe` branch is Electron
    packaging this build does not have), unelevated. Settings → General gains "Start Station when
    I sign in" (On/Off) only where the platform has the item; the session stores it as the
    additive v1 `startAtLogin` bool (`#[serde(default)]`, no migration), and a real launch
    replaces it with the registry's truth. A failed write leaves the last known state and puts the
    reason in a "Login item" row; a failed read at launch does the same — the toggle never flips
    on a failure. Tests never touch the registry: a fake `LoginItem` drives the
    toggle/session/relaunch paths. **No close-to-tray change:** Electron's tray also hid the
    window on *minimize* (`MainWindowManager`), but this slice changed neither minimize nor close;
    closing still quits through the same hooks.
    Verified (this slice): `cargo fmt --all --check` clean; `cargo test -p station-shell` **196**
    (12 in the new `tests_tray` module); `cargo test --workspace` **520 pass / 0 fail**; `cargo
    clippy --workspace --all-targets --exclude station-bridge -- -D warnings` exits 0; `cargo
    build --release -p station-shell` ok (**29,496,832 bytes**). **Windowed run** (release,
    scratch `STATION_SESSION_PATH`, the Gmail seed): a message-only `RuneTray` window appeared in
    the shell's pid with **no** tray stderr line (Rune's `NIM_ADD` succeeded); with the window
    minimized (`IsIconic` true) the tray callback (`WM_APP+1`, lParam `WM_LBUTTONUP`) restored it
    in **0.7 s** with `GetForegroundWindow() == shell`; `WM_CLOSE` exited **0** in **0.5 s**,
    `instance.json` was gone, process counts went 0/0 → 1 shell + 8 `msedgewebview2` → **0/0**,
    and the scratch dir was deleted. The real `HKCU\…\Run\Station` value was **absent before and
    after** (the probe never toggled), so no user login item was touched. Not observed: the
    menu's rendering or its Exit pick (`TrackPopupMenu` is modal), the icon's pixels in the
    notification area, and a real registry write/remove.
20. **Done (slice 20)** — a page's own title and badge reach the chrome. The injected script now
    carries the preload's observers (`static/preload/preload.js:8-84` for `meta[name^=browserx-]`;
    `document.title` stands in for Electron's `page-title-updated` webview event) over the one
    existing page channel: `{kind:"meta", metas:{…}}` and `{kind:"title", title:"…"}`, deduped in
    the page and delivered by the frame poll. The shell trims them and **refuses** — never
    truncates — text over its caps or with control characters (`Refusal::BadText`): title **256**
    chars, badge **32**; the whole message keeps the existing 32 KiB / 128-pending limits, and the
    latest title/badge per tab replaces the queued one (one slot each). `AppTab` carries
    `page_title`/`badge` in memory only — the manifest label is the fallback, and clearing
    restores it; `rebuild_tabs` preserves both by tab id. The top bar and the rail's accessible
    name draw the page title; the rail draws the badge over the icon corner with
    `tab-model::Badge`'s TS split — a count (drawn as `999+` past the cap) or a dot. Sender
    identity is the existing check (an installed app's web tab; the Rune-drawn store and unknown
    ids are refused), and a title/badge update never selects, navigates or attaches a page.
    **What it cannot do yet:** the per-site scripts that *write* `browserx-badge` are not injected
    (inventory slice 2's data-table half), so only a page that sets the meta itself shows a badge
    today; favicons and per-tab history remain unported. Verified: `cargo fmt --all --check`
    clean; `cargo test -p station-shell` **206** (10 in the new `tests_page_meta` module);
    `cargo test --workspace` **530 pass / 0 fail**; `cargo clippy --workspace --all-targets
    --exclude station-bridge -- -D warnings` exits 0; `cargo build --release -p station-shell`
    ok. **Drawn-headlessly only:** the labels/badges are proven through Rune's element tree and
    the session document; no windowed page-title/badge observation was run.
21. **Done (slice 21)** — a hosted page gets the Electron session's identity. **UA:** every new
    web view is built with `wry::WebViewBuilder::with_user_agent` set to `session.ts:67`'s
    `defaultUserAgent` ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) … Chrome/150.0.0.0 …"),
    or to the manifest's `bx_override_user_agent` when it has one (the TS `<webview useragent>`
    path, `tab-webcontents/overrideProvider.ts:35-36`); the manifest value is capped at **512**
    chars and control-character-stripped, a blank one falls back to the session UA. The
    **per-request** half of `session.ts` is not reproducible: wry exposes no header interception
    (`with_user_agent` is per-webview; `with_url_and_headers` only sets the first load's headers),
    so `getUserAgentForApp`'s `https://accounts.google.com` → `Chrome/87.0.4280.141` override, the
    localhost Referer stripping and the CSP-response-header deletion (`session.ts:121-143`) are not
    ported. Windowed proof: a scratch session with an unknown-manifest app whose home tab is a
    loopback HTTP URL received that exact UA on `/page` and `/favicon.ico` (no other UA on the
    wire). **Profiles:** Electron's `persist:${applicationId}` partition
    (`Application.tsx:428,480`) is one WebView2 user-data folder per app, via
    `wry::WebContext::new(Some(dir))` + `WebViewBuilder::new_with_web_context` (wry 0.57.2
    `lib.rs:921`, `web_context.rs:45`); apps with `bx_use_default_session` share
    `webview/default`. The root is beside the session document
    (`<config dir>/station/webview`; a `STATION_SESSION_PATH` scratch run keeps its pages' data in
    the scratch dir). Directory names are `app-<id>` only for non-empty, ≤64-char
    `[A-Za-z0-9_-]` ids; anything else is `app-<sha256 first 8 bytes hex>`, so an application id
    can never choose a path. Windowed: the probe app created `webview/app-probeapp`, Gmail
    (`bx_use_default_session: true`) created `webview/default`, both pages stayed attached (the
    first hidden) with different WRY HWNDs, and stderr had no `no web view` line. **Downloads:**
    wry 0.57.2 does expose `with_download_started_handler` (`lib.rs:1362`) and
    `with_download_completed_handler` (`lib.rs:1383`), so this slice registers them: started,
    complete and failed are logged — at most **32** lines per run; a URL or saved path is capped
    at **256** chars with control characters stripped. The trust decision rides the same handler
    as the concurrent slice 22's `downloads` module: the page-suggested name is sanitized, the
    target is the OS Downloads folder (never a page path), conflicts get suffixes, and at most
    **4** downloads run at once. No download UI or counter this slice. Verified (this slice): `cargo fmt --all --check` clean; `cargo test -p
    station-shell` **215** (9 in the new `tests_page_env` module); `cargo test --workspace` **539
    pass / 0 fail**; `cargo clippy --workspace --all-targets --exclude station-bridge -- -D
    warnings` exits 0; `cargo build --release -p station-shell` ok (**29,587,968 bytes**).
    Windowed run (release, scratch `STATION_SESSION_PATH` + a loopback server): UA match as above;
    process counts 0/0 → 1 shell + 15 `msedgewebview2` (two profiles' browser trees) → **0/0**;
    `WM_CLOSE` exited **0** in **0.5 s**, `instance.json` gone, scratch dir deleted. Not observed
    windowed: a download start/finish (the handlers are format-tested only), any UA on a real
    Internet page (the check is the loopback request's header), and the per-request TS header
    rules (not implemented).
22. Bottom rail semantics — snooze, settings entry point, and what the rail's "+" becomes once the
    app directory slice lands (today it selects the App Store tab).
23. Notifications: an OS source (toast bridge) and the OS do-not-disturb reader for
    `Snooze::SyncWithOs`.

## Verified

Commands (slice 8): `cargo fmt --all --check` (clean),
`cargo test --workspace` (**374 tests pass**, 0 failed — the slice-7 tally was 354; slice 8 added
19 in the new `tests_pages` module plus 1 in the bridge's sender gate; test-count reconciliation:
STATE.md said 292 and slice 7's doc said 354, both stale — 374 is what this tree runs),
`cargo clippy --workspace --all-targets --exclude station-bridge -- -D warnings` exits 0; the
whole workspace with `-D warnings` still fails only in `station-bridge`'s lib-test target
(pre-existing, untouched, see slice 7's note). `cargo build --release -p station-shell` produced
26.6 MB / 25.4 MiB (slice-7 size). Two workspace runs showed unrelated flakes **once** each
(`external_opens_have_a_rolling_minute_budget` then two `manifest-registry` golden tests) and
passed on re-run and in the final full run; per the run's instruction these were reported, not
chased — none is in this slice's modules (the budget test shares a process-global budget that
another test had filled; it passes serially and in the crate-only run).

Slice 8's pipeline is proven headlessly (`tests_pages.rs`): a fake page message goes through the
real IPC-facing entry (`notifications::receive`, the same function wry's handler calls) into the
center and the drawn panel; a hostile set (huge title/body/id, 10 000-message flood, non-JSON,
wrong types, unknown kinds) is bounded and does not panic; a row press marks read and focuses the
sender's tab. WebView2 **still cannot start in this desktop session** (README "Verified on":
`0x80070057`, no `msedgewebview2` process), so no windowed page called
`bxApi.notificationCenter.sendNotification` here and none is claimed to have.

Commands (slice 7, re-run after the hardening pass): `cargo fmt --all --check` (clean),
`cargo test --workspace` (**354 tests pass**, 0 failed — the slice-7 tally was 292 and the hardening
added 62), `cargo clippy --workspace --all-targets --exclude
station-bridge -- -D warnings` exits 0; the whole workspace with `-D warnings` **fails in
`station-bridge`** — 14 dead-code errors in its *lib test* target (`watch_*`/`unwatch_*`/`emit_*`,
`tsfn_sink`, `Hubs`, `HUBS` are reachable only through the napi-generated cdylib bindings). That
crate is untouched by this work (`git status` clean for it) and reproduces with `-p station-bridge`
alone, so it is a pre-existing warning/dead-code mismatch, reported not fixed. `cargo build
--release -p station-shell` produces **26,621,952 bytes / 25.4 MiB**, and
`target/release/station-shell.exe --snap release-check.png` renders 1 headless frame (186 layout
nodes, 573 instances, 465 glyphs) — release builds keep Rune's snapshot flags through the `tooling`
feature. Slice 6's numbers are kept in the bullets that follow.

- **Hardening (2026-11-18)** — `cargo test -p station-shell -p session-store` (60 + 18 pass),
  clippy `-D warnings` on both, `cargo check --workspace`, release build (25.4 MiB). Hostile-session
  repro: a session with `"width": 16384.0, "height": 1e30` snaps — `--snap` at the default 2×
  renders one 4096×4096 frame (exit 0), `--scale 4` one 8192×8192 frame (exit 0); the stored bound is
  now 8192 pt (`large_display_sizes_are_kept_exactly`, `a_windowed_restore_keeps_large_displays_and_shrinks_what_will_not_fit`)
  while launch stays at 2048 pt, so a legitimate 3840-pt 4K session restores at 3840 and only a
  size the display cannot give the renderer is shrunk.
- **Routing (slice 7)** — pure decisions: `routing_a_scoped_link_lands_in_that_app_and_not_the_browser`
  (a Gmail-scope URL from another tab → `FocusAndLoad` on Gmail's tab),
  `routing_a_link_that_matches_an_open_tab_only_focuses_it` (exact tab URL → `Focus`),
  `routing_unknown_links_and_unknown_tabs_stays_put` (unmatched → `External`; a tab id that is not
  ours → `Ignore`). Through the seam: `a_scoped_new_window_loads_the_app_tab_and_selects_it`
  (selects Gmail, retargets its URL, one `Load` on the open page, **no** `Attach`),
  `an_unknown_link_goes_to_the_browser_and_changes_nothing` (`https://example.com/…` recorded as
  external, no host event, selection unchanged), `a_link_to_an_unopened_app_opens_it_at_that_url`,
  `a_routed_page_that_is_not_open_attaches_at_the_routed_url` (a fresh host: the frame after the
  route attaches Todoist at the routed URL, not its home URL).
- **Popup in a real window (slice 7)** — a temporary hook loaded
  `data:text/html,<script>window.open('https://example.com/popup-smoke')</script>` into Gmail's
  page (installed by a temporary launch hook; both since removed). The log showed
  `smoke: external https://example.com/popup-smoke` — the request was denied as a window and
  handed to the OS browser. No second web view or Rune window was created.

Commands (slice 6): `cargo fmt --all` / `--check` (clean), `cargo test -p appstore-service -p
session-store -p station-shell` (**47 + 12 + 31 pass**), `cargo clippy -p appstore-service -p
session-store -p station-shell --all-targets -- -D warnings` (exit 0; only the two vendored-crate
warnings remain), `cargo run -p station-shell -- --snap settings-shot.png --scene settings`
(46 layout nodes, 165 glyphs) and `--snap notifications-shot.png --scene notifications`
(224 layout nodes, 644 glyphs; sampled notifications for the check, since removed) — both viewed
and deleted — and one scripted windowed run (below). Earlier slices' numbers are kept in the
bullets that follow.

- **Settings (slice 6)** — `settings_opens_over_the_app_and_hides_its_page` (press the gear → the
  hosted page reports `Visible(false)`, the screen shows General/My apps, the version is
  `CARGO_PKG_VERSION`, the session path row exists; Back → `Visible(true)` with **no** second
  `Attach`), `settings_uninstalls_an_app_and_persists_it` (row keys per app, Uninstall → rail row
  gone, `Detach` for the page, Settings stays open with the empty state, relaunch has one tab),
  `settings_open_switches_to_the_app`, `the_appearance_preference_drives_the_theme_and_is_remembered`
  (desktop light + Dark chosen → `shell` fill is `tok::theme(true).bg` after the paint track
  settles; relaunch keeps `Dark`), `escape_leaves_settings_and_the_notification_panel`.
- **Notifications (slice 6)** — with a prepopulated center:
  `the_notification_panel_shows_its_groups_and_rows` (panel key, "3 unread", all three titles, a
  body, the row key; pressing the moon again removes the panel), `mark_all_read_clears_the_rows`
  (rows gone, empty state, "0 unread"), `choosing_a_snooze_updates_the_center` (15 min → 900000 ms,
  1 hour → 3600000 ms, Sync with OS → `Snooze::SyncWithOs` and `is_snoozed`, Off → `None`),
  `the_panel_hides_the_page_and_closing_brings_it_back` (one `Attach` total).
- **Preset unblock (slice 6, `appstore-service`)** —
  `golden_install_configurable_manifest_without_config_opens_its_start_url` (Gmail installs onto
  `https://accounts.google.com/AddSession?…`), `installing_with_config_the_presets_drop_still_opens_the_start_url`,
  `config_the_presets_do_not_use_still_resolves_to_the_configurator`,
  `resolve_start_url_agrees_with_install_for_a_preset_app`,
  `resolve_start_url_leaves_a_plain_manifest_unchanged` (Todoist),
  `resolve_start_url_keeps_an_on_premise_custom_url` (GitLab custom URL),
  `resolve_start_url_reports_an_unknown_manifest`.
- **Scripted windowed run (slice 6)**, `STATION_SESSION_PATH` a temp file with Gmail installed by a
  temporary launch hook (since removed): a hosted `WRY_WEBVIEW` child was present and
  `visible=True`; clicking the rail's gear → `visible=False`; Escape → `visible=True`; clicking the
  moon → `visible=False`; clicking it again → `visible=True`. The same run's temporary navigation
  log showed Gmail's page going `accounts.google.com/AddSession?…` → `ServiceLogin` →
  `v3/signin/identifier` — a real page, not `about:blank` or the configurator. (An inactive
  window swallows its activation click, so the script clicks chrome once first; that is a Windows
  behaviour, not the shell's.)
- **Theme format (slice 6, `session-store`)** — `a_document_without_a_theme_opens_as_system`
  (a v1 document written before the field loads as `System`) and
  `the_theme_round_trips_as_a_camel_case_name` (saved as `"theme": "dark"`, `version` still 1).

- **Session (slice 5)** — `ApplicationService::restore` (in `appstore-service`):
  `restoring_a_live_session_reproduces_it`, `restore_refuses_a_dock_entry_it_does_not_install`,
  `restore_refuses_duplicate_application_ids`, `restore_refuses_duplicate_tab_ids`,
  `restore_refuses_a_tab_of_an_application_it_does_not_install`,
  `a_refused_restore_leaves_the_service_untouched` — every case asserts the offending id comes
  back in `RestoreError` and that a refused replay changed nothing.
- **Session (slice 5, `station-shell`)** — `installing_persists_the_rail_row_and_the_selection`
  (tile press → a second `launch` from the same file has the row, its URL and the selection),
  `uninstalling_leaves_only_the_store_on_relaunch`, `an_unknown_active_application_falls_back_to_the_store`
  (a known id restores; an id that is gone lands on the App Store),
  `a_missing_session_starts_at_the_store_and_writes_nothing`,
  `a_corrupt_session_warns_starts_fresh_and_is_left_alone` (bytes unchanged until a `select`, then
  a valid document), `an_unsupported_session_version_is_refused_and_kept`,
  `a_session_whose_dock_names_an_unknown_app_starts_fresh`,
  `the_window_placement_survives_a_session_round_trip`.
- **Two-run windowed smoke (slice 5)**, `STATION_SESSION_PATH` a temp file, temporary
  `STATION_SMOKE_INSTALL=14` + `STATION_SMOKE_MS` hooks (since removed):
  run 1 wrote `activeApplication: HCX74PCl4`, `dock: [HCX74PCl4]`, `railOrder: [HCX74PCl4]` and the
  Gmail install with its configurator home tab; run 2 (no install hook) reopened with the window at
  the file's `x: 300, y: 200` (measured: main window 1216×839 at `(300,200)`) and the Rune window
  had a visible `WRY_WEBVIEW` child 1136×756 at `(372,275)` — 300+8+64, 200+31+44: Gmail's page,
  restored on the content pane. No stderr warnings. Both runs' quit saves left `x/y` intact.
- **App Store, Rune-drawn (slice 4)** — `the_store_renders_its_sections_and_the_popular_apps`
  (sidebar title, `DISCOVER`, the three sections, `Most Popular` and the three bucket titles plus
  Gmail/Slack tiles, with **zero** host events), `searching_narrows_the_main_area_to_matching_apps`
  (click the field, `Key::Text("slack")`, the main area becomes results and Gmail disappears),
  `switching_store_sections_changes_the_main_area` (All apps shows a category header and drops the
  buckets; My custom apps shows its empty state), `my_custom_apps_lists_private_manifests` (a private
  manifest written by `request_private_application` appears).
- **Install and uninstall (slice 4)** —
  `installing_from_the_store_adds_a_rail_row_and_switches_to_the_app` (tile press → new rail row,
  top bar and selection on Gmail, one page attached at the `content` rect with the install's own
  URL), `a_second_press_on_an_installed_tile_uninstalls_it` (marker disappears, rail row gone,
  `Detach` for that page), `the_rail_starts_with_the_store_alone_and_stays_unique`.
- **Rune-drawn tabs are never hosted (slice 4)** — `the_store_tab_never_gets_a_page_while_web_tabs_do`
  (no host events for the store, one page for Gmail, hidden when the store returns and shown again,
  never re-created); `switching_between_installed_apps_hides_and_shows_without_reattaching`;
  `a_resize_reaches_the_host_as_new_bounds` (via `Runtime::set_viewport`); `closing_the_window_detaches_every_page`;
  `a_refused_attach_is_not_retried_every_frame`; `points_convert_to_device_pixels_at_the_window_scale`.
  Every assertion goes through the public seam: `webview::after_frame` and the recording
  `WebviewHost`; none inspects private state.
- **Real window, store tab** (slice 4): `EnumChildWindows` on the running window with the App Store
  active found **no child windows** — the store creates no WebView2 and nothing to hide.
- **Real window, installed app** (slice 4): with a temporary `STATION_SMOKE_INSTALL=14` launch hook
  (since removed) the same check found `WRY_WEBVIEW` visible at 1136×756 with `Chrome_WidgetWin_0`,
  `Chrome_WidgetWin_1`, `Chrome_RenderWidgetHostHWND` and `Intermediate D3D Window` inside it: same
  geometry as slice 3 (64,44, rail and top bar left to Rune), now created for an installed app.
- **Slice 3 (still true)** — a ~9 s run created the page for the then-seeded App Store tab,
  navigated to `station://appstore`, and ended at `about:blank`; WebView2 Evergreen runtime
  157.0.4341.46 present. The page-per-tab, hide-on-switch and drop-on-close behaviour is that slice's
  and is unchanged.
- Rune's API surface, read at the pinned rev: `AppSpec`/`App::new`/`on_window`/`on_frame_drawn`/
  `on_close`/`on_quit`/`run`, `WindowHandle::with_raw_handle`, `Runtime::{frame, find, presented,
  elements, set_viewport, env.scale}`, `El`/`col`/`row`/`el`/`text`/`canvas`/`spacer`,
  `Shape::stroke`/`fill`, `tok::{theme, accent, space, radius, type_}`.
- wry 0.57.2 `build_as_child`/`set_bounds`/`set_visible`, and rwh 0.6.2 `borrow_raw` — read in
  source at the versions resolved in `Cargo.lock`.
- Windows build requires the vendored patches (D2): the first `cargo fetch` without them failed on
  `parley_engine`'s missing `line-dictionaries` feature.

## Unverified

- **Z-order is only partly observed.** A `PrintWindow` capture of the running window showed the rail
  and top bar but no placeholder card in the pane, consistent with the child `HWND` covering what
  Rune draws there; `PrintWindow` does not capture WebView2's own composition, so the page's pixels
  were not seen. A screen capture was inconclusive (the window was partly occluded by another app)
  and was deleted. What the web view shows over Rune's content, and how a Rune-drawn overlay behaves
  over it, is still unmeasured.
- **Focus and input.** `with_focused(false)` is set, but keyboard-focus hand-off between Rune's focus
  graph and the WebView2 controller, and whether Rune receives any pointer or key input over the
  webview's rect, are unmeasured.
- **A real page in the pane.** Pages attach, spawn the WebView2 runtime, navigate and are visible:
  the 2026-11-18 probe (`docs/webview2-probe.md`) saw eight `msedgewebview2.exe` processes, a visible
  `WRY_WEBVIEW` child at the content rect and a visible Chromium render widget titled
  "Sign in - Google Accounts" (Gmail's signed-out landing). It is one observation without a pixel
  capture, and why the earlier attach failures happened is unknown. What a page's *pixels* look
  like, input, scrolling, cookies, permissions and DevTools inside a hosted page remain unobserved.
- **The store in a real window.** Observed at the surface level (no screenshot): a posted click on
  the rail's App Store row hides the live page (`WRY_WEBVIEW` visible → false, same HWND) and a
  press on the app's rail row returns it (visible → true, same HWND and rect) — see
  `docs/surface-hide-probe.md`. What remains unmeasured is the store's own rendering *pixels* and
  click-through beyond the tab switch; scrolling its long lists in a real window is likewise
  unmeasured.
- **Maximized restore is untested.** The smoke windows were never maximized, so `App::maximized`
  and the `restore`-frame path in `session::live_window_state` are read from Rune's API and not
  observed. Placement across a display-scale change is likewise unmeasured: the size is stored in
  points, the position in physical pixels, and the two can disagree after a DPI change.
- **Close paths.** `AppHandle::quit` (windows closed without `on_close`), a crash and a killed
  process do not save beyond the last user action; only the close/quit hooks and the immediate
  saves cover the rest.
- **What the session does not carry.** Section, search text, scroll offsets and per-tab history are
  not restored, and a web page is recreated from its URL rather than resumed (D7). The appearance
  is the one settings value that is persisted.
- **Notifications have a page source, not a windowed one.** The pipeline is proven headlessly
  (`tests_pages.rs`) through the real IPC-facing entry. Pages do render in this desktop session
  (`docs/webview2-probe.md`: a seeded Gmail launch produced the WebView2 process tree and a
  visible render widget titled "Sign in - Google Accounts"), but no GUI run was attempted in this
  turn, so no live page raised a notification and the injected script's JS-level behaviour was not
  executed in a page. The click delivery is headless-proven too (`tests_notify_click.rs`: press →
  `WebviewHost::click` → the exact `evaluate_script` string); the 8-listener cap, removal and
  throw-isolation were asserted over the exact script text, not executed. Still model-missing: an
  OS source and the `SYNC_WITH_OS` reader.
- **Settings in a real window** — observed at the surface level (no screenshot): a posted click on
  the rail's Settings row at client `(32,720)` hides the live page (`WRY_WEBVIEW` visible → false,
  same HWND) and `VK_ESCAPE` restores it (visible → true, same HWND and rect) — see
  `docs/surface-hide-probe.md`. What remains unmeasured is what Rune painted (pixels) and a
  windowed press on a My-apps row (Open/Uninstall); the screen's scroll with many installed apps is
  likewise unmeasured.
- **Appearance in a real window** is headless-proven (paint values) and snapshot-proven for the
  default; a windowed System→Dark switch was not screenshotted.
- **The panel is a column, not an overlay.** It narrows the content area by 320 pt while open; an
  overlay above a hosted page remains impossible with a child web view (D4/D8).
- **DPI.** Since slice 13 the scale is no longer only the build-time `Runtime::env.scale`:
  the frame poll reads the window's real DPI each frame (`GetDpiForWindow` on the parent
  HWND; the headless tests stand in for that read) and re-scales the pages on a sane change.
  What is **not** verified here is the windowed observation itself: no monitor with a
  different scale factor was switched to in a running window, so the live read — the values
  Windows actually reports for this window, and the timing against winit's own
  `ScaleFactorChanged` resize — has never run against a real monitor change. Placement across a
  display-scale change is likewise unmeasured: the
  size is stored in points, the position in physical pixels, and the two can disagree after a
  DPI change.
- **Window recreation and multiple windows.** `window_created` stores one parent handle per thread
  and the host is built once; a second window (or a Wayland-style close-and-reopen) would reuse the
  first host. The single-window case is what ran.
- **Non-Windows.** The same seam compiles to `WebviewError::Unsupported`; macOS/Linux child web views
  are not exercised.
- WebView2 runtime presence on target machines (Evergreen runtime) is a packaging assumption, not
  checked by any test here.
- TDD note, stated plainly: slices 2–3 wrote tests and implementation in one pass and went green on
  first execution, so they have no red→green record (slice 1's red was the missing-module compile
  failure). Slice 4 had a real one: three assertions failed against the first implementation —
  Gmail's configurator URL, `exiting` elements still counting as shown, and the Installed marker
  outliving its uninstall — and each failure changed the code or the predicate (D6) rather than the
  expectation.

## Consequences

- The shell is a pure function of `ShellState`, so every later slice adds state and a view branch,
  not a new event loop.
- The webview seam is a trait, so the pane is developed and tested without a window, and a second
  platform adapter (macOS `WKWebView`, Linux `WebKitGTK`) is a new impl of four methods.
- Each tab keeps its page alive while another shows: switching is `set_visible`, not a reload. That
  is the port's behaviour parity with Electron's `<webview>` tags, and the reason a tab's first
  activation is the expensive one.
- The web view is a sibling `HWND`, so a page cannot be blended with Rune UI (no translucency, no
  rounded clipping by Rune) and Rune cannot draw over it.
- Rune is pre-release and pinned by rev; bumping it means re-vendoring five patched crates with
  Rune's `scripts/vendor-*.sh` and re-running the headless tests.
- Chrome and app content have different compositors. Rune-drawn UI cannot float above app content;
  overlays that must (dialogs, menus over a web page) are separate Rune windows.
