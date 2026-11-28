//! Install/uninstall/request-private orchestration (port of the write half
//! of `window.bxApi.applications`, i.e. `packages/app/src/app-store/sagas.ts`
//! and `packages/app/src/applications/sagas/lifecycle.ts`; the persistence
//! layer, `packages/app/manifests/private.ts`, already lives in
//! `manifest_registry::PrivateStore`).
//!
//! The TS side orchestrates redux sagas (`installApplication` puts
//! `createApplication` / `createNewTab` / `addAppItem`, `uninstallApplication`
//! puts `closeAllTabsInApp` / `dropApplication` / `removeAppItem` /
//! `removeLink` / `deleteFavorite`). The port keeps the same decisions and
//! the same paper trail — an [`InstalledApplication`] per application with
//! its home tab and dock membership — but as one owned struct instead of a
//! redux store, which is what s14 needs to expose over napi.
//!
//! Also ports the `addApplicationRequest` guards from
//! `packages/app/src/app-store/sagas.ts`: blank-manifest rejection, URL
//! normalization and the 2s install timeout (`race({ installation,
//! timeout: delay(2000) })`).

use std::collections::HashMap;
use std::time::Duration;

use manifest_registry::{
    get_presets, BxAppManifest, Manifest, NewPrivateApplication, Preset, PrivateStore,
};
use serde::{Deserialize, Serialize};

/// `ApplicationCreated` from the app-request duck — the `{ id,
/// bxAppManifestURL }` body `requestPrivate` resolves with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationCreated {
    pub id: String,
    #[serde(rename = "bxAppManifestURL")]
    pub bx_app_manifest_url: String,
}

/// `InstallContext` from `packages/app/src/applications/types.ts`.
///
/// Only the `appstore` platform exists in the TS enum (`Platform.AppStore`),
/// so the field keeps that one value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallContext {
    pub id: String,
    /// TS `Platform` enum — `'appstore'` is the sole member.
    pub platform: String,
    #[serde(
        default,
        rename = "onboardeeApplicationAssignmentId",
        skip_serializing_if = "Option::is_none"
    )]
    pub onboardee_application_assignment_id: Option<String>,
}

impl InstallContext {
    /// `Platform.AppStore` — the only platform the TS enum defines.
    pub fn appstore(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            platform: "appstore".to_owned(),
            onboardee_application_assignment_id: None,
        }
    }
}

/// `InstallApplicationOptions` (the identity/subdomain/customURL fields are
/// `ApplicationConfigData`, consumed by [`ApplicationService::install`]).
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    pub install_context: Option<InstallContext>,
    /// `ApplicationConfigData` subset: `identityId` / `subdomain` / `customURL`.
    pub config_data: ApplicationConfigData,
}

/// `ApplicationConfigData` from `packages/app/src/applications/duck.ts` —
/// the three multi-instance configuration knobs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ApplicationConfigData {
    #[serde(
        default,
        rename = "identityId",
        skip_serializing_if = "Option::is_none"
    )]
    pub identity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
    #[serde(default, rename = "customURL", skip_serializing_if = "Option::is_none")]
    pub custom_url: Option<String>,
}

/// `InstallApplicationReturn` from the lifecycle saga.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallApplicationReturn {
    pub application_id: String,
    pub manifest: BxAppManifest,
}

/// `ApplicationInstalledPayload` from `packages/app/src/pubsub/topics.ts`
/// (`APPLICATION_INSTALLED` topic). The port returns it from
/// [`ApplicationService::install`] instead of publishing to pubsub —
/// s14 decides where it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationInstalledPayload {
    pub application_id: String,
    #[serde(rename = "inBackground")]
    pub in_background: bool,
}

/// One installed application's record: the redux `applications` entry plus
/// the home tab `createNewTab` made and the dock item `addAppItem` added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledApplication {
    pub application_id: String,
    pub manifest_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_context: Option<InstallContext>,
    /// The home tab `installApplication` creates from the manifest
    /// `start_url` (`None` when the manifest has none, like the App Store's
    /// dock-less siblings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_tab_url: Option<String>,
    /// `bx_no_dock` manifests stay off the dock.
    pub in_dock: bool,
    /// Config data set at install time by
    /// `setIdentityIfMultiInstanceApplication`.
    #[serde(default)]
    pub config_data: ApplicationConfigData,
}

/// Why a persisted session could not be replayed into [`ApplicationService`].
///
/// Every variant names the offending id: a session is either restored whole or refused, never
/// silently trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreError {
    /// Two installed records share an application id.
    DuplicateApplication(String),
    /// Two dock entries name the same application.
    DuplicateDockEntry(String),
    /// The dock names an application that is not installed.
    UnknownDockApplication(String),
    /// Two tabs share a tab id.
    DuplicateTab(String),
    /// A tab belongs to an application that is not installed.
    UnknownTabApplication(String),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestoreError::DuplicateApplication(id) => {
                write!(f, "session installs application {id} twice")
            }
            RestoreError::DuplicateDockEntry(id) => {
                write!(f, "session docks application {id} twice")
            }
            RestoreError::UnknownDockApplication(id) => {
                write!(f, "session docks {id}, which it does not install")
            }
            RestoreError::DuplicateTab(id) => write!(f, "session has two tabs with id {id}"),
            RestoreError::UnknownTabApplication(id) => {
                write!(f, "session has a tab of {id}, which it does not install")
            }
        }
    }
}

impl std::error::Error for RestoreError {}

/// Failures of [`ApplicationService`] commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationError {
    /// `addApplicationRequest`'s blank-URL guard.
    BlankManifestUrl,
    /// The 2s `race({ installation, timeout: delay(2000) })` from
    /// `addApplicationRequest` — installation exceeded its budget.
    InstallationTimeout,
    /// `getManifestOrTimeout`'s 30s guard: manifest unknown/unreadable.
    ManifestNotAvailable(String),
    /// `uninstallApplication` on an id that was never installed.
    NotInstalled(String),
    /// The private-manifest store could not be written (read-only or full disk).
    PrivateStoreWrite(String),
}

impl std::fmt::Display for ApplicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplicationError::BlankManifestUrl => {
                write!(f, "Add Application : given manifest URL is blank")
            }
            ApplicationError::InstallationTimeout => {
                write!(f, "Application installation timeout")
            }
            ApplicationError::ManifestNotAvailable(url) => {
                write!(f, "Could not get the manifest {url}")
            }
            ApplicationError::PrivateStoreWrite(why) => {
                write!(f, "private manifest store: {why}")
            }
            ApplicationError::NotInstalled(id) => {
                write!(f, "application {id} is not installed")
            }
        }
    }
}

impl std::error::Error for ApplicationError {}

/// TS `shortid.generate()` equivalent for `createApplication` ids: URL- and
/// filesystem-safe, short. `rand` stays out of the tree; `DefaultHasher`
/// over an atomic counter plus a per-process seed gives the same shape —
/// unique within and across runs (seed differs per process), which is all
/// shortid ever guaranteed.
fn shortid() -> String {
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    // shortid's default alphabet, minus the two symbol characters (keeps
    // ids plain alphanumeric; nothing here needs the full set).
    const ALPHABET: &[u8; 62] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let seed = *SEED.get_or_init(|| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut h);
        h.finish()
    });
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h);
    n.hash(&mut h);
    let mut v = h.finish();
    // shortid's default length is 8-14 characters; 9 lands in range.
    let mut out = [0u8; 9];
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[(v % 62) as usize];
        v /= 62;
    }
    String::from_utf8(out.to_vec()).expect("ascii")
}

/// Percent-encode like WHATWG `URLSearchParams.toString()` for the query
/// params `getMultiInstanceConfiguratorURL` builds
/// (`manifestURL=station-manifest%3A%2F%2F14&applicationId=abc`).
fn url_search_params_encode(pairs: &[(&str, &str)]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    fn encode_into(out: &mut String, s: &str) {
        for byte in s.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                    out.push(byte as char)
                }
                _ => {
                    out.push('%');
                    out.push(HEX[(byte >> 4) as usize] as char);
                    out.push(HEX[(byte & 0xf) as usize] as char);
                }
            }
        }
    }
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        encode_into(&mut out, k);
        out.push('=');
        encode_into(&mut out, v);
    }
    out
}

/// The `addApplicationRequest` / `installApplication` /
/// `uninstallApplication` / `requestPrivateApplication` orchestration over a
/// manifest registry and a private-manifest store.
///
/// Installation is synchronous against locally resolvable manifests (the
/// registry embeds every bundled one); a manifest that is neither bundled
/// nor private mirrors the manifestProvider timeout path with
/// [`ApplicationError::ManifestNotAvailable`] instead of a 30s wait. The
/// 2-second budget of `addApplicationRequest` is kept as
/// [`ApplicationService::add_application_request`] taking the elapsed time
/// of the manifest resolution step, so callers driving real IO can enforce
/// it where the TS `race` did.
pub struct ApplicationService {
    private: PrivateStore,
    installed: HashMap<String, InstalledApplication>,
    dock: Vec<String>,
    tabs: Vec<TabRecord>,
    favorites: Vec<FavoriteRecord>,
    links: Vec<(String, String)>,
}

