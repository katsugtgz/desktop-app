# Windows installer (NSIS) — Station native shell

Per-user, no-elevation installer for `station-shell`, implementing
`docs/rust-packaging-plan.md` §5/§7. The build inputs are entirely inside `rust/` — nothing here
reads `packages/` (the Electron tree is being deleted).

## What it installs

| Item | Where |
| --- | --- |
| `station-shell.exe` (release build) | `%LOCALAPPDATA%\Programs\Station\` |
| `station.ico` (copied from `rust/assets/station.ico`) | same dir; window/shortcut/icon file |
| `MicrosoftEdgeWebview2Setup.exe` (Evergreen bootstrapper) | same dir, run only when the runtime is absent |
| Start Menu shortcut `Station.lnk` | current user's Start Menu |
| `station://` protocol | `HKCU\Software\Classes\station` (`URL Protocol` + `"…\station-shell.exe" "%1"`) |
| Uninstall entry | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Station` |

The uninstaller deletes only those files and keys. It **never** touches
`%APPDATA%\station` (`session.json`) or `%APPDATA%\BrowserX` (`private-manifests.json`).

No admin rights are requested anywhere (`RequestExecutionLevel user`, HKCU + `$LOCALAPPDATA` only).

## Prerequisites (human)

- Rust toolchain (repo default `stable`; `rust-toolchain.toml`) and a Windows x64 host.
- **NSIS 3.13** — `choco install nsis -y` (installs to `C:\Program Files (x86)\NSIS`; Chocolatey
  does not add a `makensis` shim, so use the full path below) or the installer from
  <https://nsis.sourceforge.io/Download>.
- The WebView2 Evergreen bootstrapper, downloaded into the gitignored `rust/packaging/dist/`
  (~1.9 MB). CI does this every run; locally:

  ```powershell
  New-Item -ItemType Directory -Force rust\packaging\dist | Out-Null
  Invoke-WebRequest -Uri "https://go.microsoft.com/fwlink/p/?LinkId=2124703" `
    -OutFile "rust\packaging\dist\MicrosoftEdgeWebview2Setup.exe"
  ```

- Optional signing: Windows SDK `signtool.exe` (or Azure Artifact Signing). Unsigned is the
  default; see below.

`rust/packaging/dist/` and `release/` are gitignored (root `.gitignore`: `dist/`, `release/`).

## Build artifacts

| Artifact | Path | Produced by |
| --- | --- | --- |
| Release binary | `rust/target/release/station-shell.exe` | `cargo build --release -p station-shell` |
| Installer | `release/station-shell-setup-<version>.exe` | `makensis /DVERSION=<version> rust/packaging/installer.nsi` |
| WebView2 bootstrapper | `rust/packaging/dist/MicrosoftEdgeWebview2Setup.exe` | the download step above (never committed) |

## Build

PowerShell, from the repository root:

```powershell
cd rust
cargo build --release -p station-shell
New-Item -ItemType Directory -Force packaging\dist | Out-Null
Invoke-WebRequest -Uri "https://go.microsoft.com/fwlink/p/?LinkId=2124703" `
  -OutFile "packaging\dist\MicrosoftEdgeWebview2Setup.exe"
cd ..
New-Item -ItemType Directory -Force release | Out-Null
& "${env:ProgramFiles(x86)}\NSIS\makensis.exe" /DVERSION=0.1.0 rust\packaging\installer.nsi
```

Bash (Git Bash / CI shell):

```bash
cd rust && cargo build --release -p station-shell
mkdir -p packaging/dist
curl -L -o packaging/dist/MicrosoftEdgeWebview2Setup.exe "https://go.microsoft.com/fwlink/p/?LinkId=2124703"
cd .. && mkdir -p release
"/c/Program Files (x86)/NSIS/makensis.exe" /DVERSION=0.1.0 rust/packaging/installer.nsi
```

- `/DVERSION=<ver>` sets the display/build version; the script defaults to `0.1.0` when unset.
- All paths in `installer.nsi` are relative to the script's own directory, so `makensis` may be
  run from any working directory (the commands above still use the repo root for consistency).
- Output: `release/station-shell-setup-<version>.exe`.

## Verification matrix (human-run; not claimed by the local build)

Run these on a clean Windows 10/11 x64 VM; none of them was executed while creating this script
(only the compile step was).

1. **Install without WebView2, as a normal user.** No UAC prompt. Installs under
   `%LOCALAPPDATA%\Programs\Station`. The bootstrapper runs `/silent /install` when the registry
   `pv` value is absent; if it fails, setup still completes and writes
   `%LOCALAPPDATA%\Programs\Station\webview2-install.log` (exit code), and the shell still runs
   with Rune chrome.
2. **Protocol.** `reg query HKCU\Software\Classes\station /s` shows `URL Protocol` and the
   `shell\open\command` value; `start station://appstore` launches the exe. Known gap: the shell
   does not parse a `station://…` argv URL yet (plan §7 note), so the app opens/selects its normal
   surface — record the observed behavior.
