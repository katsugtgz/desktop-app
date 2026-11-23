//! Behavior specs for the durable session store. Written before the
//! implementation: a missing file is a fresh session, a saved session loads
//! back byte-for-byte in value terms, and no failure mode may lose the
//! previous session or leave a temp file behind.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use appstore_service::write_commands::{
    ApplicationConfigData, InstallContext, InstalledApplication,
};
use appstore_service::TabRecord;
use session_store::{
    SessionState, SessionStore, StoreError, ThemePreference, WindowState, FORMAT_VERSION, MAX_DOCK,
    MAX_FILE_BYTES, MAX_WINDOW, MIN_WINDOW_HEIGHT,
};

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "session-store-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn store(&self) -> SessionStore {
        SessionStore::new(self.path.join("session.json"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Every field populated, including nested appstore-service records.
fn full_state() -> SessionState {
    SessionState {
        window: WindowState {
            x: Some(-1200),
            y: Some(64),
            width: 1440.5,
            height: 900.25,
            maximized: true,
        },
        theme: ThemePreference::Light,
        active_application: Some("slack".to_owned()),
        rail_order: vec!["slack".to_owned()],
        installed: vec![InstalledApplication {
            application_id: "slack".to_owned(),
            manifest_url: "https://slack.com/manifest.json".to_owned(),
            install_context: Some(InstallContext::appstore("ctx-1")),
            home_tab_url: Some("https://app.slack.com/client".to_owned()),
            in_dock: true,
            config_data: ApplicationConfigData {
                identity_id: Some("team-1".to_owned()),
                subdomain: Some("acme".to_owned()),
                custom_url: Some("https://acme.slack.com".to_owned()),
            },
        }],
        dock: vec!["slack".to_owned()],
        tabs: vec![TabRecord {
            tab_id: "tab-1".to_owned(),
            application_id: "slack".to_owned(),
            url: "https://app.slack.com/client".to_owned(),
            home: true,
        }],
        start_at_login: true,
        app_config: BTreeMap::new(),
        identities: BTreeMap::new(),
        favorites: vec![session_store::FavoriteEntry {
            id: "fav-slack-1".to_owned(),
            application_id: "slack".to_owned(),
            title: "Slack General".to_owned(),
            url: "https://app.slack.com/client".to_owned(),
        }],
    }
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).expect("read file")
}

#[test]
fn missing_file_is_a_default_session() {
    let dir = TempDir::new("missing");
    let store = dir.store();

    let state = store.load().expect("missing file loads");

    assert_eq!(state, SessionState::default());
    assert_eq!(state.window.width, 1200.0);
    assert_eq!(state.window.height, 800.0);
    assert_eq!(state.window.x, None);
    assert_eq!(state.window.y, None);
    assert!(!state.window.maximized);
    assert_eq!(state.active_application, None);
    assert!(state.rail_order.is_empty());
    assert!(state.installed.is_empty());
    assert!(state.dock.is_empty());
    assert!(state.tabs.is_empty());
}

#[test]
fn save_then_load_preserves_every_field() {
    let dir = TempDir::new("roundtrip");
    let store = dir.store();
    let state = full_state();

    store.save(&state).expect("save");
    let loaded = store.load().expect("load");

    assert_eq!(loaded, state);
}

#[test]
fn saved_file_states_its_format_version() {
    let dir = TempDir::new("version");
    let store = dir.store();

    store.save(&SessionState::default()).expect("save");

    let saved: serde_json::Value =
        serde_json::from_slice(&read_bytes(&dir.path().join("session.json"))).expect("json");
    assert_eq!(saved["version"], FORMAT_VERSION);
    assert_eq!(saved["window"]["width"], 1200.0);
    assert_eq!(saved["window"]["height"], 800.0);
    // camelCase at this layer.
    assert_eq!(saved["activeApplication"], serde_json::Value::Null);
    assert_eq!(saved["railOrder"], serde_json::json!([]));
    assert_eq!(saved["startAtLogin"], false);
}

#[test]
fn second_save_replaces_the_first() {
    let dir = TempDir::new("replace");
    let store = dir.store();
    let first = full_state();
    let second = SessionState {
        active_application: Some("notion".to_owned()),
        ..SessionState::default()
    };

    store.save(&first).expect("first save");
    store.save(&second).expect("second save");

    assert_eq!(store.load().expect("load"), second);
    let raw = String::from_utf8(read_bytes(&dir.path().join("session.json"))).expect("utf8");
    assert!(
        !raw.contains("slack.com/manifest.json"),
        "old content removed"
    );
}

#[test]
fn corrupt_json_is_a_parse_error_and_the_file_is_untouched() {
    let dir = TempDir::new("corrupt");
    let path = dir.path().join("session.json");
    let garbage = b"{\"window\": tru".to_vec();
    fs::write(&path, &garbage).expect("seed corrupt file");
    let store = SessionStore::new(&path);

    match store.load() {
        Err(StoreError::Parse(_)) => {}
        other => panic!("expected Parse, got {other:?}"),
    }
    assert_eq!(read_bytes(&path), garbage);
}

#[test]
fn future_version_is_refused_without_touching_the_file() {
    let dir = TempDir::new("future");
    let path = dir.path().join("session.json");
    let future = br#"{"version": 2, "window": {"width": 640.0, "height": 480.0}}"#.to_vec();
    fs::write(&path, &future).expect("seed future file");
    let store = SessionStore::new(&path);

    match store.load() {
        Err(StoreError::UnsupportedVersion(2)) => {}
        other => panic!("expected UnsupportedVersion(2), got {other:?}"),
    }
    assert_eq!(read_bytes(&path), future);
}

#[test]
fn missing_state_fields_and_unknown_extra_fields_use_defaults() {
    let dir = TempDir::new("tolerant");
    let path = dir.path().join("session.json");
    let json = br#"{
        "version": 1,
        "window": {"width": 900.0},
        "dock": ["slack"],
        "somethingFromTheFuture": {"nested": true}
    }"#;
    fs::write(&path, json).expect("seed tolerant file");

    let state = SessionStore::new(&path).load().expect("load");

    assert_eq!(state.window.width, 900.0);
    assert_eq!(state.window.height, 800.0);
    assert_eq!(state.window.x, None);
    assert!(!state.window.maximized);
    assert_eq!(state.dock, vec!["slack".to_owned()]);
    assert!(state.installed.is_empty());
    assert_eq!(state.active_application, None);
}