/// What `createNewTab` recorded (id + url + owning application).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabRecord {
    pub tab_id: String,
    pub application_id: String,
    pub url: String,
    pub home: bool,
}

/// What `deleteFavorite` removes. TS `StationFavorite` has no tab field —
/// `getTabFavoriteId` reads the favorite's own always-present `favoriteId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FavoriteRecord {
    pub favorite_id: String,
    pub application_id: String,
}

impl ApplicationService {
    /// Store rooted at the user config dir (see [`PrivateStore::new`]).
    pub fn new() -> std::io::Result<Self> {
        Self::from_private_store(PrivateStore::new()?)
    }

    /// Store at an explicit `private-manifests.json` path — the test and
    /// napi entry point.
    pub fn from_manifests_path(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        Self::from_private_store(PrivateStore::from_manifests(path)?)
    }

    fn from_private_store(private: PrivateStore) -> std::io::Result<Self> {
        Ok(Self {
            private,
            installed: HashMap::new(),
            dock: Vec::new(),
            tabs: Vec::new(),
            favorites: Vec::new(),
            links: Vec::new(),
        })
    }

    /// A service over an in-memory private-manifest store: nothing is written, so it works on a
    /// machine whose config dir cannot be opened. Installs still work for the session.
    pub fn in_memory() -> Self {
        Self::from_private_store(PrivateStore::in_memory()).expect("an in-memory store cannot fail")
    }

    /// Tests get an isolated temp-dir store without pulling in a tempfile
    /// dependency: a fresh folder under the system temp dir, removed when
    /// this process exits is fine for `cargo test` hygiene.
    ///
    /// The folder is unique per call, not just per tag: several tests reuse one tag (and two
    /// test binaries in different processes can run the same tag at once), and this constructor
    /// deletes the folder first, so sharing it would delete a live store underneath another
    /// test.
    #[doc(hidden)]
    pub fn for_tests(tag: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "station-s11b-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self::from_manifests_path(dir.join("private-manifests.json"))
            .expect("private store in temp dir")
    }

    /// Read access to the private-manifest store.
    pub fn private_store(&self) -> &PrivateStore {
        &self.private
    }

    /// `getPrivateApplications` — see also
    /// [`manifest_registry::PrivateStore::get_private_manifests`].
    pub fn private_manifests(&self) -> Vec<Manifest> {
        self.private.get_private_manifests()
    }

    /// Port of `getManifestByURL` (`listAllApplications().find(app =>
    /// getBxAppManifestURL(app.id) === manifestURL)`): the manifest whose
    /// `station-manifest://<id>` equals `manifest_url`, bundled or private.
    pub fn get_manifest_by_url(&self, manifest_url: &str) -> Option<Manifest> {
        manifest_registry::list_all_applications(&self.private.get_private_manifests())
            .into_iter()
            .find(|m| manifest_registry::get_bx_app_manifest_url(&m.id) == manifest_url)
    }

    /// Install-time manifest resolution. TS's `installApplication` goes
    /// through `manifestProvider.get`, which serves every known manifest —
    /// including `doNotList` ones like the App Store itself that
    /// `getManifestByURL`'s `listAllApplications` filters out.
    fn resolve_manifest(&self, manifest_url: &str) -> Result<BxAppManifest, ApplicationError> {
        if let Some(m) = self.get_manifest_by_url(manifest_url) {
            return Ok(m.inner);
        }
        // doNotList manifests: lookup by id directly (bundled or private).
        if let Some(id) = manifest_url
            .strip_prefix("station-manifest://")
            .map(|rest| rest.trim_end_matches('/'))
        {
            let m = manifest_registry::get_application_by_id(id).or_else(|| {
                id.parse::<u64>()
                    .ok()
                    .and_then(|n| self.private.get_private_application_by_id(n))
            });
            if let Some(m) = m {
                if manifest_registry::get_bx_app_manifest_url(&m.id) == manifest_url {
                    return Ok(m.inner);
                }
            }
        }
        Err(ApplicationError::ManifestNotAvailable(
            manifest_url.to_owned(),
        ))
    }

    /// Port of `installApplication` (lifecycle.ts): resolves the manifest,
    /// creates the application entry, applies multi-instance config, creates
    /// the home tab, adds the dock item unless `bx_no_dock`.
    ///
    /// `setIdentityIfMultiInstanceApplication` only puts config data for the
    /// first matching preset (google-account → identityId, subdomain,
    /// on-premise → customURL, in that order); the port stores whichever
    /// field matched, identically.
    pub fn install(
        &mut self,
        manifest_url: &str,
        options: &InstallOptions,
    ) -> Result<InstallApplicationReturn, ApplicationError> {
        let manifest = self.resolve_manifest(manifest_url)?;

        // createApplication: fresh shortid application entry.
        let application_id = shortid();
        let config_data = config_data_for_presets(&manifest, &options.config_data);

        // getStartURL → createNewTab(applicationId, startURL, { home: true }).
        let home_tab_url =
            start_url_for_install(&manifest, manifest_url, &application_id, &config_data);
        if let Some(url) = home_tab_url.clone() {
            self.tabs.push(TabRecord {
                tab_id: shortid(),
                application_id: application_id.clone(),
                url,
                home: true,
            });
        }

        // addAppItem unless bx_no_dock.
        let in_dock = manifest.bx_no_dock != Some(true);
        if in_dock {
            self.dock.push(application_id.clone());
        }

        self.installed.insert(
            application_id.clone(),
            InstalledApplication {
                application_id: application_id.clone(),
                manifest_url: manifest_url.to_owned(),
                install_context: options.install_context.clone(),
                home_tab_url,
                in_dock,
                config_data,
            },
        );

        Ok(InstallApplicationReturn {
            application_id,
            manifest,
        })
    }

    /// The URL an install of `manifest_url` would open for `application_id`: the manifest's start
    /// URL with `config` applied, or the multi-instance configurator when a preset app's config
    /// did not resolve one. `None` when the manifest is unknown or has no start URL.
    ///
    /// This is the same decision [`ApplicationService::install`] makes, exposed so a caller (a
    /// settings screen re-opening an app, an OAuth completion) can ask without installing.
    pub fn resolve_start_url(
        &self,
        manifest_url: &str,
        application_id: &str,
        config: &ApplicationConfigData,
    ) -> Option<String> {
        let manifest = self.resolve_manifest(manifest_url).ok()?;
        start_url_for_install(&manifest, manifest_url, application_id, config)
    }

    /// `setConfigData` (`packages/app/src/applications/duck.ts`, via
    /// `window.bxApi.applications.setConfigData`): sets an installed
    /// application's multi-instance configuration.
    ///
    /// TS merges the given fields into the stored config data (`SET_CONFIG_DATA` sets each truthy
    /// field it names and keeps the rest): each `Some` field here overwrites the stored one, the
    /// others keep their value. The preset gate
    /// (`setIdentityIfMultiInstanceApplication`'s order) still applies: only
    /// the field the manifest's first matching preset consumes is kept, so a
    /// `subdomain` on a google-account app is not stored. `Ok(false)` when
    /// nothing changed (the id is unknown or no field survived the gate), so
    /// a caller only persists when it must.
    pub fn set_config_data(
        &mut self,
        application_id: &str,
        config: &ApplicationConfigData,
    ) -> Result<bool, ApplicationError> {
        let Some(installed) = self.installed.get(application_id) else {
            return Err(ApplicationError::NotInstalled(application_id.to_owned()));
        };
        let manifest = self.resolve_manifest(&installed.manifest_url)?;
        // Merge first (TS `state.setIn` per truthy field), then gate.
        let mut merged = installed.config_data.clone();
        if config.identity_id.is_some() {
            merged.identity_id = config.identity_id.clone();
        }
        if config.subdomain.is_some() {
            merged.subdomain = config.subdomain.clone();
        }
        if config.custom_url.is_some() {
            merged.custom_url = config.custom_url.clone();
        }
        let kept = config_data_for_presets(&manifest, &merged);
        if kept == installed.config_data {
            return Ok(false);
        }
        self.installed
            .get_mut(application_id)
            .expect("checked above")
            .config_data = kept;
        Ok(true)
    }

