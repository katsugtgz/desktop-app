//! What the shell window shows: the apps in the rail, which one is active, and the app store's
//! own state.
//!
//! This is the state an `AppSpec` hands its view. It owns the installed-application service
//! (`appstore-service`) and the store's cached lists, so the views below read it and never
//! re-query: a rail click, a store tile and the search field all write through the methods here.
//! Nothing platform-bound lives in this type — the web view host is `webview`'s, keyed by tab id.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use appstore_service::{
    get_all_categories, get_applications_by_category, get_most_popular_apps, search_applications,
    write_commands::ApplicationConfigData, ApplicationService, InstallOptions,
    InstalledApplication, PopularApps,
};
use manifest_registry::{manifest_to_minimal_application, Manifest, MinimalApplication};
use notification_center::{NotificationAction, NotificationCenter};
use rune::TextEdit;
use session_store::{
    AppConfigEntry, Identity, SessionState, SessionStore, ThemePreference, WindowState,
};
use tab_model::{Badge, TabAction, TabModel};
use url_router::{InstalledApp, OpenTab};

use crate::session;

/// What the content area shows instead of the active app's page.
///
/// The shell owns one window and one hosted page at a time, and a Rune view cannot draw over the
/// web view's child window — so Settings and the notification panel replace (or narrow) the page
/// area and the page is hidden while they show, not covered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    /// The active app: a Rune view (the store) or its hosted page.
    #[default]
    Apps,
    /// The Settings screen, filling the content area.
    Settings,
    /// The notification panel beside the content area.
    Notifications,
    /// The About Station screen, filling the content area.
    About,
    /// The Bang Quick Switcher palette (`Ctrl+K` / `Ctrl+T`), filling the content area.
    QuickSwitch,
}

/// The user's private-manifest store, or an in-memory one when the config dir cannot be opened:
/// the window must open either way. Custom apps then live for the session only (a warning says
/// so on stderr).
fn launch_service() -> ApplicationService {
    match ApplicationService::new() {
        Ok(service) => service,
        Err(why) => {
            eprintln!(
                "station-shell: cannot open the private manifest store ({why}); \
                 custom apps will not persist this run"
            );
            ApplicationService::in_memory()
        }
    }
}

/// The seconds between repeated save-failure log lines.
pub const SAVE_LOG_EVERY_S: f64 = 10.0;

/// The store's own tab URL. A tab with this URL is drawn by Rune, so `webview` never hosts it.
pub const APP_STORE_URL: &str = "station://appstore";

/// The store tab's id: stable, so its rail key stays `tab-appstore` and no application id can
/// collide with it.
const APP_STORE_ID: &str = "appstore";

/// How a tab's page reaches the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    /// Drawn by Rune in the content area (the App Store).
    Store,
    /// Hosted by a native web view over the content area.
    Web,
}

/// One app in the rail: the id keys and the web view host use, the label the user reads, the
/// URL its pane shows, and the manifest icon it draws.
///
/// The page may override two presentation facts (slice 20): its own `document.title`
/// ([`AppTab::page_title`]) and a `browserx-badge` meta ([`AppTab::badge`]). Both live only in
/// memory — the session document stores installed applications and URLs, never page labels — and
/// the manifest `label` is the fallback the moment the page clears either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppTab {
    pub id: String,
    /// The manifest's name: what the tab shows until (and whenever it is not overridden by) the
    /// page's own title.
    pub label: String,
    pub url: String,
    pub kind: TabKind,
    /// The manifest's icon URL (`MinimalApplication::icon_url`); empty draws the letter avatar.
    pub icon: String,
    /// The manifest has multi-instance presets, so Settings offers a configuration editor.
    pub configurable: bool,
    /// The page's own `document.title`, trimmed and capped by the bridge; `None` means the page
    /// never set one (or cleared it) and the manifest label shows.
    pub page_title: Option<String>,
    /// The page's `browserx-badge` meta, normalized with [`tab_model::Badge`]'s TS rule; `None`
    /// means no badge (missing meta or an empty one).
    pub badge: Option<Badge>,
    /// The page's `<link rel~="icon">` URL (`http://`/`https://`, Slice 31). Memory-only, never
    /// persisted; falls back cleanly to [`Self::icon`] whenever absent or not yet decoded.
    pub page_favicon: Option<String>,
    /// The manifest's `bx_override_user_agent`: the UA Electron's `<webview useragent>`
    /// attribute gave this app's page. `None` means the session's default UA.
    pub user_agent: Option<String>,
    /// The manifest's `bx_use_default_session`: Electron gave the app no partition (the default
    /// session) instead of `persist:<applicationId>`.
    pub use_default_session: bool,
    /// The manifest's `bx_legacy_service_id`: selects the static per-site injected script
    /// (`site_scripts::for_service`) when the web view is built.
    pub legacy_service_id: Option<String>,
}

impl AppTab {
    /// The App Store: always the rail's first row.
    pub fn store() -> Self {
        Self {
            id: APP_STORE_ID.to_owned(),
            label: "App Store".to_owned(),
            url: APP_STORE_URL.to_owned(),
            kind: TabKind::Store,
            icon: String::new(),
            configurable: false,
            page_title: None,
            badge: None,
            page_favicon: None,
            user_agent: None,
            use_default_session: false,
            legacy_service_id: None,
        }
    }

    /// An installed application's home tab, from the manifest the install returned.
    pub fn web(id: impl Into<String>, label: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            url: url.into(),
            kind: TabKind::Web,
            icon: String::new(),
            configurable: false,
            page_title: None,
            badge: None,
            page_favicon: None,
            user_agent: None,
            use_default_session: false,
            legacy_service_id: None,
        }
    }

    /// `web` with the manifest icon set: what `rebuild_tabs` builds for an installed app.
    pub fn web_with_icon(
        id: impl Into<String>,
        label: impl Into<String>,
        url: impl Into<String>,
        icon: impl Into<String>,
    ) -> Self {
        Self {
            icon: icon.into(),
            ..Self::web(id, label, url)
        }
    }

    /// `web_with_icon` with the configuration flag: the rail's real builder.
    pub fn web_full(
        id: impl Into<String>,
        label: impl Into<String>,
        url: impl Into<String>,
        icon: impl Into<String>,
        configurable: bool,
    ) -> Self {
        Self {
            configurable,
            ..Self::web_with_icon(id, label, url, icon)
        }
    }

    /// The element key of this app's rail row. Unique because tab ids are.
    pub fn key(&self) -> String {
        format!("tab-{}", self.id)
    }

    /// The label the UI shows: the page's own title once it set one, else the manifest name.
    /// The manifest name returns the moment the page clears its title (`page_title` is `None`).
    pub fn display_label(&self) -> &str {
        self.page_title.as_deref().unwrap_or(&self.label)
    }

    /// Whether Rune draws this tab's content. The App Store is the one such page today; the URL
    /// check keeps a tab built by hand with its URL out of the web view host too.
    pub fn renders_in_rune(&self) -> bool {
        self.kind == TabKind::Store || self.url == APP_STORE_URL
    }
}

/// The App Store's list, chosen in its sidebar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StoreSection {
    #[default]
    MostPopular,
    AllApps,
    MyCustomApps,
}

impl StoreSection {
    /// The sidebar's sections, top to bottom.
    pub const ALL: [StoreSection; 3] = [
        StoreSection::MostPopular,
        StoreSection::AllApps,
        StoreSection::MyCustomApps,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StoreSection::MostPopular => "Most popular",
            StoreSection::AllApps => "All apps",
            StoreSection::MyCustomApps => "My custom apps",
        }
    }

    /// The rail-style element key of its sidebar row.
    pub fn key(self) -> &'static str {
        match self {
            StoreSection::MostPopular => "store-section-most-popular",
            StoreSection::AllApps => "store-section-all-apps",
            StoreSection::MyCustomApps => "store-section-my-custom-apps",
        }
    }

    pub fn main_title(self) -> &'static str {
        match self {
            StoreSection::MostPopular => "Most Popular",
            StoreSection::AllApps => "All Apps",
            StoreSection::MyCustomApps => "My custom apps",
        }
    }
}

/// A page the shell could not attach: what the pane shows instead of a blank area.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageError {
    /// The application's rail label.
    pub application: String,
    /// What the host said (WebView2's error text).
    pub detail: String,
}

/// What pressing a store tile did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileAction {
    /// Nothing changed (the install failed).
    Ignored,
    /// The app was installed and its tab selected.
    Installed,
    /// The app was removed; the caller closes its page (`webview::close_tab`), because the host
    /// is not state's to touch.
    Uninstalled { application_id: String },
}

/// The shell's state: installed applications, the rail built from them, and the store's lists.
pub struct ShellState {
    store: ApplicationService,
    tabs: Vec<AppTab>,
    active: String,
    /// The manifest store's private applications, as the store queries want them.
    private: Vec<Manifest>,
    custom: Vec<MinimalApplication>,
    popular: PopularApps,
    categories: Vec<(String, Vec<MinimalApplication>)>,
    section: StoreSection,
    search: TextEdit,
    results: Vec<MinimalApplication>,
    theme: ThemePreference,
    /// The login item's last known registry state (Windows `HKCU\…\Run\Station`): refreshed
    /// from the registry on a real launch, written with the session after a successful toggle.
    start_at_login: bool,
    /// Why the login item could not be read or written, for the Settings row. Cleared by the
    /// next successful read; a failed write keeps the last known truth, so no toggle lies.
    start_at_login_error: Option<String>,
    /// Whether closing the main window hides Station to the Windows system tray (Slice 35).
    minimize_to_tray: bool,
    surface: Surface,
    page_error: Option<PageError>,
    /// Failed writes since the last success: logged at most every [`SAVE_LOG_EVERY_S`].
    save_failures: u32,
    /// The clock time of the last save-failure log line.
    save_logged_at: f64,
    notifications: NotificationCenter,
    /// The accounts the session stores (slice 17): what Settings lists and the Configure
    /// chooser offers. Loaded at launch and written back unchanged — nothing in this slice
    /// creates one (sign-in is a later slice), and no tokens are part of the shape.
    identities: BTreeMap<String, Identity>,
    /// The session file, when this shell persists. `None` in tests that build a state directly:
    /// they never touch a session path.
    session: Option<PathBuf>,
    /// The placement loaded from the session, replaced by the live window's when it exists.
    window: WindowState,
    /// Set when the session on disk could not be read or replayed: the file is not overwritten
    /// until the user acts (a selection, an install, an uninstall).
    save_blocked: bool,
    /// The user's rail order (slice 11): application ids, newest install last, the App Store
    /// excluded (its row is always first and never moves). The rail builds from it; `dock`
    /// stays the service's membership fact. Empty on a fresh shell until the first user
    /// action writes it.
    rail_order: Vec<String>,
    /// The manifest icons: handles per URL, fetched once per run on the icons worker, cached on
    /// disk. Views read `icon_pixels`; nothing in the state touches the network itself.
    icons: crate::icons::IconStore,
    /// The configuration editor's draft, while Settings shows an app's editor (`None` otherwise;
    /// leaving Settings drops it). `TextEdit` is the field widget; its text is the value.
    config_draft: Option<ConfigDraft>,
    /// Metadata and update-check state for the About Station surface (`about-window` crate).
    about_state: about_window::AboutWindowState,
    /// Persisted preference for including prerelease (beta) GitHub releases (Slice 34).
    include_beta_updates: bool,
    /// Trusted GitHub release URL from the latest successful update check (memory-only).
    update_url: Option<String>,
    /// Transient error message from the most recent failed update check (memory-only).
    update_error: Option<String>,
    /// True once at least one update check completed cleanly in this run (memory-only).
    update_checked: bool,
    /// Google OAuth Option B client config (from `GOOGLE_CLIENT_ID` / optional secret).
    oauth_config: Option<crate::oauth::OAuthConfig>,
    /// Transient sign-in status or error message shown in Settings (never persisted).
    oauth_status: Option<String>,
    /// Per-tab zoom factor (`0.5..=3.0`, default `1.0`; transient, never persisted).
    zoom: BTreeMap<String, f64>,
    /// Tab id whose Rune tab-actions strip is open (right-click on rail or top-bar `•••`).
    tab_menu: Option<String>,
    /// In-tab search bar state (`Ctrl+F` / `Cmd+F`; transient, never persisted).
    find: crate::find::FindState,
    /// Pinned favorites persisted in the session document (Slice 28).
    favorites: Vec<session_store::FavoriteEntry>,
    /// In-memory frecency scorer (`activity::frecency`) driving Quick Switcher ranking.
    frecency: activity::frecency::Frecency,
    /// Bang Quick Switcher palette state (`Ctrl+K` / `Ctrl+T`; transient).
    quick_switch: crate::quick_switch::QuickSwitchState,
    /// Last polled host OS Do Not Disturb state (Slice 29, active under `Snooze::SyncWithOs`).
    os_dnd_active: bool,
    /// Inline "+ Add custom app" draft while open in `App Store -> My custom apps` (Slice 32).
    custom_app_draft: Option<CustomAppDraft>,
    /// Optional user-configured downloads folder persisted in the session (Slice 33).
    download_dir: Option<String>,
    /// Inline editor for the custom downloads folder inside Settings -> General (Slice 33).
    download_dir_draft: Option<DownloadDirDraft>,
    /// Active and recent download records shown in the bottom Rune downloads bar (Slice 33).
    downloads: Vec<crate::downloads::DownloadItem>,
    next_download_id: u64,
}

