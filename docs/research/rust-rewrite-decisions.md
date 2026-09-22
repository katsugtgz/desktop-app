# Rust rewrite — decision research (open questions 1–3)

Researched 2026-09-22 against primary sources (Tauri v2 docs via ctx7 `/tauri-apps/tauri-docs`, NAPI-RS docs via `/napi-rs/website`, dist docs via `/axodotdev/cargo-dist`, upstream tauri-apps issues/discussions, Apple TN3147, repo code). Answers the three human open questions in `docs/rust-rewrite-plan.md`.

---

## 1. UI shell: Tauri v2 vs keep-Electron-shell-with-Rust-core (napi-rs)

### What Station actually needs from the shell (measured in repo)

- **Multi-webview tabs**: the app is a browser-workspace — one OS window, N live webviews (apps), only the active one visible. Rendering is `<webview>` tags (ElectronWebview.tsx wraps `Electron.WebviewTag`, `packages/app/src/common/components/ElectronWebview.tsx`), kept alive and hidden/shown via `hidden` prop + zIndex (`packages/app/src/applications/components/Application.tsx:474`, `ApplicationContainer.tsx`). NOT `BrowserView` — a repo-wide grep for `BrowserView` finds **0 hits**; the "~49 hits" figure from the task brief counts `webContents` API references (214 actual) spread over `tab-webcontents/main.ts` (30), `browser-window/main.ts` (20), `GenericWindowManager.ts` (2), etc. — all main-process orchestration of webview lifecycles (observers for dom-ready, navigation, notifications, print, downloads, auto-login, new-window interception → `packages/app/src/services/services/tab-webcontents/main.ts`).
- **Custom protocols**: exactly one privileged scheme — `bx-protocol` (`station://`), registered via `protocol.registerSchemesAsPrivileged` + `protocol.handle` serving local handler files (`packages/app/src/webui/webUIHandler.ts:7-35`). Plus `station` deep-link scheme in electron-builder.yml `protocols`. Plan's s13 (`appstore` protocol) adds one more.
- **GraphQL frontend**: Apollo Client 2 + `@getstation/reactive-graphql` + `apollo-link-reactive-schema` — a schema-linked (in-process) client, not a network server (`packages/app/package.json`). The renderer is self-contained; the shell only needs IPC.
- **Scale**: ~861 ts/tsx files repo-wide, 656 in `packages/app` (find counts; the 849 in the brief is between runs/file sets). But slices s02–s11 already port the *backend* (SDK, manifests, activity, appstore) to Rust crates; the renderer is React+redux-observers and largely shell-agnostic if the IPC seam (`services/`) is replaced.

### Tauri v2 multi-webview: real status