#[test]
fn save_creates_parent_directories() {
    let dir = TempDir::new("parents");
    let store = SessionStore::new(
        dir.path()
            .join("nested")
            .join("deeper")
            .join("session.json"),
    );

    store.save(&full_state()).expect("save into new dirs");

    assert_eq!(store.load().expect("load"), full_state());
}

#[test]
fn failed_save_into_a_directory_leaves_old_data_and_no_temp_file() {
    let dir = TempDir::new("failed-save");
    let target = dir.path().join("session.json");
    fs::create_dir_all(target.join("inside")).expect("seed directory at store path");
    fs::write(target.join("inside").join("keep.txt"), b"keep me").expect("seed inner file");
    let store = SessionStore::new(&target);

    assert!(store.save(&full_state()).is_err());

    assert!(
        target.is_dir(),
        "a directory at the store path is not replaced"
    );
    assert_eq!(
        read_bytes(&target.join("inside").join("keep.txt")),
        b"keep me"
    );
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .expect("read dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(leftovers, vec![target.file_name().unwrap().to_owned()]);
}

#[test]
fn failed_save_with_a_file_where_the_parent_belongs_leaves_it_intact() {
    let dir = TempDir::new("failed-parent");
    let blocker = dir.path().join("blocked");
    fs::write(&blocker, b"not a directory").expect("seed blocker file");
    let store = SessionStore::new(blocker.join("session.json"));

    assert!(store.save(&SessionState::default()).is_err());

    assert_eq!(read_bytes(&blocker), b"not a directory");
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .expect("read dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(leftovers, vec![blocker.file_name().unwrap().to_owned()]);
}

#[test]
fn a_document_without_a_theme_opens_as_system() {
    let dir = TempDir::new("theme-old");
    let path = dir.path().join("session.json");
    // A format-1 document written before `theme` existed.
    fs::write(
        &path,
        br#"{"version":1,"window":{"x":null,"y":null,"width":1200.0,"height":800.0,"maximized":false},
             "activeApplication":null,"railOrder":[],"installed":[],"dock":[],"tabs":[]}"#,
    )
    .expect("write");

    let state = SessionStore::new(&path).load().expect("load");

    assert_eq!(state.theme, ThemePreference::System);
}

#[test]
fn the_theme_round_trips_as_a_camel_case_name() {
    let dir = TempDir::new("theme-roundtrip");
    let store = dir.store();
    let state = SessionState {
        theme: ThemePreference::Dark,
        ..SessionState::default()
    };

    store.save(&state).expect("save");
    let saved: serde_json::Value =
        serde_json::from_slice(&read_bytes(&dir.path().join("session.json"))).expect("json");

    assert_eq!(
        saved["version"], FORMAT_VERSION,
        "additive field keeps the format"
    );
    assert_eq!(saved["theme"], "dark");
    assert_eq!(store.load().expect("load").theme, ThemePreference::Dark);
}

#[test]
fn a_non_finite_window_size_falls_back_to_the_default() {
    let dir = TempDir::new("nan-window");
    let path = dir.path().join("session.json");
    // A hostile document by hand: `1e39` overflows f32 to infinity, and a negative height is no
    // size at all. (serde_json writes NaN as `null`, so the writer cannot produce these — a file
    // someone else wrote can.)
    fs::write(
        &path,
        br#"{"version":1,"window":{"x":null,"y":null,"width":1e39,"height":-5.0,"maximized":false},
             "activeApplication":null,"railOrder":[],"installed":[],"dock":[],"tabs":[]}"#,
    )
    .expect("write");

    let loaded = SessionStore::new(&path).load().expect("load");

    assert_eq!(loaded.window.width, 1200.0, "an infinite width survived");
    assert_eq!(loaded.window.height, 800.0, "a negative height survived");
}

