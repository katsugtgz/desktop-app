# Station's Rust workspace

The native shell and the services behind it. The Electron app in the repository root is still
present and unaffected; this workspace is where the Rust port lives, one crate per concern.

```bash
cd rust
cargo run -p station-shell          # the native shell (Windows; see requirements)
cargo test --workspace              # every crate's tests (539 passing)
cargo clippy --workspace --all-targets --exclude station-bridge -- -D warnings   # see note
cargo fmt --all --check
```

`station-bridge` (the Electron napi crate) fails `-D warnings` on its own *lib test* target: the
`#[napi]` watcher/emit functions are only reachable through generated bindings, so the test build
sees them as dead code. Exclude it (as above) or drop `-D warnings` for a workspace-wide run; the
crate is otherwise untouched by the shell work.

## Requirements

- **Windows 10/11, MSVC toolchain** (`x86_64-pc-windows-msvc`, rustup's default). Rune needs no
  extra system libraries beyond the VS C++ build tools rustup asks for.
- **WebView2 Evergreen runtime** — what an installed Edge on Windows 10/11 already provides
  (`C:\Program Files (x86)\Microsoft\EdgeWebView\Application`; version 157.0.4341.46 was used for
  verification). The shell's app pages are WebView2 child windows; without the runtime a page
  cannot be created and the shell logs `station-shell: no web view for <app>: …` and keeps running.
- The workspace builds with the stable toolchain in `rust-toolchain.toml` (1.98 was used).

## Running the shell

```bash
cargo run -p station-shell                          # a window at the session's size and place
cargo run -p station-shell -- --snap shot.png       # one headless frame, no window
cargo run -p station-shell -- --snap s.png --scene settings        # the Settings screen
cargo run -p station-shell -- --snap n.png --scene notifications   # the notification panel
```

`--snap` and the other headless flags come from Rune's `tooling` feature, which
`crates/station-shell/Cargo.toml` enables so they also work in `--release` builds (a release
snapshot is the same frame; only the binary is ~25 MiB instead of a debug build).

The shell is Rune-rendered: the rail, the App Store, Settings and the notification panel are Rune
views; each installed app's page is a WebView2 child window placed on the content pane (the rail
and top bar stay Rune's). A Rune surface hides that page rather than drawing over it, and returning
shows the same page: the windowed probe (`docs/surface-hide-probe.md`) flipped the page's
`WRY_WEBVIEW` from visible to hidden and back with the same HWND and rect for the App Store, the
rail's app row, Settings and the notification panel (Escape or a second press restores). What Rune
painted is not observed — no screenshot was taken. The App Store installs apps into the rail; a
store tile on an installed app uninstalls it, and Settings can also uninstall.

## Data

| What | Where |
| --- | --- |
| Session (window placement, appearance, installed apps, dock, tabs, active app, per-app configuration) | `<config dir>/station/session.json` — `%APPDATA%\station\session.json` on Windows, `$XDG_CONFIG_HOME`/`~/.config` or `~/Library/Application Support` elsewhere |
| Private (custom) app manifests | `<config dir>/BrowserX/private-manifests.json`, written by `manifest-registry` |

A session's window size is stored up to 8192 pt (a 4K or ultrawide window is kept as it was), the
window opens at most 2048 pt (the display scale is unknown until it exists), and the restore step
shrinks anything the display cannot give the renderer's 8192-pixel readback limit.

`STATION_SESSION_PATH` overrides the session file (tests and smoke runs use it). The session is
written atomically after the user acts — install, uninstall, selection, appearance, app
configuration, window close and quit — and a session that cannot be read is reported on stderr and
left untouched until the user changes something. Per-app configuration (`appConfig`) is additive:
absent means "not configured"; values over 2 048 chars and keys naming uninstalled apps are
refused with the app id, never truncated.

## Crates

| Crate | What it is |
| --- | --- |
| `station-shell` | The native shell: Rune views, WebView2 pane, session, URL routing |
| `appstore-service` | Install/uninstall/restore and the store's read commands over the manifest registry |
| `manifest-registry` | Bundled manifests, private manifests on disk, scope and fuzzy search |
| `appstore-schema` | The appstore GraphQL SDL and its serde types |
| `activity` | Activity/frecency store (sqlx SQLite) |
| `session-store` | The durable session document (`version: 1`, atomic writes) |
| `notification-center` | The notification center state machine (groups, mark-read, snooze) |
| `url-router` | Where a URL belongs: an installed app's tab, a new tab, or the default browser |
| `webui` | `station://` request → file resolution |
| `about-window`, `tab-model`, `station-sdk`, `station-bridge` | Ported support crates (see `docs/rust-rewrite-plan.md`) |

## Rune pin and vendored patches

Rune is pinned by revision in `crates/station-shell/Cargo.toml`:

```toml
rune = { git = "https://github.com/anomalyco/rune", rev = "10a39584cc1ff739477be52e333a742a024ac956" }
```

That revision is built against **five patched published crates**. A dependency's `[patch]` does not
apply to the workspace that depends on it, so `rust/Cargo.toml` carries the patches itself and
`rust/vendor/` holds verbatim copies of Rune's `vendor/{winit,wgpu-hal,parley_engine,accesskit_windows,accesskit_macos}`
plus the `.patch` deltas. Rune's own note is explicit — "apps that build Rune patch it too".
Removing them breaks resolution (`parley_engine` has no `line-dictionaries` feature) and the Windows
build (`wgpu-hal::dx12::Instance::expose_adapter`, `winit::platform::windows::warm_keyboard_layout`).
Bumping the Rune rev means re-vendoring with Rune's `scripts/vendor-*.sh` and re-running the tests.

## Verified on

Verification environment for the claims in this file: Windows 10 (19045) x64, MSVC, WebView2
Evergreen 157.0.4341.46, 2026-11-18. **Pages render in this desktop session** — see
`docs/webview2-probe.md`: one launch of the release binary, seeded with Gmail, spawned eight
`msedgewebview2.exe` processes (browser, crashpad, GPU, network/storage/audio utilities, two
renderers), created the `WRY_WEBVIEW` child at the content rect, and exposed a visible Chromium
render widget titled "Sign in - Google Accounts" — real page content, the signed-out landing Gmail
reaches without identity config. The limits of that observation: n=1, no pixels captured (process
tree, window classes/titles and visibility are the evidence), and the binary changed between the
earlier failing runs and this probe (29,135,872 → 29,301,760 B), so why the earlier `0x80070057`
failures happened is still unknown. Without a runtime the shell still takes its documented degraded
path: it logs `station-shell: no web view for <app>: …` and keeps running.

## Feature gaps

What works and what does not, kept honest:

- **App pages**: installed apps open in a WebView2 child at their manifest start URL. Multi-instance
  preset apps (Gmail, Slack) open signed-out (Gmail's account picker); the
  `station://multi-instance-configurator` OAuth flow is not drawn yet.
- **Page title & badge**: a hosted page's `document.title` becomes the top bar and rail label (the
  manifest name is the fallback, and clearing restores it), and its
  `meta[name=browserx-badge]` meta draws a rail badge — a count (`999+` past the cap) or a dot for
  a text badge. Both are in memory only, never in the session document. Not covered yet: the
  per-site scripts that *write* the badge meta for Gmail/Slack/Outlook/…, favicons, and per-tab
  history.
- **Page environment**: a new page is created with the Electron session's user agent
  (`packages/app/src/session.ts:67` — the Mac/Chrome 150 string), or with the manifest's
  `bx_override_user_agent` for that app; each app gets its own WebView2 user-data folder under
  `<session dir>/webview/app-<id>` (Electron's `persist:<applicationId>`), while apps with
  `bx_use_default_session` share `webview/default`. Both are fixed when the web view is built; a
  page cannot change either. Not reproduced: the TS per-request header rules — the
  `accounts.google.com` UA override, localhost Referer stripping and CSP-header deletion — because
  wry exposes no header interception (`with_user_agent` is per-webview, and
  `with_url_and_headers` only sets the first load's headers). **Downloads** are allowed: the
  page-suggested name is sanitized and placed under the OS Downloads folder (never a page path,
  no overwrites, at most 4 concurrent — `downloads.rs`, slice 22), and started / complete / failed
  are logged (at most 32 lines per run, URL and path capped at 256 chars). There is no download
  UI or counter.
- **New windows**: a page's `window.open`/`target=_blank` never opens an OS window; `url-router`
  routes it to an app's tab, or the system browser. Install-from-link is not wired.
- **Deep links & single instance**: a `station://` URL the process is launched with is read
  once at startup (`deep_link::open` in `AppSpec::setup`; the OS registration itself is a
  packaging step, `docs/rust-packaging-plan.md` §7). `station://appstore` and
  `station://appstore/` open with the App Store tab selected; any other `station:` URL — another
  host, a deeper path, a query or fragment — is one stderr line and nothing else. The argument is
  bounded (2 048 chars, control characters and invalid UTF-8 refused, parsed with the same WHATWG
  parser as routing). Nothing else of the `station://` WebUI set is served (the configurator is
  not), and a link never opens the OS browser or installs anything. **A second launch does not
  open a second window**: on Windows a per-session mutex (`Local\station-shell-singleton`) makes
  the first process the primary; a second one forwards its deep link — or, with no link, just
  asks for focus — to the primary over a loopback TCP listener (`<config dir>/station/instance.json`
  names the port and a per-run token), then exits. The primary selects the store or names-and-ignores
  the URL and brings its window forward. The lock is per-Windows-session only; other platforms run
  lock-free (documented `Unsupported`), and a lock failure never keeps the shell from opening.
- **Notifications**: a hosted page raises notifications the way real pages do — `new
  Notification(title, options)`, routed by the injected script's `window.Notification` override
  (the port of Electron's `webview-inject.js`) into
  `window.bxApi.notificationCenter.sendNotification(id, notification)`. Pages may also call the
  preload's four members directly (`sendNotification` / `closeNotification` and the click-listener
  pair; wry's `with_ipc_handler` plus one injected init script, capped at 8 listeners per page;
  nothing else of `bxApi` is invented). The override keeps the TS contract — `title` required,
  `notif/<nanoid>` ids, `permission`/`requestPermission` report `granted`, `close()` removes the
  row, and `onclick`/`addEventListener('click')` fire through the same click registry. Dropped
  from the TS: the `blob:` icon fetch (Telegram-only) and the recursive same-origin iframe
  override. Messages are validated and
  bounded (caps in `docs/rust-rune-port.md` "Robustness"), delivered into the ported
  `NotificationCenter` under the sending app's group, and the panel shows them — each row drawing
  the notification's own `icon` URL when one was sent (decoded pixels when they land, the sender's
  letter avatar until then and for every refused/broken URL; at most 32 distinct notification
  icons are asked for per frame, "Robustness"). A row with a tab is
  pressable: the click marks the notification read, focuses the sender's tab (`url-router`-routed;
  an external URL still goes through the allowlist + budget path), and delivers one
  `window.__stationNotificationClick(<json id>)` to the sender's page, where a registered click
  listener runs (an absent page is skipped silently). Snooze expires from the frame poll (at most
  one scan per second). Still missing: an OS notification source and the OS "do not disturb"
  reader behind `Snooze::SyncWithOs` (stored, never expires, never claimed to).
- **Tray**: while the shell runs it keeps Station's notification-area icon (Rune's `Tray`:
  `Shell_NotifyIcon` on a message-only window and thread) with the Electron tray's two-item menu
  — **Open** brings the window forward through the single-instance focus path, **Exit** quits
  through the one `on_quit`. A primary click focuses (Electron listened for `double-click`; Rune
  reports one primary click as `Activate`, so the same action takes one click here). No settings
  entry and no notification list — the TS tray had neither. Windows only; elsewhere there is no
  tray. Observed windowed: the `RuneTray` window in the shell's process, activation restoring a
  minimized shell with foreground, and a clean exit; the menu's look and its Exit pick are not
  observed (the popup is modal), and neither are the icon's pixels in the notification area.
