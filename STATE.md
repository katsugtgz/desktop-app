# Loop State — My Project

Last run: 2026-11-18 rust-rune-port — packaging prerequisite (exe icon resource) landed; OAuth/config decision brief written for the open product decision

## High Priority (loop is acting or waiting on human)

1. **[DONE] Rune native shell slice 16 — the panel draws notification icons.** Rows draw the
   notification's `icon` URL through the existing icon store in a 20 pt square, with the
   letter-avatar fallback (manifest tint, else the id accent); asks are bounded at **32** distinct
   URLs per frame and made only while the panel is visible, and the panel's asks precede the store
   lists because the in-flight cap is drop-not-defer. Gates: `station-shell` **176**, workspace
   **492**, fmt/clippy clean, release **29,073,408 B**.
2. **[DONE] Window icon — a real defect found and fixed (windowed re-probe).**
   [`docs/window-icon-probe.md`](docs/window-icon-probe.md) caught the shell's icon creation
   returning NULL with no `WM_SETICON` sent, while an independent process succeeded with the same
   pixels. Root cause: the synthesised header wrote `biSize` with `usize::to_le_bytes()` (8 bytes
   on x64), shifting every field by four — a 4 268-byte blob instead of 4 264. Fix: embed
   `packages/app/build/icon.ico` and hand its own 32×32 / 64×64 `RT_ICON` entries to
   `CreateIconFromResourceEx` (the PNG/scale/BGRA path is deleted). Windowed re-probe on the new
   binary: `WM_GETICON` `ICON_SMALL` non-null, color **32×32 32 bpp**; `ICON_BIG` non-null,
   **64×64 32 bpp**; class icons still null, so these are the per-window `WM_SETICON` icons; clean
   close, 0 processes left. Not proven: pixel appearance, taskbar/DWM rendering per DPI, Alt+Tab.
3. **[DONE] Rune native shell slice 15 — a notification click reaches the page that raised it.**
   The injected script exposes four frozen members of `window.bxApi.notificationCenter`
   (`sendNotification`, `closeNotification`, `addNotificationClickListener`,
   `removeNotificationClickListener`) plus an internal non-writable
   `window.__stationNotificationClick` hook. A row press marks read and routes as before, then
   delivers exactly one `WebView::evaluate_script` to the **source tab only** (`WebviewHost::click`);
   the id is JSON-encoded with `serde_json` (a hostile id `a"'<script>\n` goes out as a one-line
   literal), at most **8** listeners per page, each callback in its own `try/catch`, and a
   non-attached page is skipped silently. Gates: `station-shell` **165**, workspace **481**,
   fmt/clippy clean. Live JS was never executed (headless assertions over the exact script); pages
   do render in this session now, so the gap is the observation, not the runtime.
4. **[DONE] Rune native shell slice 14 — one shell instance, live deep links.** `instance.rs`:
   named mutex `Local\station-shell-singleton` acquired **before** Rune
   (`instance::handoff_if_secondary()`); the primary owns a loopback TCP listener on `127.0.0.1`
   (ephemeral port) with `<config>/station/instance.json` `{port, token}` written atomically and
   removed on clean exit; a secondary sends `token\nurl\n` (or a focus-only ping) and exits
   without a window. One decision for both argv and IPC — `deep_link::act` is now shared, so
   there is no second copy of the URL rules. Caps in the Robustness table: 4 096 B request, 2 s
   I/O timeout, 32-hex per-run token (constant-length compare), **8** accepted per run, **10**
   per rolling **60 s** (links and pings together), file cap 4 096 B. Also:
   [`docs/rust-rune-port.md`](docs/rust-rune-port.md) and `rust/README.md` updated (transport
   choice, threat model: loopback bind + token gates local processes, not a secret from the
   user's own account). Gates: `station-shell` **156**, workspace **472 passing twice**, fmt
   clean, clippy exit 0, release 29,301,760 B. A two-process smoke is being run now; do not
   claim it until it reports.
5. **[DONE] Rune native shell slice 13 — a monitor DPI change reaches the pages.** `webview.rs`
   now reads the window's live DPI (`GetDpiForWindow` on the stored parent HWND,
   `Win32_UI_HiDpi` feature, dpi/96) and, from the frame poll, adopts a sane change (accepted
   **0.5–4.0×**, ±0.001 deadband; 0/NaN/10× refused and logged at most once per 10 s) by calling
   the host's `set_scale` and re-applying attached pages' bounds; `sync` consults the same live
   read so it cannot revert the adoption. Design fact: Rune's windowed app never updates
   `Runtime::env.scale` (only an internal `zoom.window_scale` follows winit's
   `ScaleFactorChanged`). Gates: `station-shell` **133**, workspace **449**, fmt clean, clippy
   `--exclude station-bridge -D warnings` exit 0, release ok. Unverified: a real monitor switch
   (WebView2 cannot start in this desktop session); the seam test forces the platform read
   instead.
