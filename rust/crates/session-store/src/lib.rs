//! Durable session state for the native shell.
//!
//! One small JSON document holds what the shell restores on launch: window
//! placement, the active application, rail and dock order, installed
//! applications and tabs. The file carries an explicit [`FORMAT_VERSION`];
//! a version this build does not understand is refused instead of silently
//! discarded, and every [`SessionStore::save`] is atomic — a failure leaves
//! the previous session in place.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use appstore_service::{InstalledApplication, TabRecord};
use serde::{Deserialize, Serialize};

/// The only on-disk format this crate reads and writes.
pub const FORMAT_VERSION: u32 = 1;

/// The most a session document may occupy. A real one is a few KiB; anything near this is not a
/// session, and parsing it would only cost memory.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Caps on the collections a document may name. A real shell has a handful of apps; a hostile
/// document is refused instead of being materialized.
pub const MAX_INSTALLED: usize = 512;
pub const MAX_DOCK: usize = 512;
pub const MAX_TABS: usize = 2048;
pub const MAX_RAIL: usize = 512;
/// The longest one configuration value (an identity id, a subdomain or a custom URL) may be.
/// A real value is a shortid or a hostname; anything near this is not one.
pub const MAX_CONFIG_VALUE_CHARS: usize = 2048;

/// The most identities a session may store. A real user has one or two Google accounts; a
/// document asking for more is refused instead of materialized.
pub const MAX_IDENTITIES: usize = 32;

/// The longest one identity string may be (id, provider, user id and every profile field). An
/// email or display name is tens of characters; 256 is generous and bounded.
pub const MAX_IDENTITY_FIELD_CHARS: usize = 256;

/// The most pinned favorites a session may store.
pub const MAX_FAVORITES: usize = 256;

/// Maximum characters per field on a pinned [`FavoriteEntry`].
pub const MAX_FAVORITE_FIELD_CHARS: usize = 2048;

/// Maximum characters for a custom download directory path.
pub const MAX_DOWNLOAD_DIR_CHARS: usize = 2048;

/// The window size a session may ask for, in points. Smaller is raised to the minimum, larger is
/// clamped: a hostile document cannot open a 10,000,000-pixel window.
///
/// The largest size a session may *store*, in points: 8192 keeps a 3840-pt 4K@100 % window and a
/// 5120×1440 ultrawide exactly as the user left them. This bound is scale-unaware by design — the
/// display scale is not part of the document — so the shell applies the device-pixel ceiling when
/// it restores the window (`station-shell`'s `restored_size`), not here.
pub const MIN_WINDOW_WIDTH: f32 = 320.0;
pub const MIN_WINDOW_HEIGHT: f32 = 240.0;
pub const MAX_WINDOW: f32 = 8192.0;

/// Where the window was and how big it was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WindowState {
    /// `None` when the shell has never placed the window (or the platform
    /// gave no position); the OS then chooses.
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            width: 1200.0,
            height: 800.0,
            maximized: false,
        }
    }
}

/// How the shell picks its appearance.
///
/// Added to format 1 after it shipped: `#[serde(default)]` keeps existing documents readable
/// (they open as `System`), and a build that does not know the field ignores it — so the format
/// version stays 1 rather than forcing a migration for an additive field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemePreference {
    /// Follow the desktop's appearance.
    #[default]
    System,
    Dark,
    Light,
}