#[test]
fn large_display_sizes_are_kept_exactly() {
    let dir = TempDir::new("large-display");
    let store = dir.store();
    for (width, height) in [(3840.0, 2160.0), (5120.0, 1440.0), (8192.0, 8192.0)] {
        let mut state = SessionState::default();
        state.window.width = width;
        state.window.height = height;
        store.save(&state).expect("save");

        let loaded = store.load().expect("load");

        assert_eq!(
            (loaded.window.width, loaded.window.height),
            (width, height),
            "a legitimate large window was shrunk"
        );
    }
}

#[test]
fn a_16k_point_window_is_clamped_to_the_stored_bound() {
    let dir = TempDir::new("16k-window");
    let path = dir.path().join("session.json");
    fs::write(
        &path,
        br#"{"version":1,"window":{"x":null,"y":null,"width":16384.0,"height":16384.0,"maximized":false},
             "activeApplication":null,"railOrder":[],"installed":[],"dock":[],"tabs":[]}"#,
    )
    .expect("write");

    let loaded = SessionStore::new(&path).load().expect("load");

    assert_eq!(loaded.window.width, MAX_WINDOW);
    assert_eq!(loaded.window.height, MAX_WINDOW);
    // The stored bound is scale-unaware; the shell's restore step applies the device-pixel
    // ceiling with the display's real scale.
    assert_eq!(loaded.window.width, 8192.0);
}

#[test]
fn a_hostile_window_size_is_clamped() {
    let dir = TempDir::new("huge-window");
    let store = dir.store();
    let mut state = SessionState::default();
    state.window.width = 10_000_000.0;
    state.window.height = 1.0;
    store.save(&state).expect("save");

    let loaded = store.load().expect("load");

    // 8192 pt: the stored bound keeps large displays (a 3840-pt 4K window, a 5120×1440 ultrawide)
    // exactly as they were; the shell applies the device-pixel ceiling when it restores.
    assert_eq!(MAX_WINDOW, 8192.0);
    assert_eq!(loaded.window.width, MAX_WINDOW);
    assert_eq!(loaded.window.height, MIN_WINDOW_HEIGHT);
}

