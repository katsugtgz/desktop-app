//! Pure URL-routing decisions (port of `packages/app/src/urlrouter`).
//!
//! Scope: the decision table in `URLRouter.routeURL` plus the URL and tab
//! helpers it calls. Electron calls, redux actions, sagas' queues and
//! redirect-following are out of scope; [`Route`] names the action the
//! caller should perform.
//!
//! Source of truth, in decreasing precedence:
//!
//! 1. an exact open-tab URL match, then a hash-stripped ("loose") match;
//! 2. the origin tab's own application scopes;
//! 3. any installed application's scopes;
//! 4. the app-store catalogue's scopes (install, then open);
//! 5. the external browser.
//!
//! TS references: `URLRouter.ts`, `types.ts`, `constants.ts`,
//! `urlDispatcherProvider.ts`, `tabs/selectors.ts` (`getTabMatchingURL`),
//! `tab-webcontents/api.ts` (`removeHashFromURL`), `applications/get.ts`,
//! `applications/selectors.ts` (`getApplicationCustomURL*`).
//!
//! Not ported, with the reason:
//!
//! - `sagas.ts`: redirect following, the URL dedupe `queue`, and the
//!   `originalUrl`/`afterFollowingRedirects` re-dispatch are side effects;
//!   [`Route::ExternalBrowser`] carries the pre-redirect URL the TS would
//!   have handed to `openExternal` after resolution.
//! - `urlDispatcherProvider.ts`: resolving a webContents id to a tab and
//!   application is Electron state; the caller fills [`Origin`].
//! - `manifestProvider.getFirstValue(manifestURL)`: fetching/inspecting the
//!   manifest is I/O, so [`InstalledApp`]/[`StoreApp`] already carry the
//!   `scope`/`extended_scopes` the TS reads. A manifest with no `scope`
//!   disables the app exactly like `getScopes`' early `return undefined`.
//! - `isNotInstallableApplication` reads redux `applicationSettings`;
//!   the `opted_out` field on [`StoreApp`] is that flag.
//! - `radix-router` internals: the catalogue lookup's static-path priority
//!   is modelled as longest matching scope path (see
//!   `find_in_all_scopes`); the tree's own traversal is library behaviour,
//!   not a rule in `URLRouter.ts`.
//!
//! `bx_single_page` and `start_url` do not participate in routing at all;
//! they are kept on [`InstalledApp`] so callers do not have to project the
//! application record.

use std::collections::HashMap;

use url::Url;

/// Port of `normalizeURL` (`URLRouter.ts`): `new NodeURL(url).toString()`.
///
/// Used only for on-premise `customURL` scopes. Returns `None` where the TS
/// throws; see [`RouteRequest::route`] for what that means for a whole
/// routing decision.
pub fn normalize_url(url: &str) -> Option<String> {
    Url::parse(url).ok().map(|parsed| parsed.to_string())
}

/// Port of `removeHashFromURL` (`tab-webcontents/api.ts`):
/// `url.split('#')[0]`.
pub fn strip_hash(url: &str) -> &str {
    url.split_once('#').map_or(url, |(before, _)| before)
}

/// Port of `ParsedURL` (`urlrouter/types.ts`).
///
/// `subdomain` is only set when the hostname has exactly three dot-separated
/// labels; every other host length keeps the whole host in `domain`
/// (`parseUrl`'s `splitHostname.length === 3` check). `port` is `None`
/// where Node's `url.port` is `''`, and default ports are already gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    pub protocol: String,
    pub subdomain: Option<String>,
    pub domain: String,
    pub port: Option<u16>,
    pub pathname: String,
}

/// Port of `URLRouter.parseUrl`.
///
/// Returns `None` where the TS throws (invalid URL) or returns `undefined`
/// (empty input). The WHATWG parser here and Node's `url` module both
/// lowercase the host, drop default ports and collapse dot segments.
pub fn parse_url(raw: &str) -> Option<ParsedUrl> {
    if raw.is_empty() {
        return None;
    }
    let parsed = Url::parse(raw).ok()?;
    let hostname = parsed.host_str().unwrap_or_default();
    let labels: Vec<&str> = hostname.split('.').collect();
    let (subdomain, domain) = if labels.len() == 3 {
        (Some(labels[0].to_string()), labels[1..].join("."))
    } else {
        (None, hostname.to_string())
    };

    Some(ParsedUrl {
        protocol: format!("{}:", parsed.scheme()),
        subdomain,
        domain,
        port: parsed.port(),
        pathname: parsed.path().to_string(),
    })
}