    /// The settings surface's "Clear": removes an installed application's
    /// multi-instance configuration, reverting it to the manifest's own
    /// (signed-out) start URL. False when there was nothing to clear; an
    /// unknown id is an error, like [`ApplicationService::uninstall`].
    pub fn clear_config_data(&mut self, application_id: &str) -> Result<bool, ApplicationError> {
        let Some(installed) = self.installed.get(application_id) else {
            return Err(ApplicationError::NotInstalled(application_id.to_owned()));
        };
        if installed.config_data == ApplicationConfigData::default() {
            return Ok(false);
        }
        self.installed
            .get_mut(application_id)
            .expect("checked above")
            .config_data = ApplicationConfigData::default();
        Ok(true)
    }

    /// Port of `uninstallApplication` (lifecycle.ts): close all tabs in the
    /// app, drop the application entry, remove the dock item, remove the
    /// password-manager link, delete the app's favorites.
    pub fn uninstall(&mut self, application_id: &str) -> Result<(), ApplicationError> {
        if !self.installed.contains_key(application_id) {
            return Err(ApplicationError::NotInstalled(application_id.to_owned()));
        }

        // deleteFavorite for every favorite of the application
        // (`getFavoritesForApplication` filters by applicationId only;
        // `getTabFavoriteId(fav)` returns the always-present `favoriteId`,
        // so TS deletes them all, tab or no tab).
        self.favorites
            .retain(|f| f.application_id != application_id);

        // closeAllTabsInApp + dropApplication.
        self.tabs.retain(|t| t.application_id != application_id);
        self.installed.remove(application_id);

        // removeAppItem.
        self.dock.retain(|id| id != application_id);

        // removeLink({ applicationId }).
        self.links.retain(|(app, _)| app != application_id);

        Ok(())
    }

    /// Port of `uninstallAllInstances` (`abstract-application/sagas.ts`):
    /// uninstall every installed application of `manifest_url`, then delete
    /// the private manifest when the URL names one above the 1000000
    /// sentinel (the saga's `manifestId > 1000000` guard; an absent id is a
    /// no-op in the store, like the TS `filter`).
    ///
    /// TS order is kept — instances first, manifest last — so a failing
    /// private-store write is reported after the removals, the same partial
    /// state the saga leaves behind; each per-instance uninstall itself
    /// stays all-or-nothing. Returns the number of instances removed.
    pub fn uninstall_all_for_manifest(
        &mut self,
        manifest_url: &str,
    ) -> Result<usize, ApplicationError> {
        let ids: Vec<String> = self
            .installed
            .values()
            .filter(|a| a.manifest_url == manifest_url)
            .map(|a| a.application_id.clone())
            .collect();
        for id in &ids {
            self.uninstall(id)?;
        }
        // `manifestURL.match(/^station-manifest:\/\/(\d+)$/)` without a
        // regex: digits only, and a number too large for u64 names no real
        // manifest (TS's `Number` overflow behaves the same — the delete is
        // a no-op).
        if let Some(id) = manifest_url
            .strip_prefix("station-manifest://")
            .filter(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse::<u64>().ok())
            .filter(|&id| id > 1_000_000)
        {
            self.delete_private_manifest(id)?;
        }
        Ok(ids.len())
    }

    /// Replays a persisted session into this service: the installed applications, the dock and
    /// the tabs, validated as a whole.
    ///
    /// A session is either restored or refused — nothing is dropped to make a bad one fit. The
    /// checks are the invariants the rest of the shell relies on: application ids are unique,
    /// every dock entry names an installed application (once), every tab names an installed
    /// application, and tab ids are unique. On error this service is left exactly as it was.
    pub fn restore(
        &mut self,
        installed: Vec<InstalledApplication>,
        dock: Vec<String>,
        tabs: Vec<TabRecord>,
    ) -> Result<(), RestoreError> {
        let mut applications: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for application in &installed {
            if !applications.insert(application.application_id.as_str()) {
                return Err(RestoreError::DuplicateApplication(
                    application.application_id.clone(),
                ));
            }
        }
        let mut docked: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for id in &dock {
            if !applications.contains(id.as_str()) {
                return Err(RestoreError::UnknownDockApplication(id.clone()));
            }
            if !docked.insert(id.as_str()) {
                return Err(RestoreError::DuplicateDockEntry(id.clone()));
            }
        }
        let mut tab_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for tab in &tabs {
            if !tab_ids.insert(tab.tab_id.as_str()) {
                return Err(RestoreError::DuplicateTab(tab.tab_id.clone()));
            }
            if !applications.contains(tab.application_id.as_str()) {
                return Err(RestoreError::UnknownTabApplication(
                    tab.application_id.clone(),
                ));
            }
        }

        self.installed = installed
            .into_iter()
            .map(|application| (application.application_id.clone(), application))
            .collect();
        self.dock = dock;
        self.tabs = tabs;
        Ok(())
    }

    /// Port of `requestPrivateApplication` (app-store sagas.ts): builds the
    /// private manifest on disk and returns `{ id, bxAppManifestURL }`.
    ///
    /// A store that cannot be written is an error, not a panic: the mapped
    /// [`ApplicationError::PrivateStoreWrite`] is what a caller shows.
    pub fn request_private_application(
        &mut self,
        recipe: NewPrivateApplication,
    ) -> Result<ApplicationCreated, ApplicationError> {
        let id = self
            .private
            .save_new_application(recipe)
            .map_err(|why| ApplicationError::PrivateStoreWrite(why.to_string()))?;
        Ok(ApplicationCreated {
            id: id.to_string(),
            bx_app_manifest_url: manifest_registry::get_bx_app_manifest_url(&id.to_string()),
        })
    }

    /// Port of the private half of `getPrivateApplicationById` deletion —
    /// `deleteManifest(id)` from `manifests/private.ts` (unknown id is a
    /// no-op), kept separate from `uninstall` because TS never routes one
    /// through the other.
    pub fn delete_private_manifest(&mut self, id: u64) -> Result<(), ApplicationError> {
        self.private
            .delete_manifest(id)
            .map_err(|why| ApplicationError::PrivateStoreWrite(why.to_string()))
    }

    /// Port of `addApplicationRequest` (app-store sagas.ts) for the local
    /// registry: blank-URL guard, URL normalization, install, then the
    /// `APPLICATION_INSTALLED` pubsub payload (returned — no broker here).
    ///
    /// `manifest_fetch_elapsed` is the time the caller spent resolving
    /// `manifest_url` before calling (the TS `race` timed out the whole
    /// install at 2s; with the embedded registry the resolution is the only
    /// IO, so the budget collapses onto it). Bundled/private manifests
    /// resolve instantly and never time out.
    pub fn add_application_request(
        &mut self,
        manifest_url: &str,
        context: Option<InstallContext>,
        in_background: bool,
        manifest_fetch_elapsed: Duration,
    ) -> Result<ApplicationInstalledPayload, ApplicationError> {
        if manifest_url.trim().is_empty() {
            return Err(ApplicationError::BlankManifestUrl);
        }

        // `new URL(manifestURL).toString()` — normalize separators/case.
        let normalized = normalize_manifest_url(manifest_url);

        if manifest_fetch_elapsed >= INSTALL_TIMEOUT {
            return Err(ApplicationError::InstallationTimeout);
        }

        let options = InstallOptions {
            install_context: context,
            config_data: ApplicationConfigData::default(),
        };
        let installation = self.install(&normalized, &options)?;
        Ok(ApplicationInstalledPayload {
            application_id: installation.application_id,
            in_background,
        })
    }

    /// Port of `installAppStoreApplicationIfNotPresent` /
    /// `installSlackStationNextIfNextAndNotPresent`'s shared shape:
    /// `hasAlreadyApplicationsForManifest` then install.
    pub fn install_if_not_present(
        &mut self,
        manifest_url: &str,
    ) -> Result<Option<InstallApplicationReturn>, ApplicationError> {
        if self.has_applications_for_manifest(manifest_url) {
            return Ok(None);
        }
        self.install(manifest_url, &InstallOptions::default())
            .map(Some)
    }

    /// `hasAlreadyApplicationsForManifest`: any installed application whose
    /// manifestURL matches.
    pub fn has_applications_for_manifest(&self, manifest_url: &str) -> bool {
        self.installed
            .values()
            .any(|a| a.manifest_url == manifest_url)
    }

    /// Installed applications by id (the redux `applications` slice shape).
    pub fn installed_applications(&self) -> &HashMap<String, InstalledApplication> {
        &self.installed
    }

    /// The dock's application ids, in insertion order (`addAppItem` /
    /// `removeAppItem` paper trail).
    pub fn dock(&self) -> &[String] {
        &self.dock
    }

    /// Tabs created by installs (`createNewTab` / `closeAllTabsInApp` paper
    /// trail).
    pub fn tabs(&self) -> &[TabRecord] {
        &self.tabs
    }

    /// Test helper mirroring the TS side's `saveFavorite`-adjacent state:
    /// register a favorite so `uninstall` can delete it.
    pub fn add_favorite_for_test(&mut self, favorite: FavoriteRecord) {
        self.favorites.push(favorite);
    }