/// Inline editor draft for Settings -> General -> Downloads folder.
#[derive(Debug, PartialEq)]
pub struct DownloadDirDraft {
    pub edit: TextEdit,
    pub error: Option<String>,
}

/// Draft fields for adding a user-defined custom web app in `App Store -> My custom apps`.
#[derive(Debug, PartialEq)]
pub struct CustomAppDraft {
    pub name: TextEdit,
    pub start_url: TextEdit,
    pub scope: TextEdit,
    pub icon_url: TextEdit,
    pub theme_color: TextEdit,
    pub error: Option<String>,
}

impl Default for CustomAppDraft {
    fn default() -> Self {
        Self {
            name: TextEdit::new(""),
            start_url: TextEdit::new(""),
            scope: TextEdit::new(""),
            icon_url: TextEdit::new(""),
            theme_color: TextEdit::new("#2B5CE6"),
            error: None,
        }
    }
}

/// The settings editor's draft for one installed application.
#[derive(Debug, PartialEq)]
pub struct ConfigDraft {
    /// The application being edited.
    pub application_id: String,
    /// The labelled field (identity id or custom URL, per the manifest's presets).
    pub field: ConfigField,
    pub edit: TextEdit,
}

/// The one configuration field a manifest's presets consume — what the editor shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigField {
    /// `google-account` presets: an identity id string, linked to a stored OAuth identity.
    Identity,
    /// `on-premise` presets: the instance's full URL.
    CustomUrl,
    /// `subdomain` presets: the instance's subdomain.
    Subdomain,
}

impl ConfigField {
    /// The editor row's label.
    pub fn label(self) -> &'static str {
        match self {
            ConfigField::Identity => "Account",
            ConfigField::CustomUrl => "Server URL",
            ConfigField::Subdomain => "Domain",
        }
    }
}

impl ShellState {
    /// A shell over `store`: caches the lists the store renders, then builds the rail.
    pub fn new(store: ApplicationService) -> Self {
        let private = store.private_manifests();
        let popular = get_most_popular_apps(&private);
        // The sidebar's order comes from `get_all_categories` (sorted, Miscellaneous last); the
        // buckets come from `get_applications_by_category`. A category with no apps is dropped
        // (`Miscellaneous` is forced last even when empty).
        let grouped = get_applications_by_category(&private);
        let categories = get_all_categories(&private)
            .into_iter()
            .filter_map(|category| {
                let apps = grouped
                    .iter()
                    .find(|(name, _)| *name == category)
                    .map(|(_, apps)| apps.clone())
                    .unwrap_or_default();
                (!apps.is_empty()).then_some((category, apps))
            })
            .collect();
        let custom = private
            .iter()
            .map(manifest_to_minimal_application)
            .collect();
        let mut state = Self {
            store,
            tabs: Vec::new(),
            active: APP_STORE_ID.to_owned(),
            private,
            custom,
            popular,
            categories,
            section: StoreSection::default(),
            search: TextEdit::new(""),
            results: Vec::new(),
            theme: ThemePreference::System,
            start_at_login: false,
            start_at_login_error: None,
            minimize_to_tray: false,
            surface: Surface::Apps,
            page_error: None,
            save_failures: 0,
            save_logged_at: 0.0,
            notifications: NotificationCenter::new(),
            identities: BTreeMap::new(),
            session: None,
            window: WindowState::default(),
            save_blocked: false,
            rail_order: Vec::new(),
            icons: crate::icons::IconStore::new(),
            config_draft: None,
            about_state: about_window::AboutWindowState::default(),
            include_beta_updates: false,
            update_url: None,
            update_error: None,
            update_checked: false,
            oauth_config: crate::oauth::OAuthConfig::from_env(),
            oauth_status: None,
            zoom: BTreeMap::new(),
            tab_menu: None,
            find: crate::find::FindState::default(),
            favorites: Vec::new(),
            frecency: activity::frecency::Frecency::new(
                activity::frecency::FrecencyOptions::default(),
            ),
            quick_switch: crate::quick_switch::QuickSwitchState::default(),
            os_dnd_active: false,
            custom_app_draft: None,
            download_dir: None,
            download_dir_draft: None,
            downloads: Vec::new(),
            next_download_id: 1,
        };
        state.rebuild_tabs();
        state
    }

    /// The shell a launch starts in: the session at [`session::path`] restored over the user's
    /// private manifest store.
    pub fn launch() -> Self {
        let state = Self::launch_in(launch_service(), &session::path());
        // The registry is the login item's truth: the session's value is only what this shell
        // last saw. Tests build their state through `launch_in` and use a fake login item.
        #[cfg(not(test))]
        let state = {
            let mut state = state;
            if crate::autolaunch::supported() {
                state.refresh_start_at_login();
            }
            state
        };
        state
    }

    /// The session at `path` restored over `service`: what `launch` does, with the store given.
    ///
    /// A session that cannot be read or replayed is reported on stderr and the shell starts with
    /// the App Store alone; the file is left as it is until the user acts.
    pub fn launch_in(service: ApplicationService, path: &Path) -> Self {
        let file = SessionStore::new(path);
        let (mut stored, readable) = match file.load() {
            Ok(state) => (state, true),
            Err(why) => {
                eprintln!(
                    "station-shell: {why}; starting with an empty session ({} is left untouched)",
                    path.display()
                );
                (SessionState::default(), false)
            }
        };
        let active = stored.active_application.take();
        let window = std::mem::take(&mut stored.window);
        let theme = stored.theme;
        let rail_order = std::mem::take(&mut stored.rail_order);
        let identities = std::mem::take(&mut stored.identities);
        let favorites = std::mem::take(&mut stored.favorites);
        let download_dir = stored.download_dir.take();
        let include_beta_updates = stored.include_beta_updates;
        let minimize_to_tray = stored.minimize_to_tray;
        let mut service = service;
        let mut replayed = true;
        if !(stored.installed.is_empty() && stored.dock.is_empty() && stored.tabs.is_empty()) {
            if let Err(why) = service.restore(stored.installed, stored.dock, stored.tabs) {
                eprintln!(
                    "station-shell: {why}; starting with an empty session ({} is left untouched)",
                    path.display()
                );
                replayed = false;
            }
        }
        let mut state = Self::new(service);
        state.session = Some(path.to_owned());
        state.window = window;
        state.theme = theme;
        state.start_at_login = stored.start_at_login;
        state.minimize_to_tray = minimize_to_tray;
        state.include_beta_updates = include_beta_updates;
        state
            .about_state
            .set_beta_included_in_updates(include_beta_updates);
        state.identities = identities;
        state.download_dir = download_dir.clone();
        if let Some(dir) = download_dir {
            crate::downloads::set_custom_dir(Some(PathBuf::from(dir)));
        }
        if replayed {
            state.favorites = favorites;
        }
        state.save_blocked = !readable || !replayed;
        // The order is the user's when it names exactly the docked applications (the document's
        // own validation guarantees that shape); anything else falls back to install order, and
        // the next user action heals the document.
        let docked: std::collections::HashSet<&str> =
            state.store.dock().iter().map(|id| id.as_str()).collect();
        let matches = rail_order.len() == docked.len()
            && rail_order.iter().all(|id| docked.contains(id.as_str()));
        if matches {
            state.rail_order = rail_order;
            state.rebuild_tabs();
        }
        if let Some(active) = active {
            // A session naming an application it no longer has falls back to the App Store.
            state.set_active(&active);
        }
        state
    }

    /// The rail: the App Store, then every docked application in the user's order —
    /// `rail_order` when it names the docked set exactly (the slice-11 authority), install
    /// order otherwise (a fresh shell, a healed document). Install appends, uninstall drops:
    /// `reconcile_rail_order` keeps the two views in step first.
    fn rebuild_tabs(&mut self) {
        self.reconcile_rail_order();
        // Page-derived presentation (slices 20/31) survives a rebuild (an install, a config
        // change, a reorder): it is keyed by tab id, and a tab that leaves drops its entry with
        // it.
        struct PagePresentation {
            title: Option<String>,
            badge: Option<Badge>,
            favicon: Option<String>,
        }
        let previous: std::collections::HashMap<String, PagePresentation> = self
            .tabs
            .iter()
            .map(|tab| {
                (
                    tab.id.clone(),
                    PagePresentation {
                        title: tab.page_title.clone(),
                        badge: tab.badge.clone(),
                        favicon: tab.page_favicon.clone(),
                    },
                )
            })
            .collect();
        // Ids first: the service borrows end where the per-tab resolution starts.
        let docked: Vec<String> = self.rail_order.clone();
        let mut tabs = vec![AppTab::store()];
        for id in docked {
            let Some(application) = self.store.installed_applications().get(&id) else {
                continue;
            };
            // The URL the app's stored configuration resolves to (the install's own decision);
            // a record whose manifest is gone keeps its home tab's URL.
            let url = self
                .store
                .resolve_start_url(&application.manifest_url, &id, &application.config_data)
                .or_else(|| application.home_tab_url.clone())
                .unwrap_or_default();
            // The manifest's icon rides the tab so a rail row draws it without re-querying; the
            // presets decide whether Settings offers a configuration editor.
            let manifest = self.store.get_manifest_by_url(&application.manifest_url);
            let icon = manifest
                .as_ref()
                .map(|manifest| manifest.icon.clone())
                .unwrap_or_default();
            let configurable = manifest
                .as_ref()
                .map(|manifest| !manifest_registry::get_presets(&manifest.inner).is_empty())
                .unwrap_or(false);
            // The Electron session layer's two per-app inputs: a manifest UA override (the
            // `<webview useragent>` attribute) and whether the app shared the default session
            // instead of `persist:<applicationId>`.
            let user_agent = manifest
                .as_ref()
                .and_then(|manifest| manifest.inner.bx_override_user_agent.clone());
            let use_default_session = manifest
                .as_ref()
                .and_then(|manifest| manifest.inner.bx_use_default_session)
                .unwrap_or(false);
            let legacy_service_id = manifest
                .as_ref()
                .and_then(|manifest| manifest.inner.bx_legacy_service_id.clone());
            let mut tab = AppTab {
                user_agent,
                use_default_session,
                legacy_service_id,
                ..AppTab::web_full(
                    application.application_id.clone(),
                    self.label_for(&application.manifest_url, &id),
                    url,
                    icon,
                    configurable,
                )
            };
            if let Some(prev) = previous.get(&tab.id) {
                tab.page_title = prev.title.clone();
                tab.badge = prev.badge.clone();
                tab.page_favicon = prev.favicon.clone();
            }
            tabs.push(tab);
        }
        if !tabs.iter().any(|tab| tab.id == self.active) {
            self.active = APP_STORE_ID.to_owned();
        }
        self.tabs = tabs;
    }

