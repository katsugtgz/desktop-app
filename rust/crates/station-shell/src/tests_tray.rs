//! Slice 18 tests: the tray icon's model and pixels, and the launch-at-login item.
//!
//! The tray is windowed-only (Rune's `Tray` needs a notification area); what is testable here is
//! its pure model — the Electron menu, the event mapping, and the ICO entry decode — plus the
//! login item's exact value/quoting and its Settings toggle round-trip through the session.
//! The registry itself is never touched: a fake [`LoginItem`] stands in, and the real
//! `SystemLoginItem` is only reached from the production launch and the row's press.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use appstore_service::ApplicationService;
use rune::{Env, Runtime};
use session_store::{SessionState, SessionStore, FORMAT_VERSION};

use crate::autolaunch::{self, LoginItem};
use crate::ico;
use crate::settings;
use crate::shell;
use crate::state::ShellState;
use crate::tray;
use crate::webview;

const WINDOW: (f32, f32) = (1200.0, 800.0);

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn tag(name: &str) -> String {
    format!(
        "station-shell-tray-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn session_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("{}.json", tag(name)));
    let _ = std::fs::remove_file(&path);
    path
}

/// A shell launched from `path` over an isolated private-manifest store.
fn launched(path: &Path) -> ShellState {
    ShellState::launch_in(ApplicationService::for_tests(&tag("store")), path)
}

fn runtime(state: ShellState) -> Runtime<ShellState> {
    Runtime::new(
        state,
        shell::view,
        WINDOW,
        Env {
            system_dark: true,
            scale: 1.0,
        },
    )
}

fn draw(rt: &mut Runtime<ShellState>) {
    rt.frame(1.0 / 60.0);
    webview::after_frame(rt);
}

fn press(rt: &mut Runtime<ShellState>, key: &str) {
    let (x, y) = rt
        .find(key)
        .unwrap_or_else(|| panic!("{key} is not laid out"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
}

fn element_text(rt: &Runtime<ShellState>, key: &str) -> Option<String> {
    rt.elements()
        .into_iter()
        .find(|element| !element.exiting && element.key.as_deref() == Some(key))
        .and_then(|element| element.text.or(element.label))
}

/// The login item's in-memory stand-in: no registry, so the tests never touch the user's login
/// items. `fail_set` makes the write fail while leaving the stored truth alone — the case the
/// Settings row must report.
struct FakeLoginItem {
    enabled: Cell<bool>,
    fail_set: bool,
    set_calls: Cell<u32>,
}

impl FakeLoginItem {
    fn new(enabled: bool) -> Self {
        Self {
            enabled: Cell::new(enabled),
            fail_set: false,
            set_calls: Cell::new(0),
        }
    }

    fn failing() -> Self {
        Self {
            enabled: Cell::new(false),
            fail_set: true,
            set_calls: Cell::new(0),
        }
    }
}

impl LoginItem for FakeLoginItem {
    fn enabled(&self) -> Result<bool, String> {
        Ok(self.enabled.get())
    }

    fn set(&self, enable: bool) -> Result<(), String> {
        self.set_calls.set(self.set_calls.get() + 1);
        if self.fail_set {
            return Err("the registry denied the write".to_owned());
        }
        self.enabled.set(enable);
        Ok(())
    }
}

// ---- Tray model -------------------------------------------------------------------------

#[test]
fn the_tray_menu_is_the_electron_tray_menu() {
    assert_eq!(tray::MENU.len(), 2, "only Open and Exit, no invented items");
    assert_eq!(tray::MENU[0].label, "Open");
    assert_eq!(tray::MENU[0].command, tray::Command::Open);
    assert_eq!(tray::MENU[1].label, "Exit");
    assert_eq!(tray::MENU[1].command, tray::Command::Quit);
    assert!(
        tray::MENU.iter().all(|item| item.enabled),
        "both items start enabled"
    );

    let rune_menu = tray::rune_menu();
    assert_eq!(
        rune_menu,
        vec![
            rune::TrayItem::action("open", "Open"),
            rune::TrayItem::action("quit", "Exit"),
        ]
    );
}

#[test]
fn tray_events_map_to_focus_or_quit() {
    assert_eq!(
        tray::command_for(&rune::TrayEvent::Activate { token: None }),
        Some(tray::Command::Open),
        "a primary click brings the window forward"
    );
    assert_eq!(
        tray::command_for(&rune::TrayEvent::Menu("open".to_owned())),
        Some(tray::Command::Open)
    );
    assert_eq!(
        tray::command_for(&rune::TrayEvent::Menu("quit".to_owned())),
        Some(tray::Command::Quit)
    );
    assert_eq!(
        tray::command_for(&rune::TrayEvent::Menu("settings".to_owned())),
        None,
        "an unknown menu id does nothing"
    );
}

#[test]
fn the_tray_icon_decodes_from_the_embedded_ico() {
    let pixels = ico::rgba(tray::TRAY_ICO, 16).expect("the 16 px entry decodes");
    assert_eq!((pixels.width, pixels.height), (16, 16));
    assert_eq!(pixels.rgba.len(), 16 * 16 * 4);
    assert!(
        pixels
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] != 0),
        "the icon is not fully transparent"
    );
    assert!(
        ico::rgba(tray::TRAY_ICO, 20)
            .expect("the nearest entry at least 20")
            .width
            >= 20,
        "a size the ICO lacks scales from a larger entry, never a smaller one"
    );
    assert!(
        ico::rgba(&[], 16).is_none(),
        "a missing file is no pixels, not a panic"
    );
}

#[test]
fn the_window_icon_asset_holds_both_sizes() {
    // Moved with the parser: the window icon's own asset still parses to the 32/64 entries and
    // their `RT_ICON` headers (doubled `biHeight` for the AND mask).
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/station.ico");
    let bytes = std::fs::read(path).expect("the window icon asset is in the repo");
    let entries = ico::entries(&bytes).expect("the window icon parses");
    for size in [32u32, 64] {
        let entry = ico::entry_for(&entries, size).expect("the size is in the ICO");
        assert_eq!((entry.width, entry.height), (size, size));
        assert!(!entry.payload.is_empty());
        let header = u32::from_le_bytes(entry.payload[0..4].try_into().unwrap());
        let width = i32::from_le_bytes(entry.payload[4..8].try_into().unwrap());
        let height = i32::from_le_bytes(entry.payload[8..12].try_into().unwrap());
        assert_eq!(header, 40, "a BITMAPINFOHEADER, not a PNG entry");
        assert_eq!(width, size as i32);
        assert_eq!(height, size as i32 * 2, "the AND mask doubles biHeight");
    }
}

#[test]
fn the_ico_picker_prefers_exact_then_larger_then_largest() {
    let payload = b"x";
    let entry = |width, height| ico::Entry {
        width,
        height,
        payload,
    };
    let mixed = [entry(16, 16), entry(48, 48), entry(256, 256)];
    assert_eq!(ico::entry_for(&mixed, 16).expect("exact").width, 16);
    assert_eq!(ico::entry_for(&mixed, 48).expect("exact").width, 48);
    assert_eq!(ico::entry_for(&mixed, 32).expect("next larger").width, 48);
    assert_eq!(ico::entry_for(&mixed, 64).expect("next larger").width, 256);
    assert_eq!(ico::entry_for(&mixed, 128).expect("next larger").width, 256);

    let all_smaller = [entry(16, 16), entry(24, 24)];
    assert_eq!(ico::entry_for(&all_smaller, 32).expect("largest").width, 24);
    assert!(ico::entry_for(&[], 32).is_none());
}

#[test]
fn png_entries_are_skipped_and_malformed_directories_refused() {
    let bitmap = [0u8; 4];
    let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let bitmap_offset = ico::HEADER_BYTES + 2 * ico::ENTRY_BYTES;
    let png_offset = bitmap_offset + bitmap.len();
    let mut file = vec![0, 0, 1, 0, 2, 0];
    file.extend_from_slice(&[32, 32, 0, 0, 1, 0, 32, 0, 4, 0, 0, 0]);
    file.extend_from_slice(&(bitmap_offset as u32).to_le_bytes());
    file.extend_from_slice(&[64, 64, 0, 0, 1, 0, 32, 0, 8, 0, 0, 0]);
    file.extend_from_slice(&(png_offset as u32).to_le_bytes());
    file.extend_from_slice(&bitmap);
    file.extend_from_slice(&png);

    let entries = ico::entries(&file).expect("the synthetic ICO parses");
    assert_eq!(entries.len(), 1);
    assert_eq!((entries[0].width, entries[0].height), (32, 32));
    assert_eq!(entries[0].payload, &bitmap);

    assert!(ico::entries(&[]).is_err());
    assert!(
        ico::entries(&[1, 0, 1, 0, 0, 0]).is_err(),
        "reserved must be zero"
    );
    assert!(
        ico::entries(&[0, 0, 2, 0, 0, 0]).is_err(),
        "type must be an icon"
    );
    let mut truncated = vec![0, 0, 1, 0, 1, 0];
    truncated.extend_from_slice(&[32, 32, 0, 0, 1, 0, 32, 0, 64, 0, 0, 0]);
    truncated.extend_from_slice(&0u32.to_le_bytes());
    assert!(
        ico::entries(&truncated).is_err(),
        "a payload past the file is refused"
    );
}

// ---- Launch at login --------------------------------------------------------------------

#[test]
fn the_login_item_is_the_electron_value_and_quoting() {
    assert_eq!(autolaunch::VALUE_NAME, "Station");
    assert_eq!(
        autolaunch::RUN_KEY,
        r"Software\Microsoft\Windows\CurrentVersion\Run"
    );
    let command =
        autolaunch::command_line(Path::new(r"C:\Program Files\Station\station-shell.exe"));
    assert_eq!(
        command, "\"C:\\Program Files\\Station\\station-shell.exe\"",
        "the path is quoted, with no arguments"
    );
    assert!(!command.contains(" --"), "no Squirrel/Electron arguments");
    assert!(autolaunch::supported() == cfg!(target_os = "windows"));
}

#[test]
fn the_login_toggle_round_trips_through_the_session() {
    let path = session_path("roundtrip");
    let mut state = launched(&path);
    let item = FakeLoginItem::new(false);
    assert!(!state.start_at_login());

    state.set_start_at_login(true, &item);
    assert!(state.start_at_login(), "the toggle follows the registry");
    assert_eq!(item.set_calls.get(), 1);
    assert!(item.enabled.get(), "the item points at the shell now");
    let saved = SessionStore::new(&path).load().expect("the session saves");
    assert!(saved.start_at_login, "startAtLogin was not persisted");
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        document["startAtLogin"], true,
        "the additive field is camelCase and the format stays 1"
    );
    assert_eq!(document["version"], FORMAT_VERSION);

    let relaunched = launched(&path);
    assert!(relaunched.start_at_login(), "a relaunch lost the value");

    state.set_start_at_login(false, &item);
    assert!(!state.start_at_login());
    assert!(
        !SessionStore::new(&path)
            .load()
            .expect("the session saves")
            .start_at_login,
        "turning it off was not persisted"
    );
}