6. **[DONE] Rune native shell slice 12 — a launched `station://` URL is honoured.** New
   `rust/crates/station-shell/src/deep_link.rs`: pure `open(state, args) -> Outcome` called once
   from `AppSpec::setup` with `std::env::args_os()`. `station://appstore` (plus trailing slash,
   plus an uppercase `STATION://` scheme exactly as the WHATWG parser treats schemes) selects the
   store through the same path as a rail press. Every other `station:` URL is **not served**: one
   stderr line naming it, state untouched (the configurator and the rest of the WebUI set are not
   served — no pretending). Oversized (>**2048** chars) and control-containing arguments refused;
   non-UTF-8 argv cannot panic (`args_os`); a deep link never opens a second window, the OS
   browser, or an install. Parsing reuses `url_router::parse_url`. Gates, on this tree:
   `station-shell` **130** (116 + 14), workspace **446 passing**, fmt clean, clippy
   `--exclude station-bridge -D warnings` exit 0 (one pre-existing vendored `parley_engine`
   warning), release build ok. Honest remaining gap: a **second** launch opens its own window —
   there is no single-instance lock anywhere in the project (watch list), and mid-run URL arrival
   is out of scope until that lock exists.
7. **[DONE] Rune native shell slice 11 — rail order is the user's** (independently verified).
   Ordering authority: `railOrder` in the session is the source of truth when it names the docked
   set exactly; `dock` stays the service's membership fact; install appends, uninstall removes;
   otherwise the rail falls back to install order and heals on the next write. The affordance is
   **Settings → My apps** (`settings-move-up-<id>` / `settings-move-down-<id>`, absent at the
   ends, never for the store row), not the rail: Rune gives a pressed rail row no keyboard focus,
   so a rail key path could not work (recorded as D11 in `docs/rust-rune-port.md`, after a failed
   attempt). `session-store` refuses unknown/duplicate/over-cap `railOrder` on **load and save**
   (whole document refused, offending id named), so a hostile document is left byte-identical;
   every move writes the session immediately through the same atomic path as every other user
   action. Gates, independently re-run: `station-shell` **116**, `session-store` **29**, workspace
   **432 passing** — six consecutive clean workspace runs (baseline + five), fmt clean, clippy
   `--exclude station-bridge -D warnings` exit 0, release 29,251,584 B. A single workspace failure
   one worker saw did **not** reproduce in six runs; the verifier audited the shared-state
   suspects (routing queue/budget are guarded, icon store is per-instance, bridge static is
   serialized) and fixed nothing — recorded honestly: no fix was made, none was needed.
8. **[DONE] Rune native shell slice 10 — per-app configuration owned by the native shell.**
   `session-store` v1 gains an additive `appConfig` (`AppConfigEntry`: identityId / subdomain /
   customURL — the TS `ApplicationConfigData` set, no invented fields), validated on **load and
   save**, values ≤ 2 048 chars, keys must name installed apps, refuse-never-truncate, atomic.
   `appstore-service`: `set_config_data` / `clear_config_data`. `station-shell`: `AppTab.configurable`
   from the manifest's presets, Settings Configure/Configured row + in-place editor (Domain /
   Server URL; Gmail's google-account preset says "Sign-in is not available in the native shell
   yet" — no fake field), config-aware `rebuild_tabs` so install **and** relaunch open the
   configured URL, `webview::load_if_attached` navigates the open page on save, Clear reverts and
   persists, hostile value refused with the file byte-identical. Gates: `session-store` 25,
   `appstore-service` 59, `station-shell` 106, workspace **418 passing** (two consecutive full
   runs), fmt/clippy clean, release 29,244,416 B. Honest note: WebView2 still cannot start in
   this desktop session, so the windowed page navigation on Save is proven through the recording
   host, not observed.
