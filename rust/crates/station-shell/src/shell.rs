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

/// Keys no control took: Escape leaves Settings or the notification panel, back to the app.
pub fn on_key(state: &mut ShellState, event: &rune::KeyEvent, _: &mut rune::Tx) -> bool {
    event.key == rune::Key::Escape && state.close_overlay()
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
    col().key("shell").w_full().h_full().fill(p.t.bg).child(
        row()
            .key("body")
            .w_full()
            .grow(1.0)
            .child(rail::rail(state, p))
            .child(
                col()
                    .key("pane")
                    .grow(1.0)
                    .h_full()
                    .min_h(0.0)
                    .child(placeholder::top_bar(state, p))
                    .child(body),
            ),
    )
}