    /// Keeps `rail_order` in step with the dock before the rail builds from it: an installed
    /// (docked) application the order does not name is appended (install), an order entry the
    /// dock no longer has is dropped (uninstall). Dock-less installs (`bx_no_dock`) never join.
    /// This is the one writer of membership on the order; the user's reordering never changes
    /// the set, only the sequence.
    fn reconcile_rail_order(&mut self) {
        let docked: std::collections::HashSet<String> = self.store.dock().iter().cloned().collect();
        self.rail_order.retain(|id| docked.contains(id));
        for id in self.store.dock() {
            if !self.rail_order.contains(id) {
                self.rail_order.push(id.clone());
            }
        }
    }

    /// Moves `application_id` one step through the rail: earlier when `up`, later otherwise.
    /// The App Store row is the rail's first and never moves, so the first app cannot go up and
    /// the last cannot go down. A move is a user action: the session is written at once (D7).
    /// False (and no write) when the application is unknown, is the store row, or sits at the
    /// edge. The active tab is untouched: reordering changes where a row sits, not what shows.
    pub fn move_rail_row(&mut self, application_id: &str, up: bool) -> bool {
        let Some(index) = self.rail_order.iter().position(|id| id == application_id) else {
            return false;
        };
        // The store row is above every app row (it lives in `self.tabs`, not in the order): the
        // first app cannot move up. The last app cannot move down.
        let target = if up {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        let Some(target) = target else { return false };
        if target >= self.rail_order.len() {
            return false;
        }
        self.rail_order.swap(index, target);
        self.rebuild_tabs();
        self.persist();
        true
    }

    /// The rail order as it is now (tests: the drawn rail is the one view that shows it).
    #[cfg(test)]
    pub fn rail_order(&self) -> &[String] {
        &self.rail_order
    }

    /// An installed application's name, from its manifest; the id when the manifest is gone (the
    /// install's own name covers the usual path, this is the rebuilt-rail path).
    fn label_for(&self, manifest_url: &str, id: &str) -> String {
        self.store
            .get_manifest_by_url(manifest_url)
            .and_then(|manifest| manifest.inner.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| id.to_owned())
    }

    pub fn tabs(&self) -> &[AppTab] {
        &self.tabs
    }

    pub fn active(&self) -> &AppTab {
        self.tabs
            .iter()
            .find(|tab| tab.id == self.active)
            .unwrap_or(&self.tabs[0])
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active == id
    }

    /// Makes the tab `id` the active one and remembers it; false when the rail has no such tab.
    ///
    /// Re-selecting the tab that is already active is a valid no-op and **writes nothing**: a
    /// routed `Focus` flood for the same tab must not fsync per request.
    pub fn select(&mut self, id: &str) -> bool {
        let previous = self.active.clone();
        if !self.set_active(id) {
            return false;
        }
        self.tab_menu = None;
        if self.active != previous {
            if self.find.open {
                self.find.open = false;
                self.find.reset_matches();
                crate::webview::eval_in_tab(&previous, crate::find::CLEAR_SCRIPT);
            }
            if !self.active().renders_in_rune() {
                let frec_id = format!("tab:{}", self.active);
                self.frecency.save(activity::frecency::SaveParams {
                    search_query: None,
                    selected_id: &frec_id,
                    date_selection: None,
                });
            }
            self.persist();
        }
        true
    }

    /// Pinned favorites persisted in the session document.
    pub fn favorites(&self) -> &[session_store::FavoriteEntry] {
        &self.favorites
    }

    /// True if `tab_id` currently has a pinned favorite.
    pub fn is_favorite(&self, tab_id: &str) -> bool {
        self.favorites
            .iter()
            .any(|fav| fav.application_id == tab_id)
    }

    /// Pins or unpins `tab_id` as a favorite and persists the session immediately.
    pub fn toggle_favorite_for_tab(&mut self, tab_id: &str) -> bool {
        if let Some(pos) = self
            .favorites
            .iter()
            .position(|fav| fav.application_id == tab_id)
        {
            self.favorites.remove(pos);
            self.persist();
            return true;
        }
        if self.favorites.len() >= session_store::MAX_FAVORITES {
            return false;
        }
        let Some(tab) = self
            .tabs
            .iter()
            .find(|t| t.id == tab_id && t.kind == TabKind::Web)
        else {
            return false;
        };
        let title: String = tab
            .display_label()
            .chars()
            .filter(|c| !c.is_control())
            .take(session_store::MAX_FAVORITE_FIELD_CHARS)
            .collect();
        let url: String = tab
            .url
            .chars()
            .filter(|c| !c.is_control())
            .take(session_store::MAX_FAVORITE_FIELD_CHARS)
            .collect();
        if title.trim().is_empty() || url.trim().is_empty() {
            return false;
        }
        self.favorites.push(session_store::FavoriteEntry {
            id: format!("fav-{tab_id}"),
            application_id: tab_id.to_owned(),
            title,
            url,
        });
        self.persist();
        true
    }

    /// Opens the Bang Quick Switcher palette (`Ctrl+K` / `Ctrl+T`).
    pub fn open_quick_switch(&mut self) {
        self.close_find();
        self.close_tab_menu();
        self.surface = Surface::QuickSwitch;
        self.config_draft = None;
        self.notifications
            .apply(NotificationAction::SetVisibility { visible: false });
        self.quick_switch.reset();
    }

    /// Current Quick Switcher palette state.
    pub fn quick_switch(&self) -> &crate::quick_switch::QuickSwitchState {
        &self.quick_switch
    }

    /// Mutable access to the Quick Switcher search field.
    pub fn quick_switch_edit_mut(&mut self) -> &mut TextEdit {
        &mut self.quick_switch.edit
    }

    /// Resets the highlighted Quick Switcher row back to the first result after query edits.
    pub fn reset_quick_switch_selection(&mut self) {
        self.quick_switch.selected = 0;
    }

    /// Moves Quick Switcher selection up (`-1`) or down (`+1`), wrapping across entries.
    pub fn move_quick_switch_selection(&mut self, delta: isize) {
        let len = crate::quick_switch::entries_for(self).len();
        if len == 0 {
            self.quick_switch.selected = 0;
            return;
        }
        let current = self.quick_switch.selected.min(len - 1) as isize;
        let next = (current + delta).rem_euclid(len as isize) as usize;
        self.quick_switch.selected = next;
    }

    /// Activates the currently highlighted Quick Switcher entry (Enter).
    pub fn submit_quick_switch(&mut self) {
        let entries = crate::quick_switch::entries_for(self);
        if entries.is_empty() {
            return;
        }
        let idx = self.quick_switch.selected.min(entries.len() - 1);
        let entry = entries[idx].clone();
        self.activate_quick_switch_entry(&entry.frecency_id, entry.action);
    }

    /// Records the Quick Switcher selection in `activity::frecency` and executes its action.
    pub fn activate_quick_switch_entry(
        &mut self,
        frecency_id: &str,
        action: crate::quick_switch::QuickSwitchAction,
    ) {
        let query = self.quick_switch.query().to_owned();
        self.frecency.save(activity::frecency::SaveParams {
            search_query: (!query.is_empty()).then_some(query.as_str()),
            selected_id: frecency_id,
            date_selection: None,
        });
        match action {
            crate::quick_switch::QuickSwitchAction::OpenFavorite {
                favorite_id: _,
                application_id,
                url,
            } => {
                self.close_overlay();
                self.select(&application_id);
                if !url.is_empty() && self.tab_url(&application_id) != Some(url.as_str()) {
                    crate::webview::load_if_attached(&application_id, &url);
                }
            }
            crate::quick_switch::QuickSwitchAction::SelectTab { tab_id } => {
                self.close_overlay();
                self.select(&tab_id);
            }
            crate::quick_switch::QuickSwitchAction::OpenStore => {
                self.close_overlay();
                self.open_app_store();
            }
            crate::quick_switch::QuickSwitchAction::OpenSettings => {
                self.open_settings();
            }
            crate::quick_switch::QuickSwitchAction::OpenNotifications => {
                self.surface = Surface::Apps;
                self.toggle_notifications();
            }
            crate::quick_switch::QuickSwitchAction::OpenAbout => {
                self.open_about();
            }
            crate::quick_switch::QuickSwitchAction::InstallApp { manifest_url } => {
                self.close_overlay();
                self.install(&manifest_url);
            }
        }
    }

    /// Computes a frecency score for `result_id` under `query`.
    pub fn frecency_score(&self, query: &str, result_id: &str) -> f64 {
        self.frecency.compute_score(query, result_id)
    }

    /// Searches the App Store catalog (public + private manifests) for Quick Switcher install hits.
    pub fn search_catalog(&self, query: &str) -> Vec<MinimalApplication> {
        search_applications(query, &self.private)
    }

    /// Current in-tab find bar state (`Ctrl+F` / `Cmd+F`).
    pub fn find(&self) -> &crate::find::FindState {
        &self.find
    }

    /// Mutable reference to the find bar's `TextEdit`.
    pub fn find_edit_mut(&mut self) -> &mut TextEdit {
        &mut self.find.edit
    }

    /// Opens the in-tab find bar when a web page is visible; re-runs any non-empty query.
    pub fn open_find(&mut self) -> bool {
        if !self.shows_page() {
            return false;
        }
        self.find.open = true;
        if !self.find.query().is_empty() {
            self.dispatch_find(true);
        }
        true
    }

    /// Closes the in-tab find bar and clears highlights in the active web page.
    pub fn close_find(&mut self) -> bool {
        if !self.find.open {
            return false;
        }
        self.find.open = false;
        self.find.reset_matches();
        if !self.active().renders_in_rune() {
            let tab_id = self.active.clone();
            crate::webview::eval_in_tab(&tab_id, crate::find::CLEAR_SCRIPT);
        }
        true
    }

    /// Dispatches the current find query (`forward` = next/prev) to the active web tab.
    pub fn dispatch_find(&mut self, forward: bool) {
        if !self.shows_page() {
            return;
        }
        let tab_id = self.active.clone();
        if self.find.query().is_empty() {
            self.find.reset_matches();
            crate::webview::eval_in_tab(&tab_id, crate::find::CLEAR_SCRIPT);
            return;
        }
        if let Some(script) = crate::find::find_script(self.find.query(), forward) {
            crate::webview::eval_in_tab(&tab_id, &script);
        }
    }

    /// Applies match counts reported by the page over IPC (`kind: "find"`).
    pub fn set_find_matches(&mut self, tab_id: &str, current: u32, total: u32) -> bool {
        if !self.find.open || self.active != tab_id {
            return false;
        }
        self.find.set_matches(current, total)
    }

    /// Selects a tab by 1-based rail index (`Cmd/Ctrl+1..9`), closing any open overlay.
    pub fn select_by_index(&mut self, one_based: usize) -> bool {
        let Some(index) = one_based.checked_sub(1) else {
            return false;
        };
        let Some(tab_id) = self.tabs.get(index).map(|tab| tab.id.clone()) else {
            return false;
        };
        if self.surface != Surface::Apps {
            self.close_overlay();
        }
        self.select(&tab_id)
    }

    /// Per-tab zoom factor (`0.5..=3.0`, defaults to `1.0`).
    pub fn zoom_of(&self, tab_id: &str) -> f64 {
        self.zoom.get(tab_id).copied().unwrap_or(1.0)
    }

    /// Adjusts `tab_id`'s zoom factor by `delta`, clamped to `0.5..=3.0`, returning the new value.
    pub fn adjust_zoom(&mut self, tab_id: &str, delta: f64) -> f64 {
        let current = self.zoom_of(tab_id);
        let next = ((current + delta) * 10.0).round() / 10.0;
        let clamped = next.clamp(0.5, 3.0);
        self.zoom.insert(tab_id.to_owned(), clamped);
        clamped
    }

    /// Resets `tab_id`'s zoom factor to `1.0`.
    pub fn reset_zoom(&mut self, tab_id: &str) -> f64 {
        self.zoom.insert(tab_id.to_owned(), 1.0);
        1.0
    }

    /// The tab id whose Rune tab-actions strip is currently open, if any.
    pub fn tab_menu_for(&self) -> Option<&str> {
        self.tab_menu.as_deref()
    }

    /// Toggles the tab-actions strip for a web tab (right-click on rail or top-bar menu button).
    pub fn toggle_tab_menu(&mut self, tab_id: &str) -> bool {
        if self.tab_menu.as_deref() == Some(tab_id) {
            self.tab_menu = None;
            return true;
        }
        if !self
            .tabs
            .iter()
            .any(|tab| tab.id == tab_id && tab.kind == TabKind::Web)
        {
            return false;
        }
        self.tab_menu = Some(tab_id.to_owned());
        true
    }

    /// Closes the tab-actions strip; returns true if one was open.
    pub fn close_tab_menu(&mut self) -> bool {
        self.tab_menu.take().is_some()
    }

    /// Whether `application_id` can move up (`up == true`) or down (`up == false`) in the rail.
    pub fn can_move_rail_row(&self, application_id: &str, up: bool) -> bool {
        let Some(index) = self.rail_order.iter().position(|id| id == application_id) else {
            return false;
        };
        if up {
            index > 0
        } else {
            index + 1 < self.rail_order.len()
        }
    }

    /// `select` without writing the session: launch restores the active application with it.
    fn set_active(&mut self, id: &str) -> bool {
        if !self.tabs.iter().any(|tab| tab.id == id) {
            return false;
        }
        self.active = id.to_owned();
        true
    }

    /// The rail's "add app" affordance opens the App Store, where apps are added.
    pub fn open_app_store(&mut self) {
        self.select(APP_STORE_ID);
    }

    /// Installs the application whose manifest `manifest_url` names, adds its rail row, selects
    /// it and writes the session. False when the manifest is unknown or the install failed.
    pub fn install(&mut self, manifest_url: &str) -> bool {
        let installed = match self.store.install(manifest_url, &InstallOptions::default()) {
            Ok(installed) => installed,
            Err(_) => return false,
        };
        self.rebuild_tabs();
        // A `bx_no_dock` install is remembered but has no rail row to select; the App Store stays
        // the visible tab, which is what a dock-less application would do.
        if self
            .tabs
            .iter()
            .any(|tab| tab.id == installed.application_id)
        {
            self.active = installed.application_id;
        }
        self.persist();
        true
    }

    /// Removes the installed application and its rail row. The caller closes its page with
    /// [`crate::webview::close_tab`]; state holds no host. False when it was not installed.
    pub fn uninstall(&mut self, application_id: &str) -> bool {
        if self.store.uninstall(application_id).is_err() {
            return false;
        }
        if self.tab_menu.as_deref() == Some(application_id) {
            self.tab_menu = None;
        }
        self.zoom.remove(application_id);
        self.favorites
            .retain(|fav| fav.application_id != application_id);
        // Rebuilding drops the row and, when it was the active tab, falls back to the App Store.
        self.rebuild_tabs();
        self.persist();
        true
    }

    /// Presses a store tile: an uninstalled app is installed and selected, an installed one is
    /// removed (the settings surface owns removal next slice; the tile makes it reachable now).
    pub fn activate_tile(&mut self, tile: &MinimalApplication) -> TileAction {
        if let Some(application_id) = self.installed_application_id(&tile.bx_app_manifest_url) {
            return if self.uninstall(&application_id) {
                TileAction::Uninstalled { application_id }
            } else {
                TileAction::Ignored
            };
        }
        if self.install(&tile.bx_app_manifest_url) {
            TileAction::Installed
        } else {
            TileAction::Ignored
        }
    }

    /// Whether an application installed from `manifest_url` is in the rail: the tile's marker.
    pub fn is_installed(&self, manifest_url: &str) -> bool {
        self.installed_application_id(manifest_url).is_some()
    }

    /// An installed application's stored configuration (empty when not configured, or when the
    /// id is unknown).
    pub fn config_of(&self, application_id: &str) -> AppConfigEntry {
        let Some(installed) = self.store.installed_applications().get(application_id) else {
            return AppConfigEntry::default();
        };
        AppConfigEntry {
            identity_id: installed.config_data.identity_id.clone(),
            subdomain: installed.config_data.subdomain.clone(),
            custom_url: installed.config_data.custom_url.clone(),
        }
    }

    /// Saves an installed application's configuration: the service decides what its presets
    /// keep, the tab's URL is re-resolved from it, and the session is written at once (D7).
    /// False when the id is unknown, the manifest is gone, or the value is not usable (over the
    /// document cap) — a refused value changes nothing, in memory or on disk.
    pub fn set_app_config(&mut self, application_id: &str, config: AppConfigEntry) -> bool {
        // The cap is checked on the value as given: `trimmed` would silently cut a hostile one
        // to size, and the document's own rule is refuse, not truncate.
        if config
            .values()
            .into_iter()
            .flatten()
            .any(|value| value.chars().count() > session_store::MAX_CONFIG_VALUE_CHARS)
        {
            return false;
        }
        let config = trimmed(config);
        if self
            .store
            .set_config_data(application_id, &to_service_config(&config))
            .is_err()
        {
            return false;
        }
        self.re_resolve_tab_url(application_id);
        self.persist();
        true
    }

    /// The accounts the session stores, keyed by `identityId` (slices 17 / 25).
    pub fn identities(&self) -> &BTreeMap<String, Identity> {
        &self.identities
    }

    /// Adds or replaces a stored identity (zero tokens) and persists the session immediately.
    /// Returns false if the session identity cap or field bounds would be exceeded.
    pub fn add_identity(&mut self, identity: Identity) -> bool {
        if identity.identity_id.is_empty()
            || (!self.identities.contains_key(&identity.identity_id)
                && self.identities.len() >= session_store::MAX_IDENTITIES)
        {
            return false;
        }
        let fields = [
            identity.identity_id.as_str(),
            identity.provider.as_str(),
            identity.user_id.as_str(),
            identity.profile_data.display_name.as_str(),
            identity.profile_data.first_name.as_str(),
            identity.profile_data.last_name.as_str(),
            identity.profile_data.email.as_str(),
            identity.profile_data.image_url.as_str(),
        ];
        if fields
            .iter()
            .any(|v| v.chars().count() > session_store::MAX_IDENTITY_FIELD_CHARS)
        {
            return false;
        }
        self.identities
            .insert(identity.identity_id.clone(), identity);
        self.persist();
        true
    }

    /// Removes a stored account by `identity_id`, clears any app config referencing it, and
    /// persists the session immediately.
    pub fn remove_identity(&mut self, identity_id: &str) -> bool {
        if self.identities.remove(identity_id).is_none() {
            return false;
        }
        let affected: Vec<String> = self
            .store
            .installed_applications()
            .values()
            .filter(|app| app.config_data.identity_id.as_deref() == Some(identity_id))
            .map(|app| app.application_id.clone())
            .collect();
        for app_id in affected {
            let _ = self.store.clear_config_data(&app_id);
        }
        self.rebuild_tabs();
        self.persist();
        true
    }

    /// True when `GOOGLE_CLIENT_ID` is configured for native Google sign-in.
    pub fn oauth_configured(&self) -> bool {
        self.oauth_config.is_some()
    }

    /// Overrides the OAuth configuration in tests without mutating process env vars.
    #[cfg(test)]
    pub fn set_oauth_config_for_tests(&mut self, config: Option<crate::oauth::OAuthConfig>) {
        self.oauth_config = config;
    }

    /// True while a Google sign-in flow is awaiting the browser callback or token exchange.
    pub fn oauth_in_progress(&self) -> bool {
        crate::oauth::is_in_progress()
    }

    /// Transient sign-in status or error text shown in Settings.
    pub fn oauth_status(&self) -> Option<&str> {
        self.oauth_status.as_deref()
    }

    /// Starts Google sign-in in the system browser using PKCE + loopback HTTP callback.
    pub fn start_google_sign_in(&mut self) {
        let _ = self.start_google_sign_in_with(
            std::sync::Arc::new(crate::oauth::GoogleTokenExchanger),
            crate::routing::open_external,
        );
    }

    /// Testable seam for starting Google sign-in with an injected token exchanger & browser opener.
    pub fn start_google_sign_in_with(
        &mut self,
        exchanger: std::sync::Arc<dyn crate::oauth::TokenExchanger>,
        open_browser: impl FnOnce(&str),
    ) -> Result<String, crate::oauth::OAuthError> {
        let Some(config) = self.oauth_config.clone() else {
            let err = crate::oauth::OAuthError::NotConfigured;
            self.oauth_status = Some(err.to_string());
            return Err(err);
        };
        match crate::oauth::start_google_sign_in(
            config,
            exchanger,
            open_browser,
            crate::routing::current_waker(),
        ) {
            Ok(url) => {
                self.oauth_status =
                    Some("Waiting for Google sign-in in your browser...".to_owned());
                Ok(url)
            }
            Err(err) => {
                self.oauth_status = Some(err.to_string());
                Err(err)
            }
        }
    }

    /// Cancels an in-progress Google sign-in flow.
    pub fn cancel_google_sign_in(&mut self) {
        if crate::oauth::cancel_sign_in() {
            self.oauth_status = Some("Sign-in cancelled.".to_owned());
        }
    }

    /// Drains a completed background OAuth sign-in result into the session (called by `poll_at`).
    pub fn poll_oauth(&mut self) -> bool {
        let Some(outcome) = crate::oauth::take_completed() else {
            return false;
        };
        match outcome {
            Ok(identity) => {
                let label = if !identity.profile_data.email.is_empty() {
                    identity.profile_data.email.clone()
                } else {
                    identity.identity_id.clone()
                };
                if self.add_identity(identity) {
                    self.oauth_status = Some(format!("Signed in as {label}."));
                } else {
                    self.oauth_status = Some(
                        "Account limit reached; remove an existing account before signing in."
                            .to_owned(),
                    );
                }
            }
            Err(err) => {
                self.oauth_status = Some(err.to_string());
            }
        }
        true
    }

    /// Removes an installed application's configuration: the tab reverts to the manifest's own
    /// start URL and the session is written at once. False when there was nothing to clear.
    pub fn clear_app_config(&mut self, application_id: &str) -> bool {
        match self.store.clear_config_data(application_id) {
            Ok(true) => {}
            _ => return false,
        }
        self.re_resolve_tab_url(application_id);
        self.persist();
        true
    }

    /// Re-points a configured app's rail tab at the URL its configuration now resolves to. The
    /// rail is rebuilt from the service's records, so install and launch take the same path.
    /// A missing manifest (uninstalled mid-edit, a private manifest deleted underneath the
    /// session) keeps the tab's last URL: the record's home tab is the fallback.
    fn re_resolve_tab_url(&mut self, _application_id: &str) {
        self.rebuild_tabs();
    }

    /// A rail tab's current URL, by tab id (tests and the settings surface use it to see where a
    /// configured app points now).
    pub fn tab_url(&self, application_id: &str) -> Option<&str> {
        self.tabs
            .iter()
            .find(|tab| tab.id == application_id)
            .map(|tab| tab.url.as_str())
    }

    fn installed_application_id(&self, manifest_url: &str) -> Option<String> {
        self.store
            .installed_applications()
            .values()
            .find(|application| application.manifest_url == manifest_url)
            .map(|application| application.application_id.clone())
    }

    pub fn section(&self) -> StoreSection {
        self.section
    }

    pub fn show_section(&mut self, section: StoreSection) {
        self.section = section;
        if section != StoreSection::MyCustomApps {
            self.custom_app_draft = None;
        }
    }

    /// Open inline custom-app draft while editing in `App Store -> My custom apps`.
    pub fn custom_app_draft(&self) -> Option<&CustomAppDraft> {
        self.custom_app_draft.as_ref()
    }

    /// Mutable access to the open custom-app draft for Rune `text_input` fields.
    pub fn custom_app_draft_mut(&mut self) -> Option<&mut CustomAppDraft> {
        self.custom_app_draft.as_mut()
    }

    /// Opens the inline "+ Add custom app" form inside `App Store -> My custom apps`.
    pub fn open_custom_app_form(&mut self) {
        self.section = StoreSection::MyCustomApps;
        self.search = TextEdit::new("");
        self.results.clear();
        self.custom_app_draft = Some(CustomAppDraft::default());
    }

    /// Closes the inline custom-app form, discarding unsaved input. True if it was open.
    pub fn close_custom_app_form(&mut self) -> bool {
        self.custom_app_draft.take().is_some()
    }

    /// Validates the open custom-app draft, saves it to the private-manifest store, refreshes
    /// the App Store catalog, and installs the new app into the rail. On validation or disk
    /// error the draft stays open with its `error` populated.
    pub fn submit_custom_app_draft(&mut self) -> bool {
        let Some(draft) = self.custom_app_draft.as_ref() else {
            return false;
        };
        let recipe = match validate_custom_app_draft(draft) {
            Ok(recipe) => recipe,
            Err(why) => {
                if let Some(d) = self.custom_app_draft.as_mut() {
                    d.error = Some(why);
                }
                return false;
            }
        };
        let created = match self.store.request_private_application(recipe) {
            Ok(created) => created,
            Err(why) => {
                if let Some(d) = self.custom_app_draft.as_mut() {
                    d.error = Some(why.to_string());
                }
                return false;
            }
        };
        self.custom_app_draft = None;
        self.refresh_store_catalog();
        self.install(&created.bx_app_manifest_url)
    }

    /// Uninstalls every installed instance of a custom app manifest and deletes it from the
    /// private-manifest store. Returns removed application ids so the caller closes their pages
    /// via [`crate::webview::close_tab`].
    pub fn delete_custom_app(&mut self, id: u64, manifest_url: &str) -> Vec<String> {
        let mut removed: Vec<String> = self
            .store
            .installed_applications()
            .values()
            .filter(|app| app.manifest_url == manifest_url)
            .map(|app| app.application_id.clone())
            .collect();
        removed.sort();
        if let Err(why) = self.store.uninstall_all_for_manifest(manifest_url) {
            eprintln!("station-shell: could not uninstall custom app {id}: {why}");
            removed.clear();
        }
        for app_id in &removed {
            if self.tab_menu.as_deref() == Some(app_id.as_str()) {
                self.tab_menu = None;
            }
            self.zoom.remove(app_id);
            self.favorites.retain(|fav| fav.application_id != *app_id);
        }
        if let Err(why) = self.store.delete_private_manifest(id) {
            eprintln!("station-shell: could not delete custom app {id}: {why}");
        }
        self.refresh_store_catalog();
        self.rebuild_tabs();
        self.persist();
        removed
    }

    /// Recomputes cached store lists (`private`, `custom`, `popular`, `categories`, `results`)
    /// after adding or removing a private manifest.
    fn refresh_store_catalog(&mut self) {
        let private = self.store.private_manifests();
        let popular = get_most_popular_apps(&private);
        let grouped = get_applications_by_category(&private);
        let categories = get_all_categories(&private)
            .into_iter()
            .filter_map(|category| {
                let apps = grouped
                    .iter()
                    .find(|(name, _)| *name == category)
                    .map(|(_, apps)| apps.clone())
                    .unwrap_or_default();
                (!apps.is_empty()).then_some((category, apps))
            })
            .collect();
        let custom = private
            .iter()
            .map(manifest_to_minimal_application)
            .collect();
        self.private = private;
        self.popular = popular;
        self.categories = categories;
        self.custom = custom;
        self.refresh_search();
    }

    pub fn search(&self) -> &TextEdit {
        &self.search
    }

    /// The field's text, as the store queries see it.
    pub fn query(&self) -> &str {
        &self.search.text
    }

    pub fn search_mut(&mut self) -> &mut TextEdit {
        &mut self.search
    }

    /// Re-runs the search over every listable application; the field calls this on each change,
    /// so the view never searches.
    pub fn refresh_search(&mut self) {
        let query = self.search.text.trim();
        self.results = if query.is_empty() {
            Vec::new()
        } else {
            search_applications(query, &self.private)
        };
    }

    pub fn results(&self) -> &[MinimalApplication] {
        &self.results
    }

    /// Queues the fetch for every app the shell can draw an icon for (the rail's tabs, live page
    /// favicons, and the store's lists). Called from the frame poll — the view never mutates
    /// state, so the asking happens here; once per URL per run.
    pub fn prefetch_icons(&mut self) {
        for tab in &self.tabs {
            if let Some(favicon) = tab.page_favicon.as_deref() {
                self.icons.pixels(favicon);
            }
            self.icons.pixels(&tab.icon);
        }
        // The notification panel's rows, only while it is the surface (slice 16). The panel
        // module owns which rows are drawn and the per-frame bound
        // (`MAX_NOTIFICATION_ICON_URLS`); this is the one ask path, the store dedupes.
        //
        // Before the store's lists: [`crate::icons::MAX_INFLIGHT`] is a drop-not-defer queue,
        // and a cold store asks for every listable app's icon at once. What the user is looking
        // at (the panel) must not lose its asks to off-screen store tiles; the store's own
        // bound already drops its tail on a cold frame and asks it on the next poll.
        if self.surface == Surface::Notifications {
            for url in crate::notifications::icon_urls(&self.notifications) {
                self.icons.pixels(&url);
            }
        }
        for url in self.store_list_icon_urls() {
            self.icons.pixels(&url);
        }
    }

    /// The icon URLs the store's cached lists and current results draw.
    fn store_list_icon_urls(&self) -> Vec<String> {
        let mut urls: Vec<String> = Vec::new();
        urls.extend(
            self.popular
                .cream_of_the_crop_apps
                .iter()
                .map(|a| a.icon_url.clone()),
        );
        urls.extend(self.popular.runner_ups.iter().map(|a| a.icon_url.clone()));
        urls.extend(self.popular.noteworthy.iter().map(|a| a.icon_url.clone()));
        for (_, category) in &self.categories {
            urls.extend(category.iter().map(|a| a.icon_url.clone()));
        }
        urls.extend(self.custom.iter().map(|a| a.icon_url.clone()));
        urls.extend(self.results.iter().map(|a| a.icon_url.clone()));
        urls
    }

    /// The pixels drawing `url`'s icon, or `None` — no icon, a URL the shell refuses, or a
    /// fetch that has not landed (or failed): the view draws its letter avatar.
    pub fn icon_pixels(&self, url: &str) -> Option<&rune::Pixels> {
        self.icons.pixels_read(url)
    }

    /// The pixels drawing a rail tab: the decoded live page favicon when present and ready,
    /// falling back to the manifest icon (and then the letter avatar when neither is decoded).
    pub fn tab_icon_pixels(&self, tab: &AppTab) -> Option<&rune::Pixels> {
        tab.page_favicon
            .as_deref()
            .and_then(|url| self.icons.pixels_read(url))
            .or_else(|| self.icons.pixels_read(&tab.icon))
    }

    /// The letter-avatar identity for `application_id` (the notification panel's row icons):
    /// the manifest's name and `theme_color`, or the id with no tint when the application or
    /// its manifest is gone — `icons::avatar` then derives the accent from the id, exactly as
    /// the rail's rows and the store's tiles do.
    pub fn avatar_identity(&self, application_id: &str) -> (String, String) {
        let Some(installed) = self.store.installed_applications().get(application_id) else {
            return (application_id.to_owned(), String::new());
        };
        let Some(manifest) = self.store.get_manifest_by_url(&installed.manifest_url) else {
            return (application_id.to_owned(), String::new());
        };
        let name = manifest
            .inner
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| application_id.to_owned());
        (name, manifest.inner.theme_color.clone().unwrap_or_default())
    }