#[test]
fn an_oversized_document_is_refused_without_being_parsed() {
    let dir = TempDir::new("too-large");
    let path = dir.path().join("session.json");
    // Space-padded JSON: valid prefix, huge file. The size gate must refuse it unread.
    let mut file = fs::File::create(&path).expect("create");
    file.write_all(b"{\"version\":1,\"installed\":[],\"dock\":[],\"tabs\":[]}")
        .expect("write");
    file.set_len(MAX_FILE_BYTES + 1).expect("extend");
    drop(file);

    match SessionStore::new(&path).load() {
        Err(StoreError::TooLarge(bytes)) => assert!(bytes > MAX_FILE_BYTES),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn a_document_with_too_many_entries_is_refused() {
    let dir = TempDir::new("too-many");
    let path = dir.path().join("session.json");
    let dock: Vec<String> = (0..=MAX_DOCK).map(|index| format!("app-{index}")).collect();
    let document = serde_json::json!({
        "version": FORMAT_VERSION,
        "installed": [],
        "dock": dock,
        "tabs": [],
        "railOrder": [],
    });
    fs::write(&path, serde_json::to_vec(&document).unwrap()).expect("write");

    match SessionStore::new(&path).load() {
        Err(StoreError::TooManyEntries { field, count, max }) => {
            assert_eq!(field, "dock");
            assert_eq!(count, MAX_DOCK + 1);
            assert_eq!(max, MAX_DOCK);
        }
        other => panic!("expected TooManyEntries, got {other:?}"),
    }
}

// ---- Rail order (`railOrder`, slice 11) ----------------------------------------------------

#[test]
fn a_rail_order_naming_the_docked_apps_round_trips() {
    let dir = TempDir::new("rail-order-roundtrip");
    let store = dir.store();
    let mut state = one_app_state();
    state.rail_order = vec![app_id(0)];

    store.save(&state).expect("save");
    assert_eq!(store.load().expect("load").rail_order, vec![app_id(0)]);
}

#[test]
fn a_rail_order_naming_an_undocked_app_is_refused_whole() {
    let dir = TempDir::new("rail-order-unknown");
    let path = dir.path().join("session.json");
    let document = serde_json::json!({
        "version": FORMAT_VERSION,
        "installed": [serde_json::json!({
            "application_id": app_id(0),
            "manifest_url": "https://example.com/manifest.json",
            "in_dock": true,
        })],
        "dock": [app_id(0)],
        "tabs": [],
        "railOrder": [app_id(0), "ghost-app"],
    });
    fs::write(&path, serde_json::to_vec(&document).unwrap()).expect("write");

    match SessionStore::new(&path).load() {
        Err(StoreError::BadRailOrder {
            application_id,
            why,
        }) => {
            assert_eq!(application_id, "ghost-app");
            assert_eq!(why, "the application is not docked");
        }
        other => panic!("expected BadRailOrder, got {other:?}"),
    }
    // Refused, not repaired: the bytes stay as they were.
    assert_eq!(
        read_bytes(&path),
        serde_json::to_vec(&document).unwrap(),
        "a refused document was rewritten"
    );
}

#[test]
fn a_rail_order_naming_an_app_twice_is_refused() {
    let dir = TempDir::new("rail-order-duplicate");
    let path = dir.path().join("session.json");
    let document = serde_json::json!({
        "version": FORMAT_VERSION,
        "installed": [serde_json::json!({
            "application_id": app_id(0),
            "manifest_url": "https://example.com/manifest.json",
            "in_dock": true,
        })],
        "dock": [app_id(0)],
        "tabs": [],
        "railOrder": [app_id(0), app_id(0)],
    });
    fs::write(&path, serde_json::to_vec(&document).unwrap()).expect("write");

    match SessionStore::new(&path).load() {
        Err(StoreError::BadRailOrder {
            application_id,
            why,
        }) => {
            assert_eq!(application_id, app_id(0));
            assert_eq!(why, "the application appears twice");
        }
        other => panic!("expected BadRailOrder, got {other:?}"),
    }
}

#[test]
fn save_refuses_a_rail_order_the_document_cannot_have() {
    let dir = TempDir::new("rail-order-save");
    let store = dir.store();
    let mut state = one_app_state();
    state.rail_order = vec![app_id(0), "ghost-app".to_owned()];

    match store.save(&state) {
        Err(StoreError::BadRailOrder { application_id, .. }) => {
            assert_eq!(application_id, "ghost-app");
        }
        other => panic!("expected BadRailOrder, got {other:?}"),
    }
    assert!(
        !dir.path().join("session.json").exists(),
        "a refused save wrote a file"
    );
}

// ---- Per-application configuration (`appConfig`) ----------------------------------------

use session_store::{AppConfigEntry, MAX_CONFIG_VALUE_CHARS};

fn app_id(index: usize) -> String {
    format!("application-{index}")
}

fn installed_app(id: &str) -> InstalledApplication {
    InstalledApplication {
        application_id: id.to_owned(),
        manifest_url: "https://example.com/manifest.json".to_owned(),
        install_context: None,
        home_tab_url: Some("https://example.com".to_owned()),
        in_dock: true,
        config_data: ApplicationConfigData::default(),
    }
}

/// A session with one installed application (`application-0`).
fn one_app_state() -> SessionState {
    SessionState {
        installed: vec![installed_app(&app_id(0))],
        dock: vec![app_id(0)],
        ..SessionState::default()
    }
}

#[test]
fn app_config_round_trips_with_camel_case_keys() {
    let dir = TempDir::new("app-config-roundtrip");
    let store = dir.store();
    let mut state = one_app_state();
    state.app_config.insert(
        app_id(0),
        AppConfigEntry {
            identity_id: Some("identity-1".to_owned()),
            subdomain: Some("acme".to_owned()),
            custom_url: Some("https://gitlab.corp.acme.dev".to_owned()),
        },
    );

    store.save(&state).expect("save");
    let saved: serde_json::Value =
        serde_json::from_slice(&read_bytes(&dir.path().join("session.json"))).expect("json");
    assert_eq!(saved["version"], FORMAT_VERSION, "additive field keeps v1");
    assert_eq!(saved["appConfig"][&app_id(0)]["identityId"], "identity-1");
    assert_eq!(
        saved["appConfig"][&app_id(0)]["customURL"],
        "https://gitlab.corp.acme.dev"
    );
    assert_eq!(store.load().expect("load"), state);
}

#[test]
fn a_document_without_app_config_opens_with_none() {
    let dir = TempDir::new("app-config-old");
    let path = dir.path().join("session.json");
    // A format-1 document written before `appConfig` existed.
    fs::write(
        &path,
        br#"{"version":1,"window":{"width":1200.0,"height":800.0},
             "activeApplication":null,"railOrder":[],"installed":[],"dock":[],"tabs":[]}"#,
    )
    .expect("write");

    let state = SessionStore::new(&path).load().expect("load");

    assert!(
        state.app_config.is_empty(),
        "absent config is not configured"
    );
}

#[test]
fn empty_and_whitespace_entries_are_refused() {
    let dir = TempDir::new("app-config-empty");
    let path = dir.path().join("session.json");
    let document = serde_json::json!({
        "version": FORMAT_VERSION,
        "installed": [{
            "application_id": app_id(0),
            "manifest_url": "https://example.com/manifest.json",
            "in_dock": true,
        }],
        "dock": [app_id(0)],
        "tabs": [],
        "appConfig": { "application-0": { "identityId": "   " } },
    });
    fs::write(&path, serde_json::to_vec(&document).unwrap()).expect("write");

    match SessionStore::new(&path).load() {
        Err(StoreError::BadConfig { application_id, .. }) => {
            assert_eq!(application_id, app_id(0));
        }
        other => panic!("expected BadConfig, got {other:?}"),
    }
}

#[test]
fn an_oversized_config_value_is_refused_and_names_the_app() {
    let dir = TempDir::new("app-config-huge");
    let store = dir.store();
    // A session already on disk, so the refused save must leave it byte-for-byte.
    store.save(&one_app_state()).expect("seed");
    let mut state = one_app_state();
    state.app_config.insert(
        app_id(0),
        AppConfigEntry {
            custom_url: Some("x".repeat(MAX_CONFIG_VALUE_CHARS + 1)),
            ..AppConfigEntry::default()
        },
    );

    let before = read_bytes(&dir.path().join("session.json"));
    match store.save(&state) {
        Err(StoreError::BadConfig {
            application_id,
            why,
        }) => {
            assert_eq!(application_id, app_id(0));
            assert!(why.contains("2048"), "{why}");
        }
        other => panic!("expected BadConfig, got {other:?}"),
    }
    assert_eq!(
        read_bytes(&dir.path().join("session.json")),
        before,
        "a refused save replaced the session"
    );
}

#[test]
fn config_for_an_uninstalled_application_is_refused() {
    let dir = TempDir::new("app-config-unknown");
    let path = dir.path().join("session.json");
    let document = serde_json::json!({
        "version": FORMAT_VERSION,
        "installed": [{
            "application_id": app_id(0),
            "manifest_url": "https://example.com/manifest.json",
            "in_dock": true,
        }],
        "dock": [app_id(0)],
        "tabs": [],
        "appConfig": { "application-gone": { "subdomain": "acme" } },
    });
    fs::write(&path, serde_json::to_vec(&document).unwrap()).expect("write");
    let saved = read_bytes(&path);

    match SessionStore::new(&path).load() {
        Err(StoreError::BadConfig {
            application_id,
            why,
        }) => {
            assert_eq!(application_id, "application-gone");
            assert_eq!(why, "the application is not installed");
        }
        other => panic!("expected BadConfig, got {other:?}"),
    }
    assert_eq!(read_bytes(&path), saved, "a refused document was replaced");
}

#[test]
fn the_last_value_for_a_duplicated_config_key_wins() {
    let dir = TempDir::new("app-config-dup");
    let path = dir.path().join("session.json");
    // JSON objects cannot hold duplicate keys; when a hostile one does anyway, serde's map
    // visitor takes the last occurrence — the document loads with that one value, not an error.
    let document = r#"{
        "version": 1,
        "installed": [{
            "application_id": "application-0",
            "manifest_url": "https://example.com/manifest.json",
            "in_dock": true
        }],
        "dock": ["application-0"],
        "tabs": [],
        "appConfig": {
            "application-0": { "subdomain": "first" },
            "application-0": { "subdomain": "second" }
        }
    }"#;
    fs::write(&path, document).expect("write");

    let state = SessionStore::new(&path).load().expect("load");

    assert_eq!(
        state
            .app_config
            .get("application-0")
            .map(|c| c.subdomain.as_deref()),
        Some(Some("second"))
    );
}