/// Everything the shell restores on launch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionState {
    pub window: WindowState,
    pub theme: ThemePreference,
    pub active_application: Option<String>,
    /// Application ids in rail order.
    pub rail_order: Vec<String>,
    pub installed: Vec<InstalledApplication>,
    /// Application ids pinned to the dock.
    pub dock: Vec<String>,
    pub tabs: Vec<TabRecord>,
    /// The last login-item state this shell observed/wrote (Windows `HKCU\…\Run\Station`).
    ///
    /// Added to format 1 after it shipped: `#[serde(default)]` keeps existing documents
    /// readable (they open as `false`), and a build that does not know the field ignores it — the
    /// format version stays 1. The registry remains the truth on launch; this is the value a
    /// headless or non-Windows run still round-trips.
    #[serde(default)]
    pub start_at_login: bool,
    /// Signed-in identities, keyed by their `identityId` (`"google-<sub>"`), exactly the shape
    /// `StationUserIdentity` has in `packages/app/src/user-identities/types.ts` — without the
    /// `accessToken`/`refreshToken` the TS store also kept: this shell calls no token-backed API,
    /// so it stores none.
    ///
    /// Added to format 1 after it shipped: `#[serde(default)]` keeps existing documents readable
    /// (they open with no accounts), and a build that does not know the field ignores it — the
    /// format version stays 1.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub identities: BTreeMap<String, Identity>,
    /// Per-application configuration (the `ApplicationConfigData` a preset
    /// app's start URL is templated with), keyed by application id.
    ///
    /// Added to format 1 after it shipped: `#[serde(default)]` keeps existing
    /// documents readable (they open with nothing configured), and a build
    /// that does not know the field ignores it — the format version stays 1.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub app_config: BTreeMap<String, AppConfigEntry>,
    /// Pinned favorites shown at the top of the Bang Quick Switcher (Slice 28).
    ///
    /// Added to format 1 after it shipped: `#[serde(default)]` keeps existing documents readable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub favorites: Vec<FavoriteEntry>,
    /// User-configured custom downloads directory (Slice 33); absent uses the OS Downloads folder.
    ///
    /// Added to format 1 after it shipped: `#[serde(default)]` keeps existing documents readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_dir: Option<String>,
    /// Whether pre-release (beta) GitHub releases are included when checking for updates (Slice 34).
    ///
    /// Added to format 1 after it shipped: `#[serde(default)]` keeps existing documents readable
    /// (default `false`), and older builds ignore it — format version stays 1.
    #[serde(default)]
    pub include_beta_updates: bool,
    /// Whether closing the main window hides Station to the Windows system tray instead of
    /// quitting (Slice 35). Additive v1 field (default `false`).
    #[serde(default)]
    pub minimize_to_tray: bool,
}

/// One pinned favorite belonging to an installed application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteEntry {
    pub id: String,
    pub application_id: String,
    pub title: String,
    pub url: String,
}

/// One signed-in profile: the stored half of `StationUserIdentity`
/// (`packages/app/src/user-identities/types.ts`), minus the tokens.
///
/// `identity_id` is the TS `"${provider}-${userId}"` form (`createIdentity` in
/// `user-identities/duck.ts`) and is also the key in [`SessionState::identities`]. `provider`
/// stays the raw stored string (`"google"` today); every profile field is a plain string, as
/// `processGoogleAuthData` (`api.ts`) fills `''` rather than omitting one.
///
/// Deliberately absent: `accessToken`/`refreshToken`. The TS redux store kept them, but this
/// shell calls no token-backed API, and Google requires tokens at rest to be encrypted — storing
/// none sidesteps that (see `docs/oauth-config-options.md`, B4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub identity_id: String,
    pub provider: String,
    /// The provider's stable user id (`api.ts`: `nameObject.metadata.source.id`).
    pub user_id: String,
    pub profile_data: IdentityProfile,
}

impl Identity {
    /// The first field past [`MAX_IDENTITY_FIELD_CHARS`], named for the refusal message.
    fn oversized_field(&self) -> Option<&'static str> {
        [
            (
                "its id is longer than 256 characters",
                self.identity_id.as_str(),
            ),
            (
                "its provider is longer than 256 characters",
                self.provider.as_str(),
            ),
            (
                "its user id is longer than 256 characters",
                self.user_id.as_str(),
            ),
            (
                "its display name is longer than 256 characters",
                self.profile_data.display_name.as_str(),
            ),
            (
                "its first name is longer than 256 characters",
                self.profile_data.first_name.as_str(),
            ),
            (
                "its last name is longer than 256 characters",
                self.profile_data.last_name.as_str(),
            ),
            (
                "its email is longer than 256 characters",
                self.profile_data.email.as_str(),
            ),
            (
                "its image URL is longer than 256 characters",
                self.profile_data.image_url.as_str(),
            ),
        ]
        .into_iter()
        .find(|(_, value)| value.chars().count() > MAX_IDENTITY_FIELD_CHARS)
        .map(|(why, _)| why)
    }
}