3. **Shortcut icon.** Start Menu → Station shows the Station icon; the launched window/taskbar
   icon is the same asset.
4. **Uninstall.** Run `Uninstall.exe` (or Settings → Apps). `%LOCALAPPDATA%\Programs\Station`,
   the Start Menu shortcut and both HKCU keys are gone; `%APPDATA%\station\session.json` and
   `%APPDATA%\BrowserX\private-manifests.json` still exist, byte-identical.
5. **Missing runtime after install.** Uninstall WebView2, launch Station: expect
   `station-shell: no web view for <app>: …` in stderr and a running Rune shell (degraded, not a
   crash).
6. **Upgrade in place.** Install over an existing install: `InstallDirRegKey` keeps the same
   directory; session/private manifests are untouched.

## Signing

The installer is **unsigned by default** — Windows SmartScreen will show its warning (plan §4;
the Electron contract already sets `verifyUpdateCodeSignature: false`). The CI job has an optional
`signtool` step gated on repository secrets `STATION_SIGNING_PFX_BASE64` and
`STATION_SIGNING_PFX_PASSWORD`; with either absent it is a no-op and the unsigned installer is
uploaded. Azure Artifact Signing is not wired yet (human decision 1 in the plan).

## CI

`package-windows` in `.github/workflows/release-candidate.yml` and `.github/workflows/release.yml`
builds the release binary, installs NSIS, downloads the bootstrapper, runs
`makensis /DVERSION=<tag>`, and uploads `release/station-shell-setup-<ver>.exe` as the
`station-shell-setup-<ver>` artifact. Test workflows are untouched.

## Local build record (2026-11-18, this workstation)

NSIS 3.13.0 was installed via `choco install nsis -y` (exit 0). The installer was built three times
as the inputs changed: 10,331,205 B (stale 29,496,832 B binary), 10,356,995 B (rebuilt 29,574,656 B
binary; used for the fresh-session smoke), and finally **10,357,011 B** after the uninstall fix
described below (used for the page-hosting smoke):

```
Output: "...\release\station-shell-setup-0.1.0.exe"
Total size: 10357011 / 31866582 bytes (32.5%)
```

`sha256 release/station-shell-setup-0.1.0.exe` =
`a8f4e71ec582c046354735f1808b536773cf3b9d963b64ef55e7eb75dff9d185`. A `7z l` listing shows the
payload: `station-shell.exe` (29,574,656 B), `station.ico` (133,921 B),
`MicrosoftEdgeWebview2Setup.exe` (1,897,688 B), `Uninstall.exe` (172,878 B). That is a **compile**
proof only; runtime behaviour is in the smoke section below.

## Installer smoke (2026-11-18) — executed; per-user, temp dir, no user-data writes

Final target: `release/station-shell-setup-0.1.0.exe`, **10,357,011 B**, sha256
`a8f4e71ec582c046354735f1808b536773cf3b9d963b64ef55e7eb75dff9d185`. The fresh-session run below
used the preceding 10,356,995 B build; install/launch/uninstall behaviour is identical, and the
rebuild only adds the uninstall fix recorded under “installed binary hosts a page”.

**Before state:** both HKCU keys missing (`ERROR: The system was unable to find the specified
registry key or value.`), `%LOCALAPPDATA%\Programs\Station` and the Start Menu shortcut absent,
WebView2 `pv` = `157.0.4341.60` (HKLM WOW6432Node), 0 `station-shell` / 0 `msedgewebview2`
processes. Existing data: `%APPDATA%\station\session.json` (780 B) sha256
`7282d976f76469ea8e2f50f14021dfb2bf0bf5090dd37d581ff2f02c76dece78`, `%APPDATA%\station\log.log`
sha256 `7ed1720059e0173b8070c3327bc7cd7eced5977bcf12f6c01d07b454fee2b700`; `%APPDATA%\BrowserX`
empty.

**Install.**

```
release\station-shell-setup-0.1.0.exe /S /D=C:\Users\asd\AppData\Local\Temp\station-install-test
  (NSIS /D last, unquoted) → exit code 0
```

Installed: `station-shell.exe` **29,574,656 B**, `station.ico` 133,921 B, `Uninstall.exe`
172,864 B, `MicrosoftEdgeWebview2Setup.exe` 1,897,688 B. Start Menu `Station.lnk` created (2,257 B).
Both user hashes were unchanged after install.

**Registry values (verbatim):**

