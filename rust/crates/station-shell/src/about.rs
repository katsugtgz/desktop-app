//! About Station screen: a Rune surface backed by the `about-window` crate.
//!
//! Replaces Electron's separate `about-window` BrowserWindow (`packages/app/src/about-window/`)
//! with an in-window Rune view that follows the shell's theme tokens and Escape-to-close rule
//! (`about_window::CloseReason::EscapePressed`).

use about_window::{copyright_year, AboutWindowState};
use rune::tok::{self, radius, space, type_};
use rune::{col, el, row, text, El, ScrollSpec};

use crate::shell::Paint;
use crate::state::ShellState;

/// Canonical GitHub Releases URL opened by the v1 "Check for updates" button.
pub const RELEASES_URL: &str = "https://github.com/katsugtgz/desktop-app/releases";

/// Current copyright year passed to `about_window::copyright_year`.
pub const COPYRIGHT_YEAR: i32 = 2026;

/// Builds the [`AboutWindowState`] snapshot rendered by [`view`].
pub fn window_state(state: &ShellState) -> AboutWindowState {
    let mut info = state.about_state().clone();
    info.set_app_metadata("Station", env!("CARGO_PKG_VERSION"));
    if crate::autolaunch::supported() {
        info.set_auto_launch_enabled(state.start_at_login());
    }
    info
}

/// Native runtime metadata rows (replaces Electron's `process.versions` block).
pub fn runtime_rows() -> Vec<(&'static str, String)> {
    let web_runtime = if cfg!(target_os = "windows") {
        "WebView2 (wry 0.57)"
    } else {
        "WebView2 (Windows host only)"
    };
    vec![
        ("Shell", format!("Station {}", env!("CARGO_PKG_VERSION"))),
        ("UI", "Rune (native GPU)".to_owned()),
        ("Web views", web_runtime.to_owned()),
        (
            "Platform",
            format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH),
        ),
    ]
}

/// Renders the About Station screen in the content pane.
pub fn view(state: &ShellState, p: Paint) -> El {
    let info = window_state(state);
    let footer_copy = format!(
        "Copyright {} Station contributors · Apache-2.0",
        copyright_year(COPYRIGHT_YEAR)
    );

    col()
        .key("content")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .fill(p.t.bg)
        .child(header(p))
        .child(
            col()
                .key("about-body")
                .grow(1.0)
                .w_full()
                .min_h(0.0)
                .px(space::S6)
                .py(space::S6)
                .gap(space::S6)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(hero_card(&info, &footer_copy, p))
                .child(updates_card(&info, p))
                .child(runtime_card(&info, p)),
        )
}

fn header(p: Paint) -> El {
    row()
        .key("about-header")
        .w_full()
        .h(44.0)
        .px(space::S6)
        .gap(space::S3)
        .center_y()
        .fill(p.t.bg)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.t.separator)
        .child(
            el().key("about-back")
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
            text("About Station")
                .key("about-title")
                .font(type_::TITLE)
                .color(p.t.text),
        )
}

fn hero_card(info: &AboutWindowState, footer_copy: &str, p: Paint) -> El {
    let badge = el()
        .key("about-badge")
        .size(44.0, 44.0)
        .center()
        .radius(radius::MD)
        .fill(tok::accent(0, p.dark))
        .child(
            text("S")
                .font(type_::TITLE)
                .color(rune::color::hex("#FFFFFF").unwrap_or(p.t.text))
                .a11y_hidden(),
        );

    col()
        .key("about-hero")
        .w_full()
        .max_w(640.0)
        .padding(space::S3)
        .gap(space::S3)
        .radius(radius::MD)
        .fill(p.t.card)
        .border(1.0, p.t.card_border)
        .child(
            row().gap(space::S3).center_y().child(badge).child(
                col()
                    .gap(2.0)
                    .child(
                        text(info.app_name.clone())
                            .key("about-app-name")
                            .font(type_::TITLE)
                            .color(p.t.text),
                    )
                    .child(
                        text(format!("Version {} — Native Rust Shell", info.app_version))
                            .key("about-app-version")
                            .font(type_::BODY)
                            .color(p.t.text_secondary),
                    ),
            ),
        )
        .child(
            text(footer_copy.to_owned())
                .key("about-copyright")
                .font(type_::LABEL)
                .color(p.t.text_secondary),
        )
}

