# Station Desktop Application

Station is a native Rust desktop workspace browser built with **Rune** (native GPU shell UI) and **WebView2** (`wry`, Windows 10/11) under [`rust/`](rust/).

> The legacy Electron/TypeScript implementation (`packages/`) has been retired into git history; see [`docs/full-rust-port-program.md`](docs/full-rust-port-program.md) and [`docs/rust-rune-port.md`](docs/rust-rune-port.md) for the port architecture and product decisions.

## Requirements

- **Rust stable toolchain** (see [`rust/rust-toolchain.toml`](rust/rust-toolchain.toml) or install via [rustup](https://rustup.rs))
- **Windows 10/11 (x64, MSVC)** for running the desktop shell with live WebView2 pages (WebView2 Evergreen runtime is preinstalled on Windows 10/11 via Microsoft Edge)
- Linux/macOS hosts can build, lint, test all 13 workspace crates, render headless Rune snapshots (`--snap`), and cross-typecheck Windows paths via `x86_64-pc-windows-gnu`
- On Debian/Ubuntu, install Rune's build dependencies with `sudo apt-get install libxkbcommon-dev libxkbcommon-x11-dev`.

## Build, Run & Test

```bash
cd rust

# Run the native shell (Windows)
cargo run -p station-shell

# Build a release binary
cargo build --release -p station-shell

# Format, lint, and test all 13 workspace crates
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Headless UI Snapshots

```bash
cd rust
cargo run -p station-shell -- --snap shot.png
cargo run -p station-shell -- --snap settings.png --scene settings
cargo run -p station-shell -- --snap notifications.png --scene notifications
cargo run -p station-shell -- --snap about.png --scene about
cargo run -p station-shell -- --snap quick-switch.png --scene quick-switch
cargo run -p station-shell -- --snap custom-app.png --scene custom-app
cargo run -p station-shell -- --snap downloads.png --scene downloads
```

## Windows Packaging (NSIS)

Build the per-user NSIS installer (`release/station-shell-setup-<VERSION>.exe`) after building the release shell and staging the Microsoft Edge WebView2 bootstrapper at `rust/packaging/dist/MicrosoftEdgeWebview2Setup.exe`:

```powershell
cargo build --release -p station-shell --manifest-path rust/Cargo.toml
makensis /DVERSION=0.1.0 rust/packaging/installer.nsi
```

See [`.github/workflows/release-candidate.yml`](.github/workflows/release-candidate.yml) for the automated `windows-2025` build and optional Authenticode signing flow (`STATION_SIGNING_PFX_BASE64` / `STATION_SIGNING_PFX_PASSWORD`).

## Configuration & Data

| Item | Location / Env |
| --- | --- |
| Session document (`version: 1`, atomic writes) | `%APPDATA%\station\session.json` (Windows), `$XDG_CONFIG_HOME/station/session.json` (Linux), `~/Library/Application Support/station/session.json` (macOS); override with `STATION_SESSION_PATH` |
| Private / custom app manifests | `<config dir>/BrowserX/private-manifests.json` |
| Crash reports (8 KiB cap, 20 newest kept) | `<config dir>/station/crashes/` |
| Google OAuth Option B (PKCE + loopback) | Set `GOOGLE_CLIENT_ID` (and optional `GOOGLE_CLIENT_SECRET`) in the environment |

### Import from Electron Station

Quit both the Electron and native versions of Station. Run the import before saving a native
session. The first-run picker shows the import command when it finds the default Electron
database.

```powershell
& "$env:LOCALAPPDATA\Programs\Station\station-shell.exe" --import-electron
```

The default source is `%APPDATA%\Stationv2\db\station.db`. Pass the database path after
`--import-electron` for a custom Electron profile. The import creates a native session only when
its session file is absent and leaves the old database untouched. It brings across the app list,
rail order, favorites, and supported general and app settings. Open tab links from apps on your
rail appear in the Quick Switcher under Favorites, which has room for 256 links including
existing favorites. Custom app names and icons from the legacy private manifest file must be
recreated. Sign in to your web apps again after importing.

### Browser extensions and password saving on Windows

Open a web app, then go to **Settings → Browser extensions** to install an unpacked extension
from a folder you trust. Extensions are installed in that app's WebView2 profile. Open each app
whose profile should use the extension.

Enable **Offer to save website passwords** in Settings to let WebView2 offer to save new
passwords in its own profiles. This starts off by default. Previously saved passwords can still
autofill when the setting is off.

## Documentation

- [`rust/README.md`](rust/README.md) — workspace crates, vendored Rune patches, and feature status
- [`docs/rust-rune-port.md`](docs/rust-rune-port.md) — shell architecture ADR, robustness limits, and slice log
- [`docs/full-rust-port-program.md`](docs/full-rust-port-program.md) — feature ledger and Electron retirement program
- [`docs/oauth-config-options.md`](docs/oauth-config-options.md) — Google OAuth Option B design
- [`docs/rust-packaging-plan.md`](docs/rust-packaging-plan.md) — NSIS installer contract