9. **[DONE] Rune native shell slice 9 — real app icons + window identity.** `manifest-registry`
   exposes `Manifest.icon` from the bundled definitions (638 manifests carry filenames CDN URLs;
   2 have none and keep the letter avatar). `station-shell/src/icons.rs`: `Fetcher` seam +
   `HttpFetcher` (ureq 2.12 `default-features=false, features=["tls"]` → rustls, no `-sys`),
   `image` 0.25 decode, disk cache `<config>/station/icons/<sha256(url)>.bin` (temp+fsync+rename,
   1 500-file LRU), one fetch worker, `Pixels` handles read synchronously by the views; caps
   http/https only · 1 MiB · 10 s · 8 192 px/side · 1 500 files · 128 in flight (surplus dropped,
   never deferred) · one ask per URL per run, failures never retry. `platform_icon.rs` applies
   `WM_SETICON` (32 + 64 px from `packages/app/build/icon_512x512.png`) from
   `webview::window_created`. Headless raster proof viewed (decoded icon fills the clipped
   avatar; broken URL keeps the tinted letter). Gates: `station-shell` 97, `manifest-registry`
   50, workspace 394, fmt/clippy clean, release 29,135,872 B. Windowed taskbar icon NOT observed
   (WebView2 down this session).
10. **[DONE] Rune native shell slice 8 — notifications get a real source.** A hosted page raises
   notifications the way the Electron app's pages do: `window.bxApi.notificationCenter.sendNotification(id, notification)`
   and `closeNotification(id)` over wry's `with_ipc_handler` plus an injected init script that
   exposes only those two members (nothing else of `bxApi` is invented). Messages are validated
   and bounded (128 held / 64 per app / 256 total; 32 KiB per message; title 512, body 4 096,
   icon 2 048, id 256; non-JSON/oversized/unknown-sender refused, never a panic), delivered into
   the ported `NotificationCenter` under the sender's app, shown by the unchanged panel; a row
   press marks it read (the `sagaNotificationClick` order) and focuses the sender's tab through
   the existing `url-router` decision — external URLs still via the http/https/mailto allowlist +
   rolling budget; never a second OS window (D9). Snooze expiry is driven from the frame poll at
   most once per second, so a finite snooze ends and the notification is visible again;
   `Snooze::SyncWithOs` stays stored with **no** OS reader (recorded as a remaining gap, not
   faked). Verification: `cargo test --workspace` **374 passing** (354 documented before, 292 in
   STATE.md before — both stale; 374 is what this tree runs), `cargo fmt --all --check` clean,
   `cargo clippy --workspace --all-targets --exclude station-bridge -- -D warnings` exit 0,
   release build ok. WebView2 **still cannot start in this desktop session**, so the windowed
   `sendNotification` call was NOT observed and is not claimed. Gaps left: OS notification
   source, OS do-not-disturb reader, page-side click-listener callbacks (`Notification.onclick`
   never fires). The 19 slice-8 tests live in `rust/crates/station-shell/src/tests_pages.rs`;
   `tests.rs` took only the 5-line lock addition (2 lock lines + 3-line comment) serializing the
   budget tests on the shared bridge mutex (the flake fix).
11. **[DONE] OAuth/config decision brief — `docs/oauth-config-options.md` (324 lines).** Four
    headline findings: (1) **`station://` cannot be Google's OAuth redirect** — custom schemes must
    contain a period and `station` has none, so the callback must be a **loopback HTTP listener**
    (the `station://` handler stays for launch links only); (2) Google **refuses OAuth in embedded
    webviews** (`disallowed_useragent`), so porting the React configurator cannot sign in by
    itself; (3) the config half already exists (slice 10), so the recommendation is **Option B —
    system-browser PKCE + loopback + the existing native editor (4 slices)**, with the configurator
    port (6 slices) as the fallback and only after B's OAuth core; (4) an identity is **metadata** —
    it renders `{{identityId}}`, labels and the avatar; it does **not** sign Gmail's page in, which
    stays Gmail's own WebView2 session. Unverified highlights: the existing OAuth client's type and
    registered redirect URIs, whether an ephemeral loopback port is accepted, the consent screen's
    verification state, and whether Google's sign-in page completes inside WebView2. This brief is
    the input for the open product decision.
