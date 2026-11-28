# Full Rust port — execution program

Status: **program for execution**, 2026-11-18. Direction changed from "native shell beside
Electron" to "Electron deleted, full Rust app". This file is the ordered, per-phase instruction
set; it makes no code changes and deletes nothing by itself. Companions:
`docs/electron-retirement.md` (evidence inventory, DND list, first deletion draft),
`docs/rust-packaging-plan.md` (installer decision: NSIS per-user, §5/§7),
`docs/oauth-config-options.md` (OAuth decision brief, Option B),
`docs/rust-rune-port.md` (ADR, D1–D12, robustness caps), `rust/README.md` (verified gaps).

Nothing here touches `rust/`, `packages/`, `STATE.md` or `docs/oauth-config-options.md`; the
accounts slice (another session) and the dead-TS sweep (a third) land in parallel, and every phase
below re-reads the tree before deleting.

Legend: **S** = one slice (≤ 1 day of agent work), **M** = 2–4 slices, **L** = 5+ slices or
blocked on a product/console decision. **DND** = do-not-delete-yet (a shipped behaviour lives
there). **Pivot** = the phase after which deleting Electron code is no longer user-visible risk.

---

## 0. The advisor question, and its answer

**No advisor exists in this environment.** Checked, with results: `command -v advisor` → none;
`where advisor` → "Could not find files for the given pattern(s)"; `advisor --help` and
`advisor list` → `command not found`; no `advisor`/`advise` binary under `%APPDATA%/npm`; no
`advisor` string in `.claude/` (skills: `loop-budget`, `loop-constraints`, `loop-triage`), no
`.mcp.json`. No advisor reply is quoted anywhere in this file.

The question put to it, verbatim:

> We have a working native Rust shell (Rune UI + WebView2 pages, App Store, settings,
> notifications, single instance, deep links, per-app config, icons, DPI) and a large
> Electron/TypeScript tree that still owns: tray, autolaunch, auto-update, chrome extensions,
> downloads, password managers, context menus, in-tab search, error dialogs, onboarding, bang
> search, favorites/subdock UI, and OAuth. What order minimizes user-visible regression while
> deleting the Electron tree and porting the rest to Rust? What must never be deleted before its
> Rust replacement exists?

Answered by me (no external advisor involved):

1. **Pivot on packaging, not on deletion.** Electron is the shipped app today; `station-shell` has
   no installer and no signing (`rust/README.md` "Feature gaps", packaging plan §7). Every
   user-visible regression risk collapses only when the native app is *installable* — so the
   packaging phase (P5) comes before any deletion of shipped behaviour, not at the end. Before P5,
   only deletions that cannot change what a user sees are permitted: dead types (already landed,
   `0fd0465`), mock fixtures, and TS fallback arms behind an already-validated Rust bridge.
2. **Order by consumer, never by folder.** Data layer before its consumers, routing before tabs, UI
   leaves before the store/trunk, store before entry files, and build config/root scripts last
   (they are what produce the shipped Electron binary until P5). Deleting `packages/appstore`
   first because "the store is already native in Rune" looks right and breaks the *build* — the
   Electron renderer still imports it (`packages/app/webpack.config.base.js:18` alias,
   `webpack.config.renderer.js:89` copy).
3. **In this repo the hardest couplings are not features but build inputs.** Three Rust
   crates compile or test against files inside `packages/`
   (`rust/crates/manifest-registry/src/lib.rs:185` `include_dir!` of
   `packages/app/manifests/definitions`; `rust/crates/station-shell/src/platform_icon.rs:35`
   `include_bytes!` of `packages/app/build/icon.ico`, also named by
   `rust/crates/station-shell/station.rc` and `build.rs`; and the parity test at
   `rust/crates/appstore-schema/src/lib.rs:301-309` reading
   `packages/appstore/api-schema.graphqls`). Deleting `packages/` before moving those breaks
   `cargo build`/`cargo test` — including the very tests that gate every phase. They move in P0.
4. **Never delete before the replacement is *shipped*, not merely merged**: tray, autolaunch,
   auto-update, OS notifications, downloads, context menus, in-tab search, error dialogs,
   password managers, chrome extensions, OAuth, the `<webview>` preload surface (the full
   `bxApi`), `session.ts` webRequest shaping, the published SDK/runtime apps, and the installer
   assets. Each is a row in §2 with its replacement's state. Where the honest answer is "this is
   dropped", that is a product decision, stated as one, not a silent cut.
5. **Keep the old binary shippable until the new one passes the same matrix.** The dangerous window
   is P5→P6: native installer shipped, Electron tree deleted, clean-VM validation not yet done.
   Mitigation: P5 ships the installer while Electron still builds; at least one release cycle (or
   one clean-VM pass plus the windowed matrix) passes before P6 starts; if it fails, revert the
   release, not the tree.
6. **Parallelize only ports, never two deleters.** Feature ports (P4) are independent slices on
   different crates and can run concurrently; deletions in one package touch one build graph, so
   one agent per phase, verified by a second.

---

## 1. The finish line

"Electron deleted, full Rust app" means, concretely:

- **The shipped artifact** is `station-shell.exe` (release, Windows x64 MSVC) inside a per-user
  NSIS installer (`rust/packaging/installer.nsi`, plan §7), signed with a human-provided identity,
  registering `HKCU\Software\Classes\station` and the WebView2 bootstrapper. There is no
  `electron.exe`, no `app.asar`, no Node runtime, no Chromium download.
