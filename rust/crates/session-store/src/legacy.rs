//! One-time import from the Electron desktop app's Sequelize SQLite database.
//! Table and column names follow packages/app/src/database/model.ts at d393a3c.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use appstore_service::write_commands::ApplicationConfigData;
use appstore_service::{InstalledApplication, TabRecord};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use url::Url;

use super::{
    AppConfigEntry, FavoriteEntry, SessionState, SessionStore, MAX_CONFIG_VALUE_CHARS, MAX_DOCK,
    MAX_DOWNLOAD_DIR_CHARS, MAX_FAVORITES, MAX_FAVORITE_FIELD_CHARS, MAX_INSTALLED, MAX_TABS,
};

const APP_STORE_ID: &str = "appstore";
const MAX_URL_CHARS: usize = MAX_FAVORITE_FIELD_CHARS;
const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct ImportSummary {
    pub applications: usize,
    pub tabs: usize,
    pub favorites: usize,
    pub skipped_applications: usize,
    pub skipped_tabs: usize,
    pub skipped_favorites: usize,
}

#[derive(sqlx::FromRow)]
struct ApplicationRow {
    id: String,
    manifest: Option<String>,
    active_tab: Option<String>,
    subdomain: Option<String>,
    custom_url: Option<String>,
}

#[derive(sqlx::FromRow)]
struct TabRow {
    id: String,
    application: Option<String>,
    url: Option<String>,
    home: Option<bool>,
    title: Option<String>,
}

#[derive(sqlx::FromRow)]
struct FavoriteRow {
    id: String,
    application: Option<String>,
    title: Option<String>,
    url: Option<String>,
}

#[derive(Default, sqlx::FromRow)]
struct SettingsRow {
    start_at_login: Option<bool>,
    minimize_to_tray: Option<bool>,
    download_dir: Option<String>,
}

/// Read a closed legacy database and create the native session. An existing destination is
/// always left untouched. Only the app, tab, dock, favorite, nav and general settings data is read.
pub fn import(source: &Path, destination: &Path) -> io::Result<ImportSummary> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a native Station session already exists",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let before = source_snapshot(source)?;
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    let (state, summary) = runtime.block_on(read_session(source))?;
    if before != source_snapshot(source)? {
        return Err(io::Error::other(
            "the legacy database changed during import; close Electron Station and retry",
        ));
    }
    SessionStore::new(destination)
        .create(&state)
        .map_err(|error| match error {
            super::StoreError::Io(error) => error,
            error => io::Error::other(error),
        })?;
    Ok(summary)
}

