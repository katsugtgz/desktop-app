# OAuth and per-app configuration — two options (decision brief)

Status: brief for a human decision. No code, no ADR change (this file only).
Date: 2026-11-18. Subject: how `google-account` apps (Gmail, and every manifest whose only
preset is `google-account`) get signed in and configured in the native shell, now that
`station://multi-instance-configurator` is not served and Settings says "Sign-in is not available
in the native shell yet".

## The question

Two routes were proposed:

- **A.** Port the React configurator and serve `station://multi-instance-configurator` in a
  WebView2 page.
- **B.** A native OAuth flow (system browser) plus the `station://` callback, with the existing
  native config editor doing the configuration.

They are not equal alternatives: as the facts below show, **A cannot sign in to Google at all**
(the page is an embedded user-agent under our control), so A needs B's OAuth core and adds a page
on top. The brief still evaluates both fully so the choice is explicit.

## What I read (paths verified in this turn)

| Path | What it establishes |
| --- | --- |
| `docs/rust-rune-port.md` D6, D8, D10, Remaining gaps 3 | Store is Rune-drawn; preset apps open their formatted start URL (signed-out) instead of the configurator; per-app config already landed (`appConfig`, `set_config_data`, Settings editor, `resolve_start_url` on install/relaunch); the recorded gap is "accounts/OAuth … says sign-in is unavailable instead of collecting an identity string". |
| `packages/app/src/applications/multi-instance-configuration/webui/MultiInstanceConfigurator.tsx` (+ `components/{ChooseIdentityForm,ChooseCommonForm,NormalFlowForm}.tsx`, `index.tsx`, `handler.ts`, `../helpers.ts`) | The configurator is a React page (5 files, ~550 lines) that talks to **seven** `window.bxApi` entry points: `applications.setConfigData`, `applications.uninstall`, `manifest.getManifest`, `identities.requestLogin('google')`, `identities.addIdentitiesChangeListener`, `identities.removeIdentitiesChangeListener`, `theme.addThemeColorsChangeListener`. `getMultiInstanceConfiguratorURL(manifestURL, applicationId)` builds the URL. |
| `packages/app/webpack.config.webui.js` | The page already has a web build entry (`multiInstanceConfiguration` → `multi-instance-configuration.html`, output `dist/renderer`, `target: 'web'`) — serving it needs copied static assets, not a new bundler. |
| `packages/app/src/webui/{webUIHandler.ts,handlers.ts,handler-appstore.ts,types.ts,const.ts}` | Electron serves `station://` through `protocol.handle` with a hostname→file map and `net.fetch(pathToFileURL(...))`; two hosts registered (configurator, appstore). |
| `packages/app/src/services/services/electron-google-oauth/{main.ts,interface.ts}` | Today's sign-in is an **embedded Electron window** (`@getstation/electron-google-oauth2`) with `successRedirectURL: 'https://github.com/getstation/desktop-app'`, client id/secret from `GOOGLE_CLIENT_ID`/`GOOGLE_CLIENT_SECRET` env (`.env.example` shows both, commented). |
| `packages/app/src/user-identities/{duck.ts,api.ts,get.ts,types.ts,sagas.ts}` | An identity is `{identityId: "google-<sub>", provider, userId, profileData{displayName,firstName,lastName,email,imageURL}, accessToken?, refreshToken?}`, persisted in the redux store; `signInWithGoogle` maps the OAuth result (People API profile or id-token claims) into it. `requestSignIn` requests `userinfo.email` + `userinfo.profile`. |
| `packages/app/src/applications/{duck.ts,helpers.ts,sagas/configData.ts,resolvers.ts}` | `setConfigData(applicationId, {identityId|subdomain|customURL})`; `formatURL` renders `start_url_tpl` with **config data only** (so `{{userIdentity.profileData.email}}` renders empty in TS too); after `setConfigData` a saga navigates the tab to the new start URL, clears back-history, and **replaces the app icon with the identity's avatar**; the identity id is also exposed per app through the GraphQL resolver. |
| `packages/app/manifests/definitions/14.json` (Gmail) and `21.json` (Slack) | Gmail: `presets: ["google-account"]`, `instance_label_tpl: "{{email}}"`, `start_url`/`start_url_tpl` carry `{{userIdentity.profileData.email}}`. Slack: `presets: ["subdomain","on-premise"]`, `start_url_tpl: "https://{{subdomain}}.slack.com"`, `bx_use_default_session: true`. |
| `packages/app/src/applications/Application.tsx`, `services/services/session/main.ts` | The app's webview partition is `persist:${applicationId}` unless `bx_use_default_session`; **identityId does not select a partition** — Gmail's cookies are the app page's own. |
| `rust/crates/webui/src/lib.rs` | Ported resolution: `BX_PROTOCOL = "station"`, `ProtocolHandler{hostname,file_path}`, `resolve_webui_request(...) -> WebuiResolution::{Serve{file_url}, NotFound{body}}`, `default_handler_hostnames() == ["multi-instance-configurator","appstore"]`, `subpath_stays_in_dir`. |
| `rust/crates/station-shell/src/{deep_link.rs,instance.rs,webview.rs}` | Only `station://appstore` (exact host, no path/query) is honoured from argv/IPC; everything else is one stderr line and dropped (`deep_link::open`/`Outcome`). A single-instance lock + loopback token-gated listener already exists (`instance::{handoff_if_secondary,start_primary,InstanceFile}`). The wry builder registers no custom protocol today; its page bridge is `notifications::{INIT_SCRIPT, receive}` over `with_ipc_handler`, and `webview::load_if_attached` navigates an attached page. |
| `rust/crates/station-shell/src/{state.rs,settings.rs}` + `rust/crates/appstore-service/src/write_commands.rs` + `rust/crates/session-store/src/lib.rs` | The config half exists: `ApplicationService::{set_config_data, clear_config_data, resolve_start_url}`; `session_store::{SessionState.app_config, AppConfigEntry}` (additive, v1); `ShellState::{config_of, set_app_config}`; Settings' Configure row renders "Sign-in is not available in the native shell yet." for `google-account`. |
| `docs/rust-packaging-plan.md` §7 | The installer already plans to register `station://` under `HKCU\Software\Classes\station` → `station-shell.exe "%1"` (used for launch links, not as an OAuth redirect). |