- **The repository** is `rust/` (workspace + `vendor/` + `packaging/`), `docs/`, `LICENSE`,
  `README.md`, `.gitignore`, `.editorconfig`, `.gitattributes`, `AGENTS.md`/`LOOP.md`/`STATE.md`/
  `loop-*.md` (loop meta, kept), `.github/workflows/{rust,release-candidate}.yml`, `.claude/`,
  `.codegraph/` (dev tooling).
- **Gone**: `packages/app`, `packages/appstore`, `packages/sdk`, `dev_utils/`, `scripts/`,
  `.storybook/`, `@types/`, `yarn.lock`, `node_modules`, `package.json` (root), `tsconfig.json`,
  `tslint-ci.json`, `.eslintrc`, `.eslintignore`, `.prettierignore`, `.npmrc`, `.nvmrc`,
  `.yarn/`, `.yarnrc.yml`, `.storybook`, `packages/app/electron-builder.yml` + `build/`,
  `packages/app/webpack.config*.js`, `.github/workflows/{tests,release}.yml`. `.env*` are
  **not** in this program's deletion sets (loop-constraints names them; the client id moves to CI
  secrets / build env — human-gated, §6).
- **Moved before the tree dies** (P0): manifest definitions, the icon assets, the schema parity
  anchor, the installer assets.
- **The replacement proof** for every removed concern is a row in §2 whose "Rust equivalent" says
  *exists* or whose "decision" says *dropped* — and dropping is only legitimate when written here
  as a product decision with the regression named.

Non-goals: porting Redux/Apollo/sequelize machinery for its own sake; re-implementing Electron's
`<webview>` tag (WebView2 child windows already own page hosting, `station-shell/src/webview.rs`);
keeping macOS/Linux builds (the shell is Windows-only today: wry target-gated in
`rust/crates/station-shell/Cargo.toml`, `platform_icon.rs` `cfg(target_os = "windows")`).

---

## 2. Feature ledger

### 2a. Build couplings — these block everything else

| Concern | Today | Rust equivalent | If deleted today | Smallest honest move |
| --- | --- | --- | --- | --- |
| Manifest definitions (638 JSON apps, verified count) | `packages/app/manifests/definitions/**`, embedded into `manifest-registry` by `include_dir!` (`rust/crates/manifest-registry/src/lib.rs:185`) and compared to disk by `lib.rs:733`; the Electron build loads the same files with `require.context('./definitions')` (`packages/app/manifests/index.ts:10`) | **Compile input** of the Rust build | `cargo build` fails; the store has no apps | **P0**: make `rust/assets/manifests/definitions/` canonical; point `include_dir!`, the disk test, and the TS `require.context` at it; delete the old copy |
| App icon | `packages/app/build/icon.ico` — `include_bytes!` (`platform_icon.rs:35`), `station.rc` `IDI_ICON1 ICON "../../../packages/app/build/icon.ico"`, `build.rs` `rerun-if-changed`, electron-builder implicit `buildResources` | compile input + packaged asset | `cargo build` fails; exe/window icon vanish | **P0**: `rust/assets/icon.ico` (+ `installerIcon.ico` under `rust/packaging/assets/`), update the three Rust references and electron-builder's explicit `win.icon`/`nsis.installerIcon` paths |
| `api-schema.graphqls` parity | test reads `packages/appstore/api-schema.graphqls` (`rust/crates/appstore-schema/src/lib.rs:301-309`) | verbatim copy already embedded (`SCHEMA_GRAPHQL`, `rust/crates/appstore-schema/schema/api.graphqls`) | one **test** fails (not the build) | **P0**: commit `schema/api-schema.graphqls.sha256` and assert the source copy's digest, or retire the test with a note — then delete with `packages/appstore` (P7) |
| Installer assets | `packages/app/build/{installer.nsh,installerIcon.ico,entitlements.mac.plist}`, `electron-builder.yml` | none yet (`rust/packaging/` absent) | NSIS slice has no icon/shortcut asset | **P0/P5**: copy what NSIS needs into `rust/packaging/assets/` (plan §7 step 1 lists `icon.ico` + bootstrapper) |

### 2b. Shipped concerns (retirement doc "Must survive the shell" + the direction-change list)

