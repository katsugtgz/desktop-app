//! A first-run picker for apps from the Station catalog.

use crate::shell::Paint;
use crate::state::ShellState;
use manifest_registry::MinimalApplication;
use rune::tok::{radius, space, type_};
use rune::{col, el, row, text, El, ScrollSpec};

const MAX_SELECTED: usize = 15;
const MAX_CHOICES: usize = 10;

#[derive(Default)]
pub struct Onboarding {
    pub legacy_data_found: bool,
    pub selected: Vec<MinimalApplication>,
    pub first_installed: Option<String>,
    pub install_failed: bool,
}

impl Onboarding {
    pub fn toggle(&mut self, app: &MinimalApplication) {
        if let Some(index) = self
            .selected
            .iter()
            .position(|selected| selected.bx_app_manifest_url == app.bx_app_manifest_url)
        {
            self.selected.remove(index);
        } else if self.selected.len() < MAX_SELECTED {
            self.selected.push(app.clone());
        }
    }
}

pub fn view(state: &ShellState, onboarding: &Onboarding, p: Paint) -> El {
    let candidates = if state.query().trim().is_empty() {
        &state.popular().cream_of_the_crop_apps
    } else {
        state.results()
    };
    let mut choices = list("onboarding-candidates", p);
    let mut shown = false;
    for app in candidates
        .iter()
        .filter(|app| !app.is_chrome_extension && !state.is_installed(&app.bx_app_manifest_url))
        .take(MAX_CHOICES)
    {
        shown = true;
        let selected = onboarding
            .selected
            .iter()
            .any(|selected| selected.id == app.id);
        let label = if selected { "Selected" } else { "Select" };
        let app_to_toggle = app.clone();
        choices = choices.child(
            row()
                .key(format!("onboarding-choice-{}", app.id))
                .role("button")
                .label(app.name.clone())
                .checked(selected)
                .focusable(true)
                .h(38.0)
                .w_full()
                .px(space::S2)
                .gap(space::S2)
                .center_y()
                .radius(radius::SM)
                .fill(if selected {
                    p.t.control_track
                } else {
                    p.t.card
                })
                .hover(p.t.hover)
                .press(p.t.press)
                .on_press(move |state: &mut ShellState| state.toggle_onboarding_app(&app_to_toggle))
                .child(crate::icons::avatar(
                    &app.name,
                    &app.id,
                    &app.theme_color,
                    24.0,
                    p,
                    state.icon_pixels(&app.icon_url),
                ))
                .child(
                    text(app.name.clone())
                        .font(type_::BODY)
                        .color(p.t.text)
                        .grow(1.0)
                        .truncate()
                        .a11y_hidden(),
                )
                .child(
                    text(label)
                        .font(type_::LABEL)
                        .color(p.t.text_secondary)
                        .a11y_hidden(),
                ),
        );
    }
    if !shown {
        choices = choices.child(
            text("No matching apps")
                .font(type_::BODY)
                .color(p.t.text_secondary),
        );
    }

    let mut selection = list("onboarding-selected", p).child(
        text(format!(
            "Selected ({}/{MAX_SELECTED})",
            onboarding.selected.len()
        ))
        .font(type_::SECTION)
        .color(p.t.text),
    );
    for app in &onboarding.selected {
        let app_to_remove = app.clone();
        selection = selection.child(
            row()
                .key(format!("onboarding-remove-{}", app.id))
                .role("button")
                .label(format!("Remove {} from selection", app.name))
                .focusable(true)
                .h(30.0)
                .w_full()
                .px(space::S2)
                .gap(space::S2)
                .center_y()
                .radius(radius::SM)
                .hover(p.t.hover)
                .press(p.t.press)
                .on_press(move |state: &mut ShellState| state.toggle_onboarding_app(&app_to_remove))
                .child(
                    text(app.name.clone())
                        .font(type_::BODY)
                        .color(p.t.text)
                        .grow(1.0)
                        .truncate()
                        .a11y_hidden(),
                )
                .child(
                    text("Remove")
                        .font(type_::LABEL)
                        .color(p.t.text_secondary)
                        .a11y_hidden(),
                ),
        );
    }

    let mut pane = col()
        .key("content")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .px(space::S6)
        .py(space::S3)
        .gap(space::S3)
        .fill(p.t.bg)
        .child(
            text("Choose your first apps")
                .font(type_::TITLE)
                .color(p.t.text),
        );
    if onboarding.legacy_data_found {
        pane = pane.child(
            text("Found a previous Station workspace. To import it, close Station and run station-shell.exe --import-electron from its install folder before continuing.")
                .key("onboarding-import-hint")
                .w_full()
                .wrap()
                .font(type_::BODY)
                .color(p.t.text_secondary),
        );
    }
    pane = pane.child(crate::appstore::search_field(state, p)).child(
        row()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .gap(space::S3)
            .child(choices)
            .child(selection),
    );
    if onboarding.install_failed {
        pane = pane.child(
            text("Some apps couldn’t be installed. Try again or remove them.")
                .wrap()
                .font(type_::BODY)
                .color(p.t.text),
        );
    }
    let mut actions = row().w_full().gap(space::S2).center_y();
    if !onboarding.selected.is_empty() {
        actions = actions.child(
            action("onboarding-install", "Install selected apps", p)
                .on_press(|state: &mut ShellState| state.install_onboarding_apps()),
        );
    }
    pane.child(actions.child(action("onboarding-skip", "Skip", p).on_press(
        |state: &mut ShellState| {
            state.skip_onboarding();
        },
    )))
}