### Google's rules — checked against primary sources in this turn

- **Public client, PKCE always.** Desktop apps "cannot keep secrets"; "Always use PKCE … when
  requesting authorization codes." Token exchange takes `client_id`, `code`, `code_verifier`,
  `grant_type`, `redirect_uri`, with `client_secret` **optional**.
  (`developers.google.com/identity/protocols/oauth2/native-app`)
- **System browser + loopback or a custom scheme with a period.** The doc's flow step is "open the
  system browser and supply a local redirect URI (such as a loopback IP address)"; the custom-scheme
  form must contain a period (`com.example.app:redirect_uri_path`); the redirect must **exactly
  match** a URI registered on the client, else `redirect_uri_mismatch`; `state` is called out as
  the CSRF defence.
- **Embedded webviews are refused.** Error `disallowed_useragent`: "The authorization endpoint is
  displayed inside an embedded user-agent disallowed by Google's OAuth 2.0 Policies." Policy:
  "A developer must not direct a Google OAuth 2.0 authorization request to an embedded
  user-agent under the developer's control." (`…/oauth2/policies`, last modified 2026-08-05;
  the 2016 announcement says the same for "equivalents on Windows and OS X".)
- **Tokens at rest must be encrypted** if stored ("always store encrypted tokens at rest").

### Consequences that bound both options

1. `station://` **cannot** be Google's `redirect_uri`: custom schemes must contain a period and
   `station` has none. The OAuth callback is therefore a **loopback HTTP listener**
   (`http://127.0.0.1:<ephemeral>`), not the `station://` handler.
2. A WebView2 page **cannot** perform the Google authorization request — so "port the configurator
   and its `requestLogin('google')`" still means: open the system browser, wait on loopback, then
   tell the page the result. The page is UI only.
3. An identity does **not** make Gmail's page signed in. Gmail's start URL renders
   `Email=` empty in both implementations (verified in `helpers.ts`), the webview partition is per
   application id, and the identity's runtime roles in TS are: `{{identityId}}` in templates,
   the app's icon (avatar), and the GraphQL identity object. Signing *into* Gmail happens on
   Gmail's own page with its own cookies.