fn source_snapshot(source: &Path) -> io::Result<(u64, std::time::SystemTime)> {
    let metadata = fs::symlink_metadata(source)?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected a regular legacy SQLite database no larger than 512 MiB",
        ));
    }

    // Immutable makes SQLite leave the source directory untouched. It also ignores uncheckpointed
    // data, so refuse a pending WAL or rollback journal and have Electron close it cleanly.
    for suffix in ["-wal", "-journal"] {
        let mut sidecar = source.as_os_str().to_owned();
        sidecar.push(suffix);
        match fs::symlink_metadata(sidecar) {
            Ok(file) if file.is_file() && file.len() == 0 => {}
            Ok(_) => {
                return Err(io::Error::other(
                    "the legacy database has a pending journal; open and quit Electron Station cleanly before retrying",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok((metadata.len(), metadata.modified()?))
}

async fn read_session(source: &Path) -> io::Result<(SessionState, ImportSummary)> {
    let options = SqliteConnectOptions::new()
        .filename(source)
        .read_only(true)
        .immutable(true)
        .pragma("trusted_schema", "OFF")
        .pragma("query_only", "ON");
    let mut database = SqliteConnection::connect_with(&options)
        .await
        .map_err(io::Error::other)?;
    read_rows(&mut database).await.map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("could not read the Electron Station schema: {error}"),
        )
    })
}

async fn read_rows(
    database: &mut SqliteConnection,
) -> Result<(SessionState, ImportSummary), sqlx::Error> {
    let applications = sqlx::query_as::<_, ApplicationRow>(
        "SELECT applicationId AS id, manifestURL AS manifest, activeTabId AS active_tab,
                subdomain, customURL AS custom_url
         FROM application ORDER BY applicationId LIMIT ?1",
    )
    .bind((MAX_INSTALLED + 1) as i64)
    .fetch_all(&mut *database)
    .await?;
    let tabs = sqlx::query_as::<_, TabRow>(
        "SELECT tabId AS id, applicationId AS application, url, isApplicationHome AS home, title
         FROM tab ORDER BY tabId LIMIT ?1",
    )
    .bind((MAX_TABS + 1) as i64)
    .fetch_all(&mut *database)
    .await?;
    let dock_order = sqlx::query_scalar::<_, Option<String>>(
        "SELECT applicationId FROM dock ORDER BY \"order\", id LIMIT ?1",
    )
    .bind((MAX_DOCK + 1) as i64)
    .fetch_all(&mut *database)
    .await?;
    if applications.len() > MAX_INSTALLED || tabs.len() > MAX_TABS || dock_order.len() > MAX_DOCK {
        return Err(sqlx::Error::Protocol(
            "the legacy session exceeds Station's supported app, tab or dock capacity".to_owned(),
        ));
    }

    let favorites = sqlx::query_as::<_, FavoriteRow>(
        "SELECT favoriteId AS id, applicationId AS application, title, url
         FROM favorite ORDER BY favoriteId LIMIT ?1",
    )
    .bind(MAX_FAVORITES as i64)
    .fetch_all(&mut *database)
    .await?;
    let favorite_count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM favorite")
        .fetch_one(&mut *database)
        .await?;
    let active = sqlx::query_scalar::<_, Option<String>>(
        "SELECT tabApplicationId FROM nav ORDER BY id LIMIT 1",
    )
    .fetch_optional(&mut *database)
    .await?
    .flatten();
    // Minimize-to-tray was added in the last Electron schema revision. Older profiles can still
    // bring over their app list and download settings.
    let has_minimize = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('app') WHERE name = 'minimizeToTray')",
    )
    .fetch_one(&mut *database)
    .await?;
    let settings_sql = if has_minimize {
        "SELECT autoLaunchEnabled AS start_at_login, minimizeToTray AS minimize_to_tray,
                downloadFolder AS download_dir FROM app ORDER BY id LIMIT 1"
    } else {
        "SELECT autoLaunchEnabled AS start_at_login, NULL AS minimize_to_tray,
                downloadFolder AS download_dir FROM app ORDER BY id LIMIT 1"
    };
    let settings = sqlx::query_as::<_, SettingsRow>(settings_sql)
        .fetch_optional(database)
        .await?
        .unwrap_or_default();

    let omitted_favorites = (favorite_count as usize).saturating_sub(favorites.len());
    let (state, mut summary) = convert(applications, tabs, dock_order, favorites, active, settings);
    summary.skipped_favorites += omitted_favorites;
    Ok((state, summary))
}

