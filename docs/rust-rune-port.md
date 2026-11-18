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
active application; a missing file is simply a fresh shell. `railOrder` is written for a future
custom order — the rail is the dock's order today, so restore reads `dock`.

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
- **Nothing produces notifications yet**: there is no OS source and no web-view bridge, so the
  center stays empty in a running shell and only tests (and snapshot scenes) populate it. The
  panel's chrome, grouping, mark-all and snooze are proven headlessly; wiring a source is a later
  slice.
- **Preset apps now open a page**: `ApplicationService::resolve_start_url(manifest_url,
  application_id, config)` exposes the install-time decision, and `start_url_for_install` no
  longer falls straight to `station://multi-instance-configurator/` when a preset app has no
  identity/config — it returns the manifest's own formatted start URL (Gmail →
  `https://accounts.google.com/AddSession?…`, the signed-out account picker). This is a deliberate
  divergence from the TS saga, recorded here: the native shell does not draw the configurator yet.
  A raw config the presets do not use (`resolve_start_url` takes the caller's config verbatim)
  still yields the configurator, which is the path OAuth will drive.
- **Windowed evidence** (below) shows the page hidden while Settings or the panel is up and back
  after, and Gmail's page navigating to `accounts.google.com` rather than blank.

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

## Robustness

Limits and policies added by the hardening pass (2026-11-18), each covered by a test.

| Concern | Limit / policy |
| --- | --- |
| New-window request queue (`routing`) | **64** requests; a duplicate `(origin, url)` already queued is dropped; a full queue refuses new ones. A page's `window.open` loop cannot grow memory. |
| Browser hand-off, rolling budget (`routing`) | **10** browser launches in **any rolling 60 s**, globally across tabs and independent of the frame rate (`MAX_EXTERNAL_PER_WINDOW` / `EXTERNAL_WINDOW_S`; timestamps pruned lazily). A page looping `window.open` across frames cannot exceed it; refused requests are dropped, never queued. |
| Browser hand-off per frame (`routing`) | **5** external opens per frame, each distinct URL once, as an inner guard on the rolling budget. Beyond the cap a frame's requests are dropped, not deferred. |
| External schemes (`routing`) | **http, https, mailto** only. `file:`, `javascript:`, `data:`, app schemes and bare paths are refused (logged) and never reach the OS. On Windows the URL goes through `ShellExecuteW` — one string, no shell — so `&`, `|`, quotes are data (the old `cmd /C start` gave a page-controlled URL to a command interpreter). |
| Live pages (`webview`) | **5** attached pages. Attaching a sixth closes the least-recently-*shown* page that is not the active one; its bounds, failure and recency go with it, and re-selecting recreates it. |
| Attach retries (`webview`) | 4 attempts (the first plus three), delayed **1 s, 5 s, 30 s**. Switching to the failed tab or leaving/returning from Settings or the notification panel retries at once. A retargeted URL is a new page and clears the failure. After the fourth failure the shell waits for the user. `webview::retry` (the pane's **Retry**) forgets the failure and starts over. Recurring failure logs are one line per message per **10 s**. |
| Failed page, made visible | `ShellState::page_error` (`{ application, detail }`) is set from the frame/poll path, and the pane shows **"Could not load <app>"**, the host's error and a Retry control. It is cleared when the page attaches, on retry, and when the tab is closed. |
| Session document | Refused unread above **16 MiB**. Collections are refused (not truncated) past **512** installed, **512** dock, **2048** tabs, **512** rail entries — a refusal keeps the file's referential integrity. Window size: non-finite or ≤ 0 falls back to 1200×800, otherwise clamped to **320×240 … 8192×8192 points** — the *stored* bound is scale-unaware so a 3840-pt 4K window or a 5120×1440 ultrawide is kept as it was. |
| Window launch and restore | **Launch**: the display scale is unknown before the window exists (and `--snap` defaults to 2×), so the window opens at most **2048 pt** (`MAX_LAUNCH_POINTS`; 4096 px at 2×, 8192 px at 4×). **Restore** (`on_window`, real scale known): `restored_size` keeps the session's points up to **8192** and shrinks only what `points × scale` cannot fit under the **8192 device-px** ceiling (the 256 MiB snapshot-readback limit, `8192² × 4 B`): 3840 pt at 1× or 2× stays 3840, 8192 pt at 2× becomes 4096, 16 384 pt at 1× becomes 8192. So a large display comes back at its size and a hostile or over-scaled one shrinks instead of failing. |
| Session writes | Every user action that changes durable state — a selection, an install, an uninstall, an appearance change — is written **immediately** through `session-store`'s temp-file-and-rename, so a hard kill cannot lose the last action (an earlier 250 ms coalescing window was removed for exactly that reason). Close and quit write too. A failed write is reported at most once per **10 s**, the next user action retries it, and the shell keeps running. |
| Pane geometry | A missing pane or a non-finite rectangle hides the visible page instead of placing it at NaN; negative origins clamp to 0 and sides to 16384 points. |
| Display scale | A change in `Runtime::env.scale` calls the host's `set_scale` and re-applies every attached page's bounds; the host converts points to pixels with the new scale. |
| Waker and clock | The wake handle is a thread-local because the only writer is this process's web-view thread, and `AppSpec::poll` runs on **every** frame, so the wake is a latency hint, not a delivery guarantee: a request queued while the loop is idle is still routed by the next frame (a wake makes that frame happen sooner). |
| Degenerate geometry | The shell clamps everything it computes or loads (session sizes, pane rects). A *presented* box that is degenerate for another reason (a view that lays out a NaN size of its own) is Rune's to guard — the renderer does not reject one, and its wgpu path can panic on it; the shell's own paths cannot produce one. |
| Shutdown | `close_all` is idempotent and sets a closed flag: nothing attaches or re-attaches after it until a new host is installed. A poisoned request queue is recovered (`into_inner`), and the waker uses `try_borrow`, so no lock or borrow panic takes a frame down. |
| Private manifests | `PrivateStore::save_new_application` / `delete_manifest` return `io::Result` and roll back on failure (no more `expect`); `ApplicationService::request_private_application` / `delete_private_manifest` map that to `ApplicationError::PrivateStoreWrite`. Writes go through the same **temp file + `sync_all` + rename** pattern as the session (the old file stays on failure, the temp is cleaned up), reads are capped at **8 MiB** and **10 000 records** (a larger file is warned about and treated as empty, like corrupt JSON), and the id counter saturates instead of overflowing on a stored `u64::MAX`. When the config dir cannot be opened, `ShellState::launch` warns and falls back to an **in-memory** store so the window still opens. |

Tests (54 in `station-shell`, 16 in `session-store`, 50 in `appstore-service`): the hostile-page set
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

1. Notifications have no source: no OS notification bridge, no web-view `new-notification` channel,
   no click routing (`NotificationCenter::click_target` unused). The panel is the ported model.
   Related: `InstallAndOpen` routing does not install (the store's install path exists, but a link
   cannot trigger it yet).
2. Snooze is stored but nothing expires it: no loop calls `NotificationCenter::advance_to`, and
   `SYNC_WITH_OS` has no system-state reader.
3. Settings covers appearance, version, session path and app removal only — no accounts, no
   per-app configuration (the configurator/OAuth path), no rail reordering (the session's
   `rail_order` is written but not yet an input).
4. A hosted page's own state (scroll, history, form drafts) is lost when the shell closes; only
   the URL is restored.
5. Monitor-DPI changes still leave a web view at its build-time scale (D7/D4).
6. Icons are letters; the manifest's icon URL is unused.

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
8. Lifecycle hardening — monitor-DPI change (the web view keeps the scale it was built at), one
   webview per tab under memory pressure, crash/reload of a page, focus hand-off.
9. Icons and identity — real app icons from `manifest-registry` instead of letter glyphs; window
   icon and taskbar identity.
10. Bottom rail semantics — snooze, settings entry point, and what the rail's "+" becomes once the
    app directory slice lands (today it selects the App Store tab).
11. Notifications: a real source (OS bridge or page channel), click routing, and snooze expiry.

## Verified

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
- **A real page in the pane.** No web app has loaded in this shell: the store is a Rune view, and the
  one page this slice created (Gmail) navigates to `station://multi-instance-configurator/…`, which
  nothing serves, so it stays blank. Scrolling, input, cookies, permissions and DevTools inside a
  hosted page are untested.
- **The store in a real window.** Headless `Runtime` presses and one snapshot are the visual
  evidence: tiles, sections and search are proven headlessly, but no windowed click-through of the
  store was performed (the windowed runs only enumerated child windows). Scrolling the store's long
  lists in a real window is likewise unmeasured.
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
- **Notifications are model-only.** The panel was exercised with a test-populated center; a running
  shell's center is always empty, because no OS source, web-view bridge or click routing exists
  (Remaining gaps 1–2). Snooze is stored but never expires: nothing calls
  `NotificationCenter::advance_to`, and `SYNC_WITH_OS` has no reader.
- **Settings in a real window** was observed only as hidden/restored page + the snapshot; no
  windowed click on a My-apps row (Open/Uninstall) was scripted, and the screen's scroll with many
  installed apps is unmeasured.
- **Appearance in a real window** is headless-proven (paint values) and snapshot-proven for the
  default; a windowed System→Dark switch was not screenshotted.
- **The panel is a column, not an overlay.** It narrows the content area by 320 pt while open; an
  overlay above a hosted page remains impossible with a child web view (D4/D8).
- **DPI.** The scale is read from `Runtime::env.scale` when the host is built (first frame). Moving
  the window to a monitor with another scale factor would leave the web view at the old scale; not
  handled, not measured.
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
