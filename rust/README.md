# Station's Rust workspace

The native shell and the services behind it. The Electron app in the repository root is still
present and unaffected; this workspace is where the Rust port lives, one crate per concern.

```bash
cd rust
cargo run -p station-shell          # the native shell (Windows; see requirements)
cargo test --workspace              # every crate's tests (587 passing + 1 ignored live-Chrome OAuth test)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

All 13 crates (including `station-bridge`) pass `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` with zero exclusions.

## Requirements

- **Windows 10/11, MSVC toolchain** (`x86_64-pc-windows-msvc`, rustup's default). Rune needs no
  extra system libraries beyond the VS C++ build tools rustup asks for. (Cross-typechecking all
  `#[cfg(target_os = "windows")]` code from Linux also works via `cargo check -p station-shell
  --target x86_64-pc-windows-gnu` with `mingw-w64`.)
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
cargo run -p station-shell -- --snap a.png --scene about           # About Station screen
cargo run -p station-shell -- --snap q.png --scene quick-switch    # Bang Quick Switcher palette
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

What works and what does not, kept honest (updated through Slice 30, 2026-11-23):

- **App pages & per-site scripts**: installed apps open in a WebView2 child at their manifest start
  URL. `site_scripts.rs` (slice 23) injects the 7 static IIFE per-site scripts covering all 14
  Electron `JAVASCRIPT_INJECTIONS` legacy service IDs (Gmail, Slack, WhatsApp, Messenger,
  Outlook/Office 365, Google Drive/Calendar/Cloud/Keep/Meet, Asana) alongside `INIT_SCRIPT`.
- **Page title & badge**: a hosted page's `document.title` becomes the top bar and rail label (the
  manifest name is the fallback, and clearing restores it), and its `meta[name=browserx-badge]` meta
  draws a rail badge — a count (`999+` past the cap) or a dot for a text badge. Both are in memory
  only, never in the session document. Not covered yet: favicons and per-tab history lists.
- **Page environment & permissions**: a new page is created with the Electron session's user agent
  (`packages/app/src/session.ts:67` — the Mac/Chrome 150 string), or with the manifest's
  `bx_override_user_agent` for that app; each app gets its own WebView2 user-data folder under
  `<session dir>/webview/app-<id>` (Electron's `persist:<applicationId>`), while apps with
  `bx_use_default_session` share `webview/default`. Both are fixed when the web view is built.
  **Permissions** (slice 30): `permissions.rs` is wired to `WebViewBuilder::with_permission_handler`,
  denying WebView2's native web `Notifications` (routed through Station's bridge) and silent /
  privileged APIs (`AutomaticDownloads`, `FileSystemAccess`, `LocalFonts`, `WindowManagement`,
  `Midi`, `Sensors`, `PointerLock`, `MediaKeySystemAccess`, `Other`) while delegating interactive
  capabilities (`Camera`, `Microphone`, `DisplayCapture`, `Geolocation`, `ClipboardRead`,
  `Autoplay`) to WebView2's native prompt. Not reproduced: per-request HTTP header interception
  (`accounts.google.com` UA override, localhost Referer stripping, CSP deletion), which wry does
  not expose. **Downloads** (slice 22): page-suggested names are sanitized (including compound
  Windows reserved names like `NUL.tar.gz`), written under OS Downloads without overwriting, capped
  at 4 concurrent, and logged bounded; no download toaster UI.
- **Navigation, tab actions, in-tab search & Quick Switcher**:
  - Top bar includes Back/Fwd/Reload/Find/Star/Browser/`...`/Jump controls (`placeholder.rs`).
  - Right-clicking a rail app or pressing `...` opens the 40 pt Rune `tab_actions_bar` (Reload,
    Pin/Unpin favorite, Open in browser, Configure, Move up/down, Uninstall), shrinking `content`
    cleanly below it so WebView2 never covers it (slice 26).
  - Global shortcuts: `Escape`, `Ctrl/Cmd+K`/`T` (Bang Quick Switcher), `Ctrl/Cmd+F` (in-tab find),
    `Ctrl/Cmd+,` (Settings), `Ctrl/Cmd+1..9` (rail jump), `Ctrl/Cmd+R`/`F5` (reload),
    `Ctrl/Cmd+[`/`]` (back/forward), `Ctrl/Cmd+0`/`=`/`+`/`-` (per-tab zoom).
  - **In-tab search** (slice 27, `find.rs`): 40 pt Rune bar above `content`, literal DOM search via
    JSON-escaped `window.__stationFind` / `__stationFindClear` (no `RegExp` on user text), match
    counts over IPC (`kind: "find"`). Verified live in Chrome via `agent-browser`.
  - **Bang Quick Switcher & Favorites** (slice 28, `quick_switch.rs`): searches pinned favorites
    (persisted in `session-store` v1), installed tabs ranked by `activity::frecency`, shell actions,
    and App Store quick-install matches.
- **About Station, fatal dialogs & crash reporting**:
  - **About Station** (slice 24, `about.rs`): native Rune screen over `about-window`, reachable from
    Settings → General and Quick Switcher, with v1 "Check for updates" opening GitHub Releases.
  - **Fatal dialogs & crash reports** (slices 24 & 30, `dialogs.rs`, `crash.rs`): panic hook writes
    bounded (8 KiB) secret-free reports to `<config dir>/station/crashes/` (pruned at 20 files),
    and `catch_unwind` around `app.run()` shows a Win32 `MessageBoxW` (stderr fallback elsewhere).
- **New windows, deep links & single instance**: a page's `window.open`/`target=_blank` never opens
  an OS window (`url-router` routes to a tab or system browser). Launched and mid-run `station://`
  deep links are validated (`deep_link.rs`) and handed off to the primary instance over loopback TCP
  behind Windows named mutex `Local\station-shell-singleton` (`instance.rs`).