    /// Replaces the icons store wholesale (tests: a store with a temp cache dir or canned
    /// fetch responses).
    #[cfg(test)]
    pub fn set_icons(&mut self, icons: crate::icons::IconStore) {
        self.icons = icons;
    }

    /// Starts the icons worker: `AppSpec::setup`, once, with Rune's waker and the HTTP fetcher.
    pub fn start_icons(&mut self, waker: rune::Waker) {
        self.icons.start(Box::new(crate::icons::HttpFetcher), waker);
    }

    /// Drains finished icon jobs into their handles; true when pixels changed (the poll's
    /// "redraw" for the views that draw icons).
    pub fn drain_icons(&mut self) -> bool {
        if !self.icons.has_jobs() {
            return false;
        }
        self.icons.drain()
    }

    pub fn popular(&self) -> &PopularApps {
        &self.popular
    }

    pub fn categories(&self) -> &[(String, Vec<MinimalApplication>)] {
        &self.categories
    }

    pub fn custom(&self) -> &[MinimalApplication] {
        &self.custom
    }

    /// A shell whose notification center already holds `center` (headless tests only: nothing
    /// produces notifications at runtime yet).
    #[cfg(test)]
    pub fn with_notifications(mut self, center: NotificationCenter) -> Self {
        self.notifications = center;
        self
    }

