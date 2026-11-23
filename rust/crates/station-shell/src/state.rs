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
    /// Google OAuth Option B client config (from `GOOGLE_CLIENT_ID` / optional secret).
    oauth_config: Option<crate::oauth::OAuthConfig>,
    /// Transient sign-in status or error message shown in Settings (never persisted).
    oauth_status: Option<String>,
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
            oauth_config: crate::oauth::OAuthConfig::from_env(),
            oauth_status: None,
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
        state.identities = identities;
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
        // Page-derived presentation (slice 20) survives a rebuild (an install, a config change,
        // a reorder): it is keyed by tab id, and a tab that leaves drops its entry with it.
        let previous: std::collections::HashMap<String, (Option<String>, Option<Badge>)> = self
            .tabs
            .iter()
            .map(|tab| (tab.id.clone(), (tab.page_title.clone(), tab.badge.clone())))
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
            if let Some((title, badge)) = previous.get(&tab.id) {
                tab.page_title = title.clone();
                tab.badge = badge.clone();
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
        if self.active != previous {
            self.persist();
        }
        true
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

    /// Queues the fetch for every app the shell can draw an icon for (the rail's tabs and the
    /// store's lists). Called from the frame poll — the view never mutates state, so the
    /// asking happens here; once per URL per run.
    pub fn prefetch_icons(&mut self) {
        for tab in &self.tabs {
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

    /// What the content area shows.
    pub fn surface(&self) -> Surface {
        self.surface
    }

    /// Opens Settings in the content area; the hosted page hides while it shows.
    pub fn open_settings(&mut self) {
        self.surface = Surface::Settings;
        self.config_draft = None;
        self.notifications
            .apply(NotificationAction::SetVisibility { visible: false });
    }

    /// Opens About Station in the content area; the hosted page hides while it shows.
    pub fn open_about(&mut self) {
        self.surface = Surface::About;
        self.config_draft = None;
        self.notifications
            .apply(NotificationAction::SetVisibility { visible: false });
    }

    /// Current About Station metadata/update state.
    pub fn about_state(&self) -> &about_window::AboutWindowState {
        &self.about_state
    }

    /// V1 update check: records the check in `AboutWindowState` and opens GitHub Releases in the
    /// OS browser (no-op browser spawn under unit tests).
    pub fn check_for_updates(&mut self) {
        self.about_state.set_checking_for_update(true);
        #[cfg(not(test))]
        crate::routing::open_external(crate::about::RELEASES_URL);
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
    /// infinite / active OS-synced snooze.
    pub fn effective_snooze_duration_ms(&self) -> Option<u64> {
        match self.notifications.snooze() {
            None | Some(notification_center::Snooze::Unknown(_)) => None,
            Some(notification_center::Snooze::Finite(ms)) => Some(ms),
            Some(notification_center::Snooze::Infinite)
            | Some(notification_center::Snooze::SyncWithOs) => Some(u64::from(u32::MAX)),
        }
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
