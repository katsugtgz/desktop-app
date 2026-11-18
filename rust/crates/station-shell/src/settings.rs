//! Settings: a Rune screen filling the content area.
//!
//! It cannot be a modal over an app's page — the page is a native child window Rune cannot draw
//! over — so opening Settings hides the hosted page (`ShellState::open_settings` changes the
//! surface, which `webview::sync` follows) and shows this instead. Escape or the back button
//! returns to the app, which brings the page back without re-creating it.

use crate::shell::Paint;
use crate::state::{ShellState, TabKind};
use rune::tok::{radius, space, type_};
use rune::{col, el, row, text, El, ScrollSpec};
use session_store::ThemePreference;

/// The Settings screen's root, filling the same `content` box the store fills.
pub fn view(state: &ShellState, p: Paint) -> El {
    col()
        .key("content")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .fill(p.t.bg)
        .child(header(p))
        .child(
            col()
                .key("settings-body")
                .grow(1.0)
                .w_full()
                .min_h(0.0)
                .px(space::S6)
                .py(space::S6)
                .gap(space::S6)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(general(state, p))
                .child(my_apps(state, p)),
        )
}

fn header(p: Paint) -> El {
    row()
        .key("settings-header")
        .w_full()
        .h(44.0)
        .px(space::S6)
        .gap(space::S3)
        .center_y()
        .fill(p.t.bg)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.t.separator)
        .child(
            el().key("settings-back")
                .role("button")
                .label("Back")
                .focusable(true)
                .h(26.0)
                .px(space::S2)
                .center()
                .radius(radius::SM)
                .fill(p.t.control_track)
                .hover(p.t.hover)
                .press(p.t.press)
                .on_press(|state: &mut ShellState| {
                    state.close_overlay();
                })
                .child(
                    text("Back")
                        .font(type_::LABEL)
                        .color(p.t.text)
                        .a11y_hidden(),
                ),
        )
        .child(
            text("Settings")
                .key("settings-title")
                .font(type_::TITLE)
                .color(p.t.text),
        )
}

fn general(state: &ShellState, p: Paint) -> El {
    section(
        "settings-general",
        "General",
        p,
        vec![
            row()
                .key("settings-appearance")
                .h(36.0)
                .px(space::S3)
                .center_y()
                .justify_between()
                .child(
                    text("Appearance")
                        .font(type_::BODY)
                        .color(p.t.text)
                        .a11y_hidden(),
                )
                .child(appearance(state, p)),
            info_row(
                "settings-version-row",
                "Version",
                env!("CARGO_PKG_VERSION"),
                "settings-version",
                p,
            ),
            info_row(
                "settings-session-row",
                "Session",
                &session_path_label(state),
                "settings-session-path",
                p,
            ),
        ],
    )
}

fn appearance(state: &ShellState, p: Paint) -> El {
    let mut segments = row().key("settings-appearance-segments").gap(space::S1);
    for preference in [
        ThemePreference::System,
        ThemePreference::Dark,
        ThemePreference::Light,
    ] {
        let selected = state.theme() == preference;
        let label = match preference {
            ThemePreference::System => "System",
            ThemePreference::Dark => "Dark",
            ThemePreference::Light => "Light",
        };
        segments = segments.child(
            el().key(format!("settings-theme-{}", label.to_lowercase()))
                .role("radio")
                .label(label)
                .checked(selected)
                .focusable(true)
                .h(24.0)
                .px(space::S2)
                .center()
                .radius(radius::SM)
                .fill(if selected {
                    p.t.control_track
                } else {
                    p.t.card
                })
                .hover(p.t.hover)
                .press(p.t.press)
                .on_press(move |state: &mut ShellState| state.set_theme(preference))
                .child(
                    text(label)
                        .font(type_::LABEL)
                        .color(if selected {
                            p.t.text
                        } else {
                            p.t.text_secondary
                        })
                        .a11y_hidden(),
                ),
        );
    }
    segments
}

