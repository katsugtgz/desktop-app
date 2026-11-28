//! The window: a rail of apps beside a pane showing the active one.
//!
//! [`view`] is what `AppSpec` renders. It is the only place an appearance becomes tokens, so no
//! view below it can hardcode a colour: they all take the [`Paint`] it builds.

use crate::appstore;
use crate::notifications;
use crate::placeholder;
use crate::rail;
use crate::settings;
use crate::state::{ShellState, Surface};
use rune::tok::{self, Theme};
use rune::{col, row, Cx, El};
use session_store::ThemePreference;

/// Everything a view paints with: one appearance's tokens, and which appearance they are (the
/// accent and the window background depend on it, and `tok::Theme` keeps only the colours).
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    pub t: Theme,
    pub dark: bool,
}

impl Paint {
    pub fn of(dark: bool) -> Self {
        Self {
            t: tok::theme(dark),
            dark,
        }
    }
}

/// Keys no control took: global shortcuts matching Station's keymap (`Escape`, Quick Switcher
/// `Ctrl+K`/`Ctrl+T`, find, reload, back, forward, zoom, Settings, tab jump 1..9).
pub fn on_key(state: &mut ShellState, event: &rune::KeyEvent, _: &mut rune::Tx) -> bool {
    if event.key == rune::Key::Escape {
        return state.close_find()
            || state.close_tab_menu()
            || state.close_custom_app_form()
            || state.close_download_dir_editor()
            || (state.surface() == Surface::Apps
                && state.active().renders_in_rune()
                && state.skip_onboarding())
            || state.close_overlay();
    }
    if event.is_command('k') || event.is_command('t') {
        if state.surface() == Surface::QuickSwitch {
            state.close_overlay();
        } else {
            state.open_quick_switch();
        }
        return true;
    }
    if state.surface() == Surface::QuickSwitch {
        match event.key {
            rune::Key::Up => {
                state.move_quick_switch_selection(-1);
                return true;
            }
            rune::Key::Down => {
                state.move_quick_switch_selection(1);
                return true;
            }
            rune::Key::Enter => {
                state.submit_quick_switch();
                return true;
            }
            _ => {}
        }
    }
    if event.is_command(',') {
        state.open_settings();
        return true;
    }
    if let rune::Key::Command(digit @ '1'..='9') = event.key {
        let index = (digit as u8 - b'0') as usize;
        return state.select_by_index(index);
    }
    let active_web = state.shows_page().then(|| state.active().id.clone());
    if let Some(tab_id) = active_web {
        if event.is_command('f') {
            return state.open_find();
        }
        if event.is_command('r') || event.key == rune::Key::Function(5) {
            crate::webview::reload(&tab_id);
            return true;
        }
        if event.is_command('[') {
            crate::webview::go_back(&tab_id);
            return true;
        }
        if event.is_command(']') {
            crate::webview::go_forward(&tab_id);
            return true;
        }
        if event.is_command('=') || event.is_command('+') {
            let next = state.adjust_zoom(&tab_id, 0.1);
            crate::webview::set_zoom(&tab_id, next);
            return true;
        }
        if event.is_command('-') {
            let next = state.adjust_zoom(&tab_id, -0.1);
            crate::webview::set_zoom(&tab_id, next);
            return true;
        }
        if event.is_command('0') {
            let next = state.reset_zoom(&tab_id);
            crate::webview::set_zoom(&tab_id, next);
            return true;
        }
    }
    false
}

/// The shell's view in the appearance the session asks for: the desktop's when it is `System`,
/// otherwise the user's choice.
pub fn view(state: &ShellState, cx: &mut Cx<ShellState>) -> El {
    let dark = match state.theme() {
        ThemePreference::System => cx.system_dark(),
        ThemePreference::Dark => true,
        ThemePreference::Light => false,
    };
    view_themed(state, dark)
}

/// The shell's view in a named appearance: the seam headless tests and snapshot tooling use,
/// with the same body `view` renders.
pub fn view_themed(state: &ShellState, dark: bool) -> El {
    let p = Paint::of(dark);
    let body = match state.surface() {
        Surface::Settings => settings::view(state, p),
        Surface::About => crate::about::view(state, p),
        Surface::QuickSwitch => crate::quick_switch::view(state, p),
        Surface::Apps | Surface::Notifications => {
            let app = if state.active().renders_in_rune() {
                appstore::view(state, p)
            } else {
                placeholder::content(state, p)
            };
            if state.surface() == Surface::Notifications {
                // A row, not an overlay: Rune cannot draw over the hosted page, so the panel
                // takes its 320 pt beside the page area (the page is hidden while it shows).
                row()
                    .key("pane-row")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .child(app)
                    .child(notifications::panel(state, p))
            } else {
                app
            }
        }
    };
    let mut pane = col()
        .key("pane")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .child(placeholder::top_bar(state, p));
    if let Some(actions) = placeholder::tab_actions_bar(state, p) {
        pane = pane.child(actions);
    }
    if state.find().open && state.shows_page() {
        pane = pane.child(crate::find::bar(state, p));
    }
    pane = pane.child(body);
    if !state.downloads().is_empty() {
        pane = pane.child(crate::downloads::bar(state, p));
    }
    col().key("shell").w_full().h_full().fill(p.t.bg).child(
        row()
            .key("body")
            .w_full()
            .grow(1.0)
            .child(rail::rail(state, p))
            .child(pane),
    )
}