/// `profileData` of an [`Identity`] (`StationUserIdentity['profileData']` in
/// `packages/app/src/user-identities/types.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityProfile {
    pub display_name: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    /// TS spells this key `imageURL` (capital URL); plain camelCase would write `imageUrl`.
    #[serde(rename = "imageURL")]
    pub image_url: String,
}

/// One installed application's multi-instance configuration: the fields
/// `ApplicationConfigData` carries (`packages/app/src/applications/duck.ts`),
/// as the settings surface and `resolve_start_url` consume them.
///
/// Absent means "not configured": the application opens its manifest's own
/// start URL (signed-out, where the presets would template one) — the slice-6
/// behaviour. An entry with every field empty is the "normal flow" the TS
/// configurator submits (`setConfigData(applicationId, {})`): use the
/// manifest's plain start URL even though presets exist.
///
/// `None` and `Some("")` stay distinct: the TS getters return the raw value
/// and JS truthiness treats `''` as unconfigured, and the template layer
/// keeps that distinction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfigEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
    #[serde(default, rename = "customURL", skip_serializing_if = "Option::is_none")]
    pub custom_url: Option<String>,
}

impl AppConfigEntry {
    /// True when every field is empty or whitespace: no information, so the
    /// entry carries nothing to template with.
    pub fn is_empty(&self) -> bool {
        self.values()
            .into_iter()
            .flatten()
            .all(|value| value.trim().is_empty())
    }

    /// The set values, in field order.
    pub fn values(&self) -> [Option<&str>; 3] {
        [
            self.identity_id.as_deref(),
            self.subdomain.as_deref(),
            self.custom_url.as_deref(),
        ]
    }

    /// The values are usable lengths: every set value is at most
    /// [`MAX_CONFIG_VALUE_CHARS`] characters and is not whitespace padding.
    fn sane(&self) -> bool {
        self.values().into_iter().flatten().all(|value| {
            value.chars().count() <= MAX_CONFIG_VALUE_CHARS && !value.trim().is_empty()
        })
    }
}

/// Why a session could not be read or written.
#[derive(Debug)]
pub enum StoreError {
    /// The file could not be read, written, or renamed.
    Io(io::Error),
    /// The bytes are not the JSON document this crate writes.
    Parse(serde_json::Error),
    /// The file was written by a newer (or older) format this build must
    /// not overwrite blindly.
    UnsupportedVersion(u32),
    /// The document is larger than [`MAX_FILE_BYTES`]; it is refused unread.
    TooLarge(u64),
    /// One collection is past its cap (see [`MAX_INSTALLED`] and friends).
    TooManyEntries {
        field: &'static str,
        count: usize,
        max: usize,
    },
    /// An application's configuration cannot be used: an oversized value or
    /// a key that names no installed application. The document is refused
    /// whole — naming the offending application, never truncating it.
    BadConfig {
        application_id: String,
        why: &'static str,
    },
    /// The rail order cannot be used: it names an application twice, or one
    /// the document does not install. The document is refused whole — the
    /// order is the user's, so trimming entries would silently rearrange
    /// their rail.
    BadRailOrder {
        application_id: String,
        why: &'static str,
    },
    /// One stored identity cannot be used: its map key disagrees with its `identityId`, one of
    /// its strings is past the 256-char cap, or the document repeats its id. The document is
    /// refused whole — the account list is the user's, so dropping an entry would lose an
    /// account silently.
    BadIdentity {
        identity_id: String,
        why: &'static str,
    },
    /// One pinned favorite cannot be used: empty/control/oversized field, duplicate id, or
    /// referencing an uninstalled application.
    BadFavorite {
        favorite_id: String,
        why: &'static str,
    },
    /// The custom downloads folder cannot be used: empty, whitespace, control characters, or
    /// longer than [`MAX_DOWNLOAD_DIR_CHARS`]. Refused whole, never truncated.
    BadDownloadDir { why: &'static str },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(error) => write!(f, "session store io error: {error}"),
            StoreError::Parse(error) => write!(f, "session store parse error: {error}"),
            StoreError::UnsupportedVersion(version) => write!(
                f,
                "session store version {version} is not supported (expected {FORMAT_VERSION})"
            ),
            StoreError::TooLarge(bytes) => write!(
                f,
                "session store file is {bytes} bytes; at most {MAX_FILE_BYTES} is read"
            ),
            StoreError::TooManyEntries { field, count, max } => {
                write!(
                    f,
                    "session store has {count} {field} entries; at most {max} is accepted"
                )
            }
            StoreError::BadConfig {
                application_id,
                why,
            } => write!(
                f,
                "session store configuration for {application_id} is not usable: {why}"
            ),
            StoreError::BadRailOrder {
                application_id,
                why,
            } => write!(
                f,
                "session store rail order entry {application_id} is not usable: {why}"
            ),
            StoreError::BadIdentity { identity_id, why } => write!(
                f,
                "session store identity {identity_id} is not usable: {why}"
            ),
            StoreError::BadFavorite { favorite_id, why } => write!(
                f,
                "session store favorite {favorite_id} is not usable: {why}"
            ),
            StoreError::BadDownloadDir { why } => {
                write!(f, "session store downloadDir is not usable: {why}")
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Io(error) => Some(error),
            StoreError::Parse(error) => Some(error),
            StoreError::UnsupportedVersion(_)
            | StoreError::TooLarge(_)
            | StoreError::TooManyEntries { .. }
            | StoreError::BadConfig { .. }
            | StoreError::BadRailOrder { .. }
            | StoreError::BadIdentity { .. }
            | StoreError::BadFavorite { .. }
            | StoreError::BadDownloadDir { .. } => None,
        }
    }
}