12. **[DONE] The release exe carries an icon resource (packaging prerequisite).** New
    `rust/crates/station-shell/build.rs` + `station.rc`
    (`IDI_ICON1 ICON "../../../packages/app/build/icon.ico"`), with `embed-resource 3.0.12` as a
    Windows build-dependency. Release binary **29,073,408 → 29,207,552 B**; `ExtractIconExW` on the
    exe reports **icon_count=1, extracted=2**, both HICONs non-null (then destroyed); the debug
    build also reports 1. The runtime window-icon path (`platform_icon.rs`) is unchanged. Explorer
    and taskbar **rendering** of that resource is not visually verified — only its presence.
    Tests unchanged: workspace **492**.
13. **[NEXT — human gate]** Run the native shell on a real desktop: `cargo run -p station-shell`, install Gmail/Slack from the store, sign in, exercise Settings (appearance, uninstall, per-app Configure) and the notification panel. Then decide the next phase: **packaging/installer** vs. **Electron-tree phase 1+** — decision inputs: [`docs/electron-retirement.md`](docs/electron-retirement.md) (phase table; the three findings that need a decision first: **no single-instance lock, no crash reporting, no permission handler** exist anywhere today) and [`docs/rust-packaging-plan.md`](docs/rust-packaging-plan.md) (NSIS recommendation; decisions wanted: signing identity, per-user vs per-machine, update channel, unsigned beta). The launch-time `station://` gap that the packaging plan found is fixed (slice 12) — the deep link is honoured at launch; mid-run arrival still waits on a single-instance lock. Notification source (page channel), icons, per-app config and rail ordering are landed; identity is not.
14. **[DONE] Phase 2 wiring complete.** station-bridge cdylib (10 perform fns + 6 watcher fns), worker.ts swap behind STATION_RUST_BRIDGE=1, dead types removed. 145 tests green, clippy clean, node smoke 16/16 (baseline 13; earlier 15/15 figure stale). `UninstallApplications` now bridged end to end — `ApplicationService::uninstall_all_for_manifest` + `#[napi(js_name = "uninstallApplicationsByManifest")]` + `BRIDGED_CHANNELS` row + smoke checks — leaving `SetApplicationConfigData` and `RequestLogin` the remaining TS-only perform backends.
15. **[BLOCKED — human gate]** Live validation of the Electron bridge: run Electron with STATION_RUST_BRIDGE=1, exercise appstore (search/install/uninstall/custom apps), then flip default and continue service-by-service swap. Superseded in priority by the native shell above.

## Watch List