    /// Re-points a rail tab at `url` for this session: a routed link that belongs to that app
    /// navigates its page, and a page not attached yet opens there. The session still restores
    /// the app's own home URL (the store's tab record is unchanged).
    pub fn retarget(&mut self, tab_id: &str, url: &str) -> bool {
        match self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            Some(tab) => {
                tab.url = url.to_owned();
                true
            }
            None => false,
        }
    }

    /// A page's own `document.title` for its tab (slice 20). The bridge already trimmed and
    /// bounded it; an empty title clears the override so the manifest label returns. True when
    /// the drawn label changed. Never persisted: the session document stores no page titles.
    pub fn set_page_title(&mut self, tab_id: &str, title: &str) -> bool {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id && tab.kind == TabKind::Web)
        else {
            return false;
        };
        let next = (!title.is_empty()).then(|| title.to_owned());
        if tab.page_title == next {
            return false;
        }
        tab.page_title = next;
        true
    }

    /// A page's `browserx-badge` meta (`Some`) or its removal (`None`) for its tab. The raw
    /// string is normalized by `tab-model`'s TS rule (a base-10 integer prefix is a count,
    /// anything else is text). True when the drawn badge changed. Never persisted.
    pub fn set_page_badge(&mut self, tab_id: &str, badge: Option<&str>) -> bool {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id && tab.kind == TabKind::Web)
        else {
            return false;
        };
        let next = badge.and_then(normalize_page_badge);
        if tab.badge == next {
            return false;
        }
        tab.badge = next;
        true
    }

    /// A page's `<link rel~="icon">` URL (`Some`) or its removal (`None`) for its tab (Slice 31).
    /// Non-fetchable URLs are ignored so the current icon stays intact. True when changed; never
    /// persisted.
    pub fn set_page_favicon(&mut self, tab_id: &str, url: Option<&str>) -> bool {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id && tab.kind == TabKind::Web)
        else {
            return false;
        };
        let next = match url {
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    None
                } else if crate::icons::is_fetchable(trimmed) {
                    Some(trimmed.to_owned())
                } else {
                    return false;
                }
            }
            None => None,
        };
        if tab.page_favicon == next {
            return false;
        }
        tab.page_favicon = next;
        true
    }

    /// The installed applications in `url-router`'s shape: manifest scope, extended scopes, and
    /// the on-premise custom URL, for deciding which app answers to a link.
    pub fn routing_apps(&self) -> Vec<InstalledApp> {
        let mut apps = Vec::new();
        for (application_id, installed) in self.store.installed_applications() {
            let Some(manifest) = self.store.get_manifest_by_url(&installed.manifest_url) else {
                continue;
            };
            apps.push(InstalledApp {
                application_id: application_id.clone(),
                manifest_url: installed.manifest_url.clone(),
                scope: manifest.inner.scope.clone(),
                extended_scopes: manifest.inner.extended_scopes.clone(),
                custom_urls: installed
                    .config_data
                    .custom_url
                    .clone()
                    .into_iter()
                    .collect(),
                start_url: manifest.inner.start_url.clone(),
                single_page: manifest.inner.bx_single_page.unwrap_or(false),
            });
        }
        apps
    }

    /// The rail's tabs in `url-router`'s shape (the App Store is not a page, so it is not one).
    pub fn routing_tabs(&self) -> Vec<OpenTab> {
        self.tabs
            .iter()
            .skip(1)
            .map(|tab| OpenTab {
                tab_id: tab.id.clone(),
                application_id: tab.id.clone(),
                url: tab.url.clone(),
                is_home: true,
            })
            .collect()
    }

    /// The configuration editor's draft, while Settings shows one.
    pub fn config_draft(&self) -> Option<&ConfigDraft> {
        self.config_draft.as_ref()
    }

    /// The draft's field, for `text_input`'s accessor.
    pub fn config_draft_mut(&mut self) -> Option<&mut ConfigDraft> {
        self.config_draft.as_mut()
    }

    /// Opens the editor for an installed application: the one field its manifest's presets
    /// consume, prefilled with the stored value. A google-account preset is shown honestly as
    /// unavailable (the shell has no OAuth), with no stub field.
    ///
    /// False when the id is unknown or the manifest is gone — no editor.
    pub fn open_config_editor(&mut self, application_id: &str) -> bool {
        let Some(installed) = self.store.installed_applications().get(application_id) else {
            return false;
        };
        let Some(manifest) = self.store.get_manifest_by_url(&installed.manifest_url) else {
            return false;
        };
        let Some(field) = config_field_for(&manifest.inner) else {
            return false;
        };
        let stored = self.config_of(application_id);
        let text = match field {
            ConfigField::Identity => stored.identity_id.clone(),
            ConfigField::CustomUrl => stored.custom_url.clone(),
            ConfigField::Subdomain => stored.subdomain.clone(),
        }
        .unwrap_or_default();
        self.config_draft = Some(ConfigDraft {
            application_id: application_id.to_owned(),
            field,
            edit: TextEdit::new(text),
        });
        true
    }

    /// Drops the draft (Back, Escape, Open, Uninstall) without writing anything.
    pub fn close_config_editor(&mut self) {
        self.config_draft = None;
    }

    /// Saves the draft through [`ShellState::set_app_config`] and closes the editor.
    pub fn save_config_draft(&mut self) {
        let Some(draft) = &self.config_draft else {
            return;
        };
        let application_id = draft.application_id.clone();
        let field = draft.field;
        let text = draft.edit.text.trim().to_owned();
        let entry = match field {
            ConfigField::Identity => AppConfigEntry {
                identity_id: (!text.is_empty()).then_some(text),
                ..AppConfigEntry::default()
            },
            ConfigField::CustomUrl => AppConfigEntry {
                custom_url: (!text.is_empty()).then_some(text),
                ..AppConfigEntry::default()
            },
            ConfigField::Subdomain => AppConfigEntry {
                subdomain: (!text.is_empty()).then_some(text),
                ..AppConfigEntry::default()
            },
        };
        self.set_app_config(&application_id, entry);
        self.config_draft = None;
    }

    /// Clears an installed application's configuration and closes the editor. True when
    /// something was configured (the session moved).
    pub fn clear_config_draft(&mut self) -> bool {
        let Some(draft) = &self.config_draft else {
            return false;
        };
        let application_id = draft.application_id.clone();
        self.config_draft = None;
        self.clear_app_config(&application_id)
    }

    /// Failed writes since the last success (tests observe the retry/log throttling).
    #[cfg(test)]
    pub fn save_failures(&self) -> u32 {
        self.save_failures
    }

    /// The session file this shell writes, or `None` when it is not persisting (a test shell).
    pub fn session_path(&self) -> Option<&Path> {
        self.session.as_deref()
    }

    /// How the shell picks its appearance.
    pub fn theme(&self) -> ThemePreference {
        self.theme
    }

    /// Sets the appearance: a user action, so it is written to the session.
    pub fn set_theme(&mut self, theme: ThemePreference) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        self.persist();
    }

    /// The login item as last read/written (the registry's truth on Windows).
    pub fn start_at_login(&self) -> bool {
        self.start_at_login
    }

    /// Why the last login-item read or write failed, if it did; Settings shows this instead of
    /// pretending the toggle worked.
    pub fn start_at_login_error(&self) -> Option<&str> {
        self.start_at_login_error.as_deref()
    }

    /// Re-reads the login item so the toggle reflects the registry rather than the session's
    /// last value. Called by [`Self::launch`] on Windows.
    #[cfg_attr(test, allow(dead_code))]
    fn refresh_start_at_login(&mut self) {
        self.refresh_start_at_login_with(&crate::autolaunch::SystemLoginItem);
    }

    /// [`Self::refresh_start_at_login`] against any login item (tests use a fake).
    pub(crate) fn refresh_start_at_login_with(&mut self, item: &dyn crate::autolaunch::LoginItem) {
        match item.enabled() {
            Ok(enabled) => {
                self.start_at_login = enabled;
                self.start_at_login_error = None;
            }
            Err(why) => self.start_at_login_error = Some(why),
        }
    }

    /// The Settings control: writes or removes the login item, then re-reads it so the toggle
    /// shows what the registry actually holds. A failed write leaves the previous truth in place
    /// (never a toggle that lies), records the reason for the row, and writes nothing.
    pub(crate) fn set_start_at_login(
        &mut self,
        enable: bool,
        item: &dyn crate::autolaunch::LoginItem,
    ) {
        if let Err(why) = item.set(enable) {
            self.start_at_login_error = Some(why);
            return;
        }
        match item.enabled() {
            Ok(enabled) => {
                self.start_at_login = enabled;
                self.start_at_login_error = None;
            }
            Err(why) => {
                // The write succeeded but could not be verified: show the requested state and
                // say the registry could not be read back.
                self.start_at_login = enable;
                self.start_at_login_error = Some(why);
            }
        }
        self.persist();
    }

    /// The Settings control's press: the real login item.
    pub(crate) fn toggle_start_at_login(&mut self) {
        self.set_start_at_login(!self.start_at_login, &crate::autolaunch::SystemLoginItem);
    }

    /// Whether closing the main window hides Station to the Windows system tray instead of quitting.
    pub fn minimize_to_tray(&self) -> bool {
        self.minimize_to_tray
    }

    /// Sets minimize-to-tray preference and persists immediately to the session document.
    pub fn set_minimize_to_tray(&mut self, enabled: bool) {
        if self.minimize_to_tray == enabled {
            return;
        }
        self.minimize_to_tray = enabled;
        self.persist();
    }

    /// Toggles minimize-to-tray from the Settings -> General switch.
    pub(crate) fn toggle_minimize_to_tray(&mut self) {
        self.set_minimize_to_tray(!self.minimize_to_tray);
    }

    /// Active and recent downloads displayed in the bottom Rune downloads bar (Slice 33).
    pub fn downloads(&self) -> &[crate::downloads::DownloadItem] {
        &self.downloads
    }

    /// Applies lifecycle updates drained from `downloads::drain_events`. True if UI changed.
    pub fn apply_download_events(&mut self, events: Vec<crate::downloads::DownloadEvent>) -> bool {
        use crate::downloads::{DownloadEvent, DownloadItem, DownloadStatus, MAX_RECENT_DOWNLOADS};
        if events.is_empty() {
            return false;
        }
        for event in events {
            match event {
                DownloadEvent::Started { filename, path } => {
                    let id = self.next_download_id;
                    self.next_download_id = self.next_download_id.saturating_add(1);
                    self.downloads.insert(
                        0,
                        DownloadItem {
                            id,
                            filename,
                            path: Some(path),
                            status: DownloadStatus::InProgress,
                        },
                    );
                }
                DownloadEvent::Completed {
                    filename,
                    path,
                    success,
                } => {
                    let status = if success {
                        DownloadStatus::Completed
                    } else {
                        DownloadStatus::Failed
                    };
                    if let Some(existing) = self.downloads.iter_mut().find(|item| {
                        item.status == DownloadStatus::InProgress
                            && ((path.is_some() && item.path == path) || item.filename == filename)
                    }) {
                        existing.status = status;
                        if path.is_some() {
                            existing.path = path;
                        }
                    } else {
                        let id = self.next_download_id;
                        self.next_download_id = self.next_download_id.saturating_add(1);
                        self.downloads.insert(
                            0,
                            DownloadItem {
                                id,
                                filename,
                                path,
                                status,
                            },
                        );
                    }
                }
                DownloadEvent::Refused { filename, refusal } => {
                    let id = self.next_download_id;
                    self.next_download_id = self.next_download_id.saturating_add(1);
                    self.downloads.insert(
                        0,
                        DownloadItem {
                            id,
                            filename,
                            path: None,
                            status: DownloadStatus::Refused(refusal),
                        },
                    );
                }
            }
        }
        self.downloads.truncate(MAX_RECENT_DOWNLOADS);
        true
    }

    /// Removes one download notice from the downloads bar.
    pub fn dismiss_download(&mut self, id: u64) -> bool {
        let before = self.downloads.len();
        self.downloads.retain(|item| item.id != id);
        self.downloads.len() != before
    }

    /// Clears every download notice from the downloads bar.
    pub fn clear_downloads(&mut self) -> bool {
        if self.downloads.is_empty() {
            return false;
        }
        self.downloads.clear();
        true
    }

    /// Seeds realistic sample downloads for `--scene downloads` snapshots.
    pub fn seed_downloads_scene(&mut self) {
        self.downloads = vec![
            crate::downloads::DownloadItem {
                id: 2,
                filename: "quarterly-roadmap-2026.pdf".to_owned(),
                path: Some(PathBuf::from("/Downloads/quarterly-roadmap-2026.pdf")),
                status: crate::downloads::DownloadStatus::Completed,
            },
            crate::downloads::DownloadItem {
                id: 1,
                filename: "design-assets.zip".to_owned(),
                path: Some(PathBuf::from("/Downloads/design-assets.zip")),
                status: crate::downloads::DownloadStatus::InProgress,
            },
        ];
        self.next_download_id = 3;
    }

    /// Custom download directory override stored in the session, if configured.
    pub fn download_dir(&self) -> Option<&str> {
        self.download_dir.as_deref()
    }

    /// Human-readable active downloads folder path shown in Settings -> General.
    pub fn effective_download_dir_path(&self) -> Option<PathBuf> {
        self.download_dir
            .as_deref()
            .map(PathBuf::from)
            .or_else(crate::downloads::downloads_dir)
    }

    /// Inline downloads-folder editor draft while open in Settings -> General.
    pub fn download_dir_draft(&self) -> Option<&DownloadDirDraft> {
        self.download_dir_draft.as_ref()
    }

    pub fn download_dir_draft_mut(&mut self) -> Option<&mut DownloadDirDraft> {
        self.download_dir_draft.as_mut()
    }

    /// Opens the inline downloads-folder editor pre-filled with the active folder.
    pub fn open_download_dir_editor(&mut self) {
        let initial = self
            .effective_download_dir_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.download_dir_draft = Some(DownloadDirDraft {
            edit: TextEdit::new(initial),
            error: None,
        });
    }

    /// Closes the downloads-folder editor without saving.
    pub fn close_download_dir_editor(&mut self) -> bool {
        self.download_dir_draft.take().is_some()
    }

    /// Validates and saves the edited downloads folder, persisting it to `session.json` and
    /// updating [`crate::downloads::set_custom_dir`].
    pub fn save_download_dir_draft(&mut self) -> bool {
        let Some(raw) = self
            .download_dir_draft
            .as_ref()
            .map(|d| d.edit.text.trim().to_owned())
        else {
            return false;
        };
        if raw.is_empty() {
            self.reset_download_dir();
            return true;
        }
        if raw.chars().count() > session_store::MAX_DOWNLOAD_DIR_CHARS
            || raw.chars().any(char::is_control)
        {
            if let Some(d) = self.download_dir_draft.as_mut() {
                d.error = Some(
                    "Folder path must be 2048 chars or fewer without control chars.".to_owned(),
                );
            }
            return false;
        }
        if !Path::new(&raw).is_absolute() {
            if let Some(d) = self.download_dir_draft.as_mut() {
                d.error = Some("Folder path must be an absolute directory path.".to_owned());
            }
            return false;
        }
        self.download_dir = Some(raw.clone());
        crate::downloads::set_custom_dir(Some(PathBuf::from(raw)));
        self.download_dir_draft = None;
        self.persist();
        true
    }

    /// Resets the downloads folder back to the OS default Downloads directory.
    pub fn reset_download_dir(&mut self) {
        self.download_dir = None;
        self.download_dir_draft = None;
        crate::downloads::set_custom_dir(None);
        self.persist();
    }

    /// What the content area shows.
    pub fn surface(&self) -> Surface {
        self.surface
    }

    /// Opens Settings in the content area; the hosted page hides while it shows.
    pub fn open_settings(&mut self) {
        self.surface = Surface::Settings;
        self.config_draft = None;
        self.download_dir_draft = None;
        self.notifications
            .apply(NotificationAction::SetVisibility { visible: false });
    }

    /// Opens About Station in the content area; the hosted page hides while it shows.
    pub fn open_about(&mut self) {
        self.surface = Surface::About;
        self.config_draft = None;
        self.download_dir_draft = None;
        self.notifications
            .apply(NotificationAction::SetVisibility { visible: false });
    }

    /// Current About Station metadata/update state.
    pub fn about_state(&self) -> &about_window::AboutWindowState {
        &self.about_state
    }

    /// Whether prerelease (beta) builds are included when checking GitHub Releases.
    pub fn include_beta_updates(&self) -> bool {
        self.include_beta_updates
    }

    /// Toggles beta update inclusion and persists immediately to the session document.
    pub fn toggle_include_beta_updates(&mut self) {
        self.include_beta_updates = !self.include_beta_updates;
        self.about_state
            .set_beta_included_in_updates(self.include_beta_updates);
        self.persist();
    }

    /// Trusted GitHub release page URL discovered by the latest update check.
    pub fn update_url(&self) -> Option<&str> {
        self.update_url.as_deref()
    }

    /// Latest update-check error message, if any.
    pub fn update_error(&self) -> Option<&str> {
        self.update_error.as_deref()
    }

    /// True once an update check has completed cleanly in this run.
    pub fn update_checked(&self) -> bool {
        self.update_checked
    }

    /// Opens the discovered release page (or the repository Releases index) in the OS browser.
    pub fn open_update_release(&self) {
        let target = self
            .update_url
            .as_deref()
            .unwrap_or(crate::about::RELEASES_URL);
        crate::routing::open_external(target);
    }

    /// Starts an asynchronous GitHub Releases check against the current package version.
    ///
    /// Unit tests do not hit live GitHub by default; use [`Self::check_for_updates_with`] to drive
    /// the background worker against a fake fetcher.
    pub fn check_for_updates(&mut self) {
        #[cfg(test)]
        {
            self.about_state.set_checking_for_update(true);
            self.update_error = None;
        }
        #[cfg(not(test))]
        {
            self.check_for_updates_with(
                env!("CARGO_PKG_VERSION"),
                std::sync::Arc::new(crate::updater::GitHubReleaseFetcher::default()),
            );
        }
    }

    /// Starts an asynchronous update check against any [`crate::updater::UpdateFetcher`].
    pub fn check_for_updates_with(
        &mut self,
        current_version: &str,
        fetcher: std::sync::Arc<dyn crate::updater::UpdateFetcher>,
    ) {
        self.update_error = None;
        match crate::updater::start_check(
            current_version.to_owned(),
            self.include_beta_updates,
            fetcher,
            crate::routing::current_waker(),
        ) {
            Ok(()) => {
                self.about_state.set_checking_for_update(true);
            }
            Err(crate::updater::UpdateError::AlreadyChecking) => {
                self.about_state.set_checking_for_update(true);
            }
            Err(why) => {
                self.about_state.set_checking_for_update(false);
                self.update_error = Some(why.to_string());
            }
        }
    }

    /// Drains a finished background update check into `AboutWindowState`. Returns true when UI
    /// state changed.
    pub fn poll_updates(&mut self) -> bool {
        let Some(result) = crate::updater::take_completed() else {
            return false;
        };
        self.about_state.set_checking_for_update(false);
        match result {
            Ok(crate::updater::UpdateCheckOutcome::Available {
                release_name,
                html_url,
            }) => {
                self.about_state.set_update_is_available(release_name);
                self.update_url = Some(html_url);
                self.update_error = None;
                self.update_checked = true;
            }
            Ok(crate::updater::UpdateCheckOutcome::UpToDate) => {
                self.about_state.update_available = false;
                self.about_state.release_name = None;
                self.update_url = None;
                self.update_error = None;
                self.update_checked = true;
            }
            Err(why) => {
                self.update_error = Some(why.to_string());
            }
        }
        true
    }

    /// The rail's moon button: the notification panel opens over the content area, and closes
    /// when pressed again.
    pub fn toggle_notifications(&mut self) {
        if self.surface == Surface::Notifications {
            self.close_overlay();
            return;
        }
        self.surface = Surface::Notifications;
        self.notifications
            .apply(NotificationAction::SetVisibility { visible: true });
    }

    /// Back to the active app (Escape, a back button). True when something was showing.
    pub fn close_overlay(&mut self) -> bool {
        if self.surface == Surface::Apps {
            return false;
        }
        self.surface = Surface::Apps;
        self.config_draft = None;
        self.download_dir_draft = None;
        self.notifications
            .apply(NotificationAction::SetVisibility { visible: false });
        true
    }

    /// Whether the active tab's page belongs on screen: an app tab, with no Rune surface over
    /// it. `webview` shows and bounds the page exactly when this holds.
    pub fn shows_page(&self) -> bool {
        self.surface == Surface::Apps && !self.active().renders_in_rune()
    }

    pub fn notifications(&self) -> &NotificationCenter {
        &self.notifications
    }

    /// Pushes a tab directly (slice-8 tests build a page-source tab without a session).
    #[cfg(test)]
    pub fn tabs_mut_for_tests(&mut self) -> &mut Vec<AppTab> {
        &mut self.tabs
    }

    /// Removes every private manifest from the service's store (the gone-manifest test: a
    /// private manifest deleted underneath an installed session).
    #[cfg(test)]
    pub fn private_store_remove_all_for_tests(&mut self) {
        let ids: Vec<u64> = self
            .store
            .private_store()
            .get_private_manifests()
            .iter()
            .filter_map(|manifest| manifest.id.parse::<u64>().ok())
            .collect();
        for id in ids {
            let _ = self.store.delete_private_manifest(id);
        }
    }

    /// Applies one action to the center: the bridge (`super::notifications::deliver_to`), the
    /// panel's presses and the frame's snooze expiry all write through here.
    pub(crate) fn notifications_mut(&mut self) -> &mut NotificationCenter {
        &mut self.notifications
    }

    /// A page's `closeNotification(id)`: the stored notification is removed, exactly the
    /// preload's `'notification-close'` channel. The `#close` misinterpretation most apps have
    /// (sagas.ts) does not apply: the page is closing *its own* notification.
    pub(crate) fn close_notification(&mut self, notification_id: &str) -> bool {
        if self.notifications.notification(notification_id).is_none() {
            return false;
        }
        self.notifications
            .apply(NotificationAction::RemoveNotification {
                notification_id: notification_id.to_owned(),
            });
        true
    }

    /// A notification row pressed: the `sagaNotificationClick` order — the notification is
    /// marked read (dropped) first, then the click is routed. Nothing to route (no tab) drops
    /// it anyway, like the saga's early return. Returns the sender's tab, when the notification
    /// records one, so the view action can deliver the click message to that page (the state
    /// itself never holds the web view, D5).
    pub(crate) fn click_notification(&mut self, notification_id: &str) -> Option<String> {
        let Some(target) = self.notifications.click_target(notification_id) else {
            self.notifications
                .apply(NotificationAction::NotificationClick {
                    notification_id: notification_id.to_owned(),
                    interface_type: String::new(),
                });
            return None;
        };
        let source_tab = self
            .notifications
            .notification(notification_id)
            .and_then(|notification| notification.tab_id.clone())
            .filter(|tab_id| !tab_id.is_empty());
        self.notifications
            .apply(NotificationAction::NotificationClick {
                notification_id: notification_id.to_owned(),
                interface_type: String::new(),
            });
        match target {
            notification_center::ClickTarget::WebContents(_) => {
                // No web-contents registry in this shell; a page-sourced click carries a tab id.
            }
            notification_center::ClickTarget::Tab(tab_id) => {
                // The panel over the page closes so the focused tab's page can show; `select`
                // still runs when no overlay was up.
                if self.surface != Surface::Apps {
                    self.close_overlay();
                }
                self.select(&tab_id);
            }
        }
        source_tab
    }

    /// The panel's "Mark all read": the center drops every notification.
    pub fn mark_all_notifications_read(&mut self) {
        self.notifications.apply(NotificationAction::MarkAllAsRead);
    }

    /// The panel's snooze choice: `None` clears it, otherwise a duration the notification center
    /// parses (`"15min"`, `"1h"`, its `SYNC_WITH_OS` and `INFINITE` sentinels). The wall clock is
    /// the shell's; the center owns no clock.
    pub fn snooze_notifications(&mut self, duration: Option<&str>) {
        match duration {
            None => self
                .notifications
                .apply(NotificationAction::ResetSnoozeDuration),
            Some(duration) => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_millis() as i64)
                    .unwrap_or(0);
                self.notifications
                    .apply(NotificationAction::SetSnoozeDuration {
                        duration: duration.to_owned(),
                        at_ms: now_ms,
                    });
            }
        }
    }

    /// The current snooze state as the millisecond duration sent to hosted pages
    /// (`__stationSnoozeUpdate`): `None` when not snoozed, the finite duration in ms for a timed
    /// snooze, or a positive sentinel (`u32::MAX as u64`, exact in IEEE 754 JS numbers) for
    /// infinite or active OS-synced Do Not Disturb.
    pub fn effective_snooze_duration_ms(&self) -> Option<u64> {
        match self.notifications.snooze() {
            None | Some(notification_center::Snooze::Unknown(_)) => None,
            Some(notification_center::Snooze::Finite(ms)) => Some(ms),
            Some(notification_center::Snooze::Infinite) => Some(u64::from(u32::MAX)),
            Some(notification_center::Snooze::SyncWithOs) => {
                self.os_dnd_active.then_some(u64::from(u32::MAX))
            }
        }
    }

    /// True whenever notifications are currently suppressed (timed snooze, infinite, or
    /// `Snooze::SyncWithOs` with host OS Do Not Disturb active).
    pub fn is_effectively_snoozed(&self) -> bool {
        self.effective_snooze_duration_ms().is_some()
    }

    /// Updates the polled host OS Do Not Disturb flag; returns true if it changed.
    pub fn set_os_dnd_active(&mut self, active: bool) -> bool {
        if self.os_dnd_active == active {
            return false;
        }
        self.os_dnd_active = active;
        true
    }

    /// Human-readable app name for a tab id (for OS notification titles).
    pub fn tab_display_name(&self, tab_id: &str) -> String {
        self.tabs
            .iter()
            .find(|t| t.id == tab_id)
            .map(|t| t.label.clone())
            .unwrap_or_else(|| "Station".to_owned())
    }

    /// Writes the session: after an install, an uninstall, a selection, on close and on quit.
    ///
    /// The placement is read from the live window when it can be (on close, and while the window
    /// exists); a later save without a window — Rune reports no placement once the window's host
    /// is gone, which is the case in `on_quit` when the last window closed — keeps the placement
    /// the last save captured instead of writing the one this shell launched with.
    ///
    /// A shell built by `new` (tests) has no file and does nothing. A session that could not be
    /// read is not overwritten until the user acts; a write failure is reported and the shell
    /// keeps running.
    pub fn save(&mut self) {
        self.write_session();
    }

    /// The page the shell could not attach for the active tab, if any.
    pub fn page_error(&self) -> Option<&PageError> {
        self.page_error.as_ref()
    }

    /// Set by the web-view engine when an attach fails; true when the view-visible error changed.
    pub fn set_page_error(&mut self, error: PageError) -> bool {
        if self.page_error.as_ref() == Some(&error) {
            return false;
        }
        self.page_error = Some(error);
        true
    }

    /// Clears the error (the Retry control, or a page that attached). True when one was showing.
    pub fn clear_page_error(&mut self) -> bool {
        self.page_error.take().is_some()
    }

    /// A user action that must be durable at once: a selection, an install, an uninstall, an
    /// appearance change. Written through a temp file and a rename by `session-store`, so a hard
    /// kill leaves either the previous session or this one, never a torn file.
    fn persist(&mut self) {
        self.save_blocked = false;
        self.write_session();
    }

    /// One write attempt: the placement is read from the live window when it can be (on close,
    /// and while the window exists); a later write without a window keeps the placement the last
    /// one captured.
    ///
    /// A failure is reported (throttled); the next user action tries again, and the shell keeps
    /// running.
    fn write_session(&mut self) {
        if self.save_blocked {
            return;
        }
        let Some(path) = &self.session else {
            return;
        };
        if let Some(live) = session::live_window_state() {
            self.window = live;
        }
        match SessionStore::new(path.clone()).save(&self.session_state()) {
            Ok(()) => self.save_failures = 0,
            Err(why) => {
                self.save_failures = self.save_failures.saturating_add(1);
                let now = session::seconds();
                if self.save_failures == 1 || now - self.save_logged_at >= SAVE_LOG_EVERY_S {
                    self.save_logged_at = now;
                    eprintln!(
                        "station-shell: could not save the session to {}: {why}",
                        path.display()
                    );
                }
            }
        }
    }

    /// The document to write: what to restore, with the live window's placement when there is a
    /// window, otherwise the one this shell was launched with.
    fn session_state(&self) -> SessionState {
        // Rail order first (the dock's order), then any application the rail does not show, by
        // id: the file is stable from run to run.
        let mut installed: Vec<InstalledApplication> = self
            .tabs
            .iter()
            .skip(1)
            .filter_map(|tab| self.store.installed_applications().get(&tab.id).cloned())
            .collect();
        let mut rest: Vec<&InstalledApplication> = self
            .store
            .installed_applications()
            .values()
            .filter(|application| {
                !self
                    .tabs
                    .iter()
                    .any(|tab| tab.id == application.application_id)
            })
            .collect();
        rest.sort_by(|a, b| a.application_id.cmp(&b.application_id));
        installed.extend(rest.into_iter().cloned());
        SessionState {
            window: self.window.clone(),
            theme: self.theme,
            start_at_login: self.start_at_login,
            active_application: Some(self.active.clone()),
            // The user's order, healed against the dock (install appends, uninstall drops): the
            // document must name the docked set exactly for the next launch to take it.
            rail_order: {
                let mut order = self.rail_order.clone();
                let docked: std::collections::HashSet<&str> =
                    self.store.dock().iter().map(|id| id.as_str()).collect();
                order.retain(|id| docked.contains(id.as_str()));
                for id in self.store.dock() {
                    if !order.contains(id) {
                        order.push(id.clone());
                    }
                }
                order
            },
            installed,
            dock: self.store.dock().to_vec(),
            tabs: self.store.tabs().to_vec(),
            // Only configured applications get an entry; an empty map serializes as absent.
            app_config: self
                .store
                .installed_applications()
                .values()
                .filter(|application| {
                    let config = &application.config_data;
                    config.identity_id.is_some()
                        || config.subdomain.is_some()
                        || config.custom_url.is_some()
                })
                .map(|application| {
                    (
                        application.application_id.clone(),
                        AppConfigEntry {
                            identity_id: application.config_data.identity_id.clone(),
                            subdomain: application.config_data.subdomain.clone(),
                            custom_url: application.config_data.custom_url.clone(),
                        },
                    )
                })
                .collect(),
            // Accounts ride the document unchanged: this slice has no writer besides the (later)
            // sign-in, and dropping them on any user action would lose them.
            identities: self.identities.clone(),
            favorites: self
                .favorites
                .iter()
                .filter(|fav| {
                    self.store
                        .installed_applications()
                        .contains_key(&fav.application_id)
                })
                .cloned()
                .collect(),
            download_dir: self.download_dir.clone(),
            include_beta_updates: self.include_beta_updates,
            minimize_to_tray: self.minimize_to_tray,
        }
    }
}

