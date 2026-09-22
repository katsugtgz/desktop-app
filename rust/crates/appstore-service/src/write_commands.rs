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
    get_presets, BxAppManifest, Manifest, NewPrivateApplication, PrivateStore, Preset,
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
    #[serde(default, rename = "identityId", skip_serializing_if = "Option::is_none")]
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

    /// Tests get an isolated temp-dir store without pulling in a tempfile
    /// dependency: a fresh folder under the system temp dir, removed when
    /// this process exits is fine for `cargo test` hygiene.
    #[doc(hidden)]
    pub fn for_tests(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("station-s11b-{tag}"));
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
            let m = manifest_registry::get_application_by_id(id)
                .or_else(|| id.parse::<u64>().ok().and_then(|n| self.private.get_private_application_by_id(n)));
            if let Some(m) = m {
                if manifest_registry::get_bx_app_manifest_url(&m.id) == manifest_url {
                    return Ok(m.inner);
                }
            }
        }
        Err(ApplicationError::ManifestNotAvailable(manifest_url.to_owned()))
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
        let home_tab_url = start_url_for_install(
            &manifest,
            manifest_url,
            &application_id,
            &config_data,
        );
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
        self.favorites.retain(|f| f.application_id != application_id);

        // closeAllTabsInApp + dropApplication.
        self.tabs
            .retain(|t| t.application_id != application_id);
        self.installed.remove(application_id);

        // removeAppItem.
        self.dock.retain(|id| id != application_id);

        // removeLink({ applicationId }).
        self.links.retain(|(app, _)| app != application_id);

        Ok(())
    }

    /// Port of `requestPrivateApplication` (app-store sagas.ts): builds the
    /// private manifest on disk and returns `{ id, bxAppManifestURL }`.
    pub fn request_private_application(
        &mut self,
        recipe: NewPrivateApplication,
    ) -> ApplicationCreated {
        let id = self.private.save_new_application(recipe);
        ApplicationCreated {
            id: id.to_string(),
            bx_app_manifest_url: manifest_registry::get_bx_app_manifest_url(&id.to_string()),
        }
    }

    /// Port of the private half of `getPrivateApplicationById` deletion —
    /// `deleteManifest(id)` from `manifests/private.ts` (unknown id is a
    /// no-op), kept separate from `uninstall` because TS never routes one
    /// through the other.
    pub fn delete_private_manifest(&mut self, id: u64) {
        self.private.delete_manifest(id);
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
        self.favorites.iter().map(|f| f.favorite_id.as_str()).collect()
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
    } else if let (Some(sub), true) = (config.subdomain.clone(), presets.contains(&Preset::Subdomain))
    {
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
    // `startUrl || getMultiInstanceConfiguratorURL(manifestURL, applicationId)`.
    Some(get_multi_instance_configurator_url(manifest_url, application_id))
}