#[test]
fn an_entry_with_every_field_set_is_usable() {
    let entry = AppConfigEntry {
        identity_id: Some("identity-1".to_owned()),
        subdomain: Some("acme".to_owned()),
        custom_url: Some("https://gitlab.corp.acme.dev".to_owned()),
    };
    assert!(!entry.is_empty());
    assert_eq!(entry, entry.clone());

    assert!(AppConfigEntry::default().is_empty());
    assert!(AppConfigEntry {
        identity_id: Some(String::new()),
        ..AppConfigEntry::default()
    }
    .is_empty());
}

// ---- Identities (`identities`) ---------------------------------------------------------

use session_store::{Identity, IdentityProfile, MAX_IDENTITIES, MAX_IDENTITY_FIELD_CHARS};

/// One stored identity in the TS shape (`identityId: "google-<sub>"`), tokens deliberately
/// absent.
fn identity(id: &str, email: &str) -> Identity {
    Identity {
        identity_id: id.to_owned(),
        provider: "google".to_owned(),
        user_id: format!("user-{id}"),
        profile_data: IdentityProfile {
            display_name: "Ada Lovelace".to_owned(),
            first_name: "Ada".to_owned(),
            last_name: "Lovelace".to_owned(),
            email: email.to_owned(),
            image_url: "https://cdn.example/ada.png".to_owned(),
        },
    }
}

