//! The pane beside the rail: a top bar naming the app that is active, and the placeholder an
//! app's web view is placed over.
//!
//! Rune draws the bar; a web tab's page is an OS child window covering the content box, so this
//! placeholder is only what shows before the page paints (and the honest empty state for an app
//! whose install has no start URL).

use crate::shell::Paint;
use crate::state::ShellState;
use rune::tok::{radius, space, type_};
use rune::{col, el, row, text, El};

/// Height of the bar, in points: enough for a 13 pt title with 12 pt tracking room, as Station's
/// own tab bar is.
pub const TOP_BAR_HEIGHT: f32 = 44.0;

pub fn top_bar(state: &ShellState, p: Paint) -> El {
    let tab = state.active();
    row()
        .key("topbar")
        .w_full()
        .h(TOP_BAR_HEIGHT)
        .px(space::S6)
        .center_y()
        .justify_between()
        .fill(p.t.bg)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.t.separator)
        .child(
            text(tab.display_label().to_owned())
                .key("topbar-title")
                .font(type_::TITLE)
                .color(p.t.text),
        )
        .child(
            text(tab.url.clone())
                .key("topbar-url")
                .font(type_::MONO_CAPTION)
                .color(p.t.text_secondary),
        )
}

pub fn content(state: &ShellState, p: Paint) -> El {
    if let Some(error) = state.page_error() {
        return page_error(state, error, p);
    }
    let tab = state.active();
    col()
        .key("content")
        .grow(1.0)
        .w_full()
        .center()
        .fill(p.t.bg)
        .child(
            col()
                .key("placeholder")
                .w(420.0)
                .padding(space::S6)
                .gap(space::S2)
                .center()
                .radius(radius::LG)
                .fill(p.t.card)
                .border(1.0, p.t.card_border)
                .shadow(0.0, 16.0, p.t.shadow_soft)
                .child(
                    text(tab.label.clone())
                        .key("placeholder-label")
                        .font(type_::TITLE)
                        .color(p.t.text),
                )
                .child(
                    text(format!("{} loads here", tab.url))
                        .key("placeholder-url")
                        .font(type_::BODY)
                        .color(p.t.text_secondary),
                ),
        )
}

/// The pane's honest state when the page could not be attached: which app, what the host said,
/// and a Retry control (the shell retries by itself with backoff, then leaves it to the user).
fn page_error(state: &ShellState, error: &crate::state::PageError, p: Paint) -> El {
    let tab = state.active().id.clone();
    col()
        .key("content")
        .grow(1.0)
        .w_full()
        .center()
        .fill(p.t.bg)
        .child(
            col()
                .key("page-error")
                .w(460.0)
                .padding(space::S6)
                .gap(space::S2)
                .center()
                .radius(radius::LG)
                .fill(p.t.card)
                .border(1.0, p.t.card_border)
                .shadow(0.0, 16.0, p.t.shadow_soft)
                .child(
                    text(format!("Could not load {}", error.application))
                        .key("page-error-title")
                        .font(type_::TITLE)
                        .color(p.t.text),
                )
                .child(
                    text(error.detail.clone())
                        .key("page-error-detail")
                        .font(type_::LABEL)
                        .color(p.t.text_secondary)
                        .truncate(),
                )
                .child(
                    el().key("page-error-retry")
                        .role("button")
                        .label("Retry")
                        .focusable(true)
                        .h(26.0)
                        .px(space::S3)
                        .center()
                        .mt(space::S2)
                        .radius(radius::SM)
                        .fill(p.t.control_track)
                        .hover(p.t.hover)
                        .press(p.t.press)
                        .on_press(move |state: &mut crate::state::ShellState| {
                            // Clear the backoff and the notice; the next frame attaches again.
                            crate::webview::retry(&tab);
                            state.clear_page_error();
                        })
                        .child(
                            text("Retry")
                                .font(type_::LABEL)
                                .color(p.t.text)
                                .a11y_hidden(),
                        ),
                ),
        )
}