| Concern | Lives today | Rust today | If deleted today | Smallest honest port / decision | Size |
| --- | --- | --- | --- | --- | --- |
| Installer + signing | `packages/app/electron-builder.yml`, `packages/app/build/` | **installer script & local smoke done (P5 prep)**: `rust/packaging/installer.nsi` (per-user HKCU + WebView2 bootstrapper + `station://` protocol); signing + clean-VM pass still human-gated | no way to ship the native app | NSIS per-user installer per plan §7; signing = human identity decision (plan §8) | M |
| Auto-update | `services/services/auto-updater/lib.ts` (`electron-updater ^6.2.1`), `src/auto-update/sagas.ts` | **GitHub Releases checker + beta channel ported (slices 24, 34)**: `updater.rs` queries GitHub Releases over `ureq` rustls with SemVer 2.0 comparison, trusted URL enforcement, `Download release` button, and persisted `includeBetaUpdates` toggle; staged in-app binary replacement remains product-gated | no in-app binary replacement | **Product decision.** Real release checker + beta channel landed; staged replace/signature/rollback is L | L / S |
| Tray | `services/services/electron-app/main.ts` (`Tray`, lines 41, 136-212) | **ported incl. close-to-tray (slices 18, 35)**: `station-shell/src/tray.rs` uses Rune's `Tray` (`Shell_NotifyIcon`, `TrackPopupMenu`, Open/Exit), `rust/assets/station.ico`, and Windows `minimizeToTray` (`.keep_running()`, `webview::hide_all()`, tray/single-instance reopen via `AppHandle::open()+focus()`) | the app cannot live in the tray; closing/quitting semantics change | Done in code; Windows close-to-tray visual smoke pending | M |
| Autolaunch (login item) | `services/services/autolaunch/` | **ported (slice 18)**: `station-shell/src/autolaunch.rs` reads/writes the same `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` `Station` value (quoted exe, no args, unelevated); Settings → General toggle; additive `startAtLogin` v1 field; registry is the truth on launch | "launch at login" disappears | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value + the existing start-flag path; Settings toggle | S |
| OS notifications | `services/services/os-notification/`, `src/notifications/` | **ported (slices 8, 15, 19, 29)**: page source + `window.Notification` override (`notifications/bridge.rs`), Win32 `Shell_NotifyIconW` (`NIF_INFO`) balloons with 12/min cap & snooze suppression (`notifications/os.rs`), and `Snooze::SyncWithOs` DND polling via `SHQueryUserNotificationState` (5 s throttle, fanning out to attached pages). Windows pixel/balloon look unverified on Linux | notifications stay inside the Rune panel; OS toasts stop | Done in code; Windows visual smoke pending | M |
| Downloads | `src/downloads/`, `src/dl-toaster/`, `services/services/download/` | **ported incl. bottom bar & custom folder (slices 22, 33)**: `downloads.rs` sanitizes page-suggested names (incl. compound Windows reserved names like `NUL.tar.gz`), writes under OS Downloads or persisted custom `downloadDir` without overwriting, caps 4 concurrent, and renders a 40 pt bottom Rune bar (`Open`, `Folder`, `Dismiss`, `Clear all`) | download toaster gone | Done | S/M |
| Context menus | `src/context-menu.ts`, `src/context-menus/` | **ported (slice 26)**: WebView2 page menu kept; Rune top-bar web controls + `tab_actions_bar` (right-click rail or `...`: Reload, Pin/Unpin favorite, Open in browser, Configure, Move up/down, Uninstall) + global keyboard shortcuts | right-click loses every app action | Done | M |
| In-tab search | `src/in-tab-search/` | **ported (slice 27)**: `find.rs` (`Ctrl/Cmd+F`, 40 pt Rune bar shrinking content rect cleanly, JSON-escaped `window.__stationFind` / `__stationFindClear` literal DOM search, `kind: "find"` IPC counter); verified live in real Chrome via `agent-browser` | Ctrl+F does nothing in app pages | Done | M |
| Error dialogs | `src/dialogs/` | **ported (slice 24)**: `dialogs.rs` Win32 `MessageBoxW` fatal error dialog (stderr fallback elsewhere) around `app.run()` + pane `page_error` + Retry | load failures show a bare page; general errors vanish | Done | S |
| Password managers | `src/password-managers/` (1Password/Bitwarden providers) | none | provider integration gone | **Product decision**: WebView2's own password store (drop, zero work) or port providers (L) | L |
| Chrome extensions | `src/chrome-extensions/`, `services/services/ecx/` | none | extension-manager features and `ecx` CORS handler gone | decide: WebView2 browser extensions (`AreBrowserExtensionsEnabled` + profile dir) vs drop; ECx parity is L | L |
| OAuth / identity | `services/services/electron-google-oauth/`, `user-identities/` | **ported (slices 17, 25)**: `oauth.rs` Option B — RFC 7636 PKCE (`S256`) + loopback HTTP callback on `127.0.0.1:0` (constant-length state check, static XSS-free HTML, zero tokens stored) + Settings Accounts sign-in/cancel/remove. Verified live in Chrome; live Google account round-trip needs human Desktop client ID | Gmail/Slack stay signed-out; Settings says sign-in unavailable | Done in code; live Google round-trip is human | M |
| Session webRequest shaping / UA | `src/session.ts` (+ `electron-better-web-request`) | **ported per-webview (slice 21)**: `with_user_agent` (Electron default UA or manifest `bx_override_user_agent`) + per-app WebView2 `WebContext` profile (`persist:<id>` or default). Per-request header interception not exposed by wry | per-request header overrides (`ecx-cors`, localhost Referer) unavailable | Done within wry bounds | S |
| `<webview>` preload / `bxApi` | `src/plugins/webview-preload.js`, `plugins/injected-js/`, `plugins/slack/`, `static/preload/` | **ported (slices 8, 15, 19, 20, 23, 27, 31)**: frozen `window.bxApi.notificationCenter` (all 6 members incl. snooze listeners), `window.Notification` override, `document.title` + `browserx-badge` + live `<link rel~="icon">` favicon observer, in-tab find hooks, Chrome compat shims (`window.chrome.webstore`/`.runtime` + Google Docs markers), and all 7 per-site scripts (`site_scripts.rs`, covering all 14 legacy service IDs). Verified live in real Chrome | page integrations beyond core web | Done for all 14 legacy services + Chrome shims | L |
| Published SDK + runtime apps | `packages/sdk`, `packages/app/manifests/runtime/{gdrive,notion,slack,slite,common,utils}` (loaded by `packages/app/src/plugins/api.ts` `require.context`; renderer runtimes named by definitions `21.json` Slack, `129.json` Notion; gdrive reads `GOOGLE_CLIENT_ID`) | none | Slack/Notion renderer integrations and gdrive service runtime disappear | **Product decision**: drop the SDK runtime apps in the Rust app (pages still work; integrations do not) or port them as Rust services (L) | L |
| Single instance / deep links | Electron: none; native: done | `instance.rs`, `deep_link.rs` | — | already landed (slices 12/14); `station://` registration joins in P5 | — |
| Crash reporting | none anywhere | **ported (slice 30)**: `crash.rs` panic hook writing bounded (8 KiB) secret-free crash reports to `<config>/station/crashes/` (pruned at 20 files) | nothing to preserve | Done | S |
| Permissions prompt | none anywhere | **ported (slice 30)**: `permissions.rs` wired into `wry::WebViewBuilder::with_permission_handler` (denies web `Notifications` + silent/privileged APIs; prompts interactive camera/mic/display/geo/clipboard/autoplay) | nothing to preserve | Done | S |
| Onboarding / organizations / os-bar | `src/onboarding/`, `organizations/`, `os-bar/` | none | first-run tips, org switcher, os-bar gone | **Product decision**: drop (each is UI-only; the Rune top bar and store replace the visible parts) | S each |
| About window | `src/about-window/`, `about-window` crate | **ported (slice 24)**: `about.rs` native Rune surface over `about-window` (Settings → General + `--scene about`) | no About | Done | S |
| Bang search / favorites / subdock | `src/bang/`, `favorites/`, `ordered-favorites/`, `subdock/`, `dock-navigation/` | **ported (slice 28)**: `quick_switch.rs` (`Ctrl/Cmd+K`/`T` Bang Quick Switcher over `activity::frecency`) + persisted `FavoriteEntry` in `session-store` + top-bar Star toggle + App Store quick-install | power-user navigation lost | Done (subdock multi-tab preview intentionally simplified to rail + Quick Switcher) | M |
| Popup windows | `src/subwindows/`, `windows/GenericWindowManager.ts` | ADR D9: new windows route to a tab or the OS browser | popups behave differently (already the native behaviour) | record as the shipped decision (D9); no port | — |

