//! Slice 11 tests: the rail order is the user's, changed from Settings' My apps card and
//! written the moment it changes.
//!
//! Everything goes through the real view: a My apps row's move control (`settings-move-up-<id>`
//! / `settings-move-down-<id>`) is pressed through the runtime, and the element tree, the state
//! and the session file say what happened. The affordance lives in Settings because Rune gives
//! a rail row no keyboard focus, so a rail-row key path cannot work (D11). The ordering
//! authority is `rail_order`: the rail builds from it, `dock` stays the service's membership
//! fact, and a document whose `railOrder` does not name the docked set exactly is refused whole
//! (`session-store`) and falls back to install order.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use appstore_service::ApplicationService;
use rune::{Env, Runtime};
use session_store::{SessionStore, StoreError, ThemePreference};

use crate::shell;
use crate::state::ShellState;
use crate::webview;
use crate::webview::fake::{Event, FakeWebview};

const WINDOW: (f32, f32) = (1200.0, 800.0);

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn tag() -> String {
    format!(
        "station-shell-rail-order-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A shell launched from a fresh session file over an isolated private-manifest store.
fn launched(path: &std::path::Path) -> ShellState {
    ShellState::launch_in(ApplicationService::for_tests(&tag()), path)
}

fn session_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "station-shell-rail-order-{name}-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    path
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

/// True when an element with this exact key is drawn right now.
fn drawn(rt: &Runtime<ShellState>, key: &str) -> bool {
    rt.find(key).is_some()
}

/// The labels the rail shows, top to bottom: the App Store first, then the apps in rail order.
fn rail_labels(rt: &Runtime<ShellState>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|element| !element.exiting)
        .filter(|element| {
            element
                .key
                .as_deref()
                .is_some_and(|k| k.starts_with("tab-"))
        })
        .filter_map(|element| element.label)
        .collect()
}

/// Installs Gmail then Todoist through the store's tile presses: the rail ends
/// `[App Store, Gmail, Todoist]`, with Todoist active.
fn install_two(rt: &mut Runtime<ShellState>) -> (String, String) {
    press(rt, "store-tile-14"); // Gmail
    draw(rt);
    let gmail = rt.state.active().id.clone();
    press(rt, "tab-appstore");
    draw(rt);
    press(rt, "store-tile-157"); // Todoist
    draw(rt);
    let todoist = rt.state.active().id.clone();
    (gmail, todoist)
}

/// Selects Gmail with a row press, then opens Settings so the move controls are drawn.
fn select_gmail_and_open_settings(rt: &mut Runtime<ShellState>, gmail: &str) {
    press(rt, &format!("tab-{gmail}"));
    draw(rt);
    press(rt, "rail-settings");
    draw(rt);
}

// ---- The order the rail shows --------------------------------------------------------------

#[test]
fn the_rail_starts_at_the_store_and_install_order_backs_the_user_order() {
    let mut rt = runtime(launched(&session_path("fresh")));
    draw(&mut rt);

    assert_eq!(rail_labels(&rt), vec!["App Store".to_owned()]);

    install_two(&mut rt);

    assert_eq!(
        rail_labels(&rt),
        vec!["App Store", "Gmail", "Todoist"],
        "a fresh order is install order"
    );
    assert_eq!(
        rt.state.rail_order(),
        &[rt.state.tabs()[1].id.clone(), rt.state.tabs()[2].id.clone()],
        "the user order names the apps, newest last"
    );
}

// ---- The Settings affordance ---------------------------------------------------------------

#[test]
fn pressing_a_move_control_moves_the_row_one_step_and_touches_nothing_else() {
    let path = session_path("press-moves");
    let mut rt = runtime(launched(&path));
    draw(&mut rt);
    let (gmail, todoist) = install_two(&mut rt);

    select_gmail_and_open_settings(&mut rt, &gmail);

    // The edges: the first movable app has no up, the last no down; the store row is not
    // movable and never draws a control.
    assert!(!drawn(&rt, &format!("settings-move-up-{gmail}")));
    assert!(drawn(&rt, &format!("settings-move-down-{gmail}")));
    assert!(drawn(&rt, &format!("settings-move-up-{todoist}")));
    assert!(!drawn(&rt, &format!("settings-move-down-{todoist}")));
    assert!(!drawn(&rt, "settings-move-up-appstore"));
    assert!(!drawn(&rt, "settings-move-down-appstore"));

    // Gmail down: one step past Todoist, selection untouched (Gmail was selected to reach
    // Settings, and the move must not change it).
    press(&mut rt, &format!("settings-move-down-{gmail}"));
    draw(&mut rt);
    assert_eq!(
        rail_labels(&rt),
        vec!["App Store", "Todoist", "Gmail"],
        "the move did not land one step down"
    );
    assert_eq!(
        rt.state.active().id,
        gmail,
        "the move changed the selection"
    );

    // Gmail is last now: its up control moves it back.
    press(&mut rt, &format!("settings-move-up-{gmail}"));
    draw(&mut rt);
    assert_eq!(rail_labels(&rt), vec!["App Store", "Gmail", "Todoist"]);
    assert_eq!(
        rt.state.active().id,
        gmail,
        "the move changed the selection"
    );
}

#[test]
fn a_single_app_gets_no_move_controls() {
    let mut rt = runtime(launched(&session_path("single")));
    draw(&mut rt);
    press(&mut rt, "store-tile-14"); // Gmail
    draw(&mut rt);
    let gmail = rt.state.active().id.clone();

    press(&mut rt, "rail-settings");
    draw(&mut rt);

    assert!(!drawn(&rt, &format!("settings-move-up-{gmail}")));
    assert!(!drawn(&rt, &format!("settings-move-down-{gmail}")));
    assert_eq!(rail_labels(&rt), vec!["App Store", "Gmail"]);
}

// ---- Durable + immediate -------------------------------------------------------------------

#[test]
fn a_move_is_written_at_once_and_survives_a_relaunch() {
    let path = session_path("durable");
    let mut rt = runtime(launched(&path));
    draw(&mut rt);
    let (gmail, todoist) = install_two(&mut rt);

    select_gmail_and_open_settings(&mut rt, &gmail);
    press(&mut rt, &format!("settings-move-down-{gmail}"));
    draw(&mut rt);
    assert_eq!(rail_labels(&rt), vec!["App Store", "Todoist", "Gmail"]);
    assert_eq!(
        rt.state.active().id,
        gmail,
        "the move changed the selection"
    );

    // The step reached the document at once (D7), before any quit.
    let saved = SessionStore::new(&path).load().expect("the session loads");
    assert_eq!(
        saved.rail_order,
        vec![todoist.clone(), gmail.clone()],
        "the move is not in the document yet"
    );

    // Relaunch from the same file: the order comes back.
    let restored = launched(&path);
    let gmail_label = restored
        .tabs()
        .iter()
        .find(|tab| tab.id == gmail)
        .map(|tab| tab.label.clone())
        .unwrap();
    let todoist_label = restored
        .tabs()
        .iter()
        .find(|tab| tab.id == todoist)
        .map(|tab| tab.label.clone())
        .unwrap();
    assert_eq!(
        restored
            .tabs()
            .iter()
            .map(|tab| tab.label.clone())
            .collect::<Vec<_>>(),
        vec!["App Store".to_owned(), todoist_label, gmail_label],
        "the relaunch did not restore the user's order"
    );
    assert_eq!(
        restored.active().id,
        gmail,
        "the relaunch did not restore the selection"
    );
}

#[test]
fn a_hostile_rail_order_is_refused_and_the_document_is_untouched() {
    // An order naming an app the dock does not have (an unknown id) is refused whole by
    // `session-store`; the shell starts with the store alone and the file stays byte-for-byte.
    let path = session_path("hostile-unknown");
    let document = r#"{"version":1,"installed":[{"application_id":"ghost-app","manifest_url":"https://example.com/manifest.json","in_dock":true,"config_data":{}}],"dock":["ghost-app"],"tabs":[],"railOrder":["ghost-app","stowaway"]}"#.to_owned();
    std::fs::write(&path, &document).expect("write hostile session");

    let mut state = launched(&path);
    assert_eq!(state.tabs().len(), 1, "the refused session was replayed");
    let kept = std::fs::read_to_string(&path).expect("read back");
    assert_eq!(kept, document, "a refused document was rewritten");

    // The next user action clears the block and writes a valid session (D7's rule): a theme
    // change persists, and the healed order is the fallback install order (the dock's).
    state.set_theme(ThemePreference::Light);
    let healed = SessionStore::new(&path)
        .load()
        .expect("a valid document now");
    // The healed order is the fallback (empty `railOrder` = install order); the dock is
    // untouched and keeps its membership fact.
    assert!(healed.rail_order.is_empty());
}

#[test]
fn a_duplicated_rail_order_entry_is_refused_whole() {
    let path = session_path("hostile-duplicate");
    let document = r#"{"version":1,"installed":[{"application_id":"ghost-app","manifest_url":"https://example.com/manifest.json","in_dock":true,"config_data":{}}],"dock":["ghost-app"],"tabs":[],"railOrder":["ghost-app","ghost-app"]}"#.to_owned();
    std::fs::write(&path, &document).expect("write hostile session");

    match SessionStore::new(&path).load() {
        Err(StoreError::BadRailOrder {
            application_id,
            why,
        }) => {
            assert_eq!(application_id, "ghost-app");
            assert_eq!(why, "the application appears twice");
        }
        other => panic!("expected BadRailOrder, got {other:?}"),
    }
}

#[test]
fn an_over_cap_rail_order_is_refused() {
    // The caps are checked installed → dock → tab → rail, all 512: the document fills the
    // installed and dock caps exactly, so the refusal that trips is the rail's own.
    let path = session_path("hostile-overcap");
    let installed: Vec<String> = (0..session_store::MAX_INSTALLED)
        .map(|index| {
            format!(
                r#"{{"application_id":"app-{index}","manifest_url":"https://example.com/{index}.json","in_dock":true,"config_data":{{}}}}"#
            )
        })
        .collect();
    let dock: Vec<String> = (0..session_store::MAX_INSTALLED)
        .map(|index| format!("\"app-{index}\""))
        .collect();
    let rail: Vec<String> = (0..=session_store::MAX_RAIL)
        .map(|index| format!("\"rail-{index}\""))
        .collect();
    let document = format!(
        r#"{{"version":1,"installed":[{}],"dock":[{}],"tabs":[],"railOrder":[{}]}}"#,
        installed.join(","),
        dock.join(","),
        rail.join(",")
    );
    std::fs::write(&path, document).expect("write over-cap session");

    match SessionStore::new(&path).load() {
        Err(StoreError::TooManyEntries { field, count, max }) => {
            assert_eq!(field, "rail");
            assert_eq!(count, session_store::MAX_RAIL + 1);
            assert_eq!(max, session_store::MAX_RAIL);
        }
        other => panic!("expected TooManyEntries, got {other:?}"),
    }
}

// ---- Store installs and uninstalls keep the order sane -------------------------------------

#[test]
fn installing_appends_to_the_users_order_and_uninstalling_leaves_it() {
    let path = session_path("store-keeps-order");
    let mut rt = runtime(launched(&path));
    draw(&mut rt);
    let (gmail, todoist) = install_two(&mut rt);

    // Gmail down, so the user's order is [Todoist, Gmail]; the next install lands last.
    select_gmail_and_open_settings(&mut rt, &gmail);
    press(&mut rt, &format!("settings-move-down-{gmail}"));
    draw(&mut rt);
    assert_eq!(rail_labels(&rt), vec!["App Store", "Todoist", "Gmail"]);

    // Back to the apps, then the store: the next install lands last in the user's order.
    press(&mut rt, "settings-back");
    draw(&mut rt);
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    press(&mut rt, "store-tile-21"); // Slack
    draw(&mut rt);
    let slack = rt.state.active().id.clone();
    assert_eq!(
        rail_labels(&rt),
        vec!["App Store", "Todoist", "Gmail", "Slack"],
        "the install did not append to the user's order"
    );
    assert_eq!(
        rt.state.rail_order(),
        &[todoist.clone(), gmail.clone(), slack.clone()]
    );

    // Uninstall Todoist: its row goes, the others keep their places.
    press(&mut rt, "tab-appstore");
    draw(&mut rt);
    press(&mut rt, "store-tile-157"); // installed: the tile uninstalls
    draw(&mut rt);
    assert_eq!(
        rail_labels(&rt),
        vec!["App Store", "Gmail", "Slack"],
        "the uninstall left a hole or moved a row"
    );
    assert_eq!(rt.state.rail_order(), &[gmail.clone(), slack.clone()]);

    // The document agrees.
    let saved = SessionStore::new(&path).load().unwrap();
    assert_eq!(saved.rail_order, vec![gmail, slack]);
}

// ---- The move never disturbs the rest of the shell ------------------------------------------

#[test]
fn a_move_touches_no_page_and_no_selection() {
    let log = {
        let (host, log) = FakeWebview::new();
        webview::install(Box::new(host));
        log
    };
    let path = session_path("no-page");
    let mut rt = runtime(launched(&path));
    draw(&mut rt);
    let (gmail, _todoist) = install_two(&mut rt);

    // Settle on Gmail: its page is attached and visible; open Settings (the page is hidden,
    // not detached) and record where the move starts.
    select_gmail_and_open_settings(&mut rt, &gmail);
    let before = log.events().len();

    press(&mut rt, &format!("settings-move-down-{gmail}"));
    draw(&mut rt);

    let new_events = log.events()[before..].to_vec();
    assert_eq!(rt.state.active().id, gmail);
    assert_eq!(
        log.events().len(),
        before,
        "the move touched the hosted page: {:?}",
        new_events
    );
    assert!(
        new_events
            .iter()
            .all(|event| !matches!(event, Event::Attach { .. } | Event::Detach { .. })),
        "the move attached or detached a page"
    );
}

#[test]
fn the_store_row_presses_still_select_while_a_row_can_move() {
    let path = session_path("press-still-selects");
    let mut rt = runtime(launched(&path));
    draw(&mut rt);
    let (gmail, todoist) = install_two(&mut rt);

    // The row's press is still select (the press-to-select behaviour is unchanged), and the
    // move controls live in Settings, so a rail press can never reorder.
    press(&mut rt, &format!("tab-{gmail}"));
    draw(&mut rt);
    assert_eq!(rt.state.active().id, gmail);
    press(&mut rt, &format!("tab-{todoist}"));
    draw(&mut rt);
    assert_eq!(rt.state.active().id, todoist);
    assert_eq!(
        rail_labels(&rt),
        vec!["App Store", "Gmail", "Todoist"],
        "pressing a row reordered the rail"
    );
}