fn identity_json(id: &str) -> String {
    format!(
        r#"{{"identityId":"{id}","provider":"google","userId":"user-{id}",
            "profileData":{{"displayName":"Ada Lovelace","firstName":"Ada","lastName":"Lovelace",
            "email":"ada@example.com","imageURL":"https://cdn.example/ada.png"}}}}"#
    )
}

#[test]
fn identities_round_trip_in_the_ts_shape_without_tokens() {
    let dir = TempDir::new("identities-roundtrip");
    let store = dir.store();
    let mut state = SessionState::default();
    state.identities.insert(
        "google-ada".to_owned(),
        identity("google-ada", "ada@example.com"),
    );
    state.identities.insert(
        "google-grace".to_owned(),
        identity("google-grace", "grace@example.com"),
    );

    store.save(&state).expect("save");
    let saved: serde_json::Value =
        serde_json::from_slice(&read_bytes(&dir.path().join("session.json"))).expect("json");
    assert_eq!(saved["version"], FORMAT_VERSION, "additive field keeps v1");
    let ada = &saved["identities"]["google-ada"];
    assert_eq!(ada["identityId"], "google-ada");
    assert_eq!(ada["provider"], "google");
    assert_eq!(ada["userId"], "user-google-ada");
    assert_eq!(ada["profileData"]["displayName"], "Ada Lovelace");
    assert_eq!(ada["profileData"]["firstName"], "Ada");
    assert_eq!(ada["profileData"]["lastName"], "Lovelace");
    assert_eq!(ada["profileData"]["email"], "ada@example.com");
    assert_eq!(
        ada["profileData"]["imageURL"], "https://cdn.example/ada.png",
        "the TS spells this field imageURL, not imageUrl"
    );
    // Policy: nothing token-backed is stored in this slice.
    for token in ["accessToken", "refreshToken"] {
        assert!(
            ada.get(token).is_none(),
            "{token} must not be part of the stored identity"
        );
    }

    assert_eq!(store.load().expect("load"), state);
}