- **Supported, still `unstable`-flagged.** `window.add_child(webview_builder, position, size)` is the intended primitive (docs snippet from `tauri/src/webview/mod.rs`); `Webview` has `set_bounds/set_size/set_position/hide/show/reparent`, hide/show preserve page state. Multiple webviews per window shipped as the headline v2 feature (blog: tauri-2-0-0-beta), BUT `WebviewBuilder` is wrapped in `unstable_struct!` — `pub` only with `features = ["unstable"]`, `pub(crate)` otherwise (tauri/Cargo.toml example: `required-features = ["unstable"]`). As of the 2.x line this has NOT been stabilized.
- **Platform reality check**: works on **Windows/WebView2** (multiple community reports of exactly Station's shape — full-window UI webview + overlay browser webview via `set_bounds` — "works perfectly on Windows", r/tauri Feb 2026). **Linux/WebKitGTK is broken for child webviews**: window splits/resizes wrongly as soon as a 2nd webview exists; hide/show, offscreen positioning, and lazy create/destroy all fail (r/tauri 1qxfkeu, Feb 2026; upstream issues #10131 "stops resizing", #10420 "broken positioning", both still-open class bugs). macOS/WebKit sits between — usable but the same unstable API.
- **Verdict for Station**: Station's *product* is multi-webview. On Tauri that means betting the core UX on an API Tauri itself marks unstable, with a known-broken Linux story. The plan targets Windows+macOS first (electron-builder.yml ships mac arm64 + win nsis; linux targets exist but are secondary), so Windows is fine and macOS is probable-fine — but every `webContents` observer Station relies on (notification observer, print, download hack, new-window/URL dispatch, auto-login injection, `webPreferences` overrides at `browser-window/main.ts:49-53`) has to be re-implemented against wry/tauri event surface, which is thinner than Electron's.

### napi-rs (keep Electron shell, Rust core)

- **Mature**: v3 announced 2025-07 (WASM target, safer API, cross-compile); production users include Rspack; used from Electron main process routinely (napi.rs docs integrate: "Electron can load a Node-API addon in the main process and in a preload/renderer; prefer main/preload with narrow IPC"). Node-API = ABI-stable across Node/Electron versions, no node-gyp/electron-rebuild for the addon itself. Packaging guidance exists: keep `.node` out of ASAR (`asarUnpack`) or rely on smartUnpack (this repo's electron-builder.yml comment already documents smartUnpack unpacking any package with .node/.dll — i.e. the packaging story is already compatible).
- **Cost**: zero UI rewrite. The 861 ts/tsx files keep running; Rust crates from s02–s11 bind in via `#[napi]` exports replacing the `services/` TS layer one service at a time. WebView behavior, protocols (`protocol.handle` stays), GraphQL renderer, auto-update all keep working unchanged.
- **What you don't get**: no memory/binary-size win for the shell; still ship Chromium; still Electron security-CVE treadmill; two build systems (cargo + webpack) — already the case in this repo.

### Migration cost comparison

| | Tauri v2 | Electron + napi-rs core |
|---|---|---|
| Renderer React code | reusable, but IPC/`@tauri-apps/api` rewrite across `services/` renderer layer | unchanged |
| Main-process orchestration (~214 webContents refs, observers, auth flows, context menu, dialogs) | full rewrite against Tauri/wry APIs, several have no equivalent (download hack, chrome-extension support) | unchanged, calls Rust via napi |
| Webview tag semantics (hidden-but-alive tabs, per-tab observers) | `add_child` + hide/show — exists but `unstable`; per-webview event granularity thinner | unchanged |
| Custom protocols | `register_asynchronous_uri_scheme_protocol` + custom scheme registration — first-class, s13 straightforward | unchanged (`protocol.handle` stays) |
| Packaging | s15 rewrite (tauri.conf.json, bundler) | electron-builder.yml as-is |
| Windows+macOS today | yes (multiwebview solid on Win, unstable-flagged overall) | yes (current state) |
| Linux | multiwebview broken (WebKitGTK) | yes |

### Recommendation

**Hybrid, sequenced**: land s02–s11 Rust crates regardless (they are shell-agnostic — good plan design). Bind them into the existing Electron main process with **napi-rs** and delete the TS services layer slice-by-slice (measurable, reversible, keeps shipping). Re-evaluate the Tauri shell only when (a) Tauri stabilizes `WebviewBuilder`/multiwebview out of `unstable`, and (b) Linux-in-WebKitGTK child-webview bugs close. The plan's s12–s16 as written assume Tauri; if this decision is taken, s12/s13 become "napi binding + window-lifecycle parity" instead of Tauri slices — the slice boundaries survive, the targets change.

If the hard requirement is "no Electron eventually, ship only Win+macOS", Tauri is viable *today* but the multiwebview `unstable` flag must be accepted as a named risk in s15, with `add_child` smoke tests per platform in CI.

---

## 2. Auto-update / canary channel

### What upstream had

- `scripts/canary.js` (~530 lines, commander): a *release-train orchestrator*, not an updater — `merge` (merge labeled PRs from `getstation/desktop-app` into `canary-channel` branch of `getstation/station-canary`), `continue`, `draft <version>` (bump `electron-builder-canary.yml` version via replace-in-file, create GitHub draft release with PR-list body, notify Slack). Client side: `electron-updater` (`packages/app/src/services/services/auto-updater/lib.ts`, `allowPrerelease = false`), GitHub provider (`publish: provider: github` in electron-builder.yml), saga polling `packages/app/src/auto-update/sagas.ts`. Canary installs detect themselves by app name containing "canary" (`theme/api.ts:27`).
- So "channels" upstream = two separately-installed builds pointing at two repos' GitHub Releases (station-canary releases vs desktop-app releases). Not true in-app channel switching.

### Option A — tauri-plugin-updater (if Tauri shell)

- **Windows + macOS + Linux supported** (docs: `cfg(any(target_os = "macos", windows, target_os = "linux"))`); artifacts: macOS `.app.tar.gz`+`.sig`, Windows `.zip` of NSIS/MSI + `.sig`, signatures **mandatory** (minisign-style ed25519 keys via `tauri signer generate`; `TAURI_SIGNING_PRIVATE_KEY` env at build — `.env` files explicitly do NOT work).
- **Channels: DIY but first-class-ish.** No built-in channel field; the documented pattern is runtime endpoint selection — `UpdaterExt::updater_builder().endpoints(vec![url])` with `let channel = if beta { "beta" } else { "stable" }; format!("https://{channel}.myserver.com/...")` (updater docs "Endpoints" section, for "separate release channels"). On GitHub Releases this maps to: N draft→published releases each carrying their own `latest.json` (tauri-action generates `latest.json` per release when `includeUpdater`/updater artifacts are enabled), endpoint `https://github.com/<owner>/<repo>/releases/download/<channel-latest>/latest.json` or a `releases/latest/download/latest.json` per channel-repo. Community tooling exists (`tauri-latest-json` crate regenerates the manifest from a bundle dir; axo/anystack hosted endpoints).
- **Canary orchestration replacement**: `scripts/canary.js` logic (PR labeling, branch merge, draft release, Slack) is orthogonal to the updater — replace with `gh` CLI in CI or a small octocrab tool if desired; the electron-builder-canary.yml version bump disappears into tauri.conf.json `version`. `prerelease: false` in the old script was just GitHub-release cosmetics.

### Option B — dist (cargo-dist) + axo releases

- dist handles: build matrix, installers, GitHub release creation (`create-release`), Homebrew/npm publishing, and a **standalone `*-update` updater binary** (`install-updater = true`) that replaces-on-run — a CLI-updater model, not an in-app GUI updater. Prerelease/channel support: `publish-prereleases = true`, `force-latest` to bypass semver-prerelease skipping.
- **Mismatch for Station**: no native Tauri updater-artifact (`latest.json` + `.sig`) generation, no in-app "download, prompt, relaunch" flow without custom work, and axo releases hosting is a separate service decision. dist is built for CLI/binary distribution; Tauri apps are explicitly not its target shape (Tauri's own docs point to tauri-action for GH pipelines). Reject unless the shell decision changes to non-Tauri non-Electron.

### Option C — keep electron-updater (if Electron shell retained)

- **Channels supported today**: electron-builder "Release Using Channels" — semver prerelease tags (`2.0.0-beta.1`) + `channel` on the client updater, plus `allowPrerelease`. GitHub provider works with multi-channel via channel-suffixed `latest*.yml` files that electron-updater fetches by channel. Windows+macOS both supported (mac requires signed builds for updater verification; `verifyUpdateCodeSignature: false` currently set in this repo's win config).
- **Zero new infrastructure** — the existing sagas/service layer (`auto-updater/main.ts` wraps events, `quitAndInstall`) keeps working; canary.js can be deleted or replaced by a 30-line `gh`-based draft script since its real job was branch bookkeeping.

### Recommendation

Follows the shell decision:
- **Electron retained → Option C.** Cheapest, channels (stable/beta/canary as prerelease tiers or per-channel latest.yml) work on Win+mac out of the box. Port canary.js's merge/draft flow to `gh` CLI in CI (or drop Slack/PR-label ceremony); do not port to git2/octocrab — it's release choreography, not product code.
- **Tauri shell → Option A** (tauri-plugin-updater + tauri-action). Channels = per-channel `latest.json` endpoints selected via `updater_builder().endpoints()` at runtime (in-app channel switching, better than upstream's two-installer model), Win+mac supported today. Budget: key generation + secrets, `createUpdaterArtifacts: true`, a `latest.json` per channel in release assets. Skip dist/axo entirely (Option B) — wrong shape for a GUI app with in-app updates.

---

## 3. macOS notarization

### Upstream state (this repo)

- `scripts/notarize.js` exists and is the standard electron-builder afterSign hook using `@electron/notarize` with `appleId`/`appleIdPassword`/`teamId` from `AC_USERNAME`/`AC_PASSWORD`/`AC_TEAM_ID` env — but it is **commented out** in `packages/app/electron-builder.yml` (`# afterSign: '../../scripts/notarize.js'`), and the hook itself no-ops unless `AC_USERNAME` is set. So: disabled, deliberately, cheaply reversible in Electron.
- Note the old script uses Apple-ID + password auth (altool-era). Apple **deprecated altool for notarization, dead since 2023-11-01** (TN3147) — `@electron/notarize` current versions use notarytool, and the Apple-ID path now needs an app-specific password; preferred modern auth is an App Store Connect API key (Issuer/KeyId/.p8).

### Does Tauri v2 do it natively?

Yes — the Tauri bundler signs + notarizes as part of `tauri build` with **no afterSign hook**:

- Signing: `bundle > macOS > signingIdentity` in tauri.conf.json or `APPLE_SIGNING_IDENTITY` env; CI recipe documented (base64 `APPLE_CERTIFICATE`, keychain import, `tauri-action@v0` with env).
- Notarization: two documented auth modes — App Store Connect API (`APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH`) or Apple ID (`APPLE_ID`, `APPLE_PASSWORD` app-specific, `APPLE_TEAM_ID`). With env set, "the Tauri bundler automatically signs and notarizes your application" (v2 distribute/sign/macos). Updater artifacts (`.app.tar.gz` + `.sig`) are produced in the same build, so signed-notarized updates come free with Option A above.
- Hardened runtime / entitlements: handled by bundler config (`hardenedRuntime` equivalent via signing options); Station's `entitlements.mac.plist` maps to Tauri's macOS entitlements support in bundle config.

### Cost/effort

| Path | Effort | What's needed |
|---|---|---|
| Restore under Electron | ~0 code; ops only | Uncomment `afterSign`, modernize `scripts/notarize.js` to notarytool/ASC-API-key auth (the Apple-ID vars may still work via notarytool but ASC key is the supported path), set 3 secrets in CI. Apple Developer Program membership ($99/yr) + Developer ID Application certificate required — that's the real gate; without it nothing notarizes under either stack. |
| Enable under Tauri | small, config-driven | `signingIdentity` + 3–4 `APPLE_*` env secrets; no hook code at all. tauri-action in CI wires it. Entitlements + updater sigs in same pass. |
| Keep disabled | 0 | macOS users get Gatekeeper "unidentified developer" + right-click-open bypass (or `xattr -cr`), and hardened-runtime-dependent features (screen capture, microphone perms in webviews) may misbehave. Auto-updater on mac under Electron wants signed builds; unsigned = degraded update path. |

### Recommendation

Decouple from the shell question: the blocker is an Apple Developer Program membership + Developer ID cert, not tooling. If that exists (or is bought), **under Tauri enable it in s15 — it's pure config, ~half a day including CI secrets** (strictly cheaper than the Electron path, which additionally needs the notarize.js modernization). Under Electron it's also near-free (uncomment + rewrite auth to ASC key). Keep disabled only if there is deliberately no macOS distribution budget; in that case also drop mac from the release matrix rather than shipping un-notarizable builds users can't open cleanly on modern macOS (Sequoia's Gatekeeper makes unsigned apps progressively more painful).

---

## Summary recommendations

1. **Shell**: sequence it — Rust core via **napi-rs into the existing Electron shell** first (s02–s11 unchanged, reversible, ships continuously); Tauri shell deferred until multiwebview leaves `unstable` and WebKitGTK child-webview bugs close (Windows-only product could go Tauri today if the unstable flag is accepted as named risk in s15).
2. **Updates**: Electron → keep electron-updater (channels work; rewrite canary.js choreography as `gh` CI steps). Tauri → tauri-plugin-updater with per-channel `latest.json` endpoints via `updater_builder().endpoints()`; **reject cargo-dist/axo** (CLI-updater shape, no `latest.json`, no in-app flow).
3. **Notarization**: Tauri does it natively (env-var config, no hooks). Real cost is the $99/yr Apple Developer Program + Developer ID cert either way. Enable in s15 under Tauri (half day) or by uncommenting afterSign under Electron; don't ship unsigned mac builds silently.

### Source index

- Tauri webview/multiwebview: tauri-docs v2 (`webview/mod.rs` `add_child`, `unstable_struct!` gating), blog tauri-2-0-0-beta, issues #10131 #10420, r/tauri 1qxfkeu (Feb 2026, Win-works/Linux-broken report).
- Tauri protocols: `register_asynchronous_uri_scheme_protocol` (tauri-docs, streaming example).
- Tauri updater: v2.tauri.app/plugin/updater (platforms, signing keys, Endpoints/channel pattern, artifacts per OS); tauri-action GH docs (`latest.json` generation).
- NAPI-RS: napi.rs docs (v3 announce 2025-07; Electron integration guidance, ASAR/asarUnpack packaging notes; Node-API ABI stability).
- dist: axodotdev/cargo-dist config reference (`create-release`, `publish-prereleases`, `force-latest`, `install-updater` standalone updater).
- Apple: TN3147 (altool deprecation for notarization), developer.apple.com programs (membership cost).
- Repo: `docs/rust-rewrite-plan.md`, `scripts/canary.js`, `scripts/notarize.js`, `packages/app/electron-builder.yml`, `packages/app/src/webui/webUIHandler.ts`, `packages/app/src/services/services/{auto-updater,tab-webcontents,browser-window}/`, `packages/app/src/applications/`, `packages/app/src/common/components/ElectronWebview.tsx`.