- Robustness pass (2026-11-18) landed in `station-shell`/`session-store`/`appstore-service`/`manifest-registry`: bounded popup queue, a **rolling 10-per-60 s** browser budget on top of the 5-per-frame cap, `http`/`https`/`mailto` allowlist with `ShellExecuteW`, 5-page LRU, attach retries with backoff + a pane Retry, session size/count/window validation (stored **320×240 … 8192×8192 pt** so a 3840-pt 4K window is kept; launch capped at **2048 pt** because the scale is unknown until the window exists; restore re-applies the **8192 device-px** ceiling with the real scale, so a hostile 16384/1e30 session snaps and a large display still comes back at its size), **immediate (atomic) writes on every user action** (the 250 ms coalescing window was removed so a hard kill cannot lose the last selection; re-selecting the active tab writes nothing), DPI/pane guards, idempotent shutdown, and fallible/atomic/capped private-manifest writes (8 MiB read cap, 10 000 records, saturating ids). Limits and tests in `docs/rust-rune-port.md` "Robustness"; `manifest-registry/tests/stress.rs` (another worker's, untracked) still does not compile.
- Verification environment caveat — **RESOLVED for this session (2026-11-18)**: `docs/webview2-probe.md` observed a full `msedgewebview2` process tree, a visible `WRY_WEBVIEW` child at the content rect and a visible Chromium render widget titled "Sign in - Google Accounts" — pages render here now. n=1, no pixel capture, and the earlier `0x80070057` failures' cause is unknown (the binary changed between runs), so windowed claims no longer need a "cannot verify" qualifier but earlier slices' windowed observations are not retro-claimed.
- Native shell gaps (see `rust/README.md`): **notification gaps are now only the OS toast source and the `SyncWithOs` reader** (the page-side source landed in slice 8 and page-side click delivery in slice 15), no configurator/OAuth flow (google-account presets say sign-in is unavailable), no installer/signing, and no crash reporting / permission handler anywhere (findings gating the phase decision). Monitor DPI: a change reaches the pages (slice 13) but a **real windowed monitor switch is still unobserved** — the switch itself was not made (pages now render in this session per `docs/webview2-probe.md`). Single-instance landed (slice 14): a second launch hands off and exits, and **mid-run deep links now arrive** while a shell runs — two-process smoke **reported**: `station://appstore` → secondary exit **0 in 266 ms**, exactly one `station-shell.exe` left; `station://settings` → exit 0 in 272 ms with exactly one primary stderr line `station-shell: deep link not served (only station://appstore is): station://settings`; focus-only launch → 289 ms, no spurious primary output; clean `WM_CLOSE` → primary exited on its own and `instance.json` was gone; the next launch became a fresh primary on a **new port**; zero `station-shell`/`msedgewebview2` processes left after cleanup. The smoke could not see whether Windows actually granted foreground focus (the call ran and a failure would have logged; none did). **Focus hand-off between Rune and WebView2 is still unmeasured.**
- Window icon: **windowed-verified** — the fix hands real handles to the per-window `WM_SETICON`, and the re-probe observed them (`WM_GETICON` `ICON_SMALL` and `ICON_BIG` non-null, color 32×32 and 64×64 at 32 bpp, class icons still null); **icon handles observed; pixel/taskbar/DWM rendering and Alt+Tab not**.
- Packaging prerequisite and OAuth decision: the exe now carries an icon resource (`station.rc`; `ExtractIconExW` reports 1 icon / 2 non-null HICONs in both configs — **Explorer/taskbar rendering unverified**), and the OAuth decision input is `docs/oauth-config-options.md` (Option B recommended; `station://` cannot be Google's redirect; embedded-webview OAuth refused).
- Recurring transient test failure — **investigated, cause not proven**: ten workspace runs (three with a second `station-shell` test binary running concurrently) were all **492 passed / 0 failed**, so the symptom did not reproduce, plus five more green runs after the changes. Two real cross-test windows were found and closed: `webview::route_requests` (which every test's frame poll runs) could drain another test's queued request and spend the process-global rolling browser budget, and `notifications::bridge::deliver_to` could do the same on the bridge queue — both now take **one shared reentrant process-state lock** (`test_support.rs`, `cfg(test)` only), the old separate `routing_lock`/`bridge_lock` statics are gone, and session/private-store temp paths now include the process id plus a per-call counter. No production limit touched, no assertion weakened. Honest verdict: **symptom not reproduced; two auditable causes closed; not proven to be *the* cause.**
- Surface hide/show is **windowed-verified** (`docs/surface-hide-probe.md`): a posted App Store press `(32,32)` hides the live `WRY_WEBVIEW`, the Gmail row `(32,80)` returns it (same HWND/rect), Settings `(32,720)` + Escape and the notification moon `(32,768)` + Escape/toggle do the same, one page HWND per run; what Rune painted is not observed (no screenshot) and DOM/history/scroll survival is **unobservable from handles**.
- Rune is pinned by rev with five vendored patched crates in `rust/vendor/`; a bump means re-vendoring (documented in `rust/README.md`).
- w02 merge agent errored post-merge cleanup; content verified in main (03b222e). Branch deleted.
- ctx7 wrapper broken in subagent env during w01 — agent compiled+required cdylib instead. Fine.
- Remaining TS-only backends: UninstallApplications, SetApplicationConfigData, RequestLogin, notifications passthrough (bridge doc).

## Recent Noise (ignored this run)

---
Run log: see `loop-run-log.md`