#[test]
fn a_document_without_identities_opens_with_none() {
    let dir = TempDir::new("identities-old");
    let path = dir.path().join("session.json");
    // A format-1 document written before `identities` existed.
    fs::write(
        &path,
        br#"{"version":1,"window":{"width":1200.0,"height":800.0},
             "activeApplication":null,"railOrder":[],"installed":[],"dock":[],"tabs":[]}"#,
    )
    .expect("write");

    let state = SessionStore::new(&path).load().expect("load");

    assert!(state.identities.is_empty(), "absent identities is none");
}

#[test]
fn an_identity_key_must_match_its_identity_id() {
    let dir = TempDir::new("identities-key");
    let store = dir.store();
    let mut state = SessionState::default();
    state.identities.insert(
        "google-key".to_owned(),
        identity("google-someone-else", "ada@example.com"),
    );

    match store.save(&state) {
        Err(StoreError::BadIdentity { identity_id, .. }) => {
            assert_eq!(identity_id, "google-key");
        }
        other => panic!("expected BadIdentity, got {other:?}"),
    }
    assert!(
        !dir.path().join("session.json").exists(),
        "a refused save wrote a file"
    );
}

#[test]
fn a_loaded_identity_key_that_disagrees_with_its_id_is_refused() {
    let dir = TempDir::new("identities-key-load");
    let path = dir.path().join("session.json");
    let document = format!(
        r#"{{"version":1,"installed":[],"dock":[],"tabs":[],
            "identities":{{"google-key":{}}}}}"#,
        identity_json("google-someone-else")
    );
    fs::write(&path, document).expect("write");

    match SessionStore::new(&path).load() {
        Err(StoreError::BadIdentity { identity_id, .. }) => {
            assert_eq!(identity_id, "google-key");
        }
        other => panic!("expected BadIdentity, got {other:?}"),
    }
}