fn convert(
    applications: Vec<ApplicationRow>,
    tabs: Vec<TabRow>,
    dock_order: Vec<Option<String>>,
    favorites: Vec<FavoriteRow>,
    active: Option<String>,
    settings: SettingsRow,
) -> (SessionState, ImportSummary) {
    let docked: HashSet<&str> = dock_order.iter().filter_map(Option::as_deref).collect();
    let mut tabs_by_application: HashMap<&str, Vec<(&TabRow, &str)>> = HashMap::new();
    for tab in &tabs {
        if let (Some(application), Some(url)) = (
            tab.application.as_deref(),
            tab.url.as_deref().filter(|url| safe_web_url(url)),
        ) {
            if usable_text(&tab.id, MAX_FAVORITE_FIELD_CHARS) {
                tabs_by_application
                    .entry(application)
                    .or_default()
                    .push((tab, url));
            }
        }
    }

    let mut state = SessionState {
        start_at_login: settings.start_at_login.unwrap_or_default(),
        minimize_to_tray: settings.minimize_to_tray.unwrap_or_default(),
        download_dir: settings.download_dir.filter(|path| {
            usable_text(path, MAX_DOWNLOAD_DIR_CHARS) && Path::new(path).is_absolute()
        }),
        ..SessionState::default()
    };
    let mut installed = HashSet::new();
    let mut tab_titles = HashMap::new();
    for application in &applications {
        if !usable_text(&application.id, 64)
            || application.id == APP_STORE_ID
            || installed.contains(application.id.as_str())
        {
            continue;
        }
        let Some(manifest) = application.manifest.as_deref().and_then(native_manifest) else {
            continue;
        };
        if manifest == "station-manifest://1" {
            continue;
        }
        let usable_tabs = tabs_by_application
            .get(application.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        let home = usable_tabs
            .iter()
            .find(|(tab, _)| tab.home == Some(true))
            .or_else(|| {
                usable_tabs
                    .iter()
                    .find(|(tab, _)| Some(&tab.id) == application.active_tab.as_ref())
            })
            .or_else(|| usable_tabs.first());
        let subdomain = application.subdomain.as_ref().filter(|value| {
            usable_text(value, MAX_CONFIG_VALUE_CHARS)
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
        });
        let custom_url = application
            .custom_url
            .as_ref()
            .filter(|url| safe_web_url(url));
        let config = ApplicationConfigData {
            subdomain: subdomain.cloned(),
            custom_url: custom_url.cloned(),
            ..ApplicationConfigData::default()
        };
        if config.subdomain.is_some() || config.custom_url.is_some() {
            state.app_config.insert(
                application.id.clone(),
                AppConfigEntry {
                    subdomain: config.subdomain.clone(),
                    custom_url: config.custom_url.clone(),
                    ..AppConfigEntry::default()
                },
            );
        }
        installed.insert(application.id.as_str());
        state.installed.push(InstalledApplication {
            application_id: application.id.clone(),
            manifest_url: manifest,
            install_context: None,
            home_tab_url: home.map(|(_, url)| (*url).to_owned()),
            in_dock: docked.contains(application.id.as_str()),
            config_data: config,
        });
        for &(tab, url) in usable_tabs {
            if tab_titles.contains_key(tab.id.as_str()) {
                continue;
            }
            tab_titles.insert(tab.id.as_str(), tab.title.as_deref());
            state.tabs.push(TabRecord {
                tab_id: tab.id.clone(),
                application_id: application.id.clone(),
                url: url.to_owned(),
                home: home.is_some_and(|(home, _)| home.id == tab.id),
            });
        }
    }

    let mut seen_dock = HashSet::new();
    state.dock = dock_order
        .into_iter()
        .flatten()
        .filter(|id| installed.contains(id.as_str()) && seen_dock.insert(id.clone()))
        .collect();
    state.rail_order = state.dock.clone();
    state.active_application = active.filter(|id| state.dock.contains(id));

    // The native shell shows one hosted page per app. Preserve open tab URLs via Favorites so
    // the tabs (including the latest open document) are reachable through the Quick Switcher.
    let mut seen_ids = HashSet::new();
    let mut seen_links = HashSet::new();
    let mut skipped_favorites = 0;
    for favorite in &favorites {
        let (Some(application), Some(url)) = (favorite.application.as_ref(), favorite.url.as_ref())
        else {
            skipped_favorites += 1;
            continue;
        };
        if !seen_dock.contains(application)
            || !safe_web_url(url)
            || !usable_text(&favorite.id, MAX_FAVORITE_FIELD_CHARS)
            || state.favorites.len() == MAX_FAVORITES
            || !seen_ids.insert(favorite.id.clone())
        {
            skipped_favorites += 1;
            continue;
        }
        seen_links.insert((application.clone(), url.clone()));
        state.favorites.push(FavoriteEntry {
            id: favorite.id.clone(),
            application_id: application.clone(),
            title: favorite_title(favorite.title.as_deref(), url),
            url: url.clone(),
        });
    }
    for tab in &state.tabs {
        if state.favorites.len() == MAX_FAVORITES {
            break;
        }
        let id = format!("electron-tab:{}", tab.tab_id);
        if seen_dock.contains(&tab.application_id)
            && usable_text(&id, MAX_FAVORITE_FIELD_CHARS)
            && seen_ids.insert(id.clone())
            && seen_links.insert((tab.application_id.clone(), tab.url.clone()))
        {
            state.favorites.push(FavoriteEntry {
                id,
                application_id: tab.application_id.clone(),
                title: favorite_title(tab_titles[tab.tab_id.as_str()], &tab.url),
                url: tab.url.clone(),
            });
        }
    }

    let summary = ImportSummary {
        applications: state.installed.len(),
        tabs: state.tabs.len(),
        favorites: state.favorites.len(),
        skipped_applications: applications.len() - state.installed.len(),
        skipped_tabs: tabs.len() - state.tabs.len(),
        skipped_favorites,
    };
    (state, summary)
}

fn native_manifest(value: &str) -> Option<String> {
    let id = value
        .strip_prefix("station-manifest://")
        .map(|id| id.trim_end_matches('/'))
        .or_else(|| {
            value
                .strip_prefix("https://api.getstation.com/application-recipe/")?
                .strip_suffix("/bxAppManifest.json")
        });
    if let Some(id) = id {
        return id
            .parse::<u64>()
            .ok()
            .map(|id| manifest_registry::get_bx_app_manifest_url(&id.to_string()));
    }
    // Older side-loaded apps sometimes use a web manifest URL. Keep the legacy record and its
    // page as a fallback even when the manifest isn't in the native catalog.
    safe_web_url(value).then(|| value.to_owned())
}

fn usable_text(value: &str, max_chars: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max_chars
        && !value.chars().any(char::is_control)
}

fn safe_web_url(value: &str) -> bool {
    usable_text(value, MAX_URL_CHARS)
        && Url::parse(value).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && !url.query_pairs().any(|(key, _)| credential_parameter(&key))
                && !url.fragment().is_some_and(|fragment| {
                    url::form_urlencoded::parse(fragment.as_bytes())
                        .any(|(key, _)| credential_parameter(&key))
                })
        })
}