fn list(key: &'static str, p: Paint) -> El {
    col()
        .key(key)
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .min_w(0.0)
        .px(space::S2)
        .py(space::S2)
        .gap(space::S1)
        .radius(radius::MD)
        .fill(p.t.card)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
}

fn action(key: &'static str, label: &'static str, p: Paint) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .h(32.0)
        .px(space::S3)
        .center()
        .radius(radius::SM)
        .fill(p.t.control_track)
        .hover(p.t.hover)
        .press(p.t.press)
        .child(text(label).font(type_::BODY).color(p.t.text).a11y_hidden())
}

#[cfg(test)]
mod tests {
    use super::*;
    use appstore_service::ApplicationService;
    use rune::{Env, Runtime, TextEdit};
    use session_store::{SessionState, SessionStore};
    use std::path::{Path, PathBuf};

    fn session_path(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "station-onboarding-{tag}-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn launch(path: &Path) -> ShellState {
        let mut state = ShellState::launch_in(ApplicationService::in_memory(), path);
        state.start_onboarding_if_fresh(false);
        state
    }

    fn press(rt: &mut Runtime<ShellState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn fresh_picker_searches_and_installs_the_selection_in_order() {
        let path = session_path("install");
        let mut rt = Runtime::new(
            launch(&path),
            crate::shell::view,
            (1200.0, 800.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.frame(1.0 / 60.0);
        assert!(rt.find("onboarding-install").is_none());
        press(&mut rt, "onboarding-choice-14");

        *rt.state.search_mut() = TextEdit::new("Slack");
        rt.state.refresh_search();
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        press(&mut rt, "onboarding-choice-21");
        assert_eq!(rt.state.tabs().len(), 1);
        assert!(!path.exists());

        press(&mut rt, "onboarding-remove-21");
        assert_eq!(rt.state.onboarding().unwrap().selected.len(), 1);
        press(&mut rt, "onboarding-choice-21");
        press(&mut rt, "onboarding-install");

        let restored = launch(&path);
        assert!(restored.onboarding().is_none());
        assert_eq!(restored.tabs()[1].label, "Gmail");
        assert_eq!(restored.tabs()[2].label, "Slack");
        assert_eq!(restored.active().label, "Gmail");
        assert!(rt.state.query().is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn closing_the_picker_keeps_the_session_available_and_skip_persists() {
        let path = session_path("skip");
        let mut state = launch(&path);
        assert!(state.onboarding().is_some());
        state.save();
        assert!(!path.exists());

        assert!(state.skip_onboarding());
        assert!(path.exists());
        let restored = launch(&path);
        assert!(restored.onboarding().is_none());
        assert_eq!(restored.tabs().len(), 1);
        assert_eq!(restored.active().id, "appstore");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn existing_sessions_keep_the_store_even_with_no_installed_apps() {
        let path = session_path("existing");
        SessionStore::new(path.clone())
            .save(&SessionState::default())
            .unwrap();
        assert!(launch(&path).onboarding().is_none());

        std::fs::write(&path, b"broken session").unwrap();
        let mut state = launch(&path);
        assert!(state.onboarding().is_none());
        state.save();
        assert_eq!(std::fs::read(&path).unwrap(), b"broken session");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn legacy_profiles_get_the_import_command_before_the_picker() {
        let path = session_path("legacy");
        let mut state = ShellState::launch_in(ApplicationService::in_memory(), &path);
        state.start_onboarding_if_fresh(true);
        let mut rt = Runtime::new(
            state,
            crate::shell::view,
            (1200.0, 800.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.frame(1.0 / 60.0);
        assert!(rt.elements().into_iter().any(|element| {
            element.key.as_deref() == Some("onboarding-import-hint")
                && element
                    .text
                    .is_some_and(|text| text.contains("--import-electron"))
        }));
        assert!(!path.exists());
    }
}