---

## 3. Phases (dependency order)

Every phase: **prereqs** must be *merged and verified* first; **delete** is exact; **verify** runs
by the agent unless marked human; **rollback** is a tag the humans create, because agents must not
run destructive git. Tag convention: `port/p<N>-done` on `main`; rollback = branch from it.

### P0 — Decouple the Rust build from `packages/` (S, no user-visible change)
**Prereqs:** none (safe now).
**Move (not delete):**
- `packages/app/manifests/definitions/**` → `rust/assets/manifests/definitions/**`.
  Edit `rust/crates/manifest-registry/src/lib.rs:185` (`include_dir!` path) and the disk test at
  `lib.rs:733`; edit `packages/app/manifests/index.ts:10` `require.context('./definitions')` to
  the `rust/assets/...` path (webpack context paths accept relative paths outside the package —
  **UNVERIFIED until `yarn build` runs**; fallback: keep a copy + a `sha256` drift check in
  `rust.yml`).
- `packages/app/build/icon.ico` → `rust/assets/icon.ico`; `installerIcon.ico` →
  `rust/packaging/assets/installerIcon.ico`. Edit `platform_icon.rs:35`, `station.rc`, `build.rs`;
  add explicit `win.icon`/`nsis.installerIcon` to `packages/app/electron-builder.yml` (until it is
  deleted in P8).
- Parity anchor: add `rust/crates/appstore-schema/schema/api-schema.graphqls.sha256` and change
  `rust/crates/appstore-schema/src/lib.rs:301-309` to compare digests (the TS file dies in P7).
**Delete (this phase only):** `packages/app/manifests/definitions/` and
`packages/app/build/icon.ico` **after** the moves compile and build — nothing else.
**Verify:**
`cd rust && cargo fmt --all --check && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`;
`rg -n "packages/(app|appstore|sdk)" rust/crates` reviewed: every remaining hit is a `//`/`//!`
comment naming the TS module a port came from — **no** `include_dir!`, `include_bytes!`, `fs::read`
or `env!(...CARGO_MANIFEST_DIR...)` path may remain;
`rg -n "include_dir!|include_bytes!" rust/crates` shows only in-tree paths;
`yarn install --immutable && yarn build` (Electron still builds against the moved definitions).
**Rollback:** `port/p0-done`; restoring the two moved directories from it.
**Why first:** until this lands, *every* later deletion breaks `cargo test` — the gate for
everything else.

