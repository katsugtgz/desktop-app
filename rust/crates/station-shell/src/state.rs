//! What the shell window shows: the apps in the rail, which one is active, and the app store's
//! own state.
//!
//! This is the state an `AppSpec` hands its view. It owns the installed-application service
//! (`appstore-service`) and the store's cached lists, so the views below read it and never
//! re-query: a rail click, a store tile and the search field all write through the methods here.
//! Nothing platform-bound lives in this type — the web view host is `webview`'s, keyed by tab id.

use std::path::{Path, PathBuf};

use appstore_service::{
    get_all_categories, get_applications_by_category, get_most_popular_apps, search_applications,
    ApplicationService, InstallOptions, InstalledApplication, PopularApps,
};
use manifest_registry::{manifest_to_minimal_application, Manifest, MinimalApplication};
use notification_center::{NotificationAction, NotificationCenter};
use rune::TextEdit;
use session_store::{SessionState, SessionStore, ThemePreference, WindowState};
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

/// One app in the rail: the id keys and the web view host use, the label the user reads, and the
/// URL its pane shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppTab {
    pub id: String,
    pub label: String,
    pub url: String,
    pub kind: TabKind,
}

impl AppTab {
    /// The App Store: always the rail's first row.
    pub fn store() -> Self {
        Self {
            id: APP_STORE_ID.to_owned(),
            label: "App Store".to_owned(),
            url: APP_STORE_URL.to_owned(),
            kind: TabKind::Store,
        }
    }

    /// An installed application's home tab, from the manifest the install returned.
    pub fn web(id: impl Into<String>, label: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            url: url.into(),
            kind: TabKind::Web,
        }
    }

    /// The element key of this app's rail row. Unique because tab ids are.
    pub fn key(&self) -> String {
        format!("tab-{}", self.id)
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
    surface: Surface,
    page_error: Option<PageError>,
    /// Failed writes since the last success: logged at most every [`SAVE_LOG_EVERY_S`].
    save_failures: u32,
    /// The clock time of the last save-failure log line.
    save_logged_at: f64,
    notifications: NotificationCenter,
    /// The session file, when this shell persists. `None` in tests that build a state directly:
    /// they never touch a session path.
    session: Option<PathBuf>,
    /// The placement loaded from the session, replaced by the live window's when it exists.
    window: WindowState,
    /// Set when the session on disk could not be read or replayed: the file is not overwritten
    /// until the user acts (a selection, an install, an uninstall).
    save_blocked: bool,
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
            surface: Surface::Apps,
            page_error: None,
            save_failures: 0,
            save_logged_at: 0.0,
            notifications: NotificationCenter::new(),
            session: None,
            window: WindowState::default(),
            save_blocked: false,
        };
        state.rebuild_tabs();
        state
    }

    /// The shell a launch starts in: the session at [`session::path`] restored over the user's
    /// private manifest store.
    pub fn launch() -> Self {
        Self::launch_in(launch_service(), &session::path())
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
        state.save_blocked = !readable || !replayed;
        if let Some(active) = active {
            // A session naming an application it no longer has falls back to the App Store.
            state.set_active(&active);
        }
        state
    }

    /// The rail: the App Store, then every docked application in dock order.
    fn rebuild_tabs(&mut self) {
        let mut tabs = vec![AppTab::store()];
        for id in self.store.dock() {
            let Some(application) = self.store.installed_applications().get(id) else {
                continue;
            };
            tabs.push(AppTab::web(
                application.application_id.clone(),
                self.label_for(&application.manifest_url, id),
                application.home_tab_url.clone().unwrap_or_default(),
            ));
        }
        if !tabs.iter().any(|tab| tab.id == self.active) {
            self.active = APP_STORE_ID.to_owned();
        }
        self.tabs = tabs;
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

    /// What the content area shows.
    pub fn surface(&self) -> Surface {
        self.surface
    }

    /// Opens Settings in the content area; the hosted page hides while it shows.
    pub fn open_settings(&mut self) {
        self.surface = Surface::Settings;
        self.notifications
            .apply(NotificationAction::SetVisibility { visible: false });
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
            active_application: Some(self.active.clone()),
            rail_order: self.tabs.iter().skip(1).map(|tab| tab.id.clone()).collect(),
            installed,
            dock: self.store.dock().to_vec(),
            tabs: self.store.tabs().to_vec(),
        }
    }
}