/// Port of `URLRouter.matchScope`: protocol, domain, port and subdomain must
/// be equal, and the scope's pathname must be a raw prefix of the URL's.
///
/// A WHATWG host may contain `*`, so both wildcard subdomain branches are
/// reachable from string input (unlike a raw prefix read of the code might
/// suggest): a `*` scope covers any non-empty URL subdomain, and a `*` URL
/// is covered by any concrete scope subdomain.
pub fn scope_matches(url: &ParsedUrl, scope: &ParsedUrl) -> bool {
    let url_subdomain = url.subdomain.as_deref();
    let scope_subdomain = scope.subdomain.as_deref();
    let matching_subdomains = url_subdomain == scope_subdomain
        || (url_subdomain == Some("*") && scope_subdomain.is_some_and(|s| !s.is_empty()))
        || (url_subdomain.is_some_and(|s| !s.is_empty()) && scope_subdomain == Some("*"));

    // `!Boolean(scope.pathname)` short-circuits an empty scope path, then
    // `startsWith` is string-, not segment-, aware.
    let matching_pathname =
        scope.pathname.is_empty() || url.pathname.starts_with(scope.pathname.as_str());

    url.protocol == scope.protocol
        && url.domain == scope.domain
        && url.port == scope.port
        && matching_subdomains
        && matching_pathname
}

/// Port of `URLRouter.searchScopes`: does any scope cover the URL?
///
/// A scope that does not parse is skipped, like the TS `if (!scope) return
/// false` inside `some`.
pub fn search_scopes(raw_url: &str, scopes: &[String]) -> bool {
    if scopes.is_empty() {
        return false;
    }
    let Some(url) = parse_url(raw_url) else {
        return false;
    };
    scopes
        .iter()
        .any(|raw_scope| parse_url(raw_scope).is_some_and(|scope| scope_matches(&url, &scope)))
}

/// Port of `URLRouter.isInScope`'s scope comparison for one raw scope string.
pub fn url_in_scope(url: &str, scope: &str) -> bool {
    match (parse_url(url), parse_url(scope)) {
        (Some(url), Some(scope)) => scope_matches(&url, &scope),
        _ => false,
    }
}

/// One application installed in Station (`ApplicationImmutable`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    pub application_id: String,
    /// `getApplicationManifestURL`; routing does not read it (only the
    /// catalogue's `manifest_url` is returned by `INSTALL_AND_OPEN`).
    pub manifest_url: String,
    /// Raw manifest `scope`, comma-separated as the TS stores it. `None`
    /// (or empty) disables the app for routing: `getScopes` returns
    /// `undefined` before looking at `extended_scopes`.
    pub scope: Option<String>,
    /// Manifest `extended_scopes`.
    pub extended_scopes: Vec<String>,
    /// On-premise `customURL` values (`getApplicationCustomURL*`). They are
    /// normalized, prepended to the manifest scopes, and their presence puts
    /// this app ahead of the plain apps in `findApplicationInInstalledScopes`.
    pub custom_urls: Vec<String>,
    /// Manifest `start_url`. Routing never reads it; the home-tab URL is
    /// compared as an ordinary URL.
    pub start_url: Option<String>,
    /// Manifest `bx_single_page`. Only the "Add a new page" button
    /// (`AddApplicationButton.tsx`) reads it; routing ignores it.
    pub single_page: bool,
}

/// One open tab (`StationTabImmutable`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTab {
    pub tab_id: String,
    /// Owner shown for convenience; tab matching keys on `url` only.
    pub application_id: String,
    pub url: String,
    /// `isApplicationHome`.
    pub is_home: bool,
}

/// One app-store catalogue entry (`ApplicationItem` in `manifests`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreApp {
    pub manifest_url: String,
    /// Raw manifest `scope`; the TS radix tree inserts it as one string
    /// (unlike `getScopes`, it is not comma-split).
    pub scope: Option<String>,
    pub extended_scopes: Vec<String>,
    /// `applicationSettings[manifestURL].doNotInstall`: the user opted out.
    pub opted_out: bool,
}

/// `RoutingOrigin`: where the routing call came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Origin<'a> {
    pub tab_id: Option<&'a str>,
    pub application_id: Option<&'a str>,
}