/// A session document at a fixed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The stored session, or the default one when no file exists yet.
    ///
    /// A future [`FORMAT_VERSION`] is reported as
    /// [`StoreError::UnsupportedVersion`]; it is never replaced by a default.
    pub fn load(&self) -> Result<SessionState, StoreError> {
        // The size gate runs before the read: a 4 GiB "session" is refused, never loaded.
        let size = match fs::metadata(&self.path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SessionState::default())
            }
            Err(error) => return Err(StoreError::Io(error)),
        };
        if size > MAX_FILE_BYTES {
            return Err(StoreError::TooLarge(size));
        }
        let bytes = fs::read(&self.path).map_err(StoreError::Io)?;
        let header: VersionHeader = serde_json::from_slice(&bytes).map_err(StoreError::Parse)?;
        if header.version != FORMAT_VERSION {
            return Err(StoreError::UnsupportedVersion(header.version));
        }
        // Unknown fields (including `version`) are ignored by serde.
        let mut state: SessionState = serde_json::from_slice(&bytes).map_err(StoreError::Parse)?;
        // `serde`'s map keeps the last value of a repeated JSON key; the account list is the
        // user's, so a duplicate is refused by id instead of silently replacing an account.
        if let Some(identity_id) = duplicate_identity_id(&bytes) {
            return Err(StoreError::BadIdentity {
                identity_id,
                why: "the identity appears twice",
            });
        }
        sanitize(&mut state)?;
        Ok(state)
    }

    /// Atomically replace the stored session: write a sibling temp file,
    /// fsync it, then rename it over `path`. On failure the previous file is
    /// untouched and the temp file is removed.
    ///
    /// The document is validated (`sanitize`) before anything is written, so
    /// a state this build would refuse to *load* is never written either.
    pub fn save(&self, state: &SessionState) -> Result<(), StoreError> {
        let mut state = state.clone();
        sanitize(&mut state)?;
        let bytes = serde_json::to_vec_pretty(&WireState {
            version: FORMAT_VERSION,
            state: &state,
        })
        .map_err(StoreError::Parse)?;
        write_atomically(&self.path, &bytes).map_err(StoreError::Io)
    }
}

