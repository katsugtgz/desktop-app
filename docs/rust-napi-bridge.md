# s14 — Frontend bridge plan: `window.bxApi` → napi exports

Notes slice for `s14-appstore-ui-wiring` (amended shell decision: napi-rs into
existing Electron, NOT Tauri — see `docs/rust-rewrite-plan.md` line 5). This
document maps every `window.bxApi` method to its planned napi function. No TS
is edited in this slice; renderer migration is post-overnight, human-gated.

## Sources read (authoritative order)

1. `packages/app/src/plugins/webview-preload.js` — the **runtime** `bxApi`
   object (`contextBridge.exposeInMainWorld('bxApi', bxApi)`). Authoritative
   for what actually exists.
2. `packages/app/src/plugins/bxapi.d.ts` — type surface. **Superset of
   runtime**: declares `user`, `services`, `Runtime`, and error classes that
   `webview-preload.js` never exposes. Dead types, do not port.
3. `packages/app/src/services/services/sdkv2/worker.ts` — the worker proxy:
   `bxAPIAllowedActions` (12 perform channels) and
   `bxAPIAllowedSelectorsObservers` (3 selector channels) with
   `allowedParameters` per channel.
4. Call sites: `packages/appstore/src/{api.ts,app.tsx,app-request/sagas.ts,
   HOC/withCustomApplications.tsx,components/.../AppStoreMyCustomApps.tsx}`,
   `packages/app/src/applications/multi-instance-configuration/webui/**`,
   `packages/app/src/notification-center/webview-preload.ts`,
   `packages/app/src/plugins/injected-js/{whatsapp,messenger}InjectedScript.js`,
   `packages/app/src/static/preload/{preload.js,webview-inject.js}`.

## Transport today (semantics to preserve)

- **perform** (request/response): preload → `bx-api-perform` IPC to worker
  webContents → saga/action → `bx-api-perform-response-${channel}` back. The
  response listener is registered *before* the send (`setTimeout(1)` hack, no
  ack). Preload validates required params client-side and throws
  `TypeError('X value is missing')`.
- **subscribe** (streams): `bx-api-subscribe` IPC → worker observes an rxjs
  store selector → pushes `bx-api-subscribe-response-${channel}` on every
  change. **Unsubscribe is renderer-local only** (`ipcRenderer.off`): the
  worker-side observable subscription is never torn down. Known TS leak; the
  napi port adds an explicit unsubscribe message instead of replicating the
  leak (deliberate divergence, noted here).
- **notifications**: direct `ipcRenderer.send('new-notification' |
  'notification-close')` + `ipcRenderer.on('trigger-notification-click')`.
  Never transit the worker proxy.
- **`appIsReady`** gates every perform on `document.readyState`.
- Response envelope: query-shaped methods resolve `{ body: ... }`
  (`search`, `getMostPopularApps`, `getAllCategories`,
  `getApplicationsByCategory`, `requestPrivate`, `getPrivateApps`,
  `getManifest`); command-shaped methods (`install`, `uninstall`,
  `uninstallByManifest`, `setConfigData`, `requestLogin`) resolve raw values.
  Preload stays byte-compatible if napi returns the exact JSON the worker
  channel returns today.
- `preload.js` whitelists `bxApi` in the post-load global cleanup — keep.

## Planned shape

New crate `rust/crates/station-bridge` (cdylib, napi-rs v3, per the amended
plan). Loaded by the Electron **main** process. A thin TS glue module in main
registers `bx-api-perform` / `bx-api-subscribe` IPC handlers that call the
napi exports and forward responses; `webview-preload.js` and every renderer
stay untouched — that is the point of keeping the `window.bxApi` shape.
Streams use napi `ThreadsafeFunction`s: Rust store change → tsfn →
`webContents.send('bx-api-subscribe-response-${channel}')`.

JS-facing names keep bxApi casing via `#[napi(js_name = "...")]`.

## Method → napi mapping

Rust status legend: **exists** = function already in workspace crates;
**planned** = to be added in the s14 wiring slice (or the slice named).

### `notificationCenter` (5 methods — stay Electron IPC, passthrough)

| bxApi method | Today | napi plan |
|---|---|---|
| `sendNotification(id, notification)` | `ipcRenderer.send('new-notification', id, notification)` | Keep Electron main handler (owns windows); optional later `emit_notification(id, NotificationPayload)` in station-bridge. **planned** (s14, passthrough first) |
| `closeNotification(id)` | `ipcRenderer.send('notification-close', id)` | Same: passthrough, later `close_notification(id)`. **planned** (s14) |
| `addNotificationClickListener(fn)` | `ipcRenderer.on('trigger-notification-click')` | Stays Electron; click fan-out owned by main. No napi fn needed. **passthrough** |
| `removeNotificationClickListener(fn)` | `ipcRenderer.off(...)` | Same. **passthrough** |
| `addSnoozeDurationInMsChangeListener(fn)` | subscribe `GetSnoozeDuration` (selector `getSnoozeDurationInMs`) | `watch_snooze_duration(cb: ThreadsafeFunction)` over notification-center state in station-shell; emits `string \| undefined` (ms). **planned** (s14) |

### `applications` (10 methods)

