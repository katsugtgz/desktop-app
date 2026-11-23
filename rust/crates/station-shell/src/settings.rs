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
use session_store::{AppConfigEntry, Identity, ThemePreference};

/// The Settings screen's root, filling the same `content` box the store fills.
pub fn view(state: &ShellState, p: Paint) -> El {
    // A draft replaces the My apps card: one editor at a time, in place.
    let apps_card = if state.config_draft().is_some() {
        config_editor(state, p)
    } else {
        my_apps(state, p)
    };
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
                .child(accounts(state, p))
                .child(apps_card),
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
    let mut rows = vec![
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
    ];
    // Launch at login: a Windows login item (the Electron autolaunch value). The row is absent
    // where the platform cannot manage one, and a failed write says so instead of lying.
    if let Some(row) = start_at_login_row(state, p, crate::autolaunch::supported()) {
        rows.push(row);
    }
    if let Some(why) = state.start_at_login_error() {
        rows.push(info_row(
            "settings-start-at-login-error",
            "Login item",
            why,
            "settings-start-at-login-error-text",
            p,
        ));
    }
    rows.push(
        row()
            .key("settings-about-row")
            .h(36.0)
            .px(space::S3)
            .center_y()
            .justify_between()
            .child(
                text("About Station")
                    .font(type_::BODY)
                    .color(p.t.text)
                    .a11y_hidden(),
            )
            .child(
                el().key("settings-open-about")
                    .role("button")
                    .label("About Station")
                    .focusable(true)
                    .h(26.0)
                    .px(space::S2)
                    .center()
                    .radius(radius::SM)
                    .fill(p.t.control_track)
                    .hover(p.t.hover)
                    .press(p.t.press)
                    .on_press(|state: &mut ShellState| state.open_about())
                    .child(
                        text("Open")
                            .font(type_::LABEL)
                            .color(p.t.text)
                            .a11y_hidden(),
                    ),
            ),
    );
    section("settings-general", "General", p, rows)
}

/// The login row's testable content: present only where the platform can manage a login item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StartAtLoginRow {
    pub key: &'static str,
    pub label: &'static str,
    pub on: bool,
}

/// `Some` where the platform supports a login item, `None` otherwise (no stub row).
pub(crate) fn start_at_login_model(state: &ShellState, supported: bool) -> Option<StartAtLoginRow> {
    supported.then_some(StartAtLoginRow {
        key: "settings-start-at-login",
        label: "Start Station when I sign in",
        on: state.start_at_login(),
    })
}

/// One row: label left, On/Off right, the whole row a switch.
fn start_at_login_row(state: &ShellState, p: Paint, supported: bool) -> Option<El> {
    let model = start_at_login_model(state, supported)?;
    Some(
        row()
            .key(model.key)
            .role("switch")
            .label(model.label)
            .checked(model.on)
            .h(36.0)
            .px(space::S3)
            .center_y()
            .justify_between()
            .hover(p.t.hover)
            .press(p.t.press)
            .on_press(|state: &mut ShellState| state.toggle_start_at_login())
            .child(
                text(model.label)
                    .font(type_::BODY)
                    .color(p.t.text)
                    .a11y_hidden(),
            )
            .child(
                text(if model.on { "On" } else { "Off" })
                    .key("settings-start-at-login-value")
                    .font(type_::MONO_CAPTION)
                    .color(p.t.text_secondary),
            ),
    )
}