/// `Targets` (`urlrouter/constants.ts`), plus the unset value the TS
/// `options: { target?: Targets }` can have.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Target {
    /// No `options.target`: only pre-existing tabs and app scopes decide.
    #[default]
    Unset,
    NewTab,
    NewWindow,
    DefaultBrowser,
    DefaultBrowserBackground,
}

/// The decision, named after `URLRouterAction` in `urlrouter/types.ts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `RELOAD`: the exact-match tab is the tab that asked for routing.
    Reload { tab_id: String },
    /// `NAV_TO_TAB`: focus the application owning the matched tab.
    FocusTab { tab_id: String },
    /// `PUSH_AND_NAV_TO_TAB`: hash-stripped URL match, so the URL is pushed
    /// onto that tab's history (`changeHashAndNavigateToTab`) and focused.
    PushAndFocusTab { tab_id: String, url: String },
    /// `NAV_IN_TAB`: the origin application's own scope covers the URL, so
    /// the origin tab navigates. `tab_id` is `None` when the origin carried
    /// no tab (the TS passes `undefined` through to `navigateTabToURL`).
    OpenInTab { tab_id: Option<String>, url: String },
    /// `NEW_TAB`.
    NewTab { application_id: String, url: String },
    /// `NEW_WINDOW`: a detached tab of the same application.
    NewInstance { application_id: String, url: String },
    /// `INSTALL_AND_OPEN`: no installed app covers the URL, but the
    /// catalogue does; install that manifest, then open the URL.
    InstallAndOpen { manifest_url: String, url: String },
    /// `DEFAULT_BROWSER`.
    ExternalBrowser,
}

/// One `routeURL` call: the URL, its [`Origin`], the requested [`Target`],
/// and the state the decision is allowed to read.
#[derive(Debug, Clone)]
pub struct RouteRequest<'a> {
    pub url: &'a str,
    pub origin: Option<Origin<'a>>,
    pub target: Target,
    pub apps: &'a [InstalledApp],
    pub tabs: &'a [OpenTab],
    /// The catalogue `routeURL` consults last (`findInAllScopes`). Empty
    /// when the caller has no catalogue: the TS tree would be empty too.
    pub store_apps: &'a [StoreApp],
}

impl<'a> RouteRequest<'a> {
    /// The no-origin, no-target request (`routeURL(url)`).
    pub fn new(url: &'a str, apps: &'a [InstalledApp], tabs: &'a [OpenTab]) -> Self {
        Self {
            url,
            origin: None,
            target: Target::Unset,
            apps,
            tabs,
            store_apps: &[],
        }
    }

    pub fn with_origin(mut self, origin: Origin<'a>) -> Self {
        self.origin = Some(origin);
        self
    }

    pub fn with_target(mut self, target: Target) -> Self {
        self.target = target;
        self
    }

    pub fn with_store_apps(mut self, store_apps: &'a [StoreApp]) -> Self {
        self.store_apps = store_apps;
        self
    }