#[test]
fn an_oversized_identity_field_is_refused_and_leaves_the_file_untouched() {
    let dir = TempDir::new("identities-oversized");
    let store = dir.store();
    let mut good = SessionState::default();
    good.identities.insert(
        "google-ada".to_owned(),
        identity("google-ada", "ada@example.com"),
    );
    store.save(&good).expect("save the good document");
    let before = read_bytes(&dir.path().join("session.json"));

    let mut bad = good.clone();
    bad.identities
        .get_mut("google-ada")
        .expect("identity")
        .profile_data
        .email = "a".repeat(MAX_IDENTITY_FIELD_CHARS + 1);
    match store.save(&bad) {
        Err(StoreError::BadIdentity { identity_id, .. }) => {
            assert_eq!(identity_id, "google-ada");
        }
        other => panic!("expected BadIdentity, got {other:?}"),
    }
    assert_eq!(
        read_bytes(&dir.path().join("session.json")),
        before,
        "a refused save rewrote the document"
    );
}

#[test]
fn identity_fields_at_the_cap_are_accepted() {
    let dir = TempDir::new("identities-cap");
    let store = dir.store();
    let mut state = SessionState::default();
    let mut identity = identity("google-ada", "ada@example.com");
    identity.profile_data.email = "a".repeat(MAX_IDENTITY_FIELD_CHARS);
    state.identities.insert("google-ada".to_owned(), identity);

    store
        .save(&state)
        .expect("a field exactly at the cap is usable");
}

#[test]
fn too_many_identities_are_refused() {
    let dir = TempDir::new("identities-cap-count");
    let store = dir.store();
    let mut state = SessionState::default();
    for index in 0..=MAX_IDENTITIES {
        let id = format!("google-{index}");
        state
            .identities
            .insert(id.clone(), identity(&id, "ada@example.com"));
    }

    match store.save(&state) {
        Err(StoreError::TooManyEntries { field, count, max }) => {
            assert_eq!(field, "identity");
            assert_eq!(count, MAX_IDENTITIES + 1);
            assert_eq!(max, MAX_IDENTITIES);
        }
        other => panic!("expected TooManyEntries, got {other:?}"),
    }
    assert!(
        !dir.path().join("session.json").exists(),
        "a refused save wrote a file"
    );
}

#[test]
fn a_duplicate_identity_key_is_refused_on_load() {
    let dir = TempDir::new("identities-duplicate");
    let path = dir.path().join("session.json");
    let entry = identity_json("google-ada");
    // A JSON object can repeat a key; serde would keep the last silently.
    fs::write(
        &path,
        format!(
            r#"{{"version":1,"installed":[],"dock":[],"tabs":[],
                "identities":{{"google-ada":{entry},"google-ada":{entry}}}}}"#
        ),
    )
    .expect("write");
    let before = read_bytes(&path);

    match SessionStore::new(&path).load() {
        Err(StoreError::BadIdentity { identity_id, why }) => {
            assert_eq!(identity_id, "google-ada");
            assert_eq!(why, "the identity appears twice");
        }
        other => panic!("expected BadIdentity, got {other:?}"),
    }
    assert_eq!(read_bytes(&path), before, "a refused load rewrote the file");
}

#[test]
fn favorites_refuse_unknown_apps_duplicates_and_control_chars() {
    let dir = TempDir::new("favorites-validation");
    let store = dir.store();

    // Unknown application refused whole
    let mut bad_app = full_state();
    bad_app.favorites.push(session_store::FavoriteEntry {
        id: "fav-ghost".to_owned(),
        application_id: "not-installed".to_owned(),
        title: "Ghost".to_owned(),
        url: "https://example.com".to_owned(),
    });
    assert!(matches!(
        store.save(&bad_app),
        Err(StoreError::BadFavorite { favorite_id, .. }) if favorite_id == "fav-ghost"
    ));

    // Duplicate id refused
    let mut dup = full_state();
    let copy = dup.favorites[0].clone();
    dup.favorites.push(copy);
    assert!(matches!(
        store.save(&dup),
        Err(StoreError::BadFavorite { favorite_id, .. }) if favorite_id == "fav-slack-1"
    ));

    // Control characters refused
    let mut ctrl = full_state();
    ctrl.favorites[0].title = "Slack\nInject".to_owned();
    assert!(matches!(
        store.save(&ctrl),
        Err(StoreError::BadFavorite { .. })
    ));
}
