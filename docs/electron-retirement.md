# Electron retirement inventory

Status: proposal for human approval (2026-11-18). Read-only evidence survey; nothing here has been
executed. Companion to `docs/rust-rewrite-plan.md` (slice plan), `docs/rust-rune-port.md` (ADR) and
`docs/rust-packaging-plan.md` (packaging/installer decision input);
parity claims below are grounded in Rust **source** (`rust/crates/*/src/lib.rs` and
`rust/crates/station-shell/src/*`), not in plan documents.

## Legend

- **Superseded by** — the Rust crate/module that replaces this module's job. `— none` = no Rust
  equivalent exists today.
- **Parity** — `complete` (Rust side covers the module's behavior), `partial` (missing behavior
  named), `not ported`.
- **DND** = DO-NOT-DELETE-YET; deleting it changes shipped behavior. Reason stated inline.

Evidence tools: `rg` over `packages/` (never grep), Rust surfaces read from `lib.rs`/module files.
Every cited path was confirmed to exist with at least one `rg`/`ls` during this survey.

## What Rust covers today (ground truth)

| Rust unit | Surface verified at |
| --- | --- |
| `station-shell` | `src/{main,shell,state,rail,placeholder,appstore,settings,notifications,webview,routing,session}.rs` — Rune rail/top bar/store/settings/notification panel, WebView2 child pages (5-page LRU), new-window routing, session persistence. Slice 8 added `src/notifications/bridge.rs`: a notification-only page bridge (`window.bxApi.notificationCenter.{sendNotification,closeNotification}` injected via wry `with_ipc_handler`, bounded/deduped like the routing queue) feeding the ported `NotificationCenter`; click routes through the existing url-router decision; finite snoozes expire on the frame poll. Slice 9 added `src/icons.rs` (fetches the manifest's bundled icon once over ureq/rustls, disk-caches at `<config dir>/station/icons/<sha256(url)>.bin` via temp-file + rename; every failure path keeps the letter avatar) and `src/platform_icon.rs` (`WM_SETICON`, 32×32/64×64) so the window title bar and taskbar carry Station's own icon. |
| `appstore-service` | `search_applications`, `get_most_popular_apps`, `get_all_categories`, `get_applications_by_category`, `write_commands::ApplicationService` (install/uninstall/restore/`resolve_start_url`), `uninstall_all_for_manifest` (`write_commands.rs:509`, removes every instance of a manifest and keeps others), `set_config_data`/`clear_config_data` (slice 10: templates the preset-consuming start URL; `write_commands.rs:483,518`), `app_request` state machine |
| `manifest-registry` | `BxAppManifest`, `config_rules`, `MultiInstanceConfig`, `Preset`, chrome-extension id, bundled definitions, `PrivateStore` (private manifests on disk, 8 MiB/10k caps). Slice 9: `Manifest.icon` carries the bundled icon's remote URL (`icons[0].src`) since there is no webpack url-loader on the Rust side |
| `appstore-schema` | `SCHEMA_GRAPHQL` (`include_str!` of `schema/api.graphqls`), serde query structs, `ui_state` (replaces Apollo cache) |
| `activity` | `ActivityStore` (sqlx SQLite), `frecency`, `merge`, `QueryArgs` |
| `session-store` | `SessionState` v1 (window/theme/installed/dock/tabs/railOrder), atomic writes, validation caps. Slice 10: `app_config` map (`AppConfigEntry`: identityId / subdomain / customURL) — values ≤ `MAX_CONFIG_VALUE_CHARS` (2048), keys must name installed apps, validated on load and save, written atomically with the rest of the session |
| `notification-center` | `NotificationCenter` (groups, mark-read, `Snooze`, `ClickTarget`), `parse_duration_ms` |
| `url-router` | `parse_url`/`normalize_url`/scope matching, `route()` → `Route` |
| `webui` | `resolve_webui_request`, `default_handler_hostnames()` = `["multi-instance-configurator", "appstore"]`, subpath containment |
| `about-window` | `AboutWindowState`, `runtime_versions`, `should_close`, `copyright_year` |
| `tab-model` | `Tab`, `Badge`, `TabAction`, `TabModel` |
| `station-sdk` | `Sdk`/`SdkOptions` facade + `Provider` traits: `Tabs/Activity/Storage/Session/Config/Resources/Ipc`, `HistoryConsumer`, `SearchConsumer`, `react` trait surface |
| `station-bridge` (napi) | 10 perform exports (`search_applications`, `get_most_popular_applications`, `get_all_categories`, `get_applications_by_category`, `get_manifest_by_url`, `get_private_applications`, `install_application`, `uninstall_application`, `uninstall_applications_by_manifest` (`js_name = "uninstallApplicationsByManifest"`, `lib.rs:238`), `request_private_application`) + 3 watcher fns (`watch_theme_colors`, `watch_identities`, `watch_snooze_duration` via `watchers_napi.rs` macro). **No** `set_config_data`, **no** `request_login`, **no** notification emit (matches "Remaining TS-only backends" in `STATE.md`). |

Bridge consumer on the TS side: `packages/app/src/services/services/sdkv2/rust-bridge.ts`
(`BRIDGED_CHANNELS` = the same 10 channels), active only under `STATION_RUST_BRIDGE=1`;
`node scripts/rust-bridge-smoke.js` is the 16-check smoke (pre-slice-8 baseline was 13 checks; the
count grew with the notification-bridge and uninstall-by-manifest checks).

---

## Inventory — `packages/app/src`

| Module | Superseded by | Parity | Remaining TS dependents (verified) | Notes |
| --- | --- | --- | --- | --- |
| `plugins/bxapi.d.ts` (dead types: `BxAPI.user`, `BxAPI.services`, `Runtime`, `AuthorizationError`, `NoMethodError`, `SystemError`) | — none | not ported (intentionally — dead) | zero call sites (`docs/rust-napi-bridge.md` §Not ported) | Deletable now; type-only. |
| `about-window/` (Container/Presenter/about.html) | `about-window` crate | partial | `webpack.config.renderer.js` entry only (about.html template + `aboutRenderer` chunk) | Crate owns lifecycle state (`CloseReason`, `runtime_versions`); the Electron window + HTML view remain. Native shell has no about window. |
| `abstract-application/` | `manifest-registry` (`BxAppManifest`, `config_rules`, `MultiInstanceConfig`, `Preset`, `get_chrome_extension_id`) + bundled `definitions/` JSON | partial | imported across `applications/`, `bang/`, `dock/` (broad; `rg -l "abstract-application"`) | `duck#uninstallAllInstances` backend now exists in Rust (`uninstall_all_for_manifest`, bridged as `uninstallApplicationsByManifest`); the duck itself, `queries@local.gql`/resolvers stay TS. |
| `activity/` (model.ts sequelize, sagas, resolvers) | `activity` crate (`ActivityStore`, `frecency`, `merge`) | partial | `database/model.ts`, `rg -l "src/activity"` | Crate is sqlx SQLite; TS sequelize model + graphql resolvers/sagas not ported. |
| `api/` (logger.ts; websocket.ts already deleted in s05) | — none | not ported | `applications/components/ApplicationContainer.tsx`, `applications/sagas/{configData,index,lifecycle}.ts`, `bang/{components/BangList.tsx,duck.ts}` | Trivial logger; keep until its importers die. |
| `app/` (duck/sagas/selectors/resolvers) | `station-shell` (`AppSpec` lifecycle, `state.rs`, `session.rs`) | partial | main entry, store | App lifecycle parity exists natively (launch/restore/quit); redux-facing sagas remain while renderer lives. |
| Entry/bootstrap: `main.ts`, `app-main.js`, `app-worker.ts`, `index.js`, `index-sub.js`, `app.html`, `app-sub.html`, `worker.html`, `test.html`, `cli.ts`, `dotenv.ts`, `index.d.ts`, `types.ts` | `station-shell` binary (separate app) | partial | everything (they are the entry) | **DND** — Electron is the shipped app; `station-shell` is not packaged/shipped yet (no installer/signing, `rust/README.md` gaps). |
| `app-shortcuts.js`, `keyboard-shortcuts.ts` | — none | not ported | main/renderer entry | Shell handles only Escape (`AppSpec::on_key`); no global-shortcut layer in Rust. |
| `app-store/` (AppStore.tsx pane container, duck, sagas, selectors) | `station-shell::appstore.rs` + `state.rs` store tab | complete | `dock/Dock.js`, `in-tab-search/sagas.js` | Native shell renders the store as a Rune tab (`TabKind::Store` / `station://appstore`); pane visibility/escape handled natively. Slice 9: store tiles draw the manifest's bundled icon (disk-cached fetch, letter-avatar fallback). Electron pane stays until renderer retires. |
| `application-settings/` | `station-shell::settings.rs` Configure editor (slice 10: per-app config, presets-aware) | partial | `urlrouter/URLRouter.ts` (`isNotInstallableApplication`) | Configuration surface now exists natively; the module's installability selectors are not ported and still feed urlrouter, which keeps it alive. |
| `applications/` (duck, sagas, api, components, manifest-provider) | `appstore-service::write_commands` (install/uninstall/restore/`resolve_start_url`) + `manifest-registry` | partial | `app-store/`, `urlrouter/`, `dock/`, sagas root | Missing backend: `RequestLogin` — not exposed by `station-bridge`. `UninstallApplications` is bridged (`uninstall_all_for_manifest` → `uninstallApplicationsByManifest`). `SetApplicationConfigData` is **not a bridging gap**: slice 10 made the native shell own config data (`session-store` `appConfig` + `set_config_data`/`clear_config_data`); the TS channel stays TS-only until the Electron path drops the bridge surface or the renderer retires. **DND** (configurator below; also sole install path while bridge default is off). |
| `applications/multi-instance-configuration/` (webui handler + MultiInstanceConfigurator.tsx) | `webui` crate resolves `station://multi-instance-configurator` (file resolution only); UI — none | not ported (UI) | `webui/handlers.ts`, `webui/webUIHandler.ts` | **DND** — still served to pages; preset installs without config can land there (`docs/rust-rune-port.md` D6/D8). Slice 10: the native shell's Settings now has a Configure row + editor for the same job, but the `station://` page itself is not retired while the Electron renderer serves it. |
| `auto-update/` (sagas) | — none | not ported | `app/sagas.js` (`quitAndInstall`) | **DND** — electron-updater is the only update path. |
| `bang/` | — none | not ported | dock/nav UI | Search/UI feature; nothing in `station-shell/src` covers it. |
| `chrome-extensions/` | — none | not ported | `services/services/ecx` | **DND** — no Rust equivalent. |
| `common/`, `lib/`, `utils/` | — none | not ported (shared kit) | broad | Delete with their consumers, not before. |
| `components/`, `containers/`, `ui/`, `nav/` | `station-shell` views (own Rune views; no React port) | not ported | renderer tree | Native shell does not reuse these. |
| `context-menu.ts`, `context-menus/` | — none | not ported | webview/tab UI | **DND** — no native context menus. |
| `database/` (sequelize) | — none (only `activity` crate has its own sqlx store) | not ported | `persistence/{local.backend,umzug,migration}.ts`, `plugins/persistence.ts`, `password-managers/providers/onePassword/persistence.ts`, `activity/model.ts` | App DB is still TS-only. |
| `dialogs/` | `station-shell` pane `page_error` + Retry (webview-load errors only) | partial | renderer | **DND** — general error dialogs have no native equivalent. |
| `dl-toaster/` | — none | not ported | downloads UI | **DND** with `downloads/`. |
| `dock/`, `dock-navigation/`, `subdock/` | `station-shell::rail.rs` + `tab-model` (`Badge`) | partial | `app-store/AppStore.tsx`, renderer | Rail + badges ported; slice 9 draws each rail row's bundled icon (letter-avatar fallback). Subdock content, dock navigation/history UI not. |
| `downloads/` | — none | not ported | `services/services/download` | **DND** — no Rust download manager. |
| `favorites/`, `ordered-favorites/` | `session-store` (`dock`, `railOrder`) | partial | dock UI | Order persisted in the session; favorites UI not ported. |
| `graphql/` (apollo link stack, schema.graphql) | `appstore-schema` (SDL + query structs) | partial | renderer/worker store | Only the appstore projection is modeled in Rust; the apollo/reactive-schema client is not. |
| `history/` | `station-sdk::history` (`HistoryConsumer` watch stream) | partial | sdk consumers | Stream surface ported; TS graphql history api not. |
| `in-tab-search/` | — none | not ported | webview UI | **DND**. |
| `notification-center/` | `notification-center` crate + `station-shell::notifications.rs` (panel) + `src/notifications/bridge.rs` (page source) | partial | dock/subdock UI | Groups/mark-read/snooze + panel ported; slice 8 adds the page→shell source: hosted pages get `window.bxApi.notificationCenter.{sendNotification,closeNotification}` via wry IPC, clicks route through `url-router`, finite snoozes expire on frame poll. Still missing: OS notification source, `Snooze::SyncWithOs` reader, page-side `addNotificationClickListener` callbacks, notification webview pages (`notification-center/webview-preload.ts` path), FocusMode dock UI. Electron path **not superseded**. |
| `notifications/` (AskAuthorize UI, duck) | — none | not ported | renderer | **DND** with OS notification path. |
| `observers/` | — none | not ported | store wiring | Redux observer trunk. |
| `onboarding/` | — none | not ported | `app-store/AppStore.tsx` (tooltip) | |
| `ordered-tabs/` | `tab-model` | partial | renderer | Tab ordering state in `TabModel`; redux duck remains. |
| `organizations/` | — none | not ported | renderer | |
| `os-bar/` | — none | not ported | renderer | |
| `password-managers/` | — none | not ported | own persistence via `database/` | **DND**. |
| `persistence/` (umzug, local backend, migrations) | `session-store` (session.json replaces the persisted shell state only) | partial | `plugins/persistence.ts`, `database/` | Sequelize/umzug app-DB migration chain has no Rust equivalent. |
| `plugins/` | mixed: `SDKProvider.ts` → `station-sdk`; `webview-preload.js` (bxApi) → `station-bridge`; notification slice → `station-shell::notifications::bridge.rs` | partial | renderer, webviews | **DND** for `webview-preload.js`, `injected-js/`, `slack/`, `static/preload/*` (via `static/`): every `<webview>` still loads them. Native shell now ships a *notification-only* preload (`bridge.rs` `INIT_SCRIPT` exposes exactly `bxApi.notificationCenter.{sendNotification,closeNotification}`); the full bxApi preload remains Electron-only. |
| `pubsub/` | — none | not ported | store wiring | |
| `reducers/`, `store/`, `sagas.ts`, `state.ts` | `appstore-schema::ui_state` (install/UI-state slice) + `appstore-service::app_request` | partial | entire renderer/worker | Trunk; dies last before main. |
| `resources/` | `station-sdk::resources` (trait) | partial | sdk consumers | Trait surface only. |
| `sdk/` | `station-sdk` | partial | `plugins/SDKProvider.ts`, manifests/runtime | Facade + traits exist; TS transport stays until manifests/runtime migrate. **DND** (published SDK). |
| `services/` | see sub-table below | partial | main + worker | Trunk. |
| `session.ts` | — none | not ported | `app-main.js` (`enhanceSession`) | **DND** — webRequest listener ordering (`ecx-cors`, `bx-dynamic-user-agent`, `ecx-api-handler`) via `electron-better-web-request`. |
| `settings/` | `station-shell::settings.rs` | partial | renderer | Native: appearance + my-apps (open/uninstall/configure) + version/session path. Slice 10 adds the Configure row + editor over `session-store` `appConfig`; install and relaunch open the configured URL (`state.rs` restore goes through `resolve_start_url` with the stored config). TS settings breadth **UNVERIFIED** (did not enumerate TS cards). |
| `static/` | `webui` crate (serves files under `station://`); assets themselves stay | partial | webpack renderer/webui builds | `static/preload/*` scripts are **DND** (webview preloads). |
| `tab-webcontents/`, `tabs/` | `tab-model` + `station-shell::webview.rs` (attach/LRU/bounds/retry) | partial | dock, urlrouter | Page lifecycle ported; per-webcontents features (zoom, find, context menus) stay Electron. |
| `theme/` | `station-bridge::watch_theme_colors` | partial | renderer | Colors stream ported; scss/fonts/theming UI stay. |
| `urlrouter/` | `url-router` (`route`, `parse_url`, scope matching) + `station-shell::routing.rs` | partial | `services/services/url-router-helper`, dock | Decision core ported (Focus/FocusAndLoad/Load/External/Ignore shape per ADR D9); radix scope tree, `ManifestProvider` wiring, `urlDispatcherProvider` sagas stay. |
| `user-activities/` | `activity` crate | partial | store | Push/query ported; sagas stay. |
| `user-identities/` | `station-bridge::watch_identities` (read-only stream) | partial | bang/renderer | `RequestLogin` (`sagas#callRequestSignIn`) has **no** Rust backend. **DND**. |
| `webui/` | `webui` crate (`resolve_webui_request`, hostname guard, subpath containment) | partial | `app-main.js` (`start()`) | `protocol.handle(BX_PROTOCOL)` registration + `handlers.ts` + `WebUIGradientProvider.tsx` stay Electron. **DND** (serves `station://appstore` + configurator). |
| `windows/` | `station-shell` window lifecycle + `session-store::WindowState` placement | partial | main | Main-window placement parity exists; `GenericWindowManager` (CLI/about/popup windows) has no native equivalent. |
| `subwindows/` | — none (native shell routes new windows to the OS browser or existing tab, ADR D9) | not ported | renderer | **DND** until native popups exist. |

### `packages/app/src/services/services` (main-process services, via `servicesManager.ts`)

| Service | Superseded by | Parity | Notes |
| --- | --- | --- | --- |
| `sdkv2` (worker, `rust-bridge.ts`) | `station-bridge` | partial | 10/12 perform channels + 3 watchers bridged. `SetApplicationConfigData` stays TS-only by ownership: slice 10 made the native shell the owner of config state, so no bridge channel is wanted — it leaves when the Electron renderer path does. `RequestLogin` and the notifications passthrough remain unbridged gaps (`docs/rust-napi-bridge.md`). **DND** until default flip + gaps closed. |
| `sdkipc` | `station-sdk::ipc` (trait) | partial | Trait surface only; transport stays. |
| `manifest` | `manifest-registry` (`get_application_by_id`, `get_manifest_by_url`, search) | complete (data access) | Renderer `ManifestProvider` caching stays for the Electron tree. |
| `auto-updater` | — none | not ported | **DND** — `electron-updater` (`lib.ts` imports `electron-updater`; mock for unpackaged). |
| `os-notification` | — none | not ported | **DND** — no Rust notification source. |
| `electron-app` (tray, menu lifecycle) | `station-shell` (partial) | partial | **DND** — `Tray` at `main.ts:41,136-212` (`showTrayIcon`/`createTray`); no native tray. |
| `browser-window` | `station-shell` + `session-store::WindowState` | partial | Popup/CLI/about window management stays. |
| `url-router-helper` | `url-router` | partial | Electron dispatch glue stays. |
| `tab-webcontents` | `tab-model` + `station-shell::webview.rs` | partial | |
| `electron-google-oauth` | — none | not ported | **DND** — OAuth window flow (`@getstation/electron-google-oauth2` dep). |
| `authentication` | — none | not ported | Login sagas. |
| `download` | — none | not ported | **DND**. |
| `ecx` | — none | not ported | **DND** — `electron-chrome-extension` integration (`main.ts` imports `ECx`; autoUpdate fetcher). |
| `menu` | — none | not ported | **DND** (app/context menus). |
| `autolaunch` | — none | not ported | **DND** (login item). |
| `session` | — none | not ported | **DND** (webRequest enhancement). |
| `apollo-link` | — none | not ported | GraphQL client plumbing. |
| `process-manager`, `extendedAppMetrics` | — none | not ported | Analytics; exact behavior **UNVERIFIED** (listing only). |
| `cursor` | — none | not ported | Behavior **UNVERIFIED** (listing only). |
| `skeleton` | — none | not ported | Loading UI. |

---

## Inventory — `packages/appstore`

Consumed by the Electron app **not** via TS imports (`rg "@getstation/appstore"` in sources = 0
hits) but via build wiring: `packages/app/webpack.config.base.js:18` alias `@src → ../appstore/src/`
and `webpack.config.renderer.js:89-91` copying `../appstore/dist/appstore` into the renderer,
served at `station://appstore` by `packages/app/src/webui/handler-appstore.ts`.

| Module | Superseded by | Parity | Notes |
| --- | --- | --- | --- |
| `src/api.ts`, `src/app-request/` | `station-bridge` napi exports + `appstore-service::app_request` | partial | Same 10-channel coverage (incl. `uninstallApplicationsByManifest`); unbridged channels (setConfigData, requestLogin) keep TS sagas alive. |
| `src/graphql/` | `appstore-schema` (`SCHEMA_GRAPHQL` verbatim `include_str!`, serde query structs) | complete | For the queried projection (crate doc: transport + unused types deliberately not ported). |
| `src/reducers/`, `src/state.ts` | `appstore-schema::ui_state` | complete | Crate doc: "replacing the Apollo `InMemoryCache` + client-schema local-state layer". |
| `src/sagas/` | `appstore-service::write_commands` | partial | Only bridged channels have a Rust path; sagas remain the default dispatch. |
| `src/app.tsx`, `src/index.tsx`, `src/components/` (AppStoreAside/Content/Header/Pane/Preloader), `src/HOC/` (withCustomApplications), `src/context/` | `station-shell::appstore.rs` (Rune store: sidebar, search, discover/categories/private/search sections, install/uninstall tiles) | partial | Rune store covers the core flows (ADR D6/D8); per-component diff (my-custom-apps flow, preloader, animations) **UNVERIFIED**. |
| `src/applications/mockedMostPopularApps.ts` | `appstore-service::get_most_popular_apps` | not ported (mock) | Delete after `STATION_RUST_BRIDGE` default flip; plan gate `rg "mocked"` exit 1 (`docs/rust-rewrite-plan.md` s14). |
| `src/shared/` (constants/functions/validators/animations), `src/theme.ts`, `src/typings/`, `src/types.d.ts`, `src/utils.ts` | `theme.ts` → `station-shell` `Paint`/`tok` theme (partial); rest — none | not ported | UI support. |
| `src/static/` | — none | not ported | Build assets. |

---

## Inventory — `packages/sdk` (`@getstation/sdk`)

Dependents (verified `rg -l "@getstation/sdk"`): `packages/app/manifests/runtime/**` (activity,
gdrive, notion, …), consumed in-app through `packages/app/src/plugins/SDKProvider.ts`. **DND** —
published SDK; retiring it strands the manifest-runtime apps.

| Module | Superseded by | Parity |
| --- | --- | --- |
| `src/common.ts` | `station-sdk::common` (`ConsumerId`) | complete |
| `src/{activity,config,history,ipc,resources,search,session,storage,tabs}/` | matching `station-sdk` provider traits/consumers (`TabsProvider`, `ActivityProvider`, `StorageProvider`, `SessionProvider`, `ConfigProvider`, `ResourcesProvider`, `IpcProvider`, `HistoryConsumer`, `SearchConsumer`) | partial — trait/consumer surfaces only; no transport, no concrete impl (s02–s04 scope) |
| `src/react/` | `station-sdk::react` (consumer trait surface) | partial |
| `src/{api.ts,context.tsx,default-client.ts,errors.ts,index}/` | — none | not ported — TS client/transport stays |

---

## Must survive the shell (Electron-only today, no Rust equivalent)

| Concern | Lives today at | Evidence / note |
| --- | --- | --- |
| Code signing / notarization / installer | `packages/app/electron-builder.yml`, `packages/app/build/` (`entitlements.mac.plist`, icons, `installer.nsh`) | `rust/README.md` gap list: "no installer/signing". |
| Auto-update | `packages/app/src/services/services/auto-updater/*`, `packages/app/src/auto-update/sagas.ts`, `electron-updater` dep | Only update path; channels per `docs/rust-rewrite-plan.md` resolution 2. |
| Crash reporting | — none found (`rg crashReporter packages/app` = 0 hits) | Nothing to preserve; native shell must add its own. Finding, not a DND. |
| Single-instance lock | — none found (`rg "requestSingleInstanceLock\|second-instance" packages/app/src` = 0) | Electron app has no lock today; native shell must decide policy. Finding. |
| Deep links (`station://` OS registration) | `packages/app/electron-builder.yml:18-21` (`protocols: [station]`) | Registration comes from packaging config, not app code (`rg "setAsDefaultProtocolClient"` = 0). In-app serving: `webui/webUIHandler.ts` (`protocol.handle`). **DND**. |
| Tray | `packages/app/src/services/services/electron-app/main.ts` (lines 41, 136-212) | **DND**. |
| OS notifications | `packages/app/src/services/services/os-notification/`, `notifications/`, `notification-center/webview-preload.ts` (IPC `new-notification`/`notification-close`/`trigger-notification-click` per `docs/rust-napi-bridge.md`) | **DND** — native shell now has a *page* source (`station-shell/src/notifications/bridge.rs`) but still **no OS notification source** and no `Snooze::SyncWithOs` reader; the Electron OS path is not superseded. |
| Protocol handlers (`station://` content) | `packages/app/src/webui/webUIHandler.ts`, `handlers.ts`, `handler-appstore.ts` | **DND** while any Electron page loads `station://` content. |
| Password managers | `packages/app/src/password-managers/` | **DND**. |
| Chrome extensions | `packages/app/src/chrome-extensions/`, `packages/app/src/services/services/ecx/` | **DND**. |
| Downloads | `packages/app/src/downloads/`, `dl-toaster/`, `services/services/download/` | **DND**. |
| In-tab search | `packages/app/src/in-tab-search/` | **DND**. |
| Error dialogs | `packages/app/src/dialogs/` (main.ts imports `dialog`) | **DND** beyond the shell's pane-level page error. |
| Permissions prompts | — none found (`rg "setPermissionRequestHandler\|setPermission"` = 0) | Finding: no handler exists today; native shell must add one. |
| OAuth windows | `services/services/electron-google-oauth/`, `@getstation/electron-google-oauth2` dep, `user-identities/sagas` | **DND**. |
| Session webRequest shaping | `packages/app/src/session.ts`, `utils/sagasWebRequestHelper.ts` | **DND**. |
| Login item / autolaunch | `services/services/autolaunch/` | **DND**. |
| `<webview>` preloads / injected scripts | `packages/app/src/plugins/webview-preload.js`, `plugins/injected-js/`, `plugins/slack/`, `static/preload/` | **DND**. |
| Published SDK + manifest runtime | `packages/sdk`, `packages/app/manifests/runtime/**` | **DND**. |

---

## Deletion order (single ordered list; leaves first, trunk last)

Phase 0 — nothing to delete today: every phase below is gated on the native parity named in it.

**Phase 1 — dead surface (safe now)**
1. `packages/app/src/plugins/bxapi.d.ts` dead types (`BxAPI.user`, `BxAPI.services`, `Runtime`, error classes).
   Verify: `rg "bxApi\.user|bxApi\.services" packages/app/src` exit 1; `yarn workspace station-desktop-app build`.

**Phase 2 — appstore data layer behind the bridge** (gate: `STATION_RUST_BRIDGE=1` live validation
passes, then default flip — STATE.md item 4)
2. `packages/appstore/src/applications/mockedMostPopularApps.ts` + remaining `mocked` fixtures.
3. `packages/appstore/src/api.ts` + `app-request/` arms for the 10 bridged channels (keep the 2 unbridged: setConfigData, requestLogin).
4. `packages/app/src/services/services/sdkv2/rust-bridge.ts` + matching `worker.ts` dispatch arms.
   Verify: `node scripts/rust-bridge-smoke.js`; `yarn workspace @getstation/appstore build`; `cargo test -p station-bridge`; `rg "mocked" packages/appstore/src` exit 1.

**Phase 3 — application/identity sagas** (gate: `request_login` in `station-bridge`;
`uninstall_applications_by_manifest` landed after the survey and is bridged + smoke-covered).
Ownership note, slice 10: `setConfigData` is **no longer about bridging** — the native shell now
owns the state (`session-store` v1 `appConfig`, validated on load and save; `appstore-service`
`set_config_data`/`clear_config_data`; Settings Configure editor; install/relaunch open the
configured URL). The bridge channel stays TS-only because the Electron renderer still owns its own
copy for its own UI; it dies with the renderer or when the Electron path drops the bridge surface —
there is no Rust backend to wait for, only one owner per side, which is what today's split already
is.
5. `packages/app/src/applications/sagas/` bridged arms.
6. `packages/app/src/user-identities/` (after `request_login` lands; OAuth window stays).
7. `packages/app/src/user-activities/`, `packages/app/src/activity/` (after native activity consumption).
8. `packages/app/src/urlrouter/` + `services/services/url-router-helper/` (needs `application-settings/` decision first — it feeds `isNotInstallableApplication`).
9. `packages/app/src/tabs/`, `ordered-tabs/`, `tab-webcontents/` (+ service) after tab-model/webview parity review.
   Verify: `cargo test --workspace`; `yarn workspace station-desktop-app build`; manual install/uninstall-by-manifest/login/configurator pass.

**Phase 4 — appstore UI package**
10. `packages/appstore/` (remainder), `packages/app/src/app-store/`, `packages/app/src/webui/handler-appstore.ts`, webpack alias `@src` (`webpack.config.base.js:18`) and dist copy (`webpack.config.renderer.js:89-91`).
    Verify: `rg "@src|appstore" packages/app/webpack.config.*.js` shows no alias/copy; `yarn run build-webpack:prod`; `cargo run -p station-shell -- --snap store.png --scene store` (native store still renders).

**Phase 5 — renderer UI leaves** (each only when its native equivalent exists; see DND table)
11. `onboarding/`, `organizations/`, `os-bar/`, `dl-toaster/`, `favorites/`, `ordered-favorites/`, `dock-navigation/`, `subdock/`, `ordered-tabs/` remainder, `theme/` scss/fonts, `settings/`, `bang/`, `notifications/`, `application-settings/`, `keyboard-shortcuts.ts`, `app-shortcuts.js`, `nav/`, `ui/`, `components/`, `containers/`.
    Verify: `yarn run build-webpack:prod` green; manual renderer smoke.

**Phase 6 — shell/main trunk**
12. Displaced services one-by-one (`apollo-link`, `skeleton`, `process-manager`, `extendedAppMetrics`, `cursor`, `browser-window`, `electron-app`, `url-router-helper`, `tab-webcontents`, `sdkipc`, `manifest`, `session`, `menu`, `ecx`, `download`, `autolaunch`, `authentication`, `electron-google-oauth`, `os-notification`, `auto-updater`, `sdkv2`) — each deleted only when its row above flips to superseded.
13. `servicesManager.ts` + `services/` shell; `store/`, `reducers/`, `sagas.ts`, `state.ts`, `observers/`, `pubsub/`, `graphql/`, `database/`, `persistence/`, `plugins/SDKProvider.ts`, `session.ts`, `webui/` TS glue, `windows/`, `notification-center/`, `about-window/`.
14. Entry files (`main.ts`, `app-main.js`, `app-worker.ts`, `index*.js`, `*.html`, `cli.ts`, `dotenv.ts`) when the packaged `station-shell` binary becomes the shipped app.
    Verify: `cargo test --workspace`; packaged-app manual matrix (install, login, update, tray, notifications); `yarn workspace station-desktop-app build` until 14 lands.

**Phase 7 — build config**
15. `webpack.config.*.js`, `electron-builder.yml` (replaced by the native installer), `packages/app/build/`, `dev_utils/`, `scripts/` (incl. `scripts/sdk.ts`, `rust-bridge-smoke.js` once the bridge is unconditional), `.storybook/`, `@types/`, `tsconfig*.json`, `packages/appstore` + `packages/sdk` package entries, root `package.json` scripts.
    Verify: `yarn install && yarn run build` green; CI cargo + node jobs green; packaged smoke.

Items marked **DND** in the tables stay through Phase 6 and leave only when their native
equivalent ships; they are the reason Phase 6 cannot collapse into Phase 5.

---

## Risk notes (DO-NOT-DELETE-YET, ranked)

1. **`applications/multi-instance-configuration/webui` + `webui/` glue** — still serves
   `station://multi-instance-configurator`; preset installs can land there and
   `resolve_start_url` still yields the configurator for raw config (ADR D6/D8). Deleting breaks
   the shipped install flow.
2. **`services/services/auto-updater` + `auto-update/` + `electron-updater`** — only update
   channel; no Rust updater exists.
3. **`sdkv2` unbridged channels + `user-identities/sagas` + `electron-google-oauth`** —
   `RequestLogin` has no napi backend (`station-bridge` exports verified; `docs/rust-napi-bridge.md`
   marks it "planned"; `UninstallApplications` is bridged since the survey). `SetApplicationConfigData`
   is settled by slice 10: native shell owns config state, the TS channel simply retires with the
   Electron renderer.
4. **OS notification path** (`os-notification` service, `notifications/`, notification webview
   preloads) — slice 8 gave the native shell a *page* source
   (`station-shell/src/notifications/bridge.rs` over wry IPC, clicks routed via url-router), but
   there is still **no OS notification source** and no `Snooze::SyncWithOs` reader, and the
   Electron path (OS deliver + click fan-out) is not superseded; deleting it silences
   notifications app-wide.
5. **`plugins/webview-preload.js` + injected scripts + `static/preload/` + `session.ts`** — every
   `<webview>` in the shipped app loads them. The native shell's only preload is the
   notification-only `INIT_SCRIPT` in `notifications/bridge.rs`; the full bxApi preload has no
   native equivalent yet.
6. **Tray, downloads, in-tab search, password managers, chrome extensions (`ecx`), menus/context
   menus, autolaunch, dialogs** — no Rust equivalents; deletion is a visible feature loss.
7. **`packages/sdk` + `manifests/runtime`** — published SDK consumed by manifest apps; not
   retireable until the SDK story moves to `station-sdk`/bridge.

## UNVERIFIED

- `packages/appstore/src/components/*` per-component parity vs `station-shell::appstore.rs`
  (did not diff each component; grouped as partial).
- `packages/app/src/settings/` breadth vs `settings.rs` (did not enumerate TS settings cards).
- `services/services/{cursor,skeleton,extendedAppMetrics,process-manager}` exact behavior
  (directory listing only).
- `packages/app/src/windows/utils/GenericWindowManager.ts` full window-type scope (imports seen;
  file not read end-to-end).
- Windows installer behavior for `protocols:` (`build/installer.nsh` has no `station` hits;
  registration assumed from `electron-builder.yml` — not verified by running an install).
- `chrome-extensions/` full feature set (types + ecx wiring seen; not read end-to-end).

---

## Phase 1 executed (2026-11-18)

- **Status:** already applied in this tree before this pass, and re-verified here. Commit
  `0fd0465` ("feat(bridge): w03-watchers-dead-types") removed the dead surfaces from
  `packages/app/src/plugins/bxapi.d.ts`; the file is kept and the rest of its declarations are
  untouched. No further deletion was made in this pass.
- **Removed:** the `user` and `services` members of `Bx` (declared as `BxAPI.User` /
  `BxAPI.Services`), plus `BxAPI.Runtime`, `BxAPI.AuthorizationError`, `BxAPI.NoMethodError`,
  `BxAPI.SystemError`, and the helper types that existed only for them (`AnalyticsIdentity`,
  `ServiceAndCorrespondingApplications`, `ObservableResponse`, `ActionResponse`, the `User` /
  `Services` interfaces) and the then-unused `Observable` import. A tombstone comment in the file
  records the deletion.
- **Reference sweep** (`rg --hidden --no-ignore`, over `packages/`, including `.tsx`, `.js`, tests
  and `.storybook`): `BxAPI.user`, `BxAPI.services`, `bxApi.user`, `bxApi.services`,
  `BxAPI.Runtime`, `ObservableResponse`, `ActionResponse`, `AnalyticsIdentity`,
  `ServiceAndCorrespondingApplications` = **0 hits**. `\bRuntime\b` = 2 hits: the tombstone comment
  and `sdk/.gitignore` ("# Runtime data", not the type). `AuthorizationError`, `NoMethodError`,
  `SystemError` = 1 hit each, the tombstone comment. `webview-preload.js` exposes no
  `user`/`services` member.
- **Verification commands:** `yarn workspace station-desktop-app tsc --noEmit` and
  `yarn workspace station-desktop-app build` both stop before compiling with
  `Usage Error: Couldn't find the node_modules state file - running an install might help
  (findPackageLocation)` (exit 1). This worktree has no `node_modules`/install state and no global
  TypeScript, so no JavaScript type/build check can run here; the deletion is type-only
  (ambient `.d.ts` declarations) and removes no runtime code path.
