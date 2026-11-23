# Station native shell — Windows packaging plan

Decision-ready plan for packaging `station-shell` (the Rust shell) on Windows. Docs only — no
code, config or CI changed by this document. All repo paths verified 2026-11-18; all external
facts cite a primary source URL with access date 2026-11-18 unless marked **UNVERIFIED**.

## 1. Goal and constraints

**Goal:** a Windows 10/11 x64 installer for `station-shell` that a normal user can install and
remove without breaking their data. macOS is **out of scope** — it stays gated on the paid Apple
Developer Program, per the existing resolution: "Notarization: gated on Apple Developer Program
($99/yr + Developer ID cert), not tooling … drop mac from release matrix rather than ship
unsigned" (`docs/rust-rewrite-plan.md`, Open questions resolution 3, resolved 2026-09-22).

State that must survive the shell (from `docs/electron-retirement.md` "Must survive the shell"):

| Must survive | Today (Electron) | Native shell today |
| --- | --- | --- |
| Code signing / installer | `packages/app/electron-builder.yml` (`win.target: nsis`), `packages/app/build/` (`installer.nsh`, `icon.ico`, `installerIcon.ico`) | none — `cargo build --release -p station-shell` only ("no installer or code signing", `rust/README.md` "Packaging") |
| Auto-update | `electron-updater`, GitHub Releases publish (`electron-builder.yml` `publish: provider: github`) | none — installed-and-replaced manually |
| Deep links (`station://` OS registration) | `electron-builder.yml` `protocols: [station]` (electron-builder writes the registration; `installer.nsh` has no `station` lines) | none |

Additional constraints:

- **Per-user install, no admin prompt** (this plan's baseline; see Open decision 2 for the
  per-machine question).
- **Uninstall leaves user data alone**: `%APPDATA%\station\session.json` (session) and
  `%APPDATA%\BrowserX\private-manifests.json` (private manifests) are the shell's two on-disk
  stores (`rust/README.md` "Data"). Unlike the Electron NSIS macro
  (`packages/app/build/installer.nsh`, which asks "Delete also user data and settings?"), the
  Rust uninstaller must never offer to delete them.
- **App icon** on the window, taskbar, shortcuts and the `station://` registration. The shell
  applies `packages/app/build/icon_512x512.png` at runtime via `WM_SETICON`
  (`rust/README.md` "Window identity"); the **.exe file icon** is not embedded (no icon resource
  in the build) — packaging must carry `packages/app/build/icon.ico` as an installed file.
- **Electron packaging untouched** — `electron-builder.yml` is deleted only at retirement step 15
  (`docs/electron-retirement.md`); the native shell ships beside it.
- **Pinned toolchain**: Rune pinned by rev + 5 vendored patched crates (`docs/rust-rune-port.md`
  D2, `rust/Cargo.toml`). Packaging tooling must not touch the Rust build; a release binary is
  measured **29,135,872 bytes** (`rust/target/release/station-shell.exe`, 2026-11-18).

## 2. Tool comparison — facts

Machine tool check (recorded 2026-11-18, this workstation): `where candle` → **absent**;
`where makensis` → **absent**; `where wix` → **absent**; `where cargo-wix` → **absent**;
`where signtool` → **absent**; `where dotnet` → present (`C:\Program Files\dotnet\dotnet.exe`);
`cargo` 1.98.1 + `rustup` present. **Every installer tool below must be installed by a human or
CI before any build command in this plan runs.**