/// Brings a parsed document into the range the shell can use: window sizes are clamped to
/// something a desktop can show, and collections past their caps are refused (a refusal is safer
/// than truncating a session whose dock and tabs would then point at dropped applications).
fn sanitize(state: &mut SessionState) -> Result<(), StoreError> {
    state.window.width = sane_length(state.window.width, 1200.0, MIN_WINDOW_WIDTH);
    state.window.height = sane_length(state.window.height, 800.0, MIN_WINDOW_HEIGHT);
    for (field, count, max) in [
        ("installed", state.installed.len(), MAX_INSTALLED),
        ("dock", state.dock.len(), MAX_DOCK),
        ("tab", state.tabs.len(), MAX_TABS),
        ("rail", state.rail_order.len(), MAX_RAIL),
    ] {
        if count > max {
            return Err(StoreError::TooManyEntries { field, count, max });
        }
    }
    // The rail order is the user's custom order (slice 11): it is authoritative only when it
    // names exactly the applications the document docks, so a stale or hostile order is refused
    // whole rather than trimmed — a trimmed order would silently rearrange the user's rail.
    // Entries may repeat an id: `save` never writes one (the shell's order is a permutation),
    // and a duplicated id has no defensible position, so the document is refused naming it.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for id in &state.rail_order {
        if !seen.insert(id.as_str()) {
            return Err(StoreError::BadRailOrder {
                application_id: id.clone(),
                why: "the application appears twice",
            });
        }
        if !state.dock.iter().any(|docked| docked == id) {
            return Err(StoreError::BadRailOrder {
                application_id: id.clone(),
                why: "the application is not docked",
            });
        }
    }
    // Every entry's values must be usable, and every key must name an installed application:
    // a configuration for an app the document does not install is refused, not truncated. (No
    // count cap is needed: entries are keyed by installed application, so there can never be
    // more entries than `installed`, which is capped.)
    for (application_id, config) in &state.app_config {
        if !config.sane() {
            return Err(StoreError::BadConfig {
                application_id: application_id.clone(),
                why: "a value is empty, whitespace, or longer than 2048 characters",
            });
        }
        if !state
            .installed
            .iter()
            .any(|application| &application.application_id == application_id)
        {
            return Err(StoreError::BadConfig {
                application_id: application_id.clone(),
                why: "the application is not installed",
            });
        }
    }
    // Accounts: the map is keyed by `identityId`, so `save` cannot produce a duplicate — the
    // load-side scan refuses one the document repeats. A key that disagrees with its entry, an
    // oversized string and the count are refused whole: the account list is the user's.
    if state.identities.len() > MAX_IDENTITIES {
        return Err(StoreError::TooManyEntries {
            field: "identity",
            count: state.identities.len(),
            max: MAX_IDENTITIES,
        });
    }
    for (identity_id, identity) in &state.identities {
        if identity_id != &identity.identity_id {
            return Err(StoreError::BadIdentity {
                identity_id: identity_id.clone(),
                why: "the key does not match the identity's id",
            });
        }
        if let Some(why) = identity.oversized_field() {
            return Err(StoreError::BadIdentity {
                identity_id: identity_id.clone(),
                why,
            });
        }
    }
    if state.favorites.len() > MAX_FAVORITES {
        return Err(StoreError::TooManyEntries {
            field: "favorite",
            count: state.favorites.len(),
            max: MAX_FAVORITES,
        });
    }
    let mut seen_favorites: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for favorite in &state.favorites {
        if favorite.id.trim().is_empty()
            || favorite.application_id.trim().is_empty()
            || favorite.title.trim().is_empty()
            || favorite.url.trim().is_empty()
        {
            return Err(StoreError::BadFavorite {
                favorite_id: favorite.id.clone(),
                why: "a required favorite field is empty",
            });
        }
        if [
            favorite.id.as_str(),
            favorite.application_id.as_str(),
            favorite.title.as_str(),
            favorite.url.as_str(),
        ]
        .iter()
        .any(|v| v.chars().count() > MAX_FAVORITE_FIELD_CHARS || v.chars().any(char::is_control))
        {
            return Err(StoreError::BadFavorite {
                favorite_id: favorite.id.clone(),
                why: "a favorite field contains control characters or exceeds 2048 characters",
            });
        }
        if !seen_favorites.insert(favorite.id.as_str()) {
            return Err(StoreError::BadFavorite {
                favorite_id: favorite.id.clone(),
                why: "the favorite id appears twice",
            });
        }
        if !state
            .installed
            .iter()
            .any(|application| application.application_id == favorite.application_id)
        {
            return Err(StoreError::BadFavorite {
                favorite_id: favorite.id.clone(),
                why: "the favorite references an application that is not installed",
            });
        }
    }
    if let Some(dir) = &state.download_dir {
        if dir.trim().is_empty() {
            return Err(StoreError::BadDownloadDir {
                why: "the path is empty or whitespace",
            });
        }
        if dir.chars().count() > MAX_DOWNLOAD_DIR_CHARS || dir.chars().any(char::is_control) {
            return Err(StoreError::BadDownloadDir {
                why: "the path contains control characters or exceeds 2048 characters",
            });
        }
    }
    Ok(())
}