**Executed (2026-11-18): copied, not deleted.** This supersedes the "Move (not delete)" / "Delete
(this phase only)" wording above. `rust/assets/` now holds `definitions/` (638 files,
byte-identical to the originals), `station.ico` (from `packages/app/build/icon.ico`) and
`api-schema.graphqls` (from `packages/appstore/api-schema.graphqls`); the three Rust inputs point
there — `manifest-registry`'s `include_dir!` and disk test (`$CARGO_MANIFEST_DIR/../../assets/…`),
`platform_icon.rs`'s `include_bytes!` plus `station.rc` and `build.rs`'s rerun path
(`../../assets/station.ico` from the crate root), and the `appstore-schema` parity test. `packages/`
is untouched, so the same bytes exist twice for now: the TS tree still needs its originals, and
deleting `packages/app/manifests/definitions`, `packages/app/build/icon.ico` and the schema source
belongs to the TS-removal phase, not P0. Drift is caught meanwhile: the definitions test asserts
638 files, parses every JSON file, and compares each embedded file's bytes to the copied directory;
the schema test compares the embedded schema to the copied anchor.

### P1 — Dead surface (S) — **already landed; this phase verifies and adopts the residual**
**State today:** the dead types are **gone**. `packages/app/src/plugins/bxapi.d.ts:9-14` records
it: `user`, `services`, `Runtime`, `AuthorizationError`, `NoMethodError`, `SystemError` were
declared historically but never constructed by `webview-preload.js`; deletion landed in commit
`0fd0465` ("w03-watchers-dead-types") after an `rg` check. `rg "bxApi\.user|bxApi\.services"
packages/app/src` returns nothing today (verified 2026-11-18). **Do not re-delete it.**
**Prereqs:** P0.
**Delete:** nothing from this phase's own set unless the parallel dead-TS session leaves verified
leftovers. The rule for taking those over: zero call sites proven by `rg` (`\.tss?`-agnostic, whole
tree, excluding `node_modules`), the module named nowhere in `packages/app/package.json` scripts,
and no live import from an entry (`main.ts`, `app-main.js`, `worker.ts`, `servicesManager.ts`).
Candidates the retirement survey left with no call sites (re-verify before touching):
`packages/app/src/api/logger.ts` (only if its three importers are gone),
`@types/react-hover-observer` + its `webpack.config.renderer.js` externals entry (dies with the
build config, P8, unless the internals die first).
**Coordination:** the third session is deleting dead TS *now*. Before deleting anything here, read
`git log --oneline -5` and the file's presence; delete nothing another agent is mid-flight on —
this phase's job is to *verify and record*, not to race.
**Verify:** the three `rg` proofs above per candidate; `yarn install --immutable && yarn build`;
`cd rust && cargo test --workspace` (record the pre-phase count and require it unchanged).
**Rollback:** `port/p1-done` (likely a no-op commit: documentation of the verification).

### P2 — Appstore mocks and fallback arms behind the bridge (S/M)
**Prereqs:** the bridge live-validation human gate (STATE item 13) and the default flip of
`STATION_RUST_BRIDGE` (`packages/app/src/services/services/sdkv2/worker.ts` reads it).
**Delete:**
- `packages/appstore/src/applications/mockedMostPopularApps.ts` and its consumers' mock imports
  (`packages/appstore/src/HOC/withMostPopularApplications.ts` — rewrite to read the bridged
  channel);
- the `STATION_RUST_BRIDGE` flag reads and the TS fallback dispatch arms in
  `packages/app/src/services/services/sdkv2/worker.ts` for the **10 bridged channels** (keep the
  two unbridged: `setConfigData`, `requestLogin`);
- `packages/appstore/src/api.ts` + `packages/appstore/src/app-request/**` arms for those 10
  channels (the Rust `appstore-service` is the owner; `app_request` state machine already ported).
**Do not delete:** `rust-bridge.ts` (it becomes the only path), the two unbridged channels, any
saga whose Rust replacement is not `appstore-service`.
**Verify:** `node scripts/rust-bridge-smoke.js` (16 checks); `yarn workspace @getstation/appstore build`;
`rg "mocked" packages/appstore/src` → exit 1; `cargo test -p station-bridge`;
`cargo test --workspace`.
**Rollback:** `port/p2-done`; the flag removal is reverted by restoring `worker.ts` from it.