```
HKEY_CURRENT_USER\Software\Classes\station
    (Default)       REG_SZ    URL:Station
    URL Protocol    REG_SZ
HKEY_CURRENT_USER\Software\Classes\station\shell\open\command
    (Default)       REG_SZ    "C:\Users\asd\AppData\Local\Temp\station-install-test\station-shell.exe" "%1"

HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\Station
    DisplayName          REG_SZ    Station
    DisplayVersion       REG_SZ    0.1.0
    Publisher            REG_SZ    Station
    DisplayIcon          REG_SZ    C:\Users\asd\AppData\Local\Temp\station-install-test\station.ico
    InstallLocation      REG_SZ    C:\Users\asd\AppData\Local\Temp\station-install-test
    UninstallString      REG_SZ    "C:\Users\asd\AppData\Local\Temp\station-install-test\Uninstall.exe"
    QuietUninstallString REG_SZ    "C:\Users\asd\AppData\Local\Temp\station-install-test\Uninstall.exe" /S
    NoModify             REG_DWORD 0x1
    NoRepair             REG_DWORD 0x1
    EstimatedSize        REG_DWORD 0x793a
```

**Launch, fresh session.** The installed exe was started with
`STATION_SESSION_PATH=…\station-install-test\scratch-session.json` (fresh; the user's session was
never read or written). After 12 s: one `station-shell.exe`, window class `Window Class` / title
`Station`, **0 `msedgewebview2.exe`**. A fresh session restores no tabs, so only the Rune-drawn App
Store exists and no page attaches — session content, not an installer fault. `WM_CLOSE` → **exit
code 0**; `instance.json` present during the run and **gone** after close; the shell wrote a 277 B
scratch session; user hashes unchanged. A first attempt used a broken `EnumWindows` harness, was
killed at timeout and left `instance.json`; it was removed and the clean run repeated.

**Installed binary hosts a page.** Re-ran the launch with a **seeded scratch session** (347 B,
`%TEMP%\station-install-test\scratch-session.json`, never user data): `activeApplication:"14"`,
`installed:[{application_id:"14",manifest_url:"station-manifest://14",in_dock:true}]`, `dock:["14"]`,
one Gmail tab `https://mail.google.com/` (`tab-gmail`, home). Install exit code 0. After 12 s:

- **1 × `station-shell.exe`, 8 × `msedgewebview2.exe`**.
- Top window `Window Class` / `Station`, rect **(104,104)-(1320,943)**.
- Visible child **`WRY_WEBVIEW` rect (176,179)-(1312,935)** (the content pane inside the Rune
  chrome), with `Chrome_WidgetWin_1` titled **“Sign in - Google Accounts”**, `Chrome_RenderWidgetHostHWND`
  (“Chrome Legacy Window”) and the D3D child at the same rect.
- stderr: 19,847 B, all 129 `no icon for …: status code 404` store-icon lines; **no**
  “starting with an empty session” and no “no web view”.
- `WM_CLOSE` → **exit code 0**; `instance.json` gone after close; scratch session rewritten to
  568 B; both user hashes unchanged; `%APPDATA%\station` still exactly `log.log`, `session.json`.

**Finding — uninstaller left the WebView2 profile (fixed).** That first page-hosting run's
uninstaller removed its files, both HKCU keys and the shortcut, but left
`station-shell.exe.WebView2\EBWebView` (**16 MB** browser profile) in `$INSTDIR`; `RMDir "$INSTDIR"`
then cannot remove the directory, so a real `%LOCALAPPDATA%\Programs\Station` install would survive
uninstall. Fixed in `installer.nsi`: the uninstaller now runs
`RMDir /r "$INSTDIR\station-shell.exe.WebView2"` before `RMDir "$INSTDIR"`. The installer was rebuilt
(10,357,011 B, sha256 above) and re-verified: after `Uninstall.exe /S` the WebView2 profile and all
installer files are gone; only this smoke's scratch session/stderr remained (hence the test
directory stayed until cleanup); keys and shortcut gone; user hashes unchanged. A plain
install → uninstall with **no launch** (no WebView2 profile, no scratch files) removed `$INSTDIR`
entirely (both exit codes 0), which is the normal-install case.

**Fresh-session uninstall.** `Uninstall.exe /S` → exit code 0; files, both HKCU keys and the
shortcut removed; the directory remained only with this smoke's `scratch-session.json` and stderr
log; `%APPDATA%\station\session.json` / `log.log` and empty `BrowserX` unchanged — **byte-identical**.

**Cleanup.** Temp install directories, seeded/scratch sessions and stderr logs, and NSIS's
`%TEMP%\~nsu1.tmp\Un.exe` copies were removed. Final: 0 `station-shell` / 0 `msedgewebview2` / 0
uninstaller processes; no install dir, no Start Menu shortcut, no HKCU keys; hashes `7282d976…` /
`7ed17200…`; `%APPDATA%\station` still contains exactly `log.log`, `session.json`; WebView2 `pv`
still `157.0.4341.60`. Nothing from these smokes remains.

**Still unverified:** WebView2 bootstrapper install/failure path (runtime already present; the
bootstrapper was not run), signing, clean-VM behaviour, upgrade-in-place, and `station://` URL
handling (registration observed; the known shell argv gap remains).