fn updates_card(info: &AboutWindowState, p: Paint) -> El {
    let status = if let Some(release) = &info.release_name {
        format!("Update available: {release}")
    } else if info.checking_update {
        "Opened GitHub Releases in your browser.".to_owned()
    } else {
        "Manual updates: open GitHub Releases to check for newer builds.".to_owned()
    };

    section(
        "about-updates",
        "Updates",
        p,
        vec![row()
            .key("about-updates-row")
            .px(space::S3)
            .py(space::S2)
            .gap(space::S3)
            .center_y()
            .justify_between()
            .child(
                text(status)
                    .key("about-updates-status")
                    .font(type_::BODY)
                    .color(p.t.text_secondary)
                    .truncate(),
            )
            .child(
                el().key("about-check-updates")
                    .role("button")
                    .label("Check for updates")
                    .focusable(true)
                    .h(28.0)
                    .px(space::S3)
                    .center()
                    .radius(radius::SM)
                    .fill(p.t.control_track)
                    .hover(p.t.hover)
                    .press(p.t.press)
                    .on_press(|state: &mut ShellState| {
                        state.check_for_updates();
                    })
                    .child(
                        text("Check for updates")
                            .font(type_::LABEL)
                            .color(p.t.text)
                            .a11y_hidden(),
                    ),
            )],
    )
}

fn runtime_card(info: &AboutWindowState, p: Paint) -> El {
    let mut rows: Vec<El> = runtime_rows()
        .into_iter()
        .enumerate()
        .map(|(i, (label, value))| {
            info_row(
                format!("about-runtime-row-{i}"),
                label,
                &value,
                format!("about-runtime-val-{i}"),
                p,
            )
        })
        .collect();

    let login_status = match info.auto_launch_enabled {
        Some(true) => "Enabled",
        Some(false) => "Disabled",
        None => "Windows only",
    };
    rows.push(info_row(
        "about-autolaunch-row".to_owned(),
        "Start at login",
        login_status,
        "about-autolaunch-val".to_owned(),
        p,
    ));

    section("about-runtime", "Runtime", p, rows)
}

fn section(key: &'static str, title: &'static str, p: Paint, rows: Vec<El>) -> El {
    let mut card = col()
        .key(key)
        .w_full()
        .max_w(640.0)
        .radius(radius::MD)
        .fill(p.t.card)
        .border(1.0, p.t.card_border);
    let total = rows.len();
    for (index, item) in rows.into_iter().enumerate() {
        let item = if index + 1 < total {
            item.border_sides([0.0, 0.0, 1.0, 0.0], p.t.separator)
        } else {
            item
        };
        card = card.child(item);
    }
    col()
        .w_full()
        .gap(space::S2)
        .child(
            text(title)
                .font(type_::SECTION)
                .color(p.t.text_secondary)
                .a11y_hidden(),
        )
        .child(card)
}

fn info_row(row_key: String, label: &str, value: &str, value_key: String, p: Paint) -> El {
    row()
        .key(row_key)
        .h(36.0)
        .px(space::S3)
        .center_y()
        .justify_between()
        .child(
            text(label.to_owned())
                .font(type_::BODY)
                .color(p.t.text)
                .a11y_hidden(),
        )
        .child(
            text(value.to_owned())
                .key(value_key)
                .font(type_::MONO_CAPTION)
                .color(p.t.text_secondary)
                .truncate(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell;
    use crate::state::Surface;
    use rune::{Env, Key, KeyEvent, Mods, Runtime};

    fn runtime() -> Runtime<ShellState> {
        let state = ShellState::new(appstore_service::ApplicationService::for_tests("about-ui"));
        Runtime::new(
            state,
            shell::view,
            (1200.0, 800.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        )
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

    #[test]
    fn settings_opens_about_and_escape_returns_to_apps() {
        let mut rt = runtime();
        rt.frame(1.0 / 60.0);

        press(&mut rt, "rail-settings");
        assert_eq!(rt.state.surface(), Surface::Settings);

        press(&mut rt, "settings-open-about");
        assert_eq!(rt.state.surface(), Surface::About);
        assert!(!rt.state.shows_page());
        assert!(rt.find("about-hero").is_some());
        assert!(rt.find("about-updates").is_some());
        assert!(rt.find("about-runtime").is_some());

        // Check copyright and update check interaction
        let copy = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("about-copyright"))
            .and_then(|e| e.text)
            .expect("copyright text");
        assert!(copy.contains("2019 - 2026"));

        assert!(!rt.state.about_state().checking_update);
        press(&mut rt, "about-check-updates");
        assert!(rt.state.about_state().checking_update);

        // Escape closes About back to Apps (matching about_window::CloseReason::EscapePressed)
        let mut tx = rune::Tx::default();
        let handled = shell::on_key(
            &mut rt.state,
            &KeyEvent {
                key: Key::Escape,
                mods: Mods::default(),
            },
            &mut tx,
        );
        assert!(handled);
        assert_eq!(rt.state.surface(), Surface::Apps);
    }
}