- **Notifications, OS balloons & OS DND**: hosted pages use `new Notification(...)` or frozen
  `window.bxApi.notificationCenter` (all 6 members including snooze listeners; `notifications/bridge.rs`).
  Snooze changes fan out to attached pages via `window.__stationSnoozeUpdate(...)` (muting WhatsApp /
  Messenger audio). On Windows, new unsnoozed notifications also dispatch Win32 `Shell_NotifyIconW`
  (`NIF_INFO`) balloons with a 12/min rolling cap (`notifications/os.rs`), and `Snooze::SyncWithOs`
  polls `SHQueryUserNotificationState` every 5 s to mirror host Focus Assist / Do Not Disturb.
  (Win32 balloon appearance and live `SHQueryUserNotificationState` readings are cross-typechecked
  for Windows GNU and unit-tested via fakes; visual verification requires a real Windows host.)
- **Tray & launch at login**: Windows notification-area icon (`tray.rs`, Open/Exit) and unelevated
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Station` toggle (`autolaunch.rs`, Settings →
  General). Closing the window quits; minimize-to-tray is not ported.
- **Identity / Google OAuth Option B** (slices 17 & 25, `oauth.rs`): RFC 7636 PKCE (`S256`) +
  ephemeral loopback HTTP callback (`127.0.0.1:0`, constant-length state check, strict CSP static
  HTML, zero tokens stored on disk), configured via `GOOGLE_CLIENT_ID` (optional
  `GOOGLE_CLIENT_SECRET`), with Settings Accounts sign-in / cancel / remove. Verified end-to-end
  against real Chrome (`oauth::tests::live_chrome_agent_browser_oauth_callback_verify`); a live
  round-trip against Google's production endpoint requires a human-registered Desktop OAuth client.
- **Packaging & remaining product decisions**: `rust/packaging/installer.nsi` (per-user HKCU NSIS
  installer with WebView2 bootstrapper and `station://` registration) is built and locally smoked;
  clean-VM verification, code signing identity, and product decisions on Chrome extensions, password
  managers, SDK runtime apps, and full staged auto-updater remain human-gated before Electron
  deletion (`P6–P8`).