#[test]
fn a_failed_login_item_write_keeps_the_toggle_honest() {
    let path = session_path("failed");
    let mut state = launched(&path);
    let item = FakeLoginItem::failing();

    state.set_start_at_login(true, &item);

    assert!(
        !state.start_at_login(),
        "a failed write must not flip the toggle"
    );
    let why = state
        .start_at_login_error()
        .expect("the failure is reported");
    assert!(why.contains("denied"), "the row carries the reason: {why}");
    let saved = SessionStore::new(&path)
        .load()
        .expect("no session yet is a default");
    assert!(!saved.start_at_login, "a failed write was persisted as on");
}

#[test]
fn the_launch_reads_the_registry_truth() {
    let path = session_path("launch");
    SessionStore::new(&path)
        .save(&SessionState {
            start_at_login: true,
            ..SessionState::default()
        })
        .expect("seed the session");
    let mut state = launched(&path);
    assert!(state.start_at_login(), "the session value loads first");

    state.refresh_start_at_login_with(&FakeLoginItem::new(false));
    assert!(
        !state.start_at_login(),
        "the registry wins over the session on launch"
    );
    assert!(state.start_at_login_error().is_none());
}

#[test]
fn the_settings_row_is_absent_where_the_platform_cannot() {
    let state = launched(&session_path("row"));
    assert!(
        settings::start_at_login_model(&state, false).is_none(),
        "no stub row on a platform without a login item"
    );
    let model = settings::start_at_login_model(&state, true).expect("the Windows row");
    assert_eq!(model.key, "settings-start-at-login");
    assert_eq!(model.label, "Start Station when I sign in");
    assert!(!model.on);
}