- **Launch at login**: Settings → General has "Start Station when I sign in" on Windows, backed
  by `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Station` — the executable path in
  quotes, no arguments, unelevated, the same value `auto-launch@5.0.6` wrote for Electron. The
  session records it as the additive v1 `startAtLogin` bool, and a real launch shows the
  registry's truth; a failed read or write is said in a "Login item" row instead of a toggle that
  lies. Other platforms draw no row (documented `Unsupported`). No start-minimized argument: the
  TS autolaunch passed none either. The real registry write is not exercised by tests (a fake
  login item is used), and no run here modified the user's login items.
- **Window close**: unchanged — closing quits. Electron's tray also hid the window on *minimize*
  when its icon was visible (`MainWindowManager`); this slice did not port that and did not
  invent close-to-tray.
- **Session**: app pages are recreated from their URL; per-page scroll/history, the store's
  section and search text are not restored (the rail order and selection are). A monitor-DPI
  change is detected each frame from the window's real DPI (`GetDpiForWindow`, 0.5–4.0×
  accepted) and re-scales every page; a real monitor switch remains unobserved.
- **Rail order**: the user's order is the session's `railOrder`, restored on launch and
  reordered from Settings → My apps (move up / move down per row, written at once). The rail
  rows themselves have no reorder keys — Rune gives a rail row no keyboard focus (D11 in
  `docs/rust-rune-port.md`).