fn credential_parameter(key: &str) -> bool {
    [
        "access_token",
        "refresh_token",
        "id_token",
        "client_secret",
        "password",
    ]
    .iter()
    .any(|parameter| key.eq_ignore_ascii_case(parameter))
}

fn favorite_title(title: Option<&str>, url: &str) -> String {
    title
        .filter(|title| usable_text(title, MAX_FAVORITE_FIELD_CHARS))
        .unwrap_or(url)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const SCHEMA: &str = "
        CREATE TABLE application (
            applicationId TEXT PRIMARY KEY, manifestURL TEXT, activeTabId TEXT,
            subdomain TEXT, customURL TEXT, identityId TEXT
        );
        CREATE TABLE tab (
            tabId TEXT PRIMARY KEY, applicationId TEXT, url TEXT,
            isApplicationHome BOOLEAN, title TEXT
        );
        CREATE TABLE dock (id INTEGER PRIMARY KEY, applicationId TEXT, \"order\" INTEGER);
        CREATE TABLE favorite (
            favoriteId TEXT PRIMARY KEY, applicationId TEXT, title TEXT, url TEXT
        );
        CREATE TABLE nav (id INTEGER PRIMARY KEY, tabApplicationId TEXT);
        CREATE TABLE app (
            id INTEGER PRIMARY KEY, autoLaunchEnabled BOOLEAN, minimizeToTray BOOLEAN,
            downloadFolder TEXT
        );
        CREATE TABLE identity (identityId TEXT PRIMARY KEY, accessToken TEXT, refreshToken TEXT);
    ";

    const DATA: &str = "
        INSERT INTO application VALUES
            ('gmail', 'https://api.getstation.com/application-recipe/14/bxAppManifest.json',
             'doc', NULL, NULL, 'old-account'),
            ('slack', 'station-manifest://21/', 'slack-home', 'acme', NULL, NULL),
            ('appstore', 'station-manifest://1', 'store', NULL, NULL, NULL),
            ('bad-manifest', 'file:///program', NULL, NULL, NULL, NULL);
        INSERT INTO tab VALUES
            ('gmail-home', 'gmail', 'https://mail.google.com/mail/u/0/', 1, 'Inbox'),
            ('doc', 'gmail', 'https://docs.google.com/document/d/123', 0, 'Notes'),
            ('slack-home', 'slack', 'https://acme.slack.com', 1, 'Slack'),
            ('store', 'appstore', 'station://appstore', 1, NULL),
            ('stale', 'missing', 'https://example.test', 0, NULL),
            ('credential', 'gmail', 'https://example.test#access_token=discard-me', 0, NULL),
            ('script', 'gmail', 'javascript:alert(1)', 0, NULL);
        INSERT INTO dock VALUES (1, 'gmail', 4), (2, 'slack', 1), (3, 'missing', 0),
            (4, 'slack', 2), (5, 'appstore', 0);
        INSERT INTO favorite VALUES
            ('favorite-inbox', 'gmail', 'Inbox favorite', 'https://mail.google.com/mail/u/0/'),
            ('bad-favorite', 'gmail', 'Unsafe', 'https://user:secret@example.test');
        INSERT INTO nav VALUES (1, 'gmail');
        INSERT INTO identity VALUES ('old-account', 'old-access-token', 'old-refresh-token');
    ";

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let source = directory
            .path()
            .join("profile & archive")
            .join("station.db");
        fs::create_dir(source.parent().unwrap()).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                let options = SqliteConnectOptions::new()
                    .filename(&source)
                    .create_if_missing(true);
                let mut database = SqliteConnection::connect_with(&options).await.unwrap();
                sqlx::raw_sql(SCHEMA).execute(&mut database).await.unwrap();
                sqlx::raw_sql(DATA).execute(&mut database).await.unwrap();
                sqlx::query("INSERT INTO app VALUES (1, 1, 1, ?1)")
                    .bind(directory.path().to_str().unwrap())
                    .execute(&mut database)
                    .await
                    .unwrap();
                database.close().await.unwrap();
            });
        (directory, source)
    }

    #[test]
    fn imports_safe_apps_order_settings_and_open_tab_links() {
        let (directory, source) = fixture();
        let destination = directory.path().join("native").join("session.json");
        let old_bytes = fs::read(&source).unwrap();
        let summary = import(&source, &destination).unwrap();
        assert_eq!(
            summary,
            ImportSummary {
                applications: 2,
                tabs: 3,
                favorites: 3,
                skipped_applications: 2,
                skipped_tabs: 4,
                skipped_favorites: 1,
            }
        );
        assert_eq!(fs::read(&source).unwrap(), old_bytes);
        assert_eq!(fs::read_dir(source.parent().unwrap()).unwrap().count(), 1);

        let state = SessionStore::new(&destination).load().unwrap();
        assert_eq!(state.dock, ["slack", "gmail"]);
        assert_eq!(state.rail_order, state.dock);
        assert_eq!(state.active_application.as_deref(), Some("gmail"));
        assert!(state.start_at_login);
        assert!(state.minimize_to_tray);
        assert_eq!(state.download_dir.as_deref(), directory.path().to_str());
        assert_eq!(state.installed[0].manifest_url, "station-manifest://14");
        assert_eq!(state.installed[1].manifest_url, "station-manifest://21");
        assert_eq!(state.app_config["slack"].subdomain.as_deref(), Some("acme"));
        assert_eq!(state.favorites[0].id, "favorite-inbox");
        assert!(state
            .favorites
            .iter()
            .any(|favorite| favorite.title == "Notes"));
        assert!(state.identities.is_empty());
        let output = fs::read_to_string(&destination).unwrap();
        for secret in [
            "old-account",
            "old-access-token",
            "old-refresh-token",
            "discard-me",
        ] {
            assert!(!output.contains(secret));
        }
        let mut appstore = appstore_service::ApplicationService::in_memory();
        appstore
            .restore(state.installed, state.dock, state.tabs)
            .unwrap();
        assert_eq!(appstore.tabs().len(), 3);
    }

    #[test]
    fn never_overwrites_a_native_session_even_if_unreadable() {
        let (directory, source) = fixture();
        let destination = directory.path().join("session.json");
        import(&source, &destination).unwrap();
        let previous = fs::read(&destination).unwrap();
        assert_eq!(
            import(&source, &destination).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&destination).unwrap(), previous);

        fs::write(&destination, b"not valid JSON").unwrap();
        assert_eq!(
            import(&source, &destination).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&destination).unwrap(), b"not valid JSON");
    }

    #[test]
    fn concurrent_import_creates_a_complete_session_once() {
        let (directory, source) = fixture();
        let destination = directory.path().join("session.json");
        let (first, second) = std::thread::scope(|threads| {
            let first = threads.spawn(|| import(&source, &destination));
            let second = threads.spawn(|| import(&source, &destination));
            (first.join().unwrap(), second.join().unwrap())
        });
        let (winner, loser) = if first.is_ok() {
            (first, second)
        } else {
            (second, first)
        };
        assert_eq!(winner.unwrap().applications, 2);
        assert_eq!(loser.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            SessionStore::new(destination)
                .load()
                .unwrap()
                .installed
                .len(),
            2
        );
    }

    #[test]
    fn a_pending_wal_or_invalid_database_creates_no_session() {
        let (directory, source) = fixture();
        let destination = directory.path().join("session.json");
        let wal = source.with_extension("db-wal");
        fs::write(&wal, b"uncheckpointed data").unwrap();
        assert!(import(&source, &destination)
            .unwrap_err()
            .to_string()
            .contains("pending journal"));
        assert_eq!(fs::read(&wal).unwrap(), b"uncheckpointed data");
        assert!(!destination.exists());

        fs::remove_file(wal).unwrap();
        fs::write(&source, b"not a SQLite database").unwrap();
        assert!(import(&source, &destination).is_err());
        assert!(!destination.exists());
    }
}