#[test]
fn the_settings_toggle_draws_and_follows_the_state() {
    let path = session_path("view");
    let mut rt = runtime(launched(&path));
    draw(&mut rt);
    press(&mut rt, "rail-settings");
    draw(&mut rt);

    if autolaunch::supported() {
        assert!(rt.find("settings-start-at-login").is_some(), "no login row");
        assert_eq!(
            element_text(&rt, "settings-start-at-login-value").as_deref(),
            Some("Off")
        );
    } else {
        assert!(
            rt.find("settings-start-at-login").is_none(),
            "non-Windows platforms must not draw a stub login row"
        );
    }
    assert!(rt.find("settings-start-at-login-error").is_none());

    rt.state
        .set_start_at_login(true, &FakeLoginItem::new(false));
    rt.invalidate();
    draw(&mut rt);
    assert!(
        settings::start_at_login_model(&rt.state, true)
            .expect("model")
            .on
    );
    if autolaunch::supported() {
        assert_eq!(
            element_text(&rt, "settings-start-at-login-value").as_deref(),
            Some("On")
        );
    }

    rt.state
        .set_start_at_login(false, &FakeLoginItem::failing());
    rt.invalidate();
    draw(&mut rt);
    assert!(
        settings::start_at_login_model(&rt.state, true)
            .expect("model")
            .on,
        "the failed write did not flip the toggle"
    );
    if autolaunch::supported() {
        assert_eq!(
            element_text(&rt, "settings-start-at-login-value").as_deref(),
            Some("On"),
            "the failed write did not flip the toggle"
        );
    }
    let why = element_text(&rt, "settings-start-at-login-error-text").expect("the error row");
    assert!(why.contains("denied"), "the row says why: {why}");
}