/// `UPDATE_BADGE`'s normalization, taken from `tab-model` instead of copied: its private
/// `normalize_badge` is reached through the public reducer (an empty value clears; a base-10
/// integer prefix is a count; anything else is text), exactly the TS `normalizedBadgeValue`.
fn normalize_page_badge(value: &str) -> Option<Badge> {
    if value.is_empty() {
        return None;
    }
    let mut model = TabModel::new();
    model.apply(TabAction::add("page-meta", "page-meta", ""));
    model.apply(TabAction::UpdateBadge {
        tab_id: "page-meta".to_owned(),
        badge: value.to_owned(),
    });
    model.tab("page-meta").and_then(|tab| tab.badge.clone())
}

/// The one configuration field a manifest's presets consume, in the TS saga's order
/// (`setIdentityIfMultiInstanceApplication`: google-account, then subdomain, then on-premise).
/// `None` for a manifest with no presets — nothing to configure.
fn config_field_for(manifest: &manifest_registry::BxAppManifest) -> Option<ConfigField> {
    let presets = manifest_registry::get_presets(manifest);
    if presets.contains(&manifest_registry::Preset::GoogleAccount) {
        Some(ConfigField::Identity)
    } else if presets.contains(&manifest_registry::Preset::Subdomain) {
        Some(ConfigField::Subdomain)
    } else if presets.contains(&manifest_registry::Preset::OnPremise) {
        Some(ConfigField::CustomUrl)
    } else {
        None
    }
}

