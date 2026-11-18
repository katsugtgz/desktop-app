//! Behavior specs for the durable session store. Written before the
//! implementation: a missing file is a fresh session, a saved session loads
//! back byte-for-byte in value terms, and no failure mode may lose the
//! previous session or leave a temp file behind.

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
        rail_order: vec![
            "appstore".to_owned(),
            "slack".to_owned(),
            "notion".to_owned(),
        ],
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