// ---- Minimize / close to tray (Slice 35) ------------------------------------------------

#[test]
fn minimize_to_tray_row_is_absent_where_tray_is_unsupported() {
    let state = launched(&session_path("mtt-row"));
    assert!(
        settings::minimize_to_tray_model(&state, false).is_none(),
        "no stub tray toggle on a platform without a tray"
    );
    let model = settings::minimize_to_tray_model(&state, true).expect("Windows tray row");
    assert_eq!(model.key, "settings-minimize-to-tray");
    assert!(!model.on);
}

#[test]
fn minimize_to_tray_toggle_round_trips_through_session() {
    let path = session_path("mtt-roundtrip");
    let mut state = launched(&path);
    assert!(!state.minimize_to_tray());

    state.toggle_minimize_to_tray();
    assert!(state.minimize_to_tray());
    let saved = SessionStore::new(&path).load().expect("load saved");
    assert!(saved.minimize_to_tray);
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(document["minimizeToTray"], true);
    assert_eq!(document["version"], FORMAT_VERSION);

    let relaunched = launched(&path);
    assert!(relaunched.minimize_to_tray());

    state.toggle_minimize_to_tray();
    assert!(!state.minimize_to_tray());
    assert!(
        !SessionStore::new(&path)
            .load()
            .expect("load off")
            .minimize_to_tray
    );
}

#[test]
fn close_hides_pages_to_tray_when_enabled_and_quits_when_disabled_or_quitting() {
    use crate::webview::fake::{Event, FakeWebview};

    let path = session_path("mtt-close");
    let mut state = launched(&path);
    let app = state
        .popular()
        .cream_of_the_crop_apps
        .first()
        .expect("popular app")
        .clone();
    state.activate_tile(&app);
    assert!(state.shows_page());

    let (fake, log) = FakeWebview::new();
    webview::install(Box::new(fake));
    let mut rt = runtime(state);
    draw(&mut rt);
    let tab_id = rt.state.active().id.clone();
    assert!(
        log.events()
            .iter()
            .any(|e| matches!(e, Event::Attach { tab, .. } if tab == &tab_id)),
        "active web page attached before close"
    );

    // 1. With tray supported + minimize_to_tray enabled + not quitting -> hide page, do NOT detach.
    rt.state.set_minimize_to_tray(true);
    let mut tx = rune::Tx::default();
    let action = crate::handle_close(&mut rt.state, &mut tx, true, false);
    assert_eq!(action, crate::CloseAction::HiddenToTray);
    assert!(
        log.events()
            .iter()
            .any(|e| matches!(e, Event::Visible { tab, visible: false } if tab == &tab_id)),
        "hiding to tray must hide the visible WebView2"
    );
    assert!(
        !log.events()
            .iter()
            .any(|e| matches!(e, Event::Detach { .. })),
        "hiding to tray must keep pages attached for instant reopen"
    );

    // Reopening draws a frame -> sync shows the same attached page again without re-attaching.
    let attaches_before = log
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Attach { .. }))
        .count();
    draw(&mut rt);
    let attaches_after = log
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Attach { .. }))
        .count();
    assert_eq!(attaches_before, attaches_after);
    assert!(matches!(
        log.events().last(),
        Some(Event::Visible { tab, visible: true }) if tab == &tab_id
    ));

    // 2. When quitting is true (e.g. tray Exit), even with minimize_to_tray enabled it quits cleanly.
    let mut tx_quit = rune::Tx::default();
    let action_quit = crate::handle_close(&mut rt.state, &mut tx_quit, true, true);
    assert_eq!(action_quit, crate::CloseAction::Quit);
    assert!(
        log.events()
            .iter()
            .any(|e| matches!(e, Event::Detach { tab } if tab == &tab_id)),
        "explicit quit must detach all pages"
    );

    // 3. When tray is unsupported or minimize_to_tray is false, closing quits cleanly.
    let mut state2 = launched(&session_path("mtt-close-off"));
    let mut tx2 = rune::Tx::default();
    assert_eq!(
        crate::handle_close(&mut state2, &mut tx2, true, false),
        crate::CloseAction::Quit
    );
    state2.set_minimize_to_tray(true);
    assert_eq!(
        crate::handle_close(&mut state2, &mut tx2, false, false),
        crate::CloseAction::Quit
    );
}