    /// Port of `URLRouter.routeURL`. See the module docs for the precedence.
    ///
    /// A URL that does not parse matches no scope, and a URL that matches no
    /// rule ends at [`Route::ExternalBrowser`] — the same outcome as the TS
    /// `catch` around `routeURL`.
    pub fn route(&self) -> Route {
        // `options.target === DEFAULT_BROWSER(_BACKGROUND)` short-circuits
        // every other rule.
        if matches!(
            self.target,
            Target::DefaultBrowser | Target::DefaultBrowserBackground
        ) {
            return Route::ExternalBrowser;
        }

        if let Some(matched) = matching_tab(self.tabs, self.url) {
            return match matched {
                TabMatch::Exact(tab) => {
                    if self.origin.and_then(|origin| origin.tab_id) == Some(tab.tab_id.as_str()) {
                        Route::Reload {
                            tab_id: tab.tab_id.clone(),
                        }
                    } else {
                        Route::FocusTab {
                            tab_id: tab.tab_id.clone(),
                        }
                    }
                }
                TabMatch::Loose(tab) => Route::PushAndFocusTab {
                    tab_id: tab.tab_id.clone(),
                    url: self.url.to_string(),
                },
            };
        }

        // The origin tab's own application scopes.
        if let Some(origin) = self.origin {
            if let Some(application_id) = origin.application_id {
                if let Some(app) = self
                    .apps
                    .iter()
                    .find(|app| app.application_id == application_id)
                {
                    match app_scopes(app) {
                        Ok(scopes) if search_scopes(self.url, &scopes) => {
                            return match self.target {
                                Target::NewTab => Route::NewTab {
                                    application_id: app.application_id.clone(),
                                    url: self.url.to_string(),
                                },
                                Target::NewWindow => Route::NewInstance {
                                    application_id: app.application_id.clone(),
                                    url: self.url.to_string(),
                                },
                                _ => Route::OpenInTab {
                                    tab_id: origin.tab_id.map(str::to_string),
                                    url: self.url.to_string(),
                                },
                            };
                        }
                        // An invalid `customURL` aborts the whole decision in
                        // the TS (`normalizeURL` throws inside `routeURL`'s
                        // `try`), not just this app's scope check.
                        Err(()) => return Route::ExternalBrowser,
                        Ok(_) => {}
                    }
                }
            }
        }

        // Any installed application's scopes (`findApplicationInInstalledScopes`).
        match find_app_in_installed_scopes(self.url, self.apps) {
            Err(()) => return Route::ExternalBrowser,
            Ok(Some(app)) => {
                let application_id = app.application_id.clone();
                return if matches!(self.target, Target::NewWindow) {
                    Route::NewInstance {
                        application_id,
                        url: self.url.to_string(),
                    }
                } else {
                    Route::NewTab {
                        application_id,
                        url: self.url.to_string(),
                    }
                };
            }
            Ok(None) => {}
        }

        // App-store catalogue (`findInAllScopes`).
        if let Some(app) = find_in_all_scopes(self.url, self.store_apps) {
            return Route::InstallAndOpen {
                manifest_url: app.manifest_url.clone(),
                url: self.url.to_string(),
            };
        }

        Route::ExternalBrowser
    }
}

/// [`RouteRequest::new`]'s decision: the common `routeURL(url)` call.
pub fn route(url: &str, apps: &[InstalledApp], tabs: &[OpenTab]) -> Route {
    RouteRequest::new(url, apps, tabs).route()
}

/// How an open tab matched a URL (`getTabMatchingURL`'s `type`).
enum TabMatch<'a> {
    Exact(&'a OpenTab),
    Loose(&'a OpenTab),
}

/// Port of `getTabMatchingURL` (`tabs/selectors.ts`).
///
/// The TS builds two `Immutable.Map`s keyed by URL (`mapKeys`), so duplicate
/// keys keep their first position but take the **last** tab's value; exact
/// matches are looked up across all tabs before the hash-stripped map is
/// consulted.
fn matching_tab<'a>(tabs: &'a [OpenTab], url: &str) -> Option<TabMatch<'a>> {
    // The TS builds two `Immutable.Map`s keyed by URL (`mapKeys`), so
    // duplicate keys keep their first position but take the **last** tab's
    // value. Each map is kept as an insertion-ordered `Vec` plus a
    // `HashMap` index: linear-scan dedupe was quadratic and pathological at
    // 10k open tabs.
    let mut exact: Vec<(&'a str, &'a OpenTab)> = Vec::with_capacity(tabs.len());
    let mut exact_index: HashMap<&'a str, usize> = HashMap::with_capacity(tabs.len());
    for tab in tabs {
        match exact_index.get(tab.url.as_str()) {
            Some(&index) => exact[index].1 = tab,
            None => {
                exact_index.insert(tab.url.as_str(), exact.len());
                exact.push((tab.url.as_str(), tab));
            }
        }
    }

    if let Some(&index) = exact_index.get(url) {
        return Some(TabMatch::Exact(exact[index].1));
    }

    let mut loose: Vec<(&'a str, &'a OpenTab)> = Vec::with_capacity(exact.len());
    let mut loose_index: HashMap<&'a str, usize> = HashMap::with_capacity(exact.len());
    for (key, tab) in exact {
        let key = strip_hash(key);
        match loose_index.get(key) {
            Some(&index) => loose[index].1 = tab,
            None => {
                loose_index.insert(key, loose.len());
                loose.push((key, tab));
            }
        }
    }

    loose_index
        .get(strip_hash(url))
        .map(|&index| TabMatch::Loose(loose[index].1))
}