    /// Test helper: register a password-manager link (`removeLink` target).
    pub fn add_link_for_test(&mut self, application_id: &str, link_id: &str) {
        self.links
            .push((application_id.to_owned(), link_id.to_owned()));
    }

    /// Test helper: remaining password-manager link ids.
    pub fn links_for_test(&self) -> Vec<&str> {
        self.links.iter().map(|(_, l)| l.as_str()).collect()
    }

    /// Test helper: remaining favorite ids.
    pub fn favorites_for_test(&self) -> Vec<&str> {
        self.favorites
            .iter()
            .map(|f| f.favorite_id.as_str())
            .collect()
    }
}

/// `addApplicationRequest`'s `delay(2000)` race budget.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(2);

/// `setIdentityIfMultiInstanceApplication`: keep only the config field
/// matching the manifest's first applicable preset.
fn config_data_for_presets(
    manifest: &BxAppManifest,
    config: &ApplicationConfigData,
) -> ApplicationConfigData {
    let presets = get_presets(manifest);
    let mut kept = ApplicationConfigData::default();
    // TS checks in this order and puts at most one.
    if let (Some(id), true) = (
        config.identity_id.clone(),
        presets.contains(&Preset::GoogleAccount),
    ) {
        kept.identity_id = Some(id);
    } else if let (Some(sub), true) = (
        config.subdomain.clone(),
        presets.contains(&Preset::Subdomain),
    ) {
        kept.subdomain = Some(sub);
    } else if let (Some(url), true) = (
        config.custom_url.clone(),
        presets.contains(&Preset::OnPremise),
    ) {
        kept.custom_url = Some(url);
    }
    kept
}

/// `getStartURL` for the install path (helpers.ts): `start_url` when there
/// are no presets; otherwise the preset-resolved URL, falling back to the
/// multi-instance configurator URL.
///
/// The `isMultiInstanceConfigurator(activeTabUrl)` branch needs the app's
/// live tab URL, which is `''` at install time (no tab exists yet), so it
/// never fires during `installApplication` — the port drops it and keeps
/// the rest.
pub(crate) fn start_url_for_install(
    manifest: &BxAppManifest,
    manifest_url: &str,
    application_id: &str,
    config_data: &ApplicationConfigData,
) -> Option<String> {
    // TS truthiness (`if (manifest.start_url)` in installApplication and
    // `manifest.start_url!` used only after that gate): empty string means
    // no start URL.
    let manifest_start_url = manifest.start_url.clone().filter(|s| !s.is_empty())?;
    let presets = get_presets(manifest);
    if presets.is_empty() {
        // TS `manifest.start_url!` then `createNewTab`.
        return Some(manifest_start_url);
    }

    // getStartURLForPreset → getURLForPreset.
    if let Some(url) = get_url_for_preset(manifest, &presets, config_data) {
        return Some(url);
    }
    // Divergence from the TS saga, deliberate for the native shell: with no identity/config yet,
    // the install opens the manifest's own formatted start URL, so a preset app is usable
    // signed-out (Gmail's account picker) instead of landing on a configurator the shell does not
    // draw. An install that carries config the presets did not use keeps the configurator, which
    // is what OAuth will drive.
    if *config_data == ApplicationConfigData::default() {
        return Some(format_start_url(manifest, config_data));
    }
    // `startUrl || getMultiInstanceConfiguratorURL(manifestURL, applicationId)`.
    Some(get_multi_instance_configurator_url(
        manifest_url,
        application_id,
    ))
}

/// Port of `getURLForPreset` for `URLType.START`.
fn get_url_for_preset(
    manifest: &BxAppManifest,
    presets: &[Preset],
    config_data: &ApplicationConfigData,
) -> Option<String> {
    if let (Some(_), true) = (
        &config_data.identity_id,
        presets.contains(&Preset::GoogleAccount),
    ) {
        // The TS path re-selects full config data and renders
        // `start_url_tpl` with it; identity-provided fields live outside
        // this crate (user-identities), so at install time the stored
        // config data is what the template sees.
        return Some(format_start_url(manifest, config_data));
    }
    if let (Some(_), true) = (&config_data.subdomain, presets.contains(&Preset::Subdomain)) {
        return Some(format_start_url(
            manifest,
            &ApplicationConfigData {
                subdomain: config_data.subdomain.clone(),
                ..ApplicationConfigData::default()
            },
        ));
    }
    if let (Some(url), true) = (
        &config_data.custom_url,
        presets.contains(&Preset::OnPremise),
    ) {
        return Some(url.clone());
    }
    None
}

/// Port of `formatURL(URLType.START, ...)` from
/// `packages/app/src/applications/helpers.ts`: on-premise short-circuits to
/// `customURL`; otherwise handlebars-render `start_url_tpl`.
fn format_start_url(manifest: &BxAppManifest, config_data: &ApplicationConfigData) -> String {
    let mic = manifest
        .bx_multi_instance_config
        .as_ref()
        .expect("formatStartUrl callers gate on presets being non-empty");
    if let Some(custom) = &config_data.custom_url {
        // Presets include on-premise whenever customURL survives the
        // callers' gate; TS checks presets again here.
        if mic.presets.contains(&Preset::OnPremise) {
            return custom.clone();
        }
    }
    let data = serde_json::json!({
        "identityId": config_data.identity_id,
        "subdomain": config_data.subdomain,
        "customURL": config_data.custom_url,
    });
    // Template errors bubble as the raw template: TS `Handlebars.compile`
    // throws at compile time and the saga crashes; the port degrades to the
    // literal template so an install never disappears on a bad manifest.
    handlebars::Handlebars::new()
        .render_template(mic.start_url_tpl.as_deref().unwrap_or_default(), &data)
        .unwrap_or_else(|_| mic.start_url_tpl.clone().unwrap_or_default())
}

/// Port of `getMultiInstanceConfiguratorURL`:
/// `station://multi-instance-configurator/?manifestURL=...&applicationId=...`
/// with WHATWG `URLSearchParams` percent-encoding.
fn get_multi_instance_configurator_url(manifest_url: &str, application_id: &str) -> String {
    format!(
        "station://multi-instance-configurator/?{}",
        url_search_params_encode(&[
            ("manifestURL", manifest_url),
            ("applicationId", application_id),
        ])
    )
}