| bxApi method | Channel (worker action) | Preload required params | Rust backend | napi export |
|---|---|---|---|---|
| `install(payload)` | `InstallApplication` → `app-store/sagas#addApplicationRequest` | `manifestURL`, `context` (worker also allows `inBackground` — preload does not require it; preserve worker set) | **exists**: `appstore_service::write_commands::ApplicationService::install` + `app_request` reducer | `install_application(manifest_url, context, in_background) -> Value` |
| `uninstall(applicationId)` | `UninstallApplication` → `applications/duck#uninstallApplication` | `applicationId` | **exists**: `ApplicationService::uninstall` | `uninstall_application(application_id) -> Value` |
| `uninstallByManifest(manifestURL)` | `UninstallApplications` → `abstract-application/duck#uninstallAllInstances` | `manifestURL` | **planned**: new `uninstall_all_for_manifest(manifest_url)` in appstore-service (loop over `has_applications_for_manifest`/uninstall) | `uninstall_applications_by_manifest(manifest_url) -> Value` |
| `setConfigData(applicationId, configData)` | `SetApplicationConfigData` → `applications/duck#setConfigData` | `applicationId`, `configData` | **planned**: UI-state command; station-shell state store (config data is renderer-facing app state, not manifest data) | `set_application_config_data(application_id, config_data) -> Value` |
| `search(query)` | `SearchApplication` → `searchApplication` saga | `query` | **exists**: `appstore_service::search_applications` → `{ body: MinimalApplication[] }` | `search_applications(query) -> Value` |
| `getMostPopularApps()` | `GetMostPopularApplication` | — | **exists**: `get_most_popular_apps` → `{ body: PopularApps }` | `get_most_popular_applications() -> Value` |
| `getAllCategories()` | `GetAllCategories` | — | **exists**: `get_all_categories` → `{ body: string[] }` | `get_all_categories() -> Value` |
| `getApplicationsByCategory()` | `GetApplicationsByCategory` | — | **exists**: `get_applications_by_category` | `get_applications_by_category() -> Value` |
| `requestPrivate(payload)` | `RequestPrivateApplication` → `requestPrivateApplication` saga | `name`, `themeColor`, `bxIconURL`, `startURL`, `scope` | **exists**: `request_private_application` + `app_request` state machine (`submit_via`, cannot-fail IPC semantics documented there) | `request_private_application(recipe) -> Value` (`{ body: { id, bxAppManifestURL } }`) |
| `getPrivateApps()` | `GetPrivateApplications` | — | **exists**: `private_manifests()` (icon projection to `AppManifest` shape) | `get_private_applications() -> Value` (`{ body: AppManifest[] }`) |

### `theme` (1 method)

| bxApi method | Channel (selector) | napi export |
|---|---|---|
| `addThemeColorsChangeListener(fn)` | `GetThemeColors` (`getThemeColors`) | `watch_theme_colors(cb: ThreadsafeFunction)` — emits `string[]`; fan-out to all subscribed renderers. **planned** (s14) |

### `identities` (3 methods)

| bxApi method | Today | napi export |
|---|---|---|
| `addIdentitiesChangeListener(fn)` | subscribe `GetAllIdentities` (`getSimpleIdentitiesForProvider(state, 'google')`) | `watch_identities(cb: ThreadsafeFunction)` — provider fixed `'google'` today; keep fixed, widen later only if a caller appears. **planned** (s14) |
| `removeIdentitiesChangeListener(fn)` | renderer-local `ipcRenderer.off` only (worker leak, see Transport) | `unwatch_identities(cb)` with explicit teardown. **planned** (s14) |
| `requestLogin(provider)` | perform `RequestLogin` → `user-identities/sagas#callRequestSignIn` | `request_login(provider) -> Value`; OAuth window flow stays Electron shell, napi fn is the trigger + state hook. **planned** (s14) |

### `manifest` (1 method)

| bxApi method | Channel | Rust backend | napi export |
|---|---|---|---|
| `getManifest(manifestURL)` | `GetManifestByURL` → `getManifestByURL` saga | **exists**: `ApplicationService::get_manifest_by_url` | `get_manifest_by_url(manifest_url) -> Value` (`{ body: BxAppManifest }`) |

Note: d.ts comments "Only available on `station://` tabs" but
`webview-preload.js` exposes `manifest` unconditionally. Port the runtime
behavior (unconditional); fix the stale comment in the s14 TS slice.

## Not ported (dead surface — verify with `rg`, then delete types in s14 TS)

- `BxAPI.user` (`id`, `firstName`, `identity`) — declared in `bxapi.d.ts`,
  never constructed by any preload; zero `bxApi.user` call sites.
- `BxAPI.services` (all 4 members) — same: no runtime, no call sites.
- `Runtime` class, `AuthorizationError`/`NoMethodError`/`SystemError` —
  internal/dead; nothing throws them.

## Slice boundaries

- **This slice (s14-bridge-notes)**: this file only.
- **s14 wiring slice**: `station-bridge` crate + main-process glue +
  worker.ts replacement; renderer/preload untouched (except the dead-type
  cleanup above, which is type-only).
- Verification for the wiring slice (from plan): `rg "window\.bxApi"` in
  appstore still hits (shape kept) but `rg "mocked"` exits 1; `cargo test -p
  station-bridge`; `yarn workspace @getstation/appstore build`.

## Coverage self-check

Exposed runtime methods (from `webview-preload.js`): 20 = 5
(notificationCenter) + 10 (applications) + 1 (theme) + 3 (identities) + 1
(manifest). All 20 appear in the tables above; all 12 worker perform
channels and all 3 selector channels are accounted for.