### P3 — Data-layer arms (M, split 3a/3b)
**Prereqs:** 3a = OAuth Option B slices 1–2 (`docs/oauth-config-options.md`: identity store +
PKCE/loopback core) merged, `request_login` reachable from Settings; 3b = native routing/tab
ownership already landed (slices 9/11) + `application-settings` decision (`isNotInstallableApplication`
is still a TS selector feeding `urlrouter/URLRouter.ts`).
**Delete 3a:** `packages/app/src/user-identities/` (whole directory),
`packages/app/src/user-activities/`, `packages/app/src/activity/` (model/sagas/resolvers),
`packages/app/src/services/services/electron-google-oauth/`,
`packages/app/src/services/services/authentication/`, `packages/app/src/services/services/sdkipc/`,
`packages/app/src/about-window/` (after the Rune About modal exists).
**Delete 3b:** `packages/app/src/urlrouter/`, `packages/app/src/services/services/url-router-helper/`,
`packages/app/src/tabs/`, `packages/app/src/ordered-tabs/`,
`packages/app/src/tab-webcontents/`, `packages/app/src/services/services/tab-webcontents/`
(after `application-settings/`'s installability selectors are re-homed or dropped).
**Verify:** `cargo test --workspace`; `yarn build`; manual: install → uninstall, sign in with
Google (live round trip is human, §6), deep-link to an installed app.
**Rollback:** `port/p3a-done`, `port/p3b-done`.

### P4 — Feature ports (parallel slices; M aggregate) — the DND list, one row at a time
**Prereqs:** P0; each slice's own surface (a Rust seam exists or is added with tests).
**Port order (each flips one §2b row; each slice: red test → code → headless test, per the
project's slice discipline):**
1. Autolaunch (S) — **done (slice 18)**: registry Run value + Settings toggle (no start-minimized argument; the TS autolaunch passed none either).
2. OS notifications (M) — toast source + click → `url-router`; `Snooze::SyncWithOs` reader.
3. Tray (M) — **done (slice 18)**: icon + Open/Exit menu (show/focus, quit) over slice 14's focus path; no notifications list (the TS tray had none).
4. Error dialogs (S) + About modal (S).
5. UA / session shaping (S) — `with_user_agent`; verify Slack/Notion render.
6. Downloads (S/M) — decision first: WebView2's own UX (drop the manager) or a Rust manager.
7. Context menus (M) — tab actions in Rune; keep WebView2's page menu.
8. In-tab search (M) — spike `Find` via WebView2; Rune overlay.
9. Bang search (M) + Favorites/subdock (M) over `activity` frecency and `session-store` `dock`.
10. Preload/`bxApi` inventory and port (L) — the largest: enumerate what pages call, port members
    behind `with_initialization_script` like `notifications/bridge.rs`.
11. Password managers (L), Chrome extensions (L), SDK runtime apps (L),
    auto-update (L) — **each gated on an explicit product decision** (§2b); do not start without it.
**Verify per slice:** `cargo fmt --all --check && cargo test -p station-shell && cargo clippy -p station-shell --all-targets -- -D warnings`;
windowed smoke where the feature is OS-visible (human, §6).
**Rollback:** per-slice commits; no tag (nothing deleted).

### P5 — Packaging and the pivot (M) — **the shipped app becomes native**
**Prereqs:** P4's tray + autolaunch + OS notifications decided and landed (or explicitly dropped);
OAuth P3a merged; `station://` registration; P0 assets.
**Build:** `rust/packaging/installer.nsi` per plan §7 (per-user, `InstallDir
"$LOCALAPPDATA\Programs\Station"`, `WriteRegStr HKCU Software\Classes\station …`, WebView2
bootstrapper `ReadRegStr pv` check, uninstaller never touching `%APPDATA%\station` /
`%APPDATA%\BrowserX`); a `package-windows-native-shell` job in
`.github/workflows/release-candidate.yml` (windows-2025, `cargo build --release -p station-shell`,
`choco install nsis`, `makensis`), signing step gated on secrets.
**Delete:** nothing (this phase only adds).
**Verify (human-run, §6):** clean Win10/11 VM without WebView2 installs without UAC; runtime
present or degraded; `reg query HKCU\Software\Classes\station /s`; `start station://appstore`
launches and selects the store; uninstall leaves user data and removes the key; Start-Menu icon;
window/taskbar icon.
**Rollback:** do not delete the Electron builder until P6; keep `release.yml` producing the
Electron build as the fallback until one cycle passes.
**Why the pivot:** from here the Electron tree is a *fallback*, not the product; deleting it stops
being a feature risk and becomes a cleanup with a revertable release.

### P6 — Renderer UI leaves, then the trunk (M)
**Prereqs:** each deleted module's §2b row flipped (P4) **and** P5 shipped and validated.
**Delete, in two sub-phases:**
- 6a leaves: `src/onboarding/`, `organizations/`, `os-bar/`, `dl-toaster/`, `downloads/`,
  `favorites/`, `ordered-favorites/`, `dock-navigation/`, `subdock/`, `theme/`, `settings/`,
  `bang/`, `notifications/`, `notification-center/`, `application-settings/`, `dialogs/`,
  `context-menu.ts`, `context-menus/`, `in-tab-search/`, `password-managers/`,
  `chrome-extensions/`, `keyboard-shortcuts.ts`, `app-shortcuts.js`, `nav/`, `ui/`,
  `components/`, `containers/`, `app-store/`, `app/`, `about-window/`, `user-identities/`
  (remainder), `subwindows/`, `windows/`.
- 6b trunk: `store/`, `reducers/`, `sagas.ts`, `state.ts`, `observers/`, `pubsub/`, `graphql/`,
  `database/`, `persistence/`, `plugins/`, `session.ts`, `webui/`, `servicesManager.ts`,
  `services/`, `static/preload/` + injected scripts, `app-main.js`, `app-worker.ts`, `main.ts`,
  `index*.js`, `*.html`, `cli.ts`, `dotenv.ts`, `api/`, `common/`, `lib/`, `utils/`,
  `autolaunch`/`menu`/`ecx`/`session` service remnants.
**Verify:** `cargo test --workspace`; `yarn install --immutable && yarn build` still green while
`main.ts` exists → after 6b, `yarn build` is gone by design (P8); packaged smoke per §5.
**Rollback:** `port/p6a-done`, `port/p6b-done`.

### P7 — Remove `packages/appstore`, `packages/sdk`, manifests runtime (M)
**Prereqs:** P0 moved definitions; P4's SDK-runtime decision executed (dropped or ported); P6b
done (no TS consumer remains).
**Delete:** `packages/appstore/` (remainder: `src/**`, `api-schema.graphqls` — parity anchor
already moved in P0), `packages/sdk/`, `packages/app/manifests/runtime/`, `manifests/icons/`,
`manifests/index.ts`, `manifests/private.ts`, `packages/app/manifests/` (empty after definitions
moved).
**Verify:** `rg -n "@getstation/(sdk|appstore)" .` → 0 hits; `rg -n "api-schema.graphqls" rust/`
→ only the in-tree copy; `cargo test --workspace`; installer rebuild (P5 script) still installs.
**Rollback:** `port/p7-done`.

### P8 — Build config, root scripts, Electron CI (S)
**Prereqs:** P6b/P7 (nothing imports the packages), P5 installer green.
**Delete:** `packages/` (the now-empty tree), `dev_utils/`, `scripts/` (including
`rust-bridge-smoke.js`, `notarize.js`, `canary.js`), `.storybook/`, `@types/`, `tsconfig.json`,
`tslint-ci.json`, `.eslintrc`, `.eslintignore`, `.prettierignore`, `.npmrc`, `.nvmrc`, `.yarn/`,
`.yarnrc.yml`, `package.json`, `yarn.lock`, `packages/app/electron-builder.yml`,
`packages/app/build/`, `.github/workflows/tests.yml`, `.github/workflows/release.yml`
(release-candidate keeps only the native job).
**Edit:** `AGENTS.md` (test commands become `cargo …`), `README.md`, `.github/workflows/rust.yml`
(add a `windows-2025` job; the shell only exists on Windows).
**Verify:** fresh clone → `cd rust && cargo test --workspace`; `cargo build --release -p
station-shell`; `makensis` installer; clean-VM install; `rg -n "yarn|npm |electron" .github/ README.md AGENTS.md`
shows only intentional history.
**Rollback:** `port/p8-done` (the tag *is* the last Electron-containing commit).

**Executed (2026-11-26, user-approved on `feat/full-rust-port`):**
- P6 removed `packages/app/`, P7 removed `packages/appstore/` and `packages/sdk/`, and P8 removed
  `dev_utils/`, `scripts/`, `.storybook/`, `@types/`, `.yarn/`, `package.json`, `yarn.lock`,
  TS/ESLint/TSLint/Prettier/CodeClimate configs, and `.github/workflows/{tests,release}.yml`.
- Last Electron-containing commit on `feat/full-rust-port`: `d393a3c`.
- `.env.development`, `.env.example`, and `.env.production` were left untouched per `loop-constraints.md`.
- `.github/workflows/release-candidate.yml` keeps only the native Rust `package-windows` NSIS job
  (`tags: ['v*']` + `workflow_dispatch`); `.github/workflows/rust.yml` runs both `ubuntu-latest`
  and `windows-2025` build/clippy/test on pushes to `main` and all pull requests.
- `AGENTS.md` and `README.md` updated to Cargo workspace commands.
- Product decisions recorded by retiring Electron now: Chrome extensions, 1Password/Bitwarden
  provider integrations, legacy JS SDK runtime apps, and Electron onboarding/orgs/os-bar are
  dropped from the native Rust app (web apps still run in WebView2 with per-app profiles, Chrome
  compat shims, and WebView2's built-in password autofill); updates use the native GitHub Releases
  checker + beta channel (`updater.rs`) with manual browser download until/unless a signed staged
  updater is later prioritized.

**Follow-up (2026-11-28)** restored the first-run app picker, added opt-in WebView2 password
saving and per-profile management for unpacked extensions, and added a one-time import from the
legacy Electron database. The Quick Switcher can install the app for a pasted link and save a
workspace subdomain declared by its manifest. See [STATE.md](../STATE.md) for current release
gates and remaining parity work.

---

## 4. The honest schedule

| Item | Size | Gates phase | Notes |
| --- | --- | --- | --- |
| P0 decouple | S | P1–P8 (all) | 3 build inputs; no user-visible change |
| P1 dead types | **done** (`0fd0465`) | — | verify only; adopt the residual sweep |
| P2 mocks + flag | S/M | P3 | needs the bridge human validation |
| OAuth/identity (Option B: store, PKCE+loopback, Settings row) | M (4 slices) | P3a, P5 | `docs/oauth-config-options.md`; live round trip is human |
| Autolaunch | S | P4, P5 | registry value + toggle |
| OS notifications + SyncWithOs | M | P4, P6a | toast + click routing |
| Tray | M | P4, P5 | show/focus/quit menu |
| Error dialogs + About | S+S | P4, P6a | MessageBox + Rune modal |
| UA / session shaping | S | P4, P6b | `with_user_agent`; behaviour **UNVERIFIED** |
| Downloads | S if WebView2 UX, M if manager | P4, P6a | product decision |
| Context menus | M | P4, P6a | tab actions in Rune |
| In-tab search | M | P4, P6a | WebView2 `Find` spike first |
| Bang search | M | P4, P6a | frecency exists |
| Favorites / subdock | M | P4, P6a | session state exists |
| `<webview>` preload/`bxApi` | L | P4, P6b | **largest regression**; inventory before port |
| Password managers | L or drop | P4, P6a | product decision |
| Chrome extensions | L or drop | P4, P6a | product decision; WebView2 extension support **UNVERIFIED** |
| SDK runtime apps (Slack/Notion renderers, gdrive service) | L or drop | P4, P7 | product decision |
| Auto-update | L (parity) / S (check-and-link) | P5, P6a | product decision; recommend S for v1 |
| Installer + signing | M (+ human signing) | P5 | plan §5–§7 |
| Onboarding / orgs / os-bar | drop (S each if kept) | P6a | product decision to drop |

Total agent work: **≈ 12–16 slices** for ports + **5–6 slices** for deletions/CI, plus two
decision-gated L items (extensions, SDK runtime) whose size is the reason they need a decision.

---

## 5. CI and verification matrix

`AGENTS.md` currently says `npm test` / `npm run lint`; that stops being true at P8 (and stops
being *possible* at P6b). Replacement:

| Was | Becomes |
| --- | --- |
| `npm test` | `cd rust && cargo test --workspace` |
| `npm run lint` | `cd rust && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings` |
| `yarn build` | `cd rust && cargo build --release -p station-shell` (+ `makensis rust/packaging/installer.nsi`) |
| `.github/workflows/tests.yml` (Linux/Mac/Windows Electron builds) | deleted in P8 |
| `.github/workflows/release.yml` (Electron release) | replaced by the native job moved into `release-candidate.yml` (P5) |
| `.github/workflows/rust.yml` (Linux only) | keeps format/build/clippy/test **and** gains a `windows-2025` job: the shell's real target (wry/WebView2, `platform_icon`, `embed-resource`) is only compiled there; Linux stays for the portable crates |
| `scripts/rust-bridge-smoke.js` | deleted with `scripts/` (P8); its checks live in `cargo test -p station-bridge` |

Commands per phase boundary (agent-runnable unless marked human):

- Every phase: `cd rust && cargo fmt --all --check && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`.
- While TS exists (P0–P5): `yarn install --immutable && yarn build`; P2 adds
  `node scripts/rust-bridge-smoke.js`; P0/P2 add the specific `rg` exits named in the phase.
- After P5: `cargo build --release -p station-shell`; `makensis rust/packaging/installer.nsi`;
  `reg query HKCU\Software\Classes\station /s`.
- Human (each P5+ boundary and after every deletion phase): clean-VM install (no WebView2 →
  degraded), launched app smoke (install Gmail/Slack, sign in, open a page, notification, tray,
  autolaunch), uninstall preserving `%APPDATA%\station`, Explorer/taskbar icon, `start
  station://appstore`.

---

## 6. What only a human can do

Separate from everything above, because no agent can complete these:

1. **Signing identity / certificate** (plan §8 Open decision 1) and the CI secret it lives in.
2. **OAuth client registration** (Google console: client type Desktop, loopback redirect URIs) and
   the consent-screen verification state (`docs/oauth-config-options.md` UNVERIFIED list).
3. **OAuth live round trip** — a browser, a real Google account, a code, a token exchange.
4. **Windows visual checks**: Explorer/taskbar/window icon rendering, tray menu appearance, toast
   appearance, context-menu layout, in-tab search overlay, Alt+Tab, real monitor switch (DPI).
5. **Clean-VM installs** (no WebView2; UAC-free; uninstall semantics) and the packaged-app smoke.
6. **Product decisions that gate ports**: auto-update parity vs check-and-link, downloads manager
   vs WebView2 UX, password managers, chrome extensions, SDK runtime apps, onboarding/orgs/os-bar
   drop.
7. **Live bridge validation** before P2 (`STATION_RUST_BRIDGE=1` on the Electron app) — STATE item
   13, still open.

---

## 7. UNVERIFIED and assumptions

- **UNVERIFIED:** webpack `require.context` accepting a path outside `packages/app` for the moved
  definitions, and electron-builder 26 accepting an out-of-package `win.icon` path. Fallback for
  both: keep a copy plus a `sha256` drift check in `rust.yml` until the phase after.
- **VERIFIED:** 638 definition files (`ls packages/app/manifests/definitions/*.json | wc -l`,
  2026-11-18). The Rust `manifest-registry` reads whatever the directory holds, so the number is
  informational; the *move* is what matters.
- **UNVERIFIED:** wry/WebView2 exposure of `Find` (in-tab search), browser extensions
  (`AreBrowserExtensionsEnabled`), and download interception from the wry 0.57 API. Each needs a
  spike before its size is confirmed.
- **UNVERIFIED:** the exact membership of `webview-preload.js`'s `bxApi` surface actually called by
  pages; the port order in P4.10 assumes an inventory first.
- **UNVERIFIED:** whether any installed manifest *requires* a renderer runtime to function
  (Slack/Notion runtimes are named by definitions `21.json`/`129.json`; what degrades without them
  is not measured).
- **Assumption:** Electron must keep building until P6 starts (the fallback release). If the team
  is willing to ship native-only immediately after P5, P6–P8 collapse into one large deletion phase
  — but then the only rollback is the release artifact, so the program keeps them separate.
- **Assumption:** `rust.yml` on Linux keeps passing (it does today with Windows code cfg-gated);
  P8 adds the Windows job because "tests green on Linux" is not evidence about the shipped target.
- Sizes are planning judgements from the inventory, not measurements.