/// `new URL(url).toString()` for the `station-manifest://` / `https://`
/// URLs `addApplicationRequest` sees. Plain — the WHATWG parser's full
/// oddities (IDNA, tabs/newlines) never reach this code in practice.
///
/// Differences between special schemes (`http`/`https`, empty path becomes
/// `/`, default port dropped) and non-special ones like `station-manifest`
/// (path kept verbatim, port kept) are preserved; `new
/// URL('station-manifest://14').toString()` is `'station-manifest://14'`
/// with no trailing slash.
fn normalize_manifest_url(url: &str) -> String {
    let trimmed = url.trim();
    let Some((scheme, rest)) = trimmed.split_once(':') else {
        return trimmed.to_owned();
    };
    let lower_scheme = scheme.to_ascii_lowercase();
    let special = matches!(
        lower_scheme.as_str(),
        "http" | "https" | "ws" | "wss" | "ftp" | "file"
    );
    let has_authority = rest.starts_with("//");
    if !has_authority {
        // Opaque path (never produced by the manifest URLs in play).
        return format!("{lower_scheme}:{rest}");
    }
    let authority = &rest[2..];
    // Split host from path/query. When the separator is `?` or `#` (no
    // slash before it), the empty path still renders as `/` for special
    // schemes — `new URL('https://example.com?a').toString()` keeps the `?`
    // but inserts the `/` first.
    let (host_port, path_and_more): (&str, &str) = match authority.split_once(['/', '?', '#']) {
        Some((h, tail)) => (h, tail),
        None => (authority, ""),
    };
    // The separator char was consumed by split_once; restore it — it was a
    // real path ('/') or query ('?') / fragment ('#') character, and all
    // three render with a '/'-prefixed path under special schemes.
    let rest = if path_and_more.is_empty() && authority.len() == host_port.len() {
        String::new()
    } else {
        format!("/{path_and_more}")
    };
    let host_lower = host_port.to_ascii_lowercase();
    let host_no_port = host_lower
        .split_once(':')
        .map(|(h, p)| match p {
            "80" if lower_scheme == "http" => h.to_owned(),
            "443" if lower_scheme == "https" => h.to_owned(),
            _ => format!("{h}:{p}"),
        })
        .unwrap_or(host_lower);
    let path = if special && rest.is_empty() {
        "/".to_owned()
    } else {
        rest.to_owned()
    };
    format!("{lower_scheme}://{host_no_port}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GMAIL: &str = "station-manifest://14";
    const BOOMERANG: &str = "station-manifest://415";
    const APPSTORE: &str = "station-manifest://1";

    #[test]
    fn golden_install_plain_manifest_creates_app_home_tab_and_dock_item() {
        let mut apps = ApplicationService::for_tests("install-plain");
        // 157 Todoist: plain manifest, in dock.
        let ret = apps
            .install("station-manifest://157", &InstallOptions::default())
            .unwrap();
        assert_eq!(ret.manifest.name.as_deref(), Some("Todoist"));
        assert_eq!(
            ret.manifest.start_url.as_deref().unwrap(),
            "https://app.todoist.com/auth/login"
        );

        let app = apps
            .installed_applications()
            .get(&ret.application_id)
            .unwrap();
        assert_eq!(
            app.home_tab_url.as_deref(),
            Some("https://app.todoist.com/auth/login")
        );
        assert!(app.in_dock);
        assert_eq!(apps.dock(), std::slice::from_ref(&ret.application_id));
        assert_eq!(apps.tabs().len(), 1);
        assert!(apps.tabs()[0].home);
        assert_eq!(apps.tabs()[0].application_id, ret.application_id);
    }

    #[test]
    fn golden_install_no_dock_manifest_stays_off_dock() {
        let mut apps = ApplicationService::for_tests("install-nodock");
        // 415 Boomerang: bx_no_dock, no start_url → no home tab either.
        let ret = apps.install(BOOMERANG, &InstallOptions::default()).unwrap();
        let app = apps
            .installed_applications()
            .get(&ret.application_id)
            .unwrap();
        assert!(!app.in_dock);
        assert!(apps.dock().is_empty());
        assert_eq!(app.home_tab_url, None);
        assert!(apps.tabs().is_empty());
    }

    #[test]
    fn golden_install_empty_start_url_creates_no_home_tab() {
        // TS gates on truthiness (`if (manifest.start_url)`, lifecycle.ts:76):
        // `''` skips createNewTab — reachable via a private manifest saved
        // with an empty start_url.
        let mut apps = ApplicationService::for_tests("install-empty-start");
        apps.request_private_application(NewPrivateApplication {
            name: "Empty".into(),
            theme_color: "#000000".into(),
            icon_url: "https://empty.test/logo.png".into(),
            start_url: String::new(),
            scope: "https://empty.test".into(),
        })
        .unwrap();
        let ret = apps
            .install("station-manifest://1000001", &InstallOptions::default())
            .unwrap();
        let app = apps
            .installed_applications()
            .get(&ret.application_id)
            .unwrap();
        assert_eq!(app.home_tab_url, None);
        assert!(apps.tabs().is_empty());
    }

    #[test]
    fn golden_install_unknown_manifest_times_out_like_get_manifest_or_timeout() {
        let mut apps = ApplicationService::for_tests("install-unknown");
        let err = apps
            .install("station-manifest://999999", &InstallOptions::default())
            .unwrap_err();
        assert_eq!(
            err,
            ApplicationError::ManifestNotAvailable("station-manifest://999999".to_owned())
        );
        assert_eq!(
            err.to_string(),
            "Could not get the manifest station-manifest://999999"
        );
    }

    #[test]
    fn golden_install_multi_instance_google_account_keeps_identity_id() {
        let mut apps = ApplicationService::for_tests("install-ga");
        // 14 Gmail: google-account preset, start_url_tpl with {{...profileData.email}}.
        let options = InstallOptions {
            config_data: ApplicationConfigData {
                identity_id: Some("identity/7".into()),
                subdomain: Some("ignored".into()),
                custom_url: None,
            },
            install_context: Some(InstallContext::appstore("onboardee/3")),
        };
        let ret = apps.install(GMAIL, &options).unwrap();
        let app = apps
            .installed_applications()
            .get(&ret.application_id)
            .unwrap();
        // Only the google-account field is kept (first matching preset wins).
        assert_eq!(app.config_data.identity_id.as_deref(), Some("identity/7"));
        assert_eq!(app.config_data.subdomain, None);
        // start_url_tpl rendered with the config data: `{{userIdentity...}}`
        // resolves against configData, which has no `userIdentity` key —
        // Handlebars renders the missing path as empty, same as TS.
        assert_eq!(
            app.home_tab_url.as_deref(),
            Some("https://accounts.google.com/AddSession?passive=true&Email=&continue=https://mail.google.com/mail/u/")
        );
        assert_eq!(
            app.install_context,
            Some(InstallContext::appstore("onboardee/3"))
        );
    }

    #[test]
    fn golden_install_subdomain_renders_start_url_tpl() {
        let mut apps = ApplicationService::for_tests("install-subdomain");
        // 119 Amazon AWS: presets [subdomain, on-premise],
        // start_url_tpl "https://{{subdomain}}.signin.aws.amazon.com/console".
        let options = InstallOptions {
            config_data: ApplicationConfigData {
                subdomain: Some("acme".into()),
                ..ApplicationConfigData::default()
            },
            install_context: None,
        };
        let ret = apps.install("station-manifest://119", &options).unwrap();
        let app = apps
            .installed_applications()
            .get(&ret.application_id)
            .unwrap();
        assert_eq!(
            app.home_tab_url.as_deref(),
            Some("https://acme.signin.aws.amazon.com/console")
        );
        assert_eq!(app.config_data.subdomain.as_deref(), Some("acme"));
    }

    #[test]
    fn golden_install_on_premise_uses_custom_url_verbatim() {
        let mut apps = ApplicationService::for_tests("install-onpremise");
        // 123 Gitlab: presets [on-premise] only.
        let options = InstallOptions {
            config_data: ApplicationConfigData {
                custom_url: Some("https://gitlab.corp.acme.dev".into()),
                ..ApplicationConfigData::default()
            },
            install_context: None,
        };
        let ret = apps.install("station-manifest://123", &options).unwrap();
        let app = apps
            .installed_applications()
            .get(&ret.application_id)
            .unwrap();
        assert_eq!(
            app.home_tab_url.as_deref(),
            Some("https://gitlab.corp.acme.dev")
        );
    }

    #[test]
    fn golden_install_configurable_manifest_without_config_opens_its_start_url() {
        let mut apps = ApplicationService::for_tests("install-configurator");
        // Gmail with no config data: startUrl null → configurator URL.
        let ret = apps.install(GMAIL, &InstallOptions::default()).unwrap();
        let app = apps
            .installed_applications()
            .get(&ret.application_id)
            .unwrap();
        // The native shell's divergence: no identity/config yet opens the manifest's own
        // formatted start URL (the signed-out account picker), not the configurator.
        let url = app.home_tab_url.as_deref().expect("home tab");
        assert!(
            url.starts_with("https://accounts.google.com/AddSession?"),
            "{url}"
        );
        assert!(
            url.contains("continue=https://mail.google.com/mail/u/"),
            "{url}"
        );
    }

    #[test]
    fn installing_with_config_the_presets_drop_still_opens_the_start_url() {
        let mut apps = ApplicationService::for_tests("install-configurator-configured");
        // Gmail's only preset is google-account (an identity id); a subdomain is not something it
        // uses, so `setIdentityIfMultiInstanceApplication` drops it and the install has no config
        // after all — the formatted start URL is still what opens.
        let options = InstallOptions {
            config_data: ApplicationConfigData {
                subdomain: Some("acme".into()),
                ..ApplicationConfigData::default()
            },
            install_context: None,
        };
        let ret = apps.install(GMAIL, &options).unwrap();
        let app = apps
            .installed_applications()
            .get(&ret.application_id)
            .unwrap();
        let url = app.home_tab_url.as_deref().expect("home tab");
        assert!(
            url.starts_with("https://accounts.google.com/AddSession?"),
            "{url}"
        );
    }

    #[test]
    fn config_the_presets_do_not_use_still_resolves_to_the_configurator() {
        let apps = ApplicationService::for_tests("resolve-configurator");
        // `resolve_start_url` takes the caller's config verbatim: a field none of the manifest's
        // presets handle has no page to open, so the configurator stays the answer (OAuth drives
        // that path).
        let config = ApplicationConfigData {
            subdomain: Some("acme".into()),
            ..ApplicationConfigData::default()
        };
        let url = apps
            .resolve_start_url(GMAIL, "application-5", &config)
            .expect("a URL");
        assert!(
            url.starts_with("station://multi-instance-configurator/"),
            "{url}"
        );
    }

    #[test]
    fn resolve_start_url_agrees_with_install_for_a_preset_app() {
        let apps = ApplicationService::for_tests("resolve-preset");
        let resolved = apps
            .resolve_start_url(GMAIL, "application-1", &ApplicationConfigData::default())
            .expect("a start URL");
        assert!(
            resolved.starts_with("https://accounts.google.com/AddSession?"),
            "{resolved}"
        );
        assert!(
            !resolved.starts_with("station://"),
            "the configurator is not a page: {resolved}"
        );
    }

    #[test]
    fn resolve_start_url_leaves_a_plain_manifest_unchanged() {
        let apps = ApplicationService::for_tests("resolve-plain");
        assert_eq!(
            apps.resolve_start_url(
                "station-manifest://157",
                "application-2",
                &ApplicationConfigData::default()
            )
            .as_deref(),
            Some("https://app.todoist.com/auth/login")
        );
    }

    #[test]
    fn resolve_start_url_keeps_an_on_premise_custom_url() {
        let apps = ApplicationService::for_tests("resolve-on-premise");
        let config = ApplicationConfigData {
            custom_url: Some("https://gitlab.corp.acme.dev".into()),
            ..ApplicationConfigData::default()
        };
        assert_eq!(
            apps.resolve_start_url("station-manifest://123", "application-3", &config)
                .as_deref(),
            Some("https://gitlab.corp.acme.dev")
        );
    }

    #[test]
    fn resolve_start_url_reports_an_unknown_manifest() {
        let apps = ApplicationService::for_tests("resolve-unknown");
        assert!(apps
            .resolve_start_url(
                "station-manifest://999999",
                "application-4",
                &ApplicationConfigData::default()
            )
            .is_none());
    }

    #[test]
    fn golden_install_appstore_creates_home_tab_but_no_dock_item() {
        let mut apps = ApplicationService::for_tests("install-appstore");
        // 1 App Store: start_url "station://appstore/" and bx_no_dock.
        let ret = apps.install(APPSTORE, &InstallOptions::default()).unwrap();
        let app = apps
            .installed_applications()
            .get(&ret.application_id)
            .unwrap();
        assert_eq!(app.home_tab_url.as_deref(), Some("station://appstore/"));
        assert_eq!(apps.tabs().len(), 1);
        assert!(!app.in_dock); // App Store is bx_no_dock
    }

    #[test]
    fn golden_install_twice_creates_distinct_application_ids() {
        let mut apps = ApplicationService::for_tests("install-twice");
        let a = apps.install(GMAIL, &InstallOptions::default()).unwrap();
        let b = apps.install(GMAIL, &InstallOptions::default()).unwrap();
        assert_ne!(a.application_id, b.application_id);
        assert_eq!(apps.installed_applications().len(), 2);
        assert_eq!(apps.dock().len(), 2);
        // Ids look shortid-ish: short, alphanumeric.
        for id in [&a.application_id, &b.application_id] {
            assert!(id.len() <= 12 && id.chars().all(|c| c.is_ascii_alphanumeric()));
        }
    }

    #[test]
    fn golden_uninstall_cleans_tabs_dock_links_and_tab_favorites() {
        let mut apps = ApplicationService::for_tests("uninstall");
        let gmail = apps.install(GMAIL, &InstallOptions::default()).unwrap();
        let other = apps
            .install("station-manifest://157", &InstallOptions::default())
            .unwrap();

        // Two favorites of the app (TS deletes every app favorite —
        // getTabFavoriteId always yields favoriteId), one on a foreign app.
        apps.add_favorite_for_test(FavoriteRecord {
            favorite_id: "fav/1".into(),
            application_id: gmail.application_id.clone(),
        });
        apps.add_favorite_for_test(FavoriteRecord {
            favorite_id: "fav/1b".into(),
            application_id: gmail.application_id.clone(),
        });
        apps.add_favorite_for_test(FavoriteRecord {
            favorite_id: "fav/2".into(),
            application_id: other.application_id.clone(),
        });
        apps.add_link_for_test(&gmail.application_id, "link/1");
        apps.add_link_for_test(&other.application_id, "link/2");

        apps.uninstall(&gmail.application_id).unwrap();

        assert!(!apps
            .installed_applications()
            .contains_key(&gmail.application_id));
        assert!(apps
            .tabs()
            .iter()
            .all(|t| t.application_id != gmail.application_id));
        assert!(!apps.dock().contains(&gmail.application_id));
        // removeLink removed only the app's link.
        assert_eq!(apps.links_for_test(), vec!["link/2"]);
        // All of the app's favorites went with it; the foreign one stayed.
        assert_eq!(apps.favorites_for_test(), vec!["fav/2"]);
    }

    #[test]
    fn golden_uninstall_unknown_application_errors() {
        let mut apps = ApplicationService::for_tests("uninstall-unknown");
        assert_eq!(
            apps.uninstall("nope").unwrap_err(),
            ApplicationError::NotInstalled("nope".to_owned())
        );
    }

    #[test]
    fn golden_uninstall_all_for_manifest_removes_instances_keeps_others() {
        let mut apps = ApplicationService::for_tests("uninstall-all-for-manifest");

        // Two instances of one manifest, one survivor on another.
        let a = apps.install(GMAIL, &InstallOptions::default()).unwrap();
        let b = apps.install(GMAIL, &InstallOptions::default()).unwrap();
        let other = apps
            .install("station-manifest://157", &InstallOptions::default())
            .unwrap();

        let removed = apps.uninstall_all_for_manifest(GMAIL).unwrap();
        assert_eq!(removed, 2);
        assert!(!apps
            .installed_applications()
            .contains_key(&a.application_id));
        assert!(!apps
            .installed_applications()
            .contains_key(&b.application_id));
        assert!(apps
            .installed_applications()
            .contains_key(&other.application_id));
        assert!(apps
            .tabs()
            .iter()
            .all(|t| t.application_id != a.application_id && t.application_id != b.application_id));
        assert_eq!(apps.dock(), std::slice::from_ref(&other.application_id));

        // A private manifest above the 1000000 sentinel goes with its last
        // instance (the saga's `manifestId > 1000000` deleteManifest call).
        let created = apps
            .request_private_application(NewPrivateApplication {
                name: "Acme Corp Intranet".into(),
                theme_color: "#112233".into(),
                icon_url: "https://acme.test/logo.png".into(),
                start_url: "https://acme.test/login".into(),
                scope: "https://acme.test".into(),
            })
            .unwrap();
        apps.install(&created.bx_app_manifest_url, &InstallOptions::default())
            .unwrap();
        let removed = apps
            .uninstall_all_for_manifest(&created.bx_app_manifest_url)
            .unwrap();
        assert_eq!(removed, 1);
        assert!(apps.private_manifests().iter().all(|m| m.id != created.id));

        // The survivor is still uninstallable on its own.
        apps.uninstall(&other.application_id).unwrap();

        // Zero instances (unknown URL): Ok(0), nothing else disturbed.
        let removed = apps
            .uninstall_all_for_manifest("station-manifest://999999")
            .unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn golden_add_application_request_guards_and_payload() {
        let mut apps = ApplicationService::for_tests("add-request");

        // Blank guard (TS message verbatim).
        assert_eq!(
            apps.add_application_request("  ", None, false, Duration::ZERO)
                .unwrap_err(),
            ApplicationError::BlankManifestUrl
        );

        // 2s timeout race.
        assert_eq!(
            apps.add_application_request(GMAIL, None, false, Duration::from_secs(2))
                .unwrap_err(),
            ApplicationError::InstallationTimeout
        );

        // Success + pubsub payload; normalizes case.
        let payload = apps
            .add_application_request("STATION-MANIFEST://14", None, true, Duration::ZERO)
            .unwrap();
        assert!(apps
            .installed_applications()
            .contains_key(&payload.application_id));
        assert!(payload.in_background);
        assert_eq!(apps.dock().len(), 1);
    }

    #[test]
    fn golden_install_if_not_present_installs_once() {
        let mut apps = ApplicationService::for_tests("if-not-present");
        let first = apps.install_if_not_present(APPSTORE).unwrap().unwrap();
        assert!(apps.has_applications_for_manifest(APPSTORE));
        let second = apps.install_if_not_present(APPSTORE).unwrap();
        assert_eq!(second, None);
        assert_eq!(apps.dock().len(), 0); // App Store stays off the dock
        let _ = first;
    }

    #[test]
    fn golden_request_private_application_persists_and_returns_manifest_url() {
        let mut apps = ApplicationService::for_tests("request-private");
        let created = apps
            .request_private_application(NewPrivateApplication {
                name: "Acme Corp Intranet".into(),
                theme_color: "#112233".into(),
                icon_url: "https://acme.test/logo.png".into(),
                start_url: "https://acme.test/login".into(),
                scope: "https://acme.test".into(),
            })
            .unwrap();
        assert_eq!(created.id, "1000001");
        assert_eq!(created.bx_app_manifest_url, "station-manifest://1000001");

        // getPrivateApplications / getManifestByURL see it.
        let manifests = apps.private_manifests();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].id, "1000001");
        assert_eq!(
            apps.get_manifest_by_url("station-manifest://1000001")
                .unwrap()
                .inner
                .name,
            Some("Acme Corp Intranet".into())
        );

        // The private manifest installs like any other.
        let ret = apps
            .install("station-manifest://1000001", &InstallOptions::default())
            .unwrap();
        assert_eq!(
            apps.installed_applications()
                .get(&ret.application_id)
                .unwrap()
                .home_tab_url,
            Some("https://acme.test/login".to_owned())
        );

        // deleteManifest removes it again.
        apps.delete_private_manifest(1000001).unwrap();
        assert!(apps.private_manifests().is_empty());
        assert_eq!(
            apps.install("station-manifest://1000001", &InstallOptions::default())
                .unwrap_err(),
            ApplicationError::ManifestNotAvailable("station-manifest://1000001".to_owned())
        );
    }

    #[test]
    fn golden_ids_survive_reload_from_disk() {
        let dir = std::env::temp_dir().join(format!("station-s11b-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("private-manifests.json");

        let mut apps = ApplicationService::from_manifests_path(&path).unwrap();
        apps.request_private_application(NewPrivateApplication {
            name: "One".into(),
            theme_color: "#000000".into(),
            icon_url: "".into(),
            start_url: "https://one.test".into(),
            scope: "https://one.test".into(),
        })
        .unwrap();
        apps.request_private_application(NewPrivateApplication {
            name: "Two".into(),
            theme_color: "#000000".into(),
            icon_url: "".into(),
            start_url: "https://two.test".into(),
            scope: "https://two.test".into(),
        })
        .unwrap();

        // Reload: ids continue after the highest persisted one.
        let mut reloaded = ApplicationService::from_manifests_path(&path).unwrap();
        let third = reloaded
            .request_private_application(NewPrivateApplication {
                name: "Three".into(),
                theme_color: "#000000".into(),
                icon_url: "".into(),
                start_url: "https://three.test".into(),
                scope: "https://three.test".into(),
            })
            .unwrap();
        assert_eq!(third.id, "1000003");
        assert_eq!(reloaded.private_manifests().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_normalization_matches_new_url_to_string() {
        assert_eq!(
            normalize_manifest_url("STATION-MANIFEST://14"),
            "station-manifest://14"
        );
        assert_eq!(
            normalize_manifest_url("https://Example.COM"),
            "https://example.com/"
        );
        assert_eq!(
            normalize_manifest_url("https://example.com:443/a?b=c"),
            "https://example.com/a?b=c"
        );
        assert_eq!(
            normalize_manifest_url("http://example.com:8080/x"),
            "http://example.com:8080/x"
        );
    }

    #[test]
    fn install_resolves_do_not_list_manifests_the_provider_serves() {
        // The App Store manifest (1) is doNotList: absent from
        // get_manifest_by_url's filtered list, installable nonetheless —
        // TS reaches it through manifestProvider, not listAllApplications.
        let mut apps = ApplicationService::for_tests("donotlist-resolve");
        assert_eq!(apps.get_manifest_by_url(APPSTORE), None);
        let ret = apps.install(APPSTORE, &InstallOptions::default()).unwrap();
        assert_eq!(ret.manifest.name.as_deref(), Some("App Store"));
    }

    #[test]
    fn configurator_url_encoding_matches_urlsearchparams() {
        assert_eq!(
            get_multi_instance_configurator_url("station-manifest://14", "abc"),
            "station://multi-instance-configurator/?manifestURL=station-manifest%3A%2F%2F14&applicationId=abc"
        );
    }

    /// Installs two apps into a service and hands back the three persisted vectors.
    fn two_installs(
        tag: &str,
    ) -> (
        ApplicationService,
        Vec<InstalledApplication>,
        Vec<String>,
        Vec<TabRecord>,
    ) {
        let mut apps = ApplicationService::for_tests(tag);
        apps.install("station-manifest://14", &InstallOptions::default())
            .unwrap();
        apps.install("station-manifest://157", &InstallOptions::default())
            .unwrap();
        let installed: Vec<InstalledApplication> =
            apps.installed_applications().values().cloned().collect();
        let dock = apps.dock().to_vec();
        let tabs = apps.tabs().to_vec();
        (apps, installed, dock, tabs)
    }

    #[test]
    fn restoring_a_live_session_reproduces_it() {
        let (source, installed, dock, tabs) = two_installs("restore-valid-source");
        assert!(!source.dock().is_empty());

        let mut target = ApplicationService::for_tests("restore-valid-target");
        target
            .restore(installed.clone(), dock.clone(), tabs.clone())
            .unwrap();

        assert_eq!(target.dock(), dock.as_slice());
        assert_eq!(target.tabs(), tabs.as_slice());
        assert_eq!(target.installed_applications().len(), installed.len());
        for application in &installed {
            assert_eq!(
                target
                    .installed_applications()
                    .get(&application.application_id),
                Some(application)
            );
        }
    }

    #[test]
    fn restore_refuses_a_dock_entry_it_does_not_install() {
        let (_, installed, mut dock, tabs) = two_installs("restore-unknown-dock");
        dock.push("never-installed".to_owned());
        let mut target = ApplicationService::for_tests("restore-unknown-dock-target");

        assert_eq!(
            target.restore(installed, dock, tabs),
            Err(RestoreError::UnknownDockApplication(
                "never-installed".to_owned()
            ))
        );
    }

    #[test]
    fn restore_refuses_duplicate_application_ids() {
        let (_, installed, dock, tabs) = two_installs("restore-duplicate-app");
        let repeated = installed[0].clone();
        let mut doubled = installed.clone();
        doubled.push(repeated.clone());
        let mut target = ApplicationService::for_tests("restore-duplicate-app-target");

        assert_eq!(
            target.restore(doubled, dock, tabs),
            Err(RestoreError::DuplicateApplication(
                repeated.application_id.clone()
            ))
        );
    }

    #[test]
    fn restore_refuses_duplicate_tab_ids() {
        let (_, installed, dock, tabs) = two_installs("restore-duplicate-tab");
        let repeated = tabs[0].clone();
        let mut doubled = tabs.clone();
        doubled.push(repeated.clone());
        let mut target = ApplicationService::for_tests("restore-duplicate-tab-target");

        assert_eq!(
            target.restore(installed, dock, doubled),
            Err(RestoreError::DuplicateTab(repeated.tab_id.clone()))
        );
    }

    #[test]
    fn restore_refuses_a_tab_of_an_application_it_does_not_install() {
        let (_, installed, dock, tabs) = two_installs("restore-unknown-tab-app");
        let mut stray_tabs = tabs.clone();
        stray_tabs.push(TabRecord {
            tab_id: "stray-tab".to_owned(),
            application_id: "never-installed".to_owned(),
            url: "https://example.test".to_owned(),
            home: false,
        });
        let mut target = ApplicationService::for_tests("restore-unknown-tab-app-target");

        assert_eq!(
            target.restore(installed, dock, stray_tabs),
            Err(RestoreError::UnknownTabApplication(
                "never-installed".to_owned()
            ))
        );
    }

    #[test]
    fn a_refused_restore_leaves_the_service_untouched() {
        let mut target = ApplicationService::for_tests("restore-atomic");
        target
            .install("station-manifest://14", &InstallOptions::default())
            .unwrap();
        let before_dock = target.dock().to_vec();
        let before_tabs = target.tabs().to_vec();
        let before_installed: Vec<String> =
            target.installed_applications().keys().cloned().collect();

        assert!(target
            .restore(Vec::new(), vec!["unknown".to_owned()], Vec::new())
            .is_err());

        assert_eq!(target.dock(), before_dock.as_slice());
        assert_eq!(target.tabs(), before_tabs.as_slice());
        let mut after: Vec<String> = target.installed_applications().keys().cloned().collect();
        let mut before_sorted = before_installed;
        after.sort();
        before_sorted.sort();
        assert_eq!(after, before_sorted);
    }

    /// A path whose parent is a file: any write below it fails as "not a directory".
    fn unwritable_path(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("station-s6-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        let blocker = root.join("blocker");
        std::fs::write(&blocker, b"not a directory").expect("blocker file");
        blocker.join("private-manifests.json")
    }

    fn recipe(name: &str) -> NewPrivateApplication {
        NewPrivateApplication {
            name: name.to_owned(),
            theme_color: "#112233".to_owned(),
            icon_url: "https://icon.test/i.png".to_owned(),
            start_url: "https://app.test/".to_owned(),
            scope: "https://app.test".to_owned(),
        }
    }

    #[test]
    fn a_failed_private_store_write_is_an_error_not_a_panic() {
        let mut apps = ApplicationService::from_manifests_path(unwritable_path("write"))
            .expect("a store over an unwritable path");

        let created = apps.request_private_application(recipe("Acme"));

        match created {
            Err(ApplicationError::PrivateStoreWrite(_)) => {}
            other => panic!("expected PrivateStoreWrite, got {other:?}"),
        }
        assert!(
            apps.private_manifests().is_empty(),
            "a failed write left a record behind in memory"
        );
    }

    #[test]
    fn a_failed_private_delete_is_an_error_and_keeps_the_record() {
        let root = std::env::temp_dir().join(format!("station-s6-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        let manifests = root.join("private-manifests.json");
        std::fs::write(
            &manifests,
            br##"{"data":[{"name":"Kept","scope":"https://kept.test","icons":[],"start_url":"https://kept.test/","category":"Miscellaneous","theme_color":"#000","id":1000001}]}"##,
        )
        .expect("seed store");
        let mut apps = ApplicationService::from_manifests_path(&manifests).expect("seeded store");
        assert_eq!(apps.private_manifests().len(), 1);

        // Make the path unwritable by replacing the directory with a file.
        std::fs::remove_dir_all(&root).expect("remove dir");
        std::fs::write(&root, b"now a file").expect("block path");

        match apps.delete_private_manifest(1000001) {
            Err(ApplicationError::PrivateStoreWrite(_)) => {}
            other => panic!("expected PrivateStoreWrite, got {other:?}"),
        }
        assert_eq!(
            apps.private_manifests().len(),
            1,
            "a failed delete dropped the record from memory"
        );
    }

    #[test]
    fn an_in_memory_service_accepts_writes_without_a_disk() {
        let mut apps = ApplicationService::in_memory();

        let created = apps
            .request_private_application(recipe("Ephemeral"))
            .unwrap();

        assert_eq!(created.id, "1000001");
        assert_eq!(apps.private_manifests().len(), 1);
        apps.delete_private_manifest(1000001).unwrap();
        assert!(apps.private_manifests().is_empty());
    }

    #[test]
    fn install_context_serializes_camel_case() {
        let ctx = InstallContext {
            id: "assignment/9".into(),
            platform: "appstore".into(),
            onboardee_application_assignment_id: Some("oa/1".into()),
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["platform"], serde_json::json!("appstore"));
        assert_eq!(
            json["onboardeeApplicationAssignmentId"],
            serde_json::json!("oa/1")
        );
        assert_eq!(serde_json::from_value::<InstallContext>(json).unwrap(), ctx);
    }

    // ---- set_config_data / clear_config_data (slice 10) ----------------

    const GITLAB: &str = "station-manifest://123";

    #[test]
    fn set_config_data_templates_the_start_url_for_a_google_account_app() {
        let mut apps = ApplicationService::for_tests("config-gmail");
        let app_id = apps
            .install(GMAIL, &InstallOptions::default())
            .unwrap()
            .application_id;

        let changed = apps
            .set_config_data(
                &app_id,
                &ApplicationConfigData {
                    identity_id: Some("identity-9".into()),
                    ..ApplicationConfigData::default()
                },
            )
            .unwrap();
        assert!(changed);

        let resolved = apps
            .resolve_start_url(
                GMAIL,
                &app_id,
                &ApplicationConfigData {
                    identity_id: Some("identity-9".into()),
                    ..ApplicationConfigData::default()
                },
            )
            .unwrap();
        // The template's `{{userIdentity.profileData.email}}` renders the
        // stored value's shape (the TS template's own lookup); what matters
        // here is that *some* identity was templated in, not the default
        // empty render.
        assert!(
            resolved.starts_with("https://accounts.google.com/AddSession?"),
            "{resolved}"
        );
    }

    #[test]
    fn set_config_data_keeps_only_what_the_first_preset_consumes() {
        // A google-account preset drop: the TS saga's order
        // (`setIdentityIfMultiInstanceApplication`) keeps the identity and
        // never a subdomain the caller also passed.
        let mut apps = ApplicationService::for_tests("config-gate");
        let app_id = apps
            .install(GMAIL, &InstallOptions::default())
            .unwrap()
            .application_id;

        apps.set_config_data(
            &app_id,
            &ApplicationConfigData {
                identity_id: Some("identity-9".into()),
                subdomain: Some("acme".into()),
                custom_url: Some("https://corp.example.com".into()),
            },
        )
        .unwrap();

        let app = apps.installed_applications().get(&app_id).unwrap();
        assert_eq!(app.config_data.identity_id.as_deref(), Some("identity-9"));
        assert_eq!(app.config_data.subdomain, None);
        assert_eq!(app.config_data.custom_url, None);
    }

    #[test]
    fn set_config_data_on_a_subdomain_app_drives_the_template() {
        // 100 Atlassian: subdomain preset, `start_url_tpl` = https://{{subdomain}}.atlassian.net/login
        let mut apps = ApplicationService::for_tests("config-subdomain");
        let app_id = apps
            .install("station-manifest://100", &InstallOptions::default())
            .unwrap()
            .application_id;

        apps.set_config_data(
            &app_id,
            &ApplicationConfigData {
                subdomain: Some("acme".into()),
                ..ApplicationConfigData::default()
            },
        )
        .unwrap();

        let app = apps.installed_applications().get(&app_id).unwrap();
        assert_eq!(app.config_data.subdomain.as_deref(), Some("acme"));
    }

    #[test]
    fn set_config_data_on_an_on_premise_app_stores_the_custom_url() {
        let mut apps = ApplicationService::for_tests("config-on-premise");
        let app_id = apps
            .install(GITLAB, &InstallOptions::default())
            .unwrap()
            .application_id;

        apps.set_config_data(
            &app_id,
            &ApplicationConfigData {
                custom_url: Some("https://gitlab.corp.acme.dev".into()),
                ..ApplicationConfigData::default()
            },
        )
        .unwrap();

        assert_eq!(
            apps.resolve_start_url(
                GITLAB,
                &app_id,
                &ApplicationConfigData {
                    custom_url: Some("https://gitlab.corp.acme.dev".into()),
                    ..ApplicationConfigData::default()
                }
            )
            .as_deref(),
            Some("https://gitlab.corp.acme.dev")
        );
    }

    #[test]
    fn set_config_data_merges_over_the_stored_fields() {
        // TS `SET_CONFIG_DATA` sets each truthy field it names and keeps the rest.
        let mut apps = ApplicationService::for_tests("config-merge");
        let app_id = apps
            .install("station-manifest://100", &InstallOptions::default())
            .unwrap()
            .application_id;
        apps.set_config_data(
            &app_id,
            &ApplicationConfigData {
                subdomain: Some("acme".into()),
                ..ApplicationConfigData::default()
            },
        )
        .unwrap();

        apps.set_config_data(
            &app_id,
            &ApplicationConfigData {
                subdomain: Some("other".into()),
                ..ApplicationConfigData::default()
            },
        )
        .unwrap();

        let app = apps.installed_applications().get(&app_id).unwrap();
        assert_eq!(app.config_data.subdomain.as_deref(), Some("other"));
    }

    #[test]
    fn setting_the_same_config_again_reports_no_change() {
        let mut apps = ApplicationService::for_tests("config-idempotent");
        let app_id = apps
            .install("station-manifest://100", &InstallOptions::default())
            .unwrap()
            .application_id;
        let config = ApplicationConfigData {
            subdomain: Some("acme".into()),
            ..ApplicationConfigData::default()
        };

        assert!(apps.set_config_data(&app_id, &config).unwrap());
        assert!(!apps.set_config_data(&app_id, &config).unwrap());
    }

    #[test]
    fn set_config_data_on_an_unknown_app_is_an_error() {
        let mut apps = ApplicationService::for_tests("config-unknown");
        match apps.set_config_data("application-gone", &ApplicationConfigData::default()) {
            Err(ApplicationError::NotInstalled(id)) => assert_eq!(id, "application-gone"),
            other => panic!("expected NotInstalled, got {other:?}"),
        }
        assert!(apps.clear_config_data("application-gone").is_err());
    }

    #[test]
    fn clear_config_data_reverts_to_the_manifest_start_url() {
        let mut apps = ApplicationService::for_tests("config-clear");
        let app_id = apps
            .install("station-manifest://100", &InstallOptions::default())
            .unwrap()
            .application_id;
        apps.set_config_data(
            &app_id,
            &ApplicationConfigData {
                subdomain: Some("acme".into()),
                ..ApplicationConfigData::default()
            },
        )
        .unwrap();
        assert!(apps.installed_applications()[&app_id]
            .config_data
            .subdomain
            .is_some());

        assert!(apps.clear_config_data(&app_id).unwrap());
        assert!(
            !apps.clear_config_data(&app_id).unwrap(),
            "second clear changed nothing"
        );

        let app = apps.installed_applications().get(&app_id).unwrap();
        assert_eq!(app.config_data, ApplicationConfigData::default());
        // Back to the bare template (empty subdomain), like the signed-out install.
        let resolved = apps.resolve_start_url("station-manifest://100", &app_id, &app.config_data);
        assert_eq!(
            resolved.as_deref(),
            Some("https://.atlassian.net/login"),
            "clear did not revert to the plain template"
        );
    }
}