| | MSI via WiX (v4+/v6/v7) | NSIS | cargo-bundle | MSIX/AppX | Zip + first-run self-install |
| --- | --- | --- | --- | --- | --- |
| WebView2 handling | Official WiX custom-action sample (bootstrapper + standalone) [MS Learn](https://learn.microsoft.com/en-us/microsoft-edge/webview2/samples/wv2deploymentwixcustomactionsample) | No built-in; script runs the bootstrapper exe (pattern blessed by the distribution doc, "include the installer in your app installer and run it during setup") [MS Learn](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution) | none | none needed for Store (Store installs runtime deps poorly — bootstrapper in package is the pattern) | runtime detection only; shell already degrades (`rust/README.md`) |
| Per-user, no elevation | `Package/@Scope="perUser"` (sets `InstallPrivileges=limited`, ALLUSERS=2) [FireGiant schema](https://docs.firegiant.com/wix/schema/wxs/packagescopetype/); default is `perMachine` — must be set explicitly | native: HKCU registry + `SetShellVarContext current`, zero elevation | MSI experimental; scope control unclear — **UNVERIFIED** (per-user issue [volks73/cargo-wix#297](https://github.com/volks73/cargo-wix/issues/297) closed but the shipped behavior was not verifiable) | inherently per-user, no elevation | yes (self-copy to `%LOCALAPPDATA%\Programs`) |
| `station://` registration | Registry element, trivial | `WriteRegStr HKCU Software\Classes\station …`, trivial [NSIS manual](https://nsis.sourceforge.io/Docs/Chapter4.html#registry) | not exposed | manifest `uap:Protocol` extension — **UNVERIFIED** (schema URL not opened) | self-written HKCU key |
| Uninstall cleanliness | full file+registry control, rollback | full control; repo precedent `installer.nsh` | alpha quality | cleanest (system-managed) | weakest: no uninstaller unless self-install also writes one |
| Signing implications | sign the MSI (signtool) | sign the .exe (signtool) | optional feature flag | **signing mandatory to install** [MS Learn](https://learn.microsoft.com/en-us/windows/msix/package/signing-package-overview) | sign each shipped .exe |
| CI complexity | medium: `dotnet tool install --global wix` (v4+ is a dotnet tool; machine has dotnet) or cargo-wix (whose GH-runner caveat: windows images carry only legacy WiX v3.14.1 [cargo-wix](https://github.com/volks73/cargo-wix)) | low: NSIS zip or `choco install nsis` on windows runners | low (`cargo install cargo-bundle`) but alpha | high: packaging tool or makeappx+signing; Store path extra | lowest |
| Maintenance status (2026-11-18) | v7.0.0 (2026-04-03), v6.0.2 (2025-08-28) [tags](https://github.com/wixtoolset/wix/tags); WiX v3 **out of community support since 2025-02-06**, security window for current+previous majors ~22 months [FireGiant](https://www.firegiant.com/blog/2025/2/6/wix-v3-and-wix-v4-are-no-longer-in-community-support/) | active: 3.13 (2026-09-27) [download](https://nsis.sourceforge.io/Download); 3.11 (2025-03-08) fixed a privilege-escalation bug [changelog](https://nsis.sourceforge.io/Docs/AppendixF.html) | 0.12.0 (2026-09-20) but self-described "early alpha, no stability guarantees" [crates.io](https://crates.io/crates/cargo-bundle) | actively maintained platform, but a packaging *format* with a signing tax | nothing to maintain |

### WebView2 support detail per tool

- **NSIS**: ship the Evergreen bootstrapper (~2 MB,
  `https://go.microsoft.com/fwlink/p/?LinkId=2124703` — the link Microsoft's own samples embed
  [WebView2Samples Bundle.wxs](https://github.com/MicrosoftEdge/WebView2Samples/blob/main/SampleApps/WV2DeploymentWiXBurnBundleSample/Bundle.wxs);
  it is evergreen, content changes over time, no stable hash —
  [issue #150](https://github.com/MicrosoftEdge/WebView2Samples/issues/150)). Installer checks
  the detection registry key and, if missing, runs
  `MicrosoftEdgeWebview2Setup.exe /silent /install`. Non-elevated run installs per-user
  (auto-upgraded to per-machine if a per-machine Edge Updater exists) [MS Learn distribution](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution).
- **WiX**: the same two files wired through the official custom action / Burn bundle samples.
- **cargo-bundle**: no WebView2 concept; you would hand-roll the same check in a wrapped MSI.

## 3. WebView2 strategy

**Recommendation: Evergreen bootstrapper, checked at install time, degraded path kept at run
time. No fixed version.**

- **Bootstrapper vs fixed**: Evergreen is Microsoft's recommended mode — auto-updating runtime,
  shared across apps; fixed version ships >250 MB of binaries with the app and puts the update
  burden on us [MS Learn](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/evergreen-vs-fixed-version).
  With the shell at ~29 MB, a fixed runtime would grow the payload roughly 9×.
- **Install-time check**: registry `pv` value under
  `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}`
  (and the `HKCU` twin) present and > `0.0.0.0`; else run the bootstrapper
  `/silent /install` [MS Learn distribution](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution).
  The same page blesses the programmatic alternative,
  `GetAvailableCoreWebView2BrowserVersionString` (null ⇒ not installed).
- **Runtime behavior when missing**: already implemented and honest — the shell logs
  `station-shell: no web view for <app>: …` and keeps running
  (`rust/README.md` "Requirements"); packaging must not turn that into a crash or a blocking
  dialog. The installer check is a convenience, not a correctness dependency.
- **Smallest footprint with the pinned toolchain**: WebView2 is not a Rust dependency at all —
  wry 0.57 (`rust/crates/station-shell/Cargo.toml`) talks to the loader and discovers the
  Evergreen runtime by default. Nothing in `rust/vendor/` or the Rune pin changes. The only new
  build inputs are the bootstrapper download and the NSIS tool itself.

## 4. Code signing

Primary source for all of this:
[Code signing options for Windows app developers](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)
and [SmartScreen reputation](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation)
(MS Learn, accessed 2026-11-18).

| Option | Facts (primary source) | Consequence for Station |
| --- | --- | --- |
| Azure Artifact Signing (formerly Trusted Signing) | Basic **$9.99/account/month** (5,000 signatures/mo), Premium $99.99 (100k) [SKU doc](https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-change-sku); organizations: US/CA/EU/UK, **individuals: US/CA only**; identity validation takes **1–30 business days** and needs an Entra tenant + Azure subscription [quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart); no hardware token, CI-native | cheapest real option, fits CI directly; eligibility depends on who holds the identity (Open decision 1) |
| OV certificate | ~**$150–300/yr** per MS Learn's options page (an MSIX page says $300–500 — treat ranges as indicative), CA validation + hardware token | works outside Artifact Signing's regions; token handling in CI is extra friction |
| EV certificate | **no longer bypasses SmartScreen** (as of ~2024 it follows the same reputation path as OV) | paying the EV premium for SmartScreen is not justified per Microsoft |
| Store-signed | free, but Store distribution only | out of scope |
| Unsigned | SmartScreen shows the warning; user must pick "Run anyway"; enterprise policy may block outright; publisher + file-hash reputation build only through organic downloads | acceptable only for an internal beta, and **must be stated in the README/UI** ("unsigned — expect a SmartScreen warning"); note Electron already sets `verifyUpdateCodeSignature: false`, so update-signature checking was already relaxed |

SmartScreen mechanics (both certs and unsigned): two signals — publisher reputation and exact
file-hash reputation; warnings persist until enough clean downloads accumulate; reputation ties
to the signing identity, so signing every release with the same identity is what builds it.

## 5. Recommended path

**Primary: NSIS 3.13 per-user installer** (`rust/packaging/installer.nsi`), packaging the
`cargo build --release -p station-shell` binary + `icon.ico` + the WebView2 Evergreen
bootstrapper.

- Repo precedent: the Electron contract is already NSIS (`electron-builder.yml` `win.target`),
  with reusable assets (`packages/app/build/icon.ico`, `installerIcon.ico`) and a known macro
  pattern (`installer.nsh`).
- Meets every constraint with plain scripting: per-user HKCU everything, no elevation;
  `station://` registration; uninstall leaves `%APPDATA%\station` and `%APPDATA%\BrowserX`
  untouched; bootstrapper run at install time.
- CI: NSIS is one `choco install nsis` on the existing `windows-2025` runner
  (`.github/workflows/release.yml`).
- Tooling risk is lowest and current (3.13, 2026-09-27).

**Fallback: zip + first-run self-install.** `station-shell-<ver>-x64.zip` with the exe, `icon.ico`,
a tiny `install.cmd` (or the exe's own first-run flag) that copies to
`%LOCALAPPDATA%\Programs\Station`, writes the HKCU `station://` key, creates a Start Menu
shortcut, and runs the WebView2 bootstrapper if missing. No uninstaller (that is its honesty
cost), no elevation, works today without installing any tool. Use it if the NSIS slice stalls or
for the first internal drop.

**Rejected:** WiX/MSI (v3 — what cargo-wix uses by default and what GH runners preinstall — is
out of community support; the modern v4+ flow adds a dotnet-tool dependency and per-user scope
that cargo-wix's legacy default flow doesn't model; more moving parts for a single-exe app);
cargo-bundle (alpha, no WebView2 concept); MSIX (signing is mandatory, which blocks the unsigned
internal beta; Store is a later, separate decision).

**Artifacts the pipeline produces and where they land:**

| Artifact | Path |
| --- | --- |
| Release binary | `rust/target/release/station-shell.exe` (29,135,872 bytes measured) |
| Installer | `release/station-shell-setup-<version>.exe` (NSIS output; `release/` is already the electron-builder `directories.output`, kept as the shared landing dir) |
| WebView2 bootstrapper | downloaded at CI time to `rust/packaging/dist/MicrosoftEdgeWebview2Setup.exe` (gitignored; fwlink above) |
| Fallback zip | `release/station-shell-<version>-x64.zip` |
| GitHub release | attached alongside the Electron artifacts by the same `gh release upload` step family used by `yarn release` |

## 6. Update story

`electron-updater` stays out of scope for the native shell — it is an Electron library, and the
resolved plan is to keep it under the retained Electron shell ("Updates: keep electron-updater …
Channels work today (prerelease tiers / channel-suffixed latest.yml)",
`docs/rust-rewrite-plan.md` resolution 2).

**Fact to state explicitly: the shell has no updater today.** Updating the native shell means
downloading the new installer and installing it over the old one (NSIS upgrades in place, user
data untouched). That manual install-and-replace is the current, honest behavior and must be
documented wherever the shell is published.

Options, once the shell becomes the release artifact:

1. **Manual install-and-replace** (exists today) — zero code; document it. *Recommended now.*
2. **Channel-suffixed manifests + a small built-in updater** — publish `latest-stable.yml` /
   `latest-beta.yml` next to the installer on GitHub Releases (the same channel pattern
   electron-updater already uses); a small shell-side module fetches the channel manifest,
   downloads the installer, launches it and exits. Smallest real updater; no C dependency.
   *Recommended as the first updater slice, after per-user NSIS exists.*
3. **WinSparkle** (0.9.4, 2026-07-21, MIT, active — [releases](https://github.com/vslavik/winsparkle/releases))
   — appcast XML + a C library; FFI or sidecar process; more moving parts than option 2 for the
   same result.
4. **MSIX/Store auto-update** — platform-provided, but flips the packaging decision to MSIX with
   its signing tax; defer with macOS (Open decision 4).

## 7. Step-by-step implementation plan (NSIS path)

Prerequisites on any build machine (none present today — see §2 tool check): NSIS 3.13
(`choco install nsis` on CI; human installs from <https://nsis.sourceforge.io/Download>), plus
signtool (Windows SDK) or Azure Artifact Signing if signing (Open decision 1). Rust 1.98.1 and
dotnet are already present here; `wix`/`cargo-wix` are not needed by this path.

1. **Add `rust/packaging/installer.nsi`** — NSIS 3 script:
   - `InstallDir "$LOCALAPPDATA\Programs\Station"`, `RequestExecutionLevel user` (per-user, no
     elevation anywhere).
   - Files: `station-shell.exe`, `icon.ico` (from `packages/app/build/icon.ico`),
     `MicrosoftEdgeWebview2Setup.exe`.
   - WebView2: `ReadRegStr` the `pv` values (HKLM WOW6432Node, then HKCU); if absent/not
     >0.0.0.0 → `ExecWait '"$INSTDIR\MicrosoftEdgeWebview2Setup.exe" /silent /install'`; a
     bootstrapper failure logs a detail string and **continues** (the shell degrades by design).
   - Protocol: `WriteRegStr HKCU "Software\Classes\station" "" "URL:Station"`, `URL Protocol`
     empty string, `shell\open\command` = `"$INSTDIR\station-shell.exe" "%1"`.
   - Shortcuts: per-user Start Menu (`SetShellVarContext current`) "Station.lnk" with
     `IconIndex` from the installed `icon.ico`; optional desktop shortcut.
   - Uninstall registry key:
     `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Station`
     (`DisplayIcon` → installed ico; `NoModify`/`NoRepair`).
   - Uninstaller: delete the above exactly — **never touch `%APPDATA%\station` or
     `%APPDATA%\BrowserX`** (contrast `packages/app/build/installer.nsh`, which offers to).
2. **~~Optional follow-up (separate slice): embed the icon in the .exe~~ Done** —
   `crates/station-shell/build.rs` compiles `station.rc` (one `ICON` line referencing
   `packages/app/build/icon.ico`, relative path, Windows-only `embed-resource` build-dep) so the
   raw exe carries the icon; shortcut/registration icons still point at the installed ico file.
3. **CI job** — add `package-windows-native-shell` to `.github/workflows/release-candidate.yml`
   (mirroring the existing Windows job's shape, `runs-on: windows-2025`):
   - checkout; `cd rust && cargo build --release -p station-shell`;
   - `choco install nsis -y`;
   - download bootstrapper fwlink to `rust/packaging/dist/`;
   - `makensis /DVERSION=<ver> rust/packaging/installer.nsi`, output redirected to `release/`;
   - optional signing step gated on secrets (Artifact Signing action or signtool);
   - upload artifact / attach to the release.
   Keep `rust.yml`/`tests.yml` untouched; packaging never gates tests.
4. **Human verification commands** (each must be run manually; the ones that need an
   not-yet-installed tool are marked):
   - Build: `cd rust && cargo build --release -p station-shell`
   - Installer: `makensis rust/packaging/installer.nsi` — *requires NSIS install (absent here)*
   - Clean VM: install on a Win10/11 VM **without** WebView2 → install must not prompt UAC; after
     install, runtime present or shell running degraded.
   - Protocol: `reg query HKCU\Software\Classes\station /s`, then `start station://appstore`.
     **Known gap**: the shell does not read a `station://…` argv URL today (no `env::args` URL
     handling in `rust/crates/station-shell/src/`); registration will launch the exe, but acting
     on the URL is shell work outside packaging scope — record the observed behavior.
   - Uninstall: uninstall, then confirm `%APPDATA%\station\session.json` and
     `%APPDATA%\BrowserX\private-manifests.json` still exist, and
     `reg query HKCU\Software\Classes\station` fails (key gone).
   - Icon/shortcut: Start Menu shortcut shows the Station icon; taskbar/window icon on launch.
   - Missing-WebView2: on the clean VM, remove the runtime (`winget uninstall ...WebView2Runtime`
     or Apps settings), launch the exe → expect the documented degraded log
     `station-shell: no web view for <app>: …` and a running shell with Rune chrome.

## 8. Open human decisions

1. **Signing identity** (blocks §4/step 3 signing): Azure Artifact Signing ($9.99/mo; org must be
   in US/CA/EU/UK, individual US/CA only; 1–30-day identity validation) vs OV CA cert
   (~$150–500/yr + token) — and who/which org identity holds it.
2. **Per-user vs per-machine** install: this plan assumes per-user/no-elevation (the stated
   constraint); the Electron NSIS default today is effectively per-machine one-click
   ([electron.build nsis](https://www.electron.build/nsis), `oneClick: true`). Confirm the native
   shell should diverge.
3. **Update channel policy**: which channels the native shell publishes (`latest-stable.yml` /
   `latest-beta.yml`-style), and when native-shell artifacts join the same GitHub Releases as
   Electron's.
4. **Ship an unsigned internal beta?** (SmartScreen "Run anyway" warning; must be stated in
   README/UI.) If yes, the NSIS + zip artifacts above can land before decision 1 resolves.
5. **MSIX/Store** (and, tied to it, macOS) stay deferred — macOS behind the Apple Developer
   Program gate (`docs/rust-rewrite-plan.md` resolution 3); MSIX flips the packaging decision and
   its signing tax. Revisit only if Store distribution is wanted.
