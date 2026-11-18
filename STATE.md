# Loop State — My Project

Last run: 2026-11-18 rust-rune-port — slices 1-7 complete: Rune shell through settings, persistence and URL routing

## High Priority (loop is acting or waiting on human)

1. **[DONE] Rune native shell, slices 1-7.** `rust/crates/station-shell`: Rune window (rail, App Store, Settings, notification panel), WebView2 page per installed app, session persistence (`session-store`), and new-window routing through `url-router`. Release binary built; `rust/README.md` documents build/run/data. Verification: `cargo test --workspace` 292 passing and `cargo fmt --all --check` clean; clippy clean with `cargo clippy --workspace --all-targets --exclude station-bridge -- -D warnings` — the full workspace clippy still fails only in `station-bridge`'s lib-test target (14 pre-existing dead-code errors, crate untouched; see `rust/README.md`); headless snapshots viewed, windowed smokes for webview creation, settings/notifications and popup routing.
2. **[NEXT — human gate]** Run the native shell on a real desktop: `cargo run -p station-shell`, install Gmail/Slack from the store, sign in, exercise Settings (appearance, uninstall) and the notification panel. Review the gaps listed in `rust/README.md` and D8/D9 of `docs/rust-rune-port.md` — then decide the next phase (packaging/installer, notification source, icons/identity, or Electron-tree retirement).
3. **[DONE] Phase 2 wiring complete.** station-bridge cdylib (9 perform fns + 6 watcher fns), worker.ts swap behind STATION_RUST_BRIDGE=1, dead types removed. 145 tests green, clippy clean, node smoke 15/15.
4. **[BLOCKED — human gate]** Live validation of the Electron bridge: run Electron with STATION_RUST_BRIDGE=1, exercise appstore (search/install/uninstall/custom apps), then flip default and continue service-by-service swap. Superseded in priority by the native shell above.

## Watch List

- Robustness pass (2026-11-18) landed in `station-shell`/`session-store`/`appstore-service`/`manifest-registry`: bounded popup queue, a **rolling 10-per-60 s** browser budget on top of the 5-per-frame cap, `http`/`https`/`mailto` allowlist with `ShellExecuteW`, 5-page LRU, attach retries with backoff + a pane Retry, session size/count/window validation (stored **320×240 … 8192×8192 pt** so a 3840-pt 4K window is kept; launch capped at **2048 pt** because the scale is unknown until the window exists; restore re-applies the **8192 device-px** ceiling with the real scale, so a hostile 16384/1e30 session snaps and a large display still comes back at its size), **immediate (atomic) writes on every user action** (the 250 ms coalescing window was removed so a hard kill cannot lose the last selection; re-selecting the active tab writes nothing), DPI/pane guards, idempotent shutdown, and fallible/atomic/capped private-manifest writes (8 MiB read cap, 10 000 records, saturating ids). Limits and tests in `docs/rust-rune-port.md` "Robustness"; `manifest-registry/tests/stress.rs` (another worker's, untracked) still does not compile.
- Verification environment caveat (2026-11-18 independent verification): WebView2's browser process failed to start in this desktop session (`0x80070057` from the attach path, no `msedgewebview2` process; 10/10 reproductions incl. fresh profile and explicit browser folder), so **page rendering is not currently verified** — the child HWND was created at the exact content rect and the shell took its documented degraded path. Earlier smoke runs the same workday did render pages (navigation started / page load finished), so retest in another desktop session before claiming rendering works (see `rust/README.md` "Verified on").
- Native shell gaps (see `rust/README.md`): no notification source, snooze never expires, monitor-DPI change leaves pages at their old scale, letter avatars instead of manifest icons, no installer/signing.
- Rune is pinned by rev with five vendored patched crates in `rust/vendor/`; a bump means re-vendoring (documented in `rust/README.md`).
- w02 merge agent errored post-merge cleanup; content verified in main (03b222e). Branch deleted.
- ctx7 wrapper broken in subagent env during w01 — agent compiled+required cdylib instead. Fine.
- Remaining TS-only backends: UninstallApplications, SetApplicationConfigData, RequestLogin, notifications passthrough (bridge doc).

## Recent Noise (ignored this run)

---
Run log: see `loop-run-log.md`
