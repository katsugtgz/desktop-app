//! Slice 26 tests: keyboard shortcuts, top-bar web controls, and the Rune tab-actions strip.

use appstore_service::ApplicationService;
use rune::{Env, Key, KeyEvent, Mods, Runtime, Tx};

use crate::routing;
use crate::shell;
use crate::state::{ShellState, Surface};
use crate::webview;
use crate::webview::fake::{Event, FakeWebview};

const WINDOW: (f32, f32) = (1200.0, 800.0);

fn runtime_with_apps(tag: &str) -> (Runtime<ShellState>, webview::fake::WebviewLog) {
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));
    let mut state = ShellState::new(ApplicationService::for_tests(tag));
    assert!(state.install("station-manifest://14")); // Gmail
    assert!(state.install("station-manifest://21")); // Slack
    let mut rt = Runtime::new(
        state,
        shell::view,
        WINDOW,
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.frame(1.0 / 60.0);
    webview::after_frame(&rt);
    (rt, log)
}

fn press(rt: &mut Runtime<ShellState>, key: &str) {
    let (x, y) = rt
        .find(key)
        .unwrap_or_else(|| panic!("{key} is not laid out"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    webview::after_frame(rt);
}

fn key(rt: &mut Runtime<ShellState>, key: Key) -> bool {
    let mut tx = Tx::default();
    let handled = shell::on_key(
        &mut rt.state,
        &KeyEvent {
            key,
            mods: Mods::default(),
        },
        &mut tx,
    );
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    webview::after_frame(rt);
    handled
}

#[test]
fn keyboard_shortcuts_drive_navigation_reload_zoom_settings_and_tab_jump() {
    let (mut rt, log) = runtime_with_apps("shortcuts-keys");
    let slack_id = rt.state.active().id.clone();

    // Reload via Cmd/Ctrl+R and F5
    log.clear();
    assert!(key(&mut rt, Key::Command('r')));
    assert!(key(&mut rt, Key::Function(5)));
    assert_eq!(
        log.events(),
        vec![
            Event::Reload {
                tab: slack_id.clone()
            },
            Event::Reload {
                tab: slack_id.clone()
            },
        ]
    );

    // Back and Forward via Cmd/Ctrl+[ and ]
    log.clear();
    assert!(key(&mut rt, Key::Command('[')));
    assert!(key(&mut rt, Key::Command(']')));
    assert_eq!(
        log.events(),
        vec![
            Event::Eval {
                tab: slack_id.clone(),
                script: "window.history.back();".to_owned(),
            },
            Event::Eval {
                tab: slack_id.clone(),
                script: "window.history.forward();".to_owned(),
            },
        ]
    );

    // Zoom in / out / reset with clamping
    log.clear();
    assert!(key(&mut rt, Key::Command('=')));
    assert!((rt.state.zoom_of(&slack_id) - 1.1).abs() < 0.001);
    assert!(key(&mut rt, Key::Command('-')));
    assert!((rt.state.zoom_of(&slack_id) - 1.0).abs() < 0.001);
    assert!(key(&mut rt, Key::Command('+')));
    assert!(key(&mut rt, Key::Command('0')));
    assert!((rt.state.zoom_of(&slack_id) - 1.0).abs() < 0.001);
    assert!(log.events().iter().any(|e| matches!(e, Event::Zoom { .. })));

    // Cmd/Ctrl+, opens Settings; Escape returns to Apps
    assert!(key(&mut rt, Key::Command(',')));
    assert_eq!(rt.state.surface(), Surface::Settings);
    assert!(key(&mut rt, Key::Escape));
    assert_eq!(rt.state.surface(), Surface::Apps);

    // Cmd/Ctrl+1 selects App Store (index 1); Cmd/Ctrl+2 selects Gmail (index 2)
    assert!(key(&mut rt, Key::Command('1')));
    assert_eq!(rt.state.active().id, "appstore");
    assert!(key(&mut rt, Key::Command('2')));
    assert_ne!(rt.state.active().id, "appstore");
    assert_ne!(rt.state.active().id, slack_id);
}

#[test]
fn topbar_controls_and_tab_actions_bar_operate_and_shrink_content_cleanly() {
    let _serial = crate::test_support::lock();
    let opened = routing::record_external();
    let (mut rt, log) = runtime_with_apps("shortcuts-topbar");
    let active_id = rt.state.active().id.clone();

    let rect_before = webview::pane_rect(&rt).expect("pane rect before");

    // Top bar Back / Forward / Reload / Open in browser
    log.clear();
    press(&mut rt, "topbar-back");
    press(&mut rt, "topbar-forward");
    press(&mut rt, "topbar-reload");
    press(&mut rt, "topbar-open-browser");
    assert!(log
        .events()
        .iter()
        .any(|e| matches!(e, Event::Eval { script, .. } if script == "window.history.back();")));
    assert!(log
        .events()
        .iter()
        .any(|e| matches!(e, Event::Eval { script, .. } if script == "window.history.forward();")));
    assert!(log
        .events()
        .iter()
        .any(|e| matches!(e, Event::Reload { tab } if tab == &active_id)));
    assert_eq!(routing::recorded_external(&opened).len(), 1);

    // Open tab actions bar -> sits above content so WebView2 bounds shift cleanly below it!
    press(&mut rt, "topbar-menu");
    assert_eq!(rt.state.tab_menu_for(), Some(active_id.as_str()));
    assert!(rt.find("tab-actions-bar").is_some());
    let rect_with_menu = webview::pane_rect(&rt).expect("pane rect with menu");
    assert!(
        rect_with_menu.y > rect_before.y && rect_with_menu.h < rect_before.h,
        "tab actions bar should shrink content rect cleanly: before={rect_before:?} after={rect_with_menu:?}"
    );

    // Move up works from tab actions bar (Slack is 2nd installed app -> can move up)
    assert!(rt.find("tab-action-move-up").is_some());
    press(&mut rt, "tab-action-move-up");
    assert_eq!(rt.state.rail_order()[0], active_id);

    // Escape closes tab actions bar without closing Apps surface
    assert!(key(&mut rt, Key::Escape));
    assert_eq!(rt.state.tab_menu_for(), None);
    assert_eq!(rt.state.surface(), Surface::Apps);

    // Open menu again and uninstall cleanly
    press(&mut rt, "topbar-menu");
    press(&mut rt, "tab-action-uninstall");
    assert!(!rt.state.tabs().iter().any(|t| t.id == active_id));
    assert!(log
        .events()
        .iter()
        .any(|e| matches!(e, Event::Detach { tab } if tab == &active_id)));
}