## Option A — port the React configurator, serve `station://multi-instance-configurator`

### A1. What actually gets built

- **New** `rust/crates/station-shell/src/configurator.rs` (Rust, `station-shell`): the wry custom
  protocol registration for host `multi-instance-configurator` + an adapter from
  `webui::resolve_webui_request` (which returns a `file://` URL) to bytes read from disk; the
  `Serve`/`NotFound` shapes (404 bodies) carry over.
- **Changed** `webview.rs`: the page builder gains the protocol handler for pages that are
  configurator pages (the app pages keep `with_url`); a per-page bind of `(applicationId,
  manifestURL)` so IPC can refuse anything else.
- **New** `rust/crates/station-shell/src/webui_bridge.rs`: the seven `window.bxApi` calls the page
  makes (listed above) over `with_ipc_handler`, modelled on `notifications::bridge`
  (`INIT_SCRIPT` + `receive` + caps). `setConfigData` → `appstore_service::set_config_data`
  (already exists), `uninstall` → `ApplicationService::uninstall`, `getManifest` →
  `manifest_registry` lookup, `identities.*` → the identity store added in B1/B2 below,
  `theme.*` → Rune tokens.
- **Assets**: `packages/app/webpack.config.webui.js` already builds
  `multi-instance-configuration.html` into `dist/renderer`; packaging (plan §7 step 3's file list)
  must copy that directory next to the exe, plus a `webui_root` the Rust side serves.
- **Changed** `deep_link.rs`: accept `station://multi-instance-configurator/?manifestURL=…&applicationId=…`
  (today `not_served`) and open/select that app's configurator page.
- **Reused, not rebuilt**: `webui` (resolution + containment), `appstore-service` config commands,
  `session-store` `appConfig`.
- **Replaced TS**: the served page is the same `MultiInstanceConfigurator.tsx` bundle; what is
  replaced is *the Electron protocol handler and the `window.bxApi` preload behind it* — i.e.
  `webui/webUIHandler.ts` + the configurator's slice of `webview-preload.js`.

### A2. Protocol work

- **Mechanism**: wry custom protocol, not `data:`/`file:`. `WebViewBuilder::with_custom_protocol`
  / `with_asynchronous_custom_protocol(name, handler)` take a `Request<Vec<u8>>` and return a
  `Response`; both exist in the pinned wry 0.57.2. **Windows origin quirk, from wry's own docs:**
  a page loaded from a custom protocol has origin `http://<scheme>.<path>` (i.e.
  `http://station.multi-instance-configurator/…`) by default, or `https://…` with
  `WebViewBuilderExtWindows::with_https_scheme`. The page therefore cannot rely on
  `location.protocol === 'station:'`; the handler must accept the rewritten URL and the page's
  routing must tolerate it.
- **Security rules that must hold**: exactly the two `webui::default_handler_hostnames()` hosts,
  everything else 404 with the ported body; `webui::subpath_stays_in_dir` enforced on every
  resolved file (the ported resolver already joins only non-empty path segments); no directory
  listing; only `text/html`, `js`, `css`, `ttf/woff2`, `svg/png` content types; **no secrets in the
  URL** (the configurator URL carries `manifestURL` + `applicationId` only); IPC messages bound to
  the attached page's app id and refused for another app; a new origin (`http://station.*`) must
  not be given access to the notification bridge.
- **What `webui` already gives**: the host→file map, the two 404 bodies, the path join and the
  containment helper — everything except the byte-serving and content types.

### A3. OAuth mechanics

- The page's "Sign in with Google" cannot run in-page (policy above). It must call
  `identities.requestLogin` → the shell opens the **system browser** (already possible:
  `routing::open_external`/`ShellExecuteW`) with a PKCE authorization URL, receives the code on a
  **loopback listener**, exchanges it, and replies to the page over IPC with the new identity.
- Library: no `oauth2` crate in the tree today; the pieces are (`ureq` 2.12 rustls, `sha2`,
  `base64`, `getrandom` — all in `Cargo.lock`) — a small local implementation of the
  authorization-URL builder, PKCE S256, callback parse and token POST. Same work as B2.
- Callback landing: loopback `http://127.0.0.1:<port>`; the shell already owns a
  token-gated loopback listener for single-instance (`instance.rs`) but the OAuth listener must be
  **separate, short-lived, `state`-verified and single-shot**.

### A4. State ownership

- `session-store` `appConfig` (exists) holds `identityId`/`subdomain`/`customURL`; a **new**
  identity collection is still needed (as in B1) because `ChooseIdentityForm` lists identities and
  `setConfigData({identityId})` needs one to point at.
- `request_login` parity: TS's `bxApi.identities.requestLogin(provider)` resolves with an identity
  object; here it would resolve over IPC with the same shape read back from the Rust store.
- `setConfigData` parity: the same `set_config_data` call the native editor uses, plus the TS
  side-effects — navigate the tab to the new start URL (`webview::load_if_attached`, or the
  retarget path) and clear back-history (TS `clearHistory`; the shell has no history API — a
  divergence to record).

### A5. Effort

**6 slices.** A1 the protocol-serving spike (static page at
`station://multi-instance-configurator/`, allowlist, containment, origin quirk recorded);
A2 packaging the built bundle + content types; A3 the seven-call bridge + per-page binding;
A4 the identity/OAuth core (shared with B); A5 the deep-link host; A6 parity, CSP and hardening.
**Riskiest: A3** — the React page assumes an Electron-ish environment (`@getstation/theme`,
listener registration, `manifest.getManifest` envelope shapes); parity is only proven by the page
rendering and clicking through, i.e. a WebView2 page we do not otherwise host today.
**First slice: A1** (small, testable headlessly through a fake protocol responder; no dead end: if
the page never lands, `webui` and the allowlist rules remain useful for the appstore URL).

### A6. What it does NOT fix

- **Gmail sign-in** — the page's sign-in button still has to run B's system-browser flow; A alone
  changes the UI, not the capability.
- **The signed-out Gmail URL** — `Email=` stays empty (`helpers.ts` renders config data only), so
  an identity does not produce a signed-in start URL.
- **Slack's need** is a subdomain, which the existing native editor already collects; A re-skins it.
- **Multi-instance UX** beyond the forms: the rail shows one row per application id and a second
  press on an installed store tile uninstalls, so "add a second Gmail instance" is still not
  reachable from the store.
- **The "normal flow" empty config**: TS calls `setConfigData(id, {})`; the shell's Clear *removes*
  the entry (D10 records this), so a page that expects an empty entry would find none.

## Option B — system-browser OAuth + loopback callback + the existing native editor

### B1. What actually gets built

- **New** `rust/crates/station-shell/src/identities.rs` (Rust): the identity model
  (`{identity_id, provider, user_id, email, display_name, avatar_url}`), validation (provider
  allowlist, ≤ 32 identities, field caps, id = `google-<sub>`), and the store glue.
- **Changed** `rust/crates/session-store/src/lib.rs`: an additive `identities` map, v1 (same
  precedent as `theme` and `appConfig`: `#[serde(default)]`, refuse-never-truncate, atomic save).
- **New** `rust/crates/station-shell/src/oauth.rs` (Rust): PKCE (S256 via `sha2` + base64url +
  `getrandom`), the authorize-URL builder, a single-shot loopback listener with `state` check and
  timeout, the token POST via `ureq`, and id-token claim extraction (`sub`, `email`, `name`,
  `picture` — the four fields `api.ts` fills from the People API).
- **Changed** `settings.rs`: for a `google-account` app the Configure row replaces "Sign-in is not
  available in the native shell yet." with **Sign in with Google** + the stored identities
  (email/avatar, radio-style choose), calling the existing `set_app_config`/`set_config_data` with
  the chosen `identityId`; Clear/remove identity.
- **Reused**: `session-store` `appConfig`, `appstore-service::{set_config_data, resolve_start_url}`,
  `ShellState::{config_of,set_app_config}`, `webview::load_if_attached`, `routing::open_external`
  (`ShellExecuteW`) for the browser.
- **Replaced TS**: `electron-google-oauth/{main.ts,interface.ts}` (the embedded window),
  `user-identities/{api.ts,sagas.ts,duck.ts}` (the sign-in + identity store), and the
  google-account branch of the configurator UI. **Not** replaced: `theme`, `manifest.getManifest`,
  subdomain/on-premise forms — those are already native.

### B2. Protocol work

- **No `station://` serving is needed for OAuth.** The callback is
  `http://127.0.0.1:<ephemeral>/…` (Google's loopback form for installed apps), so no custom
  protocol, no origin rewrite, no new page.
- **Security rules that must hold**: bind `127.0.0.1` (never `0.0.0.0`), random port, accept **one**
  request then close, verify `state` (constant-length compare, as `instance::token_matches` does),
  refuse bodies/headers past a small cap, time out (e.g. 3 min) and stop listening; the redirect
  URI must be exactly the registered one (see below); never log the code or tokens.
- `webui` gives nothing here and needs nothing; the packaging §7 `station://` registration is for
  **launch** links and is unrelated (and cannot serve as the OAuth redirect: no period in the
  scheme).
- Optional, later: register a reverse-DNS scheme (`com.anomaly.station:`) if the browser callback
  should survive the shell not running — a new OS registration and a new Google client entry, not
  needed for the in-app flow.

### B3. OAuth mechanics

- **System browser + loopback + PKCE**, exactly as Google's installed-app doc lays out: open
  `https://accounts.google.com/o/oauth2/v2/auth` with `client_id`, `response_type=code`,
  `scope=openid email profile`, `redirect_uri=http://127.0.0.1:<port>`, `code_challenge` (S256),
  `state`; receive `?code=…&state=…`; POST `https://oauth2.googleapis.com/token` with
  `code_verifier` (`client_secret` optional for a public desktop client).
- **What Google requires that is outside the code**: a **Desktop app** OAuth client
  (`GOOGLE_CLIENT_ID` / optional `GOOGLE_CLIENT_SECRET`, currently env-only in the repo), the
  loopback redirect registered on it, and — if the app is public — consent-screen verification for
  sensitive scopes. `openid email profile` keeps this to non-sensitive scopes as far as this brief
  knows; **UNVERIFIED** whether the existing project's consent screen is already verified for the
  scopes TS requested (`userinfo.email`, `userinfo.profile`).
- **Callback lands** in `oauth.rs`'s listener, then into `identities.rs` → `session-store` →
  `set_app_config(identityId)` → `resolve_start_url` on the next frame.
- **Tokens**: the shell needs none (it calls no Google API; the four fields come from the id
  token), so B can store **no tokens at all** — sidestepping Google's "encrypted at rest"
  requirement. TS does persist access/refresh tokens (redux); the shell should not.

### B4. State ownership

- **`session-store`** owns identities (additive `identities` map) and the existing `appConfig`
  (`identityId` there is the pointer). Same file, same atomic write path, same refusal rules.
- `request_login` parity: `bxApi.identities.requestLogin('google')` becomes
  `identities::sign_in()` returning `Result<Identity, OAuthError>`; the Settings row and any future
  caller use it. `setConfigData` parity: `appstore_service::set_config_data` + the existing
  `ShellState::set_app_config`, with `webview::load_if_attached` for the open page.
- If the Electron renderer goes away, no parity is lost: nothing in TS *reads* the stored tokens
  beyond the sign-in call itself (`getIdentityById`, `getEmail`, `getImageURL` are the only readers
  of the identity object — see `user-identities/get.ts`, `selectors.ts`).

### B5. Effort

**4 slices.** B1 identity model + store + Settings list (fakes, no network);
B2 OAuth core (pure PKCE/URL/callback parser + the loopback listener + token POST), with the live
round trip as a manual acceptance; B3 the Settings sign-in wiring + the optional avatar-as-icon
parity (`updateApplicationIconAfterSetConfigData`); B4 hardening/docs (timeouts, `state`, caps,
no-token policy, packaging notes for the client id).
**Riskiest: B2's live round trip** — browser foregrounding and the loopback hand-off cannot be
proven headlessly, and the redirect URI must be registered on the client (console work outside the
repo).
**First slice: B1** (small, testable: store round-trip, caps, refusal cases, Settings row with a
fake identity; no dead end — the identity model is what A would consume too).

### B6. What it does NOT fix

- **Gmail's page still lands signed-out** (`Email=` empty; the identity is metadata). Signing in
  *inside* Gmail's page remains whatever Google allows in WebView2 — **UNVERIFIED** whether the
  ordinary Google sign-in page completes there (the probe only saw it render).
- **Slack** needs no OAuth; its gap is the one-field subdomain editor that already exists (and the
  missing "I have a self-hosted instance" switcher / normal-flow empty config, which the shell does
  not produce).
- **Per-app multi-instance UX**: identities are per Google account, but adding a *second instance*
  of the same manifest is still not reachable (rail = one row per application id; installed tile
  press uninstalls).
- **Token-backed features** (People API calls, offline refresh): deliberately out of scope; the
  identity carries no tokens.

## Recommendation

**Option B**, first. Three facts decide it:

1. **A's page cannot sign in to Google.** Google's policy forbids directing an OAuth
   authorization request to an embedded user-agent under the developer's control, and the
   authorization endpoint answers `disallowed_useragent`. So A's `requestLogin('google')` must do
   B's system-browser + loopback flow anyway — A is B plus a page, never a substitute.
2. **The configuration half already exists.** `appConfig`, `set_config_data`, the Settings
   Configure editor and identity-aware `resolve_start_url` landed in D10; what is missing is only
   the identity (OAuth + store + a chooser row) — B is 4 slices, A is 6 on top of the same OAuth
   core.
3. **The second named app needs no OAuth at all.** Slack's manifest is
   `subdomain`/`on-premise`; its URL is fixed by a config field the native editor already
   collects, so the urgency is the identity for Gmail (and the icon/label parity), not a second UI.

**Fallback:** pick **A** if the human requires pixel/UX parity with the TS configurator (or a
provider whose flow genuinely needs an in-app web surface), and only *after* B's OAuth core lands —
A then becomes serving the existing bundle with the seven-call bridge, with B's `identities` as its
data source. The condition that would reverse the choice outright: if Google's own
`sign-in` page turns out not to complete inside WebView2 (unverified today), B's per-app page
sign-in story is also gone, and the remaining path is identity-only metadata plus the OS browser —
which is still B.

## What neither option fixes

- Gmail's signed-out start URL (`{{userIdentity.profileData.email}}` renders empty in TS too).
- Signing in to the *app's own page*: that is Gmail's page and its cookies; **UNVERIFIED** whether
  Google permits account sign-in inside WebView2.
- Slack's "self-hosted instance" switcher and the normal-flow empty config.
- A second instance of the same manifest from the store.
- Token-backed integrations (People API, offline access, other Google APIs).

## UNVERIFIED

- **Google OAuth policy details** were checked against primary sources in this turn
  (`developers.google.com/identity/protocols/oauth2/native-app`, `…/oauth2/policies`, the 2016
  Google Developers Blog post) for: PKCE, loopback/custom-scheme redirect rules, exact-match
  redirect URIs, `state`, `disallowed_useragent`, the embedded-user-agent prohibition and
  "encrypted tokens at rest". Anything beyond those texts is **UNVERIFIED**, specifically:
  - whether the project's existing OAuth client is type **Desktop app** and which redirect URIs are
    registered on it (`redirect_uri_mismatch` risk);
  - whether an arbitrary ephemeral loopback **port** is accepted for this client (the doc says
    "random available port", but the console entry is what decides);
  - the consent screen's verification state for `userinfo.email`/`userinfo.profile` under a public
    app;
  - whether Google's *account sign-in* page (as opposed to an OAuth authorization request)
    completes inside WebView2;
  - whether the packaged releases inject `GOOGLE_CLIENT_ID` at build time (the repo only shows the
    env reads and a commented `.env.example`).
- **wry custom-protocol behaviour** is quoted from wry 0.57.2's own documentation (origin
  `http://<scheme>.<path>` on Windows, `with_https_scheme`), not exercised in this repository; the
  page's tolerance of the rewritten origin is unproven.
- **The configurator bundle serving path** (`dist/renderer` assets, content types, CSP) has not
  been tried; the webpack entry exists, nothing yet serves it.
- **Cost estimates** (4 vs 6 slices) are planning judgements from the code read, not measurements.
