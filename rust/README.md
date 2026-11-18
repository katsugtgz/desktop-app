# Station's Rust workspace

The native shell and the services behind it. The Electron app in the repository root is still
present and unaffected; this workspace is where the Rust port lives, one crate per concern.

```bash
cd rust
cargo run -p station-shell          # the native shell (Windows; see requirements)
cargo test --workspace              # every crate's tests (292 passing)
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
and top bar stay Rune's). The App Store installs apps into the rail; a store tile on an installed
app uninstalls it, and Settings can also uninstall.

## Data

| What | Where |
| --- | --- |
| Session (window placement, appearance, installed apps, dock, tabs, active app) | `<config dir>/station/session.json` — `%APPDATA%\station\session.json` on Windows, `$XDG_CONFIG_HOME`/`~/.config` or `~/Library/Application Support` elsewhere |
| Private (custom) app manifests | `<config dir>/BrowserX/private-manifests.json`, written by `manifest-registry` |

A session's window size is stored up to 8192 pt (a 4K or ultrawide window is kept as it was), the
window opens at most 2048 pt (the display scale is unknown until it exists), and the restore step
shrinks anything the display cannot give the renderer's 8192-pixel readback limit.

`STATION_SESSION_PATH` overrides the session file (tests and smoke runs use it). The session is
written atomically after the user acts — install, uninstall, selection, appearance, window close
and quit — and a session that cannot be read is reported on stderr and left untouched until the
user changes something.

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
Evergreen 157.0.4341.46, 2026-11-18. **Page rendering is not currently verified on that machine.**
On the 2026-11-18 independent verification the WebView2 browser process failed to start in this
desktop session — `0x80070057` from the attach path and no `msedgewebview2` process, 10/10
reproductions including a fresh profile and an explicit browser folder. The child `HWND` was still
created at the exact content rect and the shell took its documented degraded path (it logs
`station-shell: no web view for <app>: …` and keeps running). Earlier smoke runs the same workday
did render pages (`navigation started` / `page load finished`), so this is environment-dependent
and needs a retest in another desktop session before page rendering is claimed as working.

## Feature gaps

What works and what does not, kept honest:

- **App pages**: installed apps open in a WebView2 child at their manifest start URL. Multi-instance
  preset apps (Gmail, Slack) open signed-out (Gmail's account picker); the
  `station://multi-instance-configurator` OAuth flow is not drawn yet.
- **New windows**: a page's `window.open`/`target=_blank` never opens an OS window; `url-router`
  routes it to an app's tab, or the system browser. Install-from-link is not wired.
- **Notifications**: the panel, groups, mark-all and snooze are the ported state machine, but
  nothing produces notifications yet (no OS source, no page bridge) and snooze never expires.
- **Session**: app pages are recreated from their URL; per-page scroll/history, the store's section,
  search text and rail order are not restored. A monitor-DPI change leaves a page at its old scale.
- **Store**: installs/uninstalls and the tree/buckets/search render; icons are letter avatars (the
  manifest icon URL is unused), and there is no custom-app request flow in the UI.
- **Identity/accounts**: not ported (no login, no per-app config UI).
- **Packaging**: the shell runs from `cargo run`/`cargo build --release`; no installer or code
  signing. Electron packaging is untouched.