- **Store**: installs/uninstalls and the tree/buckets/search render; tiles and rail rows draw the
  manifest's real icon — fetched once per run from the manifest's `icons[0].src` URL, cached on
  disk under `<config dir>/station/icons`, bounded (http/https only, 1 MiB body, 10 s timeout,
  8192-px side, 1 500 files, 128 in flight — the Robustness table in `docs/rust-rune-port.md`).
  A missing, refused, or failed icon keeps the letter avatar. No custom-app request flow in the UI.
- **Window identity**: the window carries Station's icon (title bar + taskbar), applied once from
  the raw handle (`WM_SETICON`, 32/64 px) using `packages/app/build/icon.ico`'s own 32 px and
  64 px `RT_ICON` entries — the Electron build's asset, with no decode or resize
  (`platform_icon.rs`). A windowed probe on 2026-11-18 observed `WM_GETICON` answer with a 32×32
  `ICON_SMALL` and a 64×64 `ICON_BIG` (both confirmed through `GetIconInfo`), the class-icon
  fallback unused (`docs/window-icon-probe.md`). Pixel appearance and taskbar rendering specifics
  are still not photo-verified. The exe itself also carries the icon as a build-time resource:
  `crates/station-shell/build.rs` compiles `station.rc` (one `ICON` line pointing at
  `packages/app/build/icon.ico`) with `embed-resource`, so Explorer and the taskbar show the
  real icon for the bare exe — no runtime change.
- **Identity/accounts**: accounts exist as data — the session stores identities (the TS
  `StationUserIdentity` shape, no tokens), Settings lists them under **Accounts**, and a
  `google-account` app's Configure editor chooses a stored one, writing `appConfig.identityId`
  through the existing config path (the tab is re-resolved; Gmail's URL stays signed-out because
  its template's email field is empty in both implementations). What does **not** exist yet:
  sign-in (nothing creates an identity; the empty state says so) and any token-backed call.
  `station://` cannot be Google's OAuth redirect (a custom scheme needs a period —
  `docs/oauth-config-options.md`), so the later callback is loopback.
- **Packaging**: the shell runs from `cargo run`/`cargo build --release`; no installer or code
  signing. Electron packaging is untouched.