/// The stored accounts (slices 17 / 25): display name, email, and Remove button per identity,
/// plus native Google OAuth sign-in (Option B: system browser + PKCE + loopback) when
/// `GOOGLE_CLIENT_ID` is configured, or an honest unconfigured note otherwise.
fn accounts(state: &ShellState, p: Paint) -> El {
    let mut rows: Vec<El> = Vec::new();
    for identity in state.identities().values() {
        let id = identity.identity_id.clone();
        let remove_id = id.clone();
        rows.push(
            row()
                .key(format!("settings-account-{id}"))
                .h(36.0)
                .px(space::S3)
                .gap(space::S3)
                .center_y()
                .justify_between()
                .child(
                    row()
                        .grow(1.0)
                        .gap(space::S3)
                        .center_y()
                        .child(
                            text(identity_name(identity))
                                .key(format!("settings-account-name-{id}"))
                                .font(type_::BODY)
                                .color(p.t.text)
                                .truncate(),
                        )
                        .child(
                            text(identity.profile_data.email.clone())
                                .key(format!("settings-account-email-{id}"))
                                .font(type_::MONO_CAPTION)
                                .color(p.t.text_secondary)
                                .truncate(),
                        ),
                )
                .child(
                    el().key(format!("settings-account-remove-{id}"))
                        .role("button")
                        .label(format!("Remove {}", identity_name(identity)))
                        .focusable(true)
                        .h(24.0)
                        .px(space::S2)
                        .center()
                        .radius(radius::SM)
                        .fill(p.t.control_track)
                        .hover(p.t.hover)
                        .press(p.t.press)
                        .on_press(move |state: &mut ShellState| {
                            state.remove_identity(&remove_id);
                        })
                        .child(
                            text("Remove")
                                .font(type_::LABEL)
                                .color(p.t.text)
                                .a11y_hidden(),
                        ),
                ),
        );
    }
    if rows.is_empty() {
        rows.push(
            text("No accounts yet.")
                .key("settings-accounts-empty")
                .font(type_::BODY)
                .color(p.t.text_secondary)
                .px(space::S3)
                .py(space::S2),
        );
    }
    if state.oauth_in_progress() {
        rows.push(
            row()
                .key("settings-oauth-pending")
                .h(36.0)
                .px(space::S3)
                .gap(space::S3)
                .center_y()
                .justify_between()
                .child(
                    text("Waiting for Google sign-in in your browser...")
                        .key("settings-oauth-status")
                        .font(type_::LABEL)
                        .color(p.t.text_secondary)
                        .truncate(),
                )
                .child(
                    el().key("settings-oauth-cancel")
                        .role("button")
                        .label("Cancel sign-in")
                        .focusable(true)
                        .h(26.0)
                        .px(space::S2)
                        .center()
                        .radius(radius::SM)
                        .fill(p.t.control_track)
                        .hover(p.t.hover)
                        .press(p.t.press)
                        .on_press(|state: &mut ShellState| state.cancel_google_sign_in())
                        .child(
                            text("Cancel")
                                .font(type_::LABEL)
                                .color(p.t.text)
                                .a11y_hidden(),
                        ),
                ),
        );
    } else if state.oauth_configured() {
        rows.push(
            row()
                .key("settings-accounts-signin")
                .h(40.0)
                .px(space::S3)
                .gap(space::S3)
                .center_y()
                .justify_between()
                .child(
                    text(
                        state
                            .oauth_status()
                            .unwrap_or("Sign in with Google via your system browser."),
                    )
                    .key("settings-oauth-status")
                    .font(type_::LABEL)
                    .color(p.t.text_secondary)
                    .truncate(),
                )
                .child(
                    el().key("settings-google-signin")
                        .role("button")
                        .label("Sign in with Google")
                        .focusable(true)
                        .h(28.0)
                        .px(space::S3)
                        .center()
                        .radius(radius::SM)
                        .fill(p.t.control_track)
                        .hover(p.t.hover)
                        .press(p.t.press)
                        .on_press(|state: &mut ShellState| state.start_google_sign_in())
                        .child(
                            text("Sign in with Google")
                                .font(type_::LABEL)
                                .color(p.t.text)
                                .a11y_hidden(),
                        ),
                ),
        );
    } else if state.identities().is_empty() {
        rows.push(
            text("Sign-in requires GOOGLE_CLIENT_ID in the environment.")
                .key("settings-accounts-signin")
                .font(type_::LABEL)
                .color(p.t.text_secondary)
                .px(space::S3)
                .py(space::S2),
        );
    }
    section("settings-accounts", "Accounts", p, rows)
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
    let movable = state
        .tabs()
        .iter()
        .filter(|tab| tab.kind == TabKind::Web)
        .count();
    let mut rows: Vec<El> = Vec::new();
    for (index, tab) in state
        .tabs()
        .iter()
        .filter(|tab| tab.kind == TabKind::Web)
        .enumerate()
    {
        let id = tab.id.clone();
        let open_id = id.clone();
        let uninstall_id = id.clone();
        let configure_id = id.clone();
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
                        .child(move_control(&id, &tab.label, true, index > 0, p))
                        .child(move_control(&id, &tab.label, false, index + 1 < movable, p))
                        .child(configure_row(&configure_id, tab, state.config_of(&id), p))
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

/// One row's reordering control (`settings-move-up-<id>` / `settings-move-down-<id>`, D11):
/// `shown` is false exactly where the move cannot happen — the first movable app has no up,
/// the last no down, and the store row (not a movable app) never gets one. A press moves the
/// row one step and writes the session at once (`ShellState::move_rail_row`, D7's persist path).
fn move_control(id: &str, app: &str, up: bool, shown: bool, p: Paint) -> El {
    if !shown {
        return rune::spacer();
    }
    let direction = if up { "up" } else { "down" };
    el().key(format!("settings-move-{direction}-{id}"))
        .role("button")
        .label(format!("Move {app} {direction}"))
        .focusable(true)
        .size(24.0, 24.0)
        .center()
        .radius(radius::SM)
        .fill(p.t.control_track)
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press({
            let id = id.to_owned();
            move |state: &mut ShellState| {
                state.move_rail_row(&id, up);
            }
        })
        .child(
            text(if up { "↑" } else { "↓" })
                .font(type_::LABEL)
                .color(p.t.text_secondary)
                .a11y_hidden(),
        )
}

/// An app row's first control: "Configure"/"Configured" when the manifest has presets, nothing
/// when it has none (a button that could never do anything is not shown).
fn configure_row(id: &str, tab: &crate::state::AppTab, config: AppConfigEntry, p: Paint) -> El {
    if !tab.configurable {
        return rune::spacer();
    }
    let configured = !config.is_empty();
    el().key(format!("settings-configure-{id}"))
        .role("button")
        .label(format!("Configure {}", tab.label))
        .checked(configured)
        .focusable(true)
        .h(24.0)
        .px(space::S2)
        .center()
        .radius(radius::SM)
        .fill(if configured {
            p.t.control_track
        } else {
            p.t.card
        })
        .border(if configured { 1.0 } else { 0.0 }, p.t.control_off)
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press({
            let id = id.to_owned();
            move |state: &mut ShellState| {
                state.open_config_editor(&id);
            }
        })
        .child(
            text(if configured {
                "Configured"
            } else {
                "Configure"
            })
            .key(format!("settings-configure-label-{id}"))
            .font(type_::LABEL)
            .color(p.t.text)
            .a11y_hidden(),
        )
}

/// The per-app configuration editor, replacing the My apps card while it shows: the one field
/// the app's manifest presets require, Save and Clear. A google-account app shows the honest
/// state — the shell cannot sign in yet — instead of a field that would collect a value nothing
/// can use.
pub fn config_editor(state: &ShellState, p: Paint) -> El {
    let Some(draft) = state.config_draft() else {
        return my_apps(state, p);
    };
    let application_id = draft.application_id.clone();
    let field = draft.field;

    // The manifest decides what is configurable; it may be gone mid-edit (a private manifest
    // deleted underneath the session) — then there is no editor, only the honest note.
    let installed = state
        .tabs()
        .iter()
        .find(|tab| tab.id == application_id)
        .map(|tab| tab.label.clone());

    let (field_key, field_label) = (format!("config-field-{application_id}"), field.label());
    let mut card_rows: Vec<El> = Vec::new();
    card_rows.push(
        row()
            .key("config-editor-title-row")
            .h(36.0)
            .px(space::S3)
            .gap(space::S3)
            .center_y()
            .child(
                text(match field {
                    crate::state::ConfigField::Identity
                        if state.identities().is_empty() && state.oauth_configured() =>
                    {
                        "Sign in with Google to choose an account."
                    }
                    crate::state::ConfigField::Identity if state.identities().is_empty() => {
                        "Sign-in requires GOOGLE_CLIENT_ID in the environment."
                    }
                    crate::state::ConfigField::Identity => "Choose an account",
                    _ => field_label,
                })
                .key("config-editor-title")
                .font(type_::BODY)
                .color(p.t.text),
            ),
    );
    if field != crate::state::ConfigField::Identity {
        card_rows.push(
            el().key(field_key)
                .w_full()
                .h(30.0)
                .px(space::S2)
                .center_y()
                .radius(radius::MD)
                .fill(p.t.card)
                .border(1.0, p.t.control_off)
                .child(
                    rune::text_input(&draft.edit, type_::BODY)
                        .key(format!("config-edit-{application_id}"))
                        .w_full()
                        .single_line()
                        .label(field_label)
                        .placeholder(field_label, p.t.text_secondary)
                        .color(p.t.text)
                        .on_edit(rune::edit_access(|state: &mut ShellState| {
                            state
                                .config_draft_mut()
                                .map(|draft| &mut draft.edit)
                                .expect("the draft is open while its editor shows")
                        })),
                ),
        );
    }
    if field == crate::state::ConfigField::Identity {
        if state.identities().is_empty() {
            if state.oauth_configured() {
                card_rows.push(
                    row()
                        .key("config-identity-empty-hint")
                        .px(space::S3)
                        .py(space::S2)
                        .gap(space::S3)
                        .center_y()
                        .child(
                            el().key("config-google-signin")
                                .role("button")
                                .label("Sign in with Google")
                                .focusable(true)
                                .h(28.0)
                                .px(space::S3)
                                .center()
                                .radius(radius::SM)
                                .fill(p.t.control_track)
                                .hover(p.t.hover)
                                .press(p.t.press)
                                .on_press(|state: &mut ShellState| state.start_google_sign_in())
                                .child(
                                    text("Sign in with Google")
                                        .font(type_::LABEL)
                                        .color(p.t.text)
                                        .a11y_hidden(),
                                ),
                        ),
                );
            } else {
                card_rows.push(
                    text("Set GOOGLE_CLIENT_ID and sign in from Settings -> Accounts.")
                        .key("config-identity-empty-hint")
                        .font(type_::LABEL)
                        .color(p.t.text_secondary)
                        .px(space::S3),
                );
            }
        } else {
            card_rows.push(identity_chooser(state, &application_id, p));
        }
    }
    card_rows.push(
        row()
            .key("config-editor-actions")
            .h(36.0)
            .px(space::S3)
            .gap(space::S2)
            .center_y()
            .child(if field == crate::state::ConfigField::Identity {
                // Choosing an account applies at once; the draft text still holds whatever the
                // editor opened with, so a Save here would silently revert the choice.
                rune::spacer()
            } else {
                el().key(format!("config-save-{application_id}"))
                    .role("button")
                    .label("Save")
                    .focusable(true)
                    .h(24.0)
                    .px(space::S2)
                    .center()
                    .radius(radius::SM)
                    .fill(p.t.control_track)
                    .hover(p.t.hover)
                    .press(p.t.press)
                    .on_press({
                        let application_id = application_id.clone();
                        move |state: &mut ShellState| {
                            let was = state.tab_url(&application_id).map(str::to_owned);
                            state.save_config_draft();
                            // The tab's URL follows the configuration on the same frame: a page
                            // that is open navigates, one that is not attaches there next frame.
                            if let (Some(before), Some(after)) =
                                (was, state.tab_url(&application_id).map(str::to_owned))
                            {
                                if before != after {
                                    crate::webview::load_if_attached(&application_id, &after);
                                }
                            }
                        }
                    })
                    .child(
                        text("Save")
                            .font(type_::LABEL)
                            .color(p.t.text)
                            .a11y_hidden(),
                    )
            })
            .child(
                el().key(format!("config-clear-{application_id}"))
                    .role("button")
                    .label("Clear")
                    .focusable(true)
                    .h(24.0)
                    .px(space::S2)
                    .center()
                    .radius(radius::SM)
                    .fill(p.t.card)
                    .border(1.0, p.t.control_off)
                    .hover(p.t.hover)
                    .press(p.t.press)
                    .on_press({
                        let application_id = application_id.clone();
                        move |state: &mut ShellState| {
                            let was = state.tab_url(&application_id).map(str::to_owned);
                            if state.clear_config_draft() {
                                if let Some(after) =
                                    state.tab_url(&application_id).map(str::to_owned)
                                {
                                    if Some(&after) != was.as_ref() {
                                        crate::webview::load_if_attached(&application_id, &after);
                                    }
                                }
                            }
                        }
                    })
                    .child(
                        text("Clear")
                            .font(type_::LABEL)
                            .color(p.t.text_secondary)
                            .a11y_hidden(),
                    ),
            )
            .child(
                el().key("config-cancel")
                    .role("button")
                    .label("Cancel")
                    .focusable(true)
                    .h(24.0)
                    .px(space::S2)
                    .center()
                    .radius(radius::SM)
                    .fill(p.t.card)
                    .hover(p.t.hover)
                    .press(p.t.press)
                    .on_press(|state: &mut ShellState| state.close_config_editor())
                    .child(
                        text("Cancel")
                            .font(type_::LABEL)
                            .color(p.t.text_secondary)
                            .a11y_hidden(),
                    ),
            ),
    );
    let _ = installed;
    section("config-editor", "Configure app", p, card_rows)
}

/// The stored-account chooser a `google-account` app's editor shows: one radio-style row per
/// identity, the configured one checked. Choosing applies at once through
/// [`ShellState::set_app_config`] (the same path the text field's Save uses), so the tab's URL is
/// re-resolved and an open page follows if it moved — no second write path.
fn identity_chooser(state: &ShellState, application_id: &str, p: Paint) -> El {
    let current = state.config_of(application_id).identity_id;
    let mut list = col().key("config-identities").w_full().gap(space::S1);
    for identity in state.identities().values() {
        let identity_id = identity.identity_id.clone();
        let chosen = current.as_deref() == Some(identity_id.as_str());
        let label = identity_name(identity);
        let email = identity.profile_data.email.clone();
        let accessible = if email.is_empty() {
            format!("Use {label}")
        } else {
            format!("Use {label} ({email})")
        };
        let application = application_id.to_owned();
        let selected = identity_id.clone();
        list = list.child(
            el().key(format!("config-account-{identity_id}"))
                .role("radio")
                .label(accessible)
                .checked(chosen)
                .focusable(true)
                .w_full()
                .h(40.0)
                .px(space::S3)
                .center_y()
                .radius(radius::SM)
                .fill(if chosen { p.t.control_track } else { p.t.card })
                .hover(p.t.hover)
                .press(p.t.press)
                .on_press(move |state: &mut ShellState| {
                    let was = state.tab_url(&application).map(str::to_owned);
                    let entry = AppConfigEntry {
                        identity_id: Some(selected.clone()),
                        ..AppConfigEntry::default()
                    };
                    if state.set_app_config(&application, entry) {
                        if let (Some(before), Some(after)) =
                            (was, state.tab_url(&application).map(str::to_owned))
                        {
                            if before != after {
                                crate::webview::load_if_attached(&application, &after);
                            }
                        }
                    }
                })
                .child(
                    col()
                        .grow(1.0)
                        .gap(2.0)
                        .child(
                            text(label)
                                .key(format!("config-account-name-{identity_id}"))
                                .font(type_::BODY)
                                .color(p.t.text)
                                .truncate(),
                        )
                        .child(
                            text(email)
                                .key(format!("config-account-email-{identity_id}"))
                                .font(type_::LABEL)
                                .color(p.t.text_secondary)
                                .truncate(),
                        ),
                ),
        );
    }
    list
}

/// The name an account row shows: the profile's display name, else its email, else the id.
fn identity_name(identity: &Identity) -> String {
    let display = identity.profile_data.display_name.trim();
    if !display.is_empty() {
        return display.to_owned();
    }
    let email = identity.profile_data.email.trim();
    if !email.is_empty() {
        return email.to_owned();
    }
    identity.identity_id.clone()
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