/// Port of `getURLForPreset` for `URLType.START`.
fn get_url_for_preset(
    manifest: &BxAppManifest,
    presets: &[Preset],
    config_data: &ApplicationConfigData,
) -> Option<String> {
    if let (Some(_), true) = (&config_data.identity_id, presets.contains(&Preset::GoogleAccount)) {
        // The TS path re-selects full config data and renders
        // `start_url_tpl` with it; identity-provided fields live outside
        // this crate (user-identities), so at install time the stored
        // config data is what the template sees.
        return Some(format_start_url(manifest, config_data));
    }
    if let (Some(_), true) = (&config_data.subdomain, presets.contains(&Preset::Subdomain)) {
        return Some(format_start_url(manifest, &ApplicationConfigData {
            subdomain: config_data.subdomain.clone(),
            ..ApplicationConfigData::default()
        }));
    }
    if let (Some(url), true) = (&config_data.custom_url, presets.contains(&Preset::OnPremise)) {
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
    let special = matches!(lower_scheme.as_str(), "http" | "https" | "ws" | "wss" | "ftp" | "file");
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
        assert_eq!(ret.manifest.start_url.as_deref().unwrap(), "https://app.todoist.com/auth/login");

        let app = apps.installed_applications().get(&ret.application_id).unwrap();
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
        let ret = apps
            .install(BOOMERANG, &InstallOptions::default())
            .unwrap();
        let app = apps.installed_applications().get(&ret.application_id).unwrap();
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
        });
        let ret = apps
            .install("station-manifest://1000001", &InstallOptions::default())
            .unwrap();
        let app = apps.installed_applications().get(&ret.application_id).unwrap();
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
        assert_eq!(err.to_string(), "Could not get the manifest station-manifest://999999");
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
        let app = apps.installed_applications().get(&ret.application_id).unwrap();
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
        let app = apps.installed_applications().get(&ret.application_id).unwrap();
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
        let app = apps.installed_applications().get(&ret.application_id).unwrap();
        assert_eq!(
            app.home_tab_url.as_deref(),
            Some("https://gitlab.corp.acme.dev")
        );
    }

    #[test]
    fn golden_install_configurable_manifest_without_config_lands_on_configurator() {
        let mut apps = ApplicationService::for_tests("install-configurator");
        // Gmail with no config data: startUrl null → configurator URL.
        let ret = apps.install(GMAIL, &InstallOptions::default()).unwrap();
        let app = apps.installed_applications().get(&ret.application_id).unwrap();
        let expected = format!(
            "station://multi-instance-configurator/?manifestURL=station-manifest%3A%2F%2F14&applicationId={}",
            ret.application_id
        );
        assert_eq!(app.home_tab_url.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn golden_install_appstore_creates_home_tab_but_no_dock_item() {
        let mut apps = ApplicationService::for_tests("install-appstore");
        // 1 App Store: start_url "station://appstore/" and bx_no_dock.
        let ret = apps.install(APPSTORE, &InstallOptions::default()).unwrap();
        let app = apps.installed_applications().get(&ret.application_id).unwrap();
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
        let other = apps.install("station-manifest://157", &InstallOptions::default()).unwrap();

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

        assert!(!apps.installed_applications().contains_key(&gmail.application_id));
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
        let created = apps.request_private_application(NewPrivateApplication {
            name: "Acme Corp Intranet".into(),
            theme_color: "#112233".into(),
            icon_url: "https://acme.test/logo.png".into(),
            start_url: "https://acme.test/login".into(),
            scope: "https://acme.test".into(),
        });
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
            apps.installed_applications().get(&ret.application_id).unwrap().home_tab_url,
            Some("https://acme.test/login".to_owned())
        );

        // deleteManifest removes it again.
        apps.delete_private_manifest(1000001);
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
        });
        apps.request_private_application(NewPrivateApplication {
            name: "Two".into(),
            theme_color: "#000000".into(),
            icon_url: "".into(),
            start_url: "https://two.test".into(),
            scope: "https://two.test".into(),
        });

        // Reload: ids continue after the highest persisted one.
        let mut reloaded = ApplicationService::from_manifests_path(&path).unwrap();
        let third = reloaded.request_private_application(NewPrivateApplication {
            name: "Three".into(),
            theme_color: "#000000".into(),
            icon_url: "".into(),
            start_url: "https://three.test".into(),
            scope: "https://three.test".into(),
        });
        assert_eq!(third.id, "1000003");
        assert_eq!(reloaded.private_manifests().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }


    #[test]
    fn url_normalization_matches_new_url_to_string() {
        assert_eq!(normalize_manifest_url("STATION-MANIFEST://14"), "station-manifest://14");
        assert_eq!(normalize_manifest_url("https://Example.COM"), "https://example.com/");
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

    #[test]
    fn install_context_serializes_camel_case() {
        let ctx = InstallContext {
            id: "assignment/9".into(),
            platform: "appstore".into(),
            onboardee_application_assignment_id: Some("oa/1".into()),
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["platform"], serde_json::json!("appstore"));
        assert_eq!(json["onboardeeApplicationAssignmentId"], serde_json::json!("oa/1"));
        assert_eq!(serde_json::from_value::<InstallContext>(json).unwrap(), ctx);
    }
}