/// The first identity key the document repeats, if any.
///
/// `identities` is a JSON object keyed by id, and `serde`'s map keeps the last value of a
/// repeated key; the account list must not be rewritten silently. Only the keys of that object
/// are read (every value is skipped), so this does not duplicate the state parse.
fn duplicate_identity_id(bytes: &[u8]) -> Option<String> {
    use serde::de::{IgnoredAny, MapAccess, Visitor};

    /// The `identities` object's keys, with the first repeat.
    struct Identities(Option<String>);

    impl<'de> Deserialize<'de> for Identities {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Keys;

            impl<'de> Visitor<'de> for Keys {
                type Value = Identities;

                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("an object of identities keyed by id")
                }

                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Identities, A::Error> {
                    let mut seen = std::collections::HashSet::new();
                    let mut duplicate = None;
                    while let Some(key) = map.next_key::<String>()? {
                        if !seen.insert(key.clone()) && duplicate.is_none() {
                            duplicate = Some(key);
                        }
                        let _: IgnoredAny = map.next_value()?;
                    }
                    Ok(Identities(duplicate))
                }
            }

            deserializer.deserialize_map(Keys)
        }
    }

    /// The document, finding the `identities` object among the other top-level fields.
    struct Document(Option<String>);

    impl<'de> Deserialize<'de> for Document {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Fields;

            impl<'de> Visitor<'de> for Fields {
                type Value = Document;

                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("a session document")
                }

                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Document, A::Error> {
                    let mut duplicate = None;
                    while let Some(key) = map.next_key::<String>()? {
                        if key == "identities" {
                            duplicate = map.next_value::<Identities>()?.0.or(duplicate);
                        } else {
                            let _: IgnoredAny = map.next_value()?;
                        }
                    }
                    Ok(Document(duplicate))
                }
            }

            deserializer.deserialize_map(Fields)
        }
    }

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    Document::deserialize(&mut deserializer)
        .ok()
        .and_then(|document| document.0)
}

/// A window length: a non-finite or non-positive value falls back to the default, and anything
/// outside `[min, MAX_WINDOW]` is clamped into it.
fn sane_length(value: f32, fallback: f32, min: f32) -> f32 {
    if !value.is_finite() || value <= 0.0 {
        fallback
    } else {
        value.clamp(min, MAX_WINDOW)
    }
}

/// The on-disk document: the format version plus the state, flattened so the
/// state's own field names stay top-level (and camelCase).
#[derive(Serialize)]
struct WireState<'a> {
    version: u32,
    #[serde(flatten)]
    state: &'a SessionState,
}

/// The version gate. Parsed before the state so a newer document is refused
/// rather than coerced into a partial `SessionState`.
#[derive(Deserialize)]
struct VersionHeader {
    version: u32,
}

/// Write `bytes` to `path` via a sibling temp file and a rename.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }
    let directory = parent.unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "session store path has no file name",
        )
    })?;
    let temp = directory.join(temp_name(file_name));

    let result = write_and_rename(&temp, path, bytes);
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_and_rename(temp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp, path)
}

/// A sibling name unlikely to collide with another writer, including a
/// crashed one whose counter restarted.
fn temp_name(file_name: &std::ffi::OsStr) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!(
        ".{}.{}.{nanos}.{serial}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    )
}