/// The session entry as the service's `set_config_data` takes it (the same three fields).
fn to_service_config(config: &AppConfigEntry) -> ApplicationConfigData {
    ApplicationConfigData {
        identity_id: config.identity_id.clone(),
        subdomain: config.subdomain.clone(),
        custom_url: config.custom_url.clone(),
    }
}

/// The same conversion, for the tests that drive the service beside the state.
#[cfg(test)]
pub(crate) fn to_service_config_for_tests(config: &AppConfigEntry) -> ApplicationConfigData {
    to_service_config(config)
}

/// The editor's saved draft, trimmed and bounded the way the document is (`session-store`
/// refuses anything past the caps; the trim here is the polite path and the cap the hard one).
fn trimmed(mut config: AppConfigEntry) -> AppConfigEntry {
    let cut = |value: Option<String>| {
        value.map(|value| {
            value
                .trim()
                .chars()
                .take(session_store::MAX_CONFIG_VALUE_CHARS)
                .collect::<String>()
        })
    };
    config.identity_id = cut(config.identity_id);
    config.subdomain = cut(config.subdomain);
    config.custom_url = cut(config.custom_url);
    config
}

/// Validates a user-submitted custom app draft into a [`manifest_registry::NewPrivateApplication`].
fn validate_custom_app_draft(
    draft: &CustomAppDraft,
) -> Result<manifest_registry::NewPrivateApplication, String> {
    const MAX_NAME_CHARS: usize = 128;
    const MAX_URL_CHARS: usize = 2048;

    let name = draft.name.text.trim();
    if name.is_empty() {
        return Err("App name is required.".to_owned());
    }
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err("App name must be 128 characters or fewer without control chars.".to_owned());
    }

    let start_raw = draft.start_url.text.trim();
    let parsed_start = parse_http_url(start_raw, MAX_URL_CHARS)
        .ok_or_else(|| "Start URL must be a valid https:// or http:// URL.".to_owned())?;

    let scope_raw = draft.scope.text.trim();
    let scope = if scope_raw.is_empty() {
        parsed_start.origin().ascii_serialization()
    } else {
        parse_http_url(scope_raw, MAX_URL_CHARS)
            .ok_or_else(|| "Scope must be a valid https:// or http:// URL.".to_owned())?
            .as_str()
            .to_owned()
    };

    let icon_raw = draft.icon_url.text.trim();
    let icon_url = if icon_raw.is_empty() {
        String::new()
    } else {
        parse_http_url(icon_raw, MAX_URL_CHARS)
            .ok_or_else(|| "Icon URL must be a valid https:// or http:// URL.".to_owned())?
            .as_str()
            .to_owned()
    };

    let color_raw = draft.theme_color.text.trim();
    let theme_color = if color_raw.is_empty() {
        "#2B5CE6".to_owned()
    } else if is_hex_color(color_raw) {
        color_raw.to_owned()
    } else {
        return Err("Theme color must be a hex color like #2B5CE6.".to_owned());
    };

    Ok(manifest_registry::NewPrivateApplication {
        name: name.to_owned(),
        theme_color,
        icon_url,
        start_url: parsed_start.as_str().to_owned(),
        scope,
    })
}

fn parse_http_url(raw: &str, max_chars: usize) -> Option<url::Url> {
    if raw.is_empty() || raw.chars().count() > max_chars || raw.chars().any(char::is_control) {
        return None;
    }
    let parsed = url::Url::parse(raw).ok()?;
    let scheme = parsed.scheme();
    if (scheme == "http" || scheme == "https") && parsed.host_str().is_some() {
        Some(parsed)
    } else {
        None
    }
}

fn is_hex_color(raw: &str) -> bool {
    let Some(hex) = raw.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}
