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

use crate::state::TabKind;

pub fn top_bar(state: &ShellState, p: Paint) -> El {
    let tab = state.active();
    let mut right = row().key("topbar-right").gap(space::S2).center_y();
    right = right.child(
        text(tab.url.clone())
            .key("topbar-url")
            .font(type_::MONO_CAPTION)
            .color(p.t.text_secondary)
            .truncate(),
    );
    if tab.kind == TabKind::Web {
        let back_id = tab.id.clone();
        let fwd_id = tab.id.clone();
        let reload_id = tab.id.clone();
        let ext_url = tab.url.clone();
        let menu_id = tab.id.clone();
        right = right
            .child(bar_btn("topbar-back", "Back", "Back", p, move |_| {
                crate::webview::go_back(&back_id);
            }))
            .child(bar_btn("topbar-forward", "Forward", "Fwd", p, move |_| {
                crate::webview::go_forward(&fwd_id);
            }))
            .child(bar_btn("topbar-reload", "Reload", "Reload", p, move |_| {
                crate::webview::reload(&reload_id);
            }))
            .child(bar_btn(
                "topbar-open-browser",
                "Open in browser",
                "Browser",
                p,
                move |_| {
                    crate::routing::open_external(&ext_url);
                },
            ))
            .child(bar_btn(
                "topbar-menu",
                "Tab actions",
                "...",
                p,
                move |state| {
                    state.toggle_tab_menu(&menu_id);
                },
            ));
    }

    row()
        .key("topbar")
        .w_full()
        .h(TOP_BAR_HEIGHT)
        .px(space::S6)
        .gap(space::S3)
        .center_y()
        .justify_between()
        .fill(p.t.bg)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.t.separator)
        .child(
            text(tab.display_label().to_owned())
                .key("topbar-title")
                .font(type_::TITLE)
                .color(p.t.text)
                .truncate(),
        )
        .child(right)
}

/// Height of the Rune tab-actions strip (sits below the top bar so WebView2 never covers it).
pub const TAB_ACTIONS_HEIGHT: f32 = 40.0;

/// Rune tab context-action strip opened by right-clicking a rail app or pressing `...`.
pub fn tab_actions_bar(state: &ShellState, p: Paint) -> Option<El> {
    let target_id = state.tab_menu_for()?;
    let tab = state
        .tabs()
        .iter()
        .find(|t| t.id == target_id && t.kind == TabKind::Web)?;

    let id_reload = tab.id.clone();
    let ext_url = tab.url.clone();
    let id_cfg = tab.id.clone();
    let id_up = tab.id.clone();
    let id_down = tab.id.clone();
    let id_uninstall = tab.id.clone();

    let mut actions = row()
        .key("tab-actions-bar")
        .w_full()
        .h(TAB_ACTIONS_HEIGHT)
        .px(space::S6)
        .gap(space::S2)
        .center_y()
        .fill(p.t.card)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.t.separator)
        .child(
            text(format!("{}:", tab.display_label()))
                .key("tab-actions-title")
                .font(type_::LABEL)
                .color(p.t.text),
        )
        .child(bar_btn(
            "tab-action-reload",
            "Reload tab",
            "Reload",
            p,
            move |state| {
                crate::webview::reload(&id_reload);
                state.close_tab_menu();
            },
        ))
        .child(bar_btn(
            "tab-action-external",
            "Open in browser",
            "Open in browser",
            p,
            move |state| {
                crate::routing::open_external(&ext_url);
                state.close_tab_menu();
            },
        ));

    if tab.configurable {
        actions = actions.child(bar_btn(
            "tab-action-configure",
            "Configure app",
            "Configure",
            p,
            move |state| {
                state.close_tab_menu();
                state.open_settings();
                state.open_config_editor(&id_cfg);
            },
        ));
    }
    if state.can_move_rail_row(&tab.id, true) {
        actions = actions.child(bar_btn(
            "tab-action-move-up",
            "Move up in rail",
            "Move up",
            p,
            move |state| {
                state.move_rail_row(&id_up, true);
            },
        ));
    }
    if state.can_move_rail_row(&tab.id, false) {
        actions = actions.child(bar_btn(
            "tab-action-move-down",
            "Move down in rail",
            "Move down",
            p,
            move |state| {
                state.move_rail_row(&id_down, false);
            },
        ));
    }
    actions = actions
        .child(bar_btn(
            "tab-action-uninstall",
            "Uninstall app",
            "Uninstall",
            p,
            move |state| {
                state.close_tab_menu();
                if state.uninstall(&id_uninstall) {
                    crate::webview::close_tab(&id_uninstall);
                }
            },
        ))
        .child(bar_btn(
            "tab-action-close",
            "Close tab actions",
            "Close",
            p,
            |state| {
                state.close_tab_menu();
            },
        ));

    Some(actions)
}

fn bar_btn(
    key: &'static str,
    label: &'static str,
    caption: &'static str,
    p: Paint,
    on_press: impl Fn(&mut ShellState) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .h(24.0)
        .px(space::S2)
        .center()
        .radius(radius::SM)
        .fill(p.t.control_track)
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press(on_press)
        .child(
            text(caption)
                .font(type_::LABEL)
                .color(p.t.text)
                .a11y_hidden(),
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