/// Scopes consulted for one installed app, in `getScopes` order: normalized
/// custom URLs, then the comma-split manifest `scope`, then
/// `extended_scopes`, deduplicated by exact string.
///
/// `Ok(vec![])` means "no scope covers anything" — the TS returns
/// `undefined` when the manifest has no `scope`, even if `extended_scopes`
/// is populated. `Err(())` means a `customURL` failed to normalize, which in
/// the TS throws out of `getScopes` and out of `routeURL`'s `try`.
fn app_scopes(app: &InstalledApp) -> Result<Vec<String>, ()> {
    let Some(raw_scope) = app.scope.as_deref().filter(|scope| !scope.is_empty()) else {
        return Ok(Vec::new());
    };

    let mut scopes: Vec<String> = Vec::new();
    for custom_url in app.custom_urls.iter().filter(|url| !url.is_empty()) {
        let normalized = normalize_url(custom_url).ok_or(())?;
        push_unique(&mut scopes, normalized);
    }
    for scope in raw_scope.split(',') {
        push_unique(&mut scopes, scope.to_string());
    }
    for scope in &app.extended_scopes {
        push_unique(&mut scopes, scope.clone());
    }

    Ok(scopes)
}

fn push_unique(scopes: &mut Vec<String>, scope: String) {
    if !scopes.contains(&scope) {
        scopes.push(scope);
    }
}

/// Port of `findApplicationInInstalledScopes`: apps with a custom URL are
/// tried first (stable within each group), and the first app with any
/// matching scope wins — there is no longest-scope rule here.
fn find_app_in_installed_scopes<'a>(
    url: &str,
    apps: &'a [InstalledApp],
) -> Result<Option<&'a InstalledApp>, ()> {
    let mut order: Vec<usize> = (0..apps.len()).collect();
    order.sort_by_key(|&index| {
        if apps[index].custom_urls.iter().any(|url| !url.is_empty()) {
            0
        } else {
            1
        }
    });

    for index in order {
        let app = &apps[index];
        let scopes = app_scopes(app)?;
        if search_scopes(url, &scopes) {
            return Ok(Some(app));
        }
    }

    Ok(None)
}

/// `toRadixPath`: `/{protocol}/{domain}/{port}/{subdomain}{pathname}`, with
/// `null` interpolated for a missing port or subdomain, `:subdomain` for a
/// wildcard subdomain, and a root pathname dropped.
fn radix_path(parsed: &ParsedUrl) -> String {
    let subdomain = match parsed.subdomain.as_deref() {
        Some("*") => ":subdomain".to_string(),
        Some(subdomain) => subdomain.to_string(),
        None => "null".to_string(),
    };
    let pathname = if parsed.pathname.is_empty() || parsed.pathname == "/" {
        ""
    } else {
        parsed.pathname.as_str()
    };
    let port = parsed
        .port
        .map(|port| port.to_string())
        .unwrap_or_else(|| "null".to_string());

    format!(
        "/{}/{}/{}/{}{}",
        parsed.protocol, parsed.domain, port, subdomain, pathname
    )
}

/// The radix tree inserts both `path` and `path/**`, so a store scope covers
/// its exact path or any path below it, at segment boundaries.
fn store_scope_matches(url_path: &str, scope_path: &str) -> bool {
    url_path == scope_path
        || url_path
            .strip_prefix(scope_path)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Port of `findInAllScopes`: the app-store catalogue behind a radix tree.
///
/// The tree's exact-match-priority is modelled as "longest matching scope
/// path wins, input order breaks ties" — a radix lookup the TS library
/// performs, not a rule written in `URLRouter.ts`. The opt-out check happens
/// after the lookup, exactly like the TS: an opted-out winner yields `None`
/// rather than falling back to a broader catalogue scope.
fn find_in_all_scopes<'a>(url: &str, store_apps: &'a [StoreApp]) -> Option<&'a StoreApp> {
    let url_path = radix_path(&parse_url(url)?);

    let mut best: Option<(usize, &'a StoreApp)> = None;
    for app in store_apps {
        for raw_scope in app.scope.iter().chain(app.extended_scopes.iter()) {
            let Some(scope) = parse_url(raw_scope) else {
                // `tryInsertRadixPath` swallows unparseable scopes.
                continue;
            };
            let scope_path = radix_path(&scope);
            if store_scope_matches(&url_path, &scope_path)
                && best.is_none_or(|(length, _)| scope_path.len() > length)
            {
                best = Some((scope_path.len(), app));
            }
        }
    }

    best.map(|(_, app)| app).filter(|app| !app.opted_out)
}