fn info_row(key: &str, label: &str, value: &str, value_key: &str, p: Paint) -> El {
    row()
        .key(key)
        .h(36.0)
        .px(space::S3)
        .gap(space::S3)
        .center_y()
        .justify_between()
        .child(text(label).font(type_::BODY).color(p.t.text).a11y_hidden())
        .child(
            text(value)
                .key(value_key)
                .font(type_::MONO_CAPTION)
                .color(p.t.text_secondary),
        )
}

/// "Never" when this shell is not persisting (a test shell), so the row is honest about it.
fn session_path_label(state: &ShellState) -> String {
    state.session_path().map_or_else(
        || "Not saving".to_owned(),
        |path| path.display().to_string(),
    )
}

fn my_apps(state: &ShellState, p: Paint) -> El {
    let mut rows: Vec<El> = Vec::new();
    for tab in state.tabs().iter().filter(|tab| tab.kind == TabKind::Web) {
        let id = tab.id.clone();
        let open_id = id.clone();
        let uninstall_id = id.clone();
        rows.push(
            row()
                .key(format!("settings-app-{id}"))
                .h(40.0)
                .px(space::S3)
                .gap(space::S3)
                .center_y()
                .justify_between()
                .child(
                    text(tab.label.clone())
                        .key(format!("settings-app-name-{id}"))
                        .font(type_::BODY)
                        .color(p.t.text),
                )
                .child(
                    row()
                        .gap(space::S1)
                        .child(
                            el().key(format!("settings-open-{id}"))
                                .role("button")
                                .label(format!("Open {}", tab.label))
                                .focusable(true)
                                .h(24.0)
                                .px(space::S2)
                                .center()
                                .radius(radius::SM)
                                .fill(p.t.control_track)
                                .hover(p.t.hover)
                                .press(p.t.press)
                                .on_press(move |state: &mut ShellState| {
                                    state.select(&open_id);
                                    state.close_overlay();
                                })
                                .child(
                                    text("Open")
                                        .font(type_::LABEL)
                                        .color(p.t.text)
                                        .a11y_hidden(),
                                ),
                        )
                        .child(
                            el().key(format!("settings-uninstall-{id}"))
                                .role("button")
                                .label(format!("Uninstall {}", tab.label))
                                .focusable(true)
                                .h(24.0)
                                .px(space::S2)
                                .center()
                                .radius(radius::SM)
                                .fill(p.t.control_track)
                                .hover(p.t.hover)
                                .press(p.t.press)
                                .on_press(move |state: &mut ShellState| {
                                    // Stay in Settings: removing several apps is one visit.
                                    if state.uninstall(&uninstall_id) {
                                        crate::webview::close_tab(&uninstall_id);
                                    }
                                })
                                .child(
                                    text("Uninstall")
                                        .font(type_::LABEL)
                                        .color(p.t.text_secondary)
                                        .a11y_hidden(),
                                ),
                        ),
                ),
        );
    }
    if rows.is_empty() {
        rows.push(
            text("No apps installed yet.")
                .key("settings-apps-empty")
                .font(type_::BODY)
                .color(p.t.text_secondary)
                .px(space::S3)
                .py(space::S2),
        );
    }
    section("settings-my-apps", "My apps", p, rows)
}

/// A titled card: a section header and rows separated by hairlines.
fn section(key: &str, title: &str, p: Paint, rows: Vec<El>) -> El {
    let mut card = col()
        .key(key)
        .w_full()
        .padding(space::S1)
        .fill(p.t.card)
        .radius(radius::LG)
        .border(1.0, p.t.card_border);
    let count = rows.len();
    for (index, row) in rows.into_iter().enumerate() {
        card = card.child(row);
        if index + 1 < count {
            card = card.child(
                el().key(format!("{key}-sep-{index}"))
                    .h(1.0)
                    .ml(space::S3)
                    .mr(space::S3)
                    .fill(p.t.separator)
                    .a11y_hidden(),
            );
        }
    }
    col()
        .key(format!("{key}-section"))
        .w_full()
        .gap(space::S2)
        .child(
            text(title)
                .key(format!("{key}-title"))
                .font(type_::SECTION)
                .color(p.t.text_secondary)
                .ml(space::S3),
        )
        .child(card)
}
