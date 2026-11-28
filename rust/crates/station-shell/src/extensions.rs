//! Explicit installation and removal of trusted unpacked extensions in a WebView2 profile.

use crate::shell::Paint;
use crate::state::ShellState;
use rune::tok::{radius, space, type_};
use rune::{col, el, row, text, El, TextEdit};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub(crate) use windows::request;

pub enum Request {
    List,
    Install(PathBuf),
    Remove(String),
}

#[derive(Debug, Eq, PartialEq)]
pub struct Extension {
    pub id: String,
    pub name: String,
}

pub enum Update {
    List(Vec<Extension>),
    Installed(Extension),
    Removed(String),
}

type ResultUpdate = Result<Update, String>;

#[derive(Clone)]
pub struct Reply {
    channel: Sender<ResultUpdate>,
    wake: Option<rune::Waker>,
}

impl Reply {
    pub fn send(self, result: ResultUpdate) {
        if self.channel.send(result).is_err() {
            return;
        }
        if let Some(wake) = self.wake {
            wake();
        }
    }
}

pub(crate) struct Manager {
    target: Option<String>,
    edit: TextEdit,
    installed: Vec<Extension>,
    message: String,
    pending: Option<Receiver<ResultUpdate>>,
    wake: Option<rune::Waker>,
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            target: None,
            edit: TextEdit::new(""),
            installed: Vec::new(),
            message: String::new(),
            pending: None,
            wake: None,
        }
    }
}

impl Manager {
    pub fn set_waker(&mut self, wake: rune::Waker) {
        self.wake = Some(wake);
    }

    fn begin(&mut self, target: &str) -> Option<Reply> {
        if self.pending.is_some() {
            return None;
        }
        if self.target.as_deref() != Some(target) {
            self.target = Some(target.to_owned());
            self.edit = TextEdit::new("");
            self.installed.clear();
        }
        self.message.clear();
        let (channel, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        Some(Reply {
            channel,
            wake: self.wake.clone(),
        })
    }

    pub fn poll(&mut self) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };
        let result = match pending.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                Err("The app page closed before WebView2 finished.".to_owned())
            }
        };
        self.pending = None;
        self.message = match result {
            Ok(Update::List(installed)) => {
                self.installed = installed;
                if self.installed.is_empty() {
                    "No extensions installed.".to_owned()
                } else {
                    String::new()
                }
            }
            Ok(Update::Installed(extension)) => {
                self.installed.retain(|entry| entry.id != extension.id);
                self.installed.push(extension);
                self.edit = TextEdit::new("");
                "Extension installed. Reload the app page to apply it to existing pages.".to_owned()
            }
            Ok(Update::Removed(id)) => {
                self.installed.retain(|entry| entry.id != id);
                "Extension removed. Reload the app page to clear its changes.".to_owned()
            }
            Err(error) => error,
        };
        true
    }
}

fn local_extension_folder(input: &str) -> Result<PathBuf, &'static str> {
    let input = input.trim();
    let path = Path::new(input);
    if input.chars().any(char::is_control) || !path.is_absolute() {
        return Err("Enter the full path of an unpacked extension folder.");
    }
    #[cfg(target_os = "windows")]
    if !matches!(
        path.components().next(),
        Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_))
    ) {
        return Err("Choose an extension folder on a local drive.");
    }
    if !path.join("manifest.json").is_file() {
        return Err("Choose the folder that contains the extension's manifest.json.");
    }
    Ok(path.to_owned())
}

fn apply(state: &mut ShellState, request: Request) {
    if !matches!(state.surface(), crate::state::Surface::Settings)
        || state.active().renders_in_rune()
    {
        return;
    }
    let target = state.active().id.clone();
    if let Some(reply) = state.extensions.begin(&target) {
        crate::webview::request_extensions(&target, request, reply);
    }
}

fn install(state: &mut ShellState) {
    if state.extensions.target.as_deref() != Some(state.active().id.as_str())
        || state.extensions.pending.is_some()
    {
        return;
    }
    match local_extension_folder(&state.extensions.edit.text) {
        Ok(folder) => apply(state, Request::Install(folder)),
        Err(error) => state.extensions.message = error.to_owned(),
    }
}

pub(crate) fn settings_card(state: &ShellState, p: Paint) -> El {
    let active = state.active();
    let manager = &state.extensions;
    let card =
        |rows| crate::settings::section("settings-extensions", "Browser extensions", p, rows);

    if active.renders_in_rune() {
        return card(vec![note(
            "Open an app page, then return here to manage its browser extensions.",
            p,
        )]);
    }
    if manager.pending.is_some() {
        return card(vec![note("Waiting for WebView2…", p)]);
    }

    let scope = if active.use_default_session {
        "Shared app profile".to_owned()
    } else {
        format!("{} profile", active.label)
    };
    let manages_active = manager.target.as_deref() == Some(active.id.as_str());
    let refresh = button(
        "extensions-refresh",
        if manages_active { "Refresh" } else { "Manage" },
        p,
    )
    .on_press(|state: &mut ShellState| apply(state, Request::List));
    let heading = row()
        .w_full()
        .h(36.0)
        .px(space::S3)
        .center_y()
        .justify_between()
        .child(text(scope).font(type_::BODY).color(p.t.text).truncate())
        .child(refresh);
    if !manages_active {
        return card(vec![heading]);
    }

    let input = rune::text_input(&manager.edit, type_::BODY)
        .key("extensions-folder")
        .w_full()
        .single_line()
        .label("Unpacked extension folder")
        .placeholder("Full folder path", p.t.text_secondary)
        .color(p.t.text)
        .on_edit(rune::edit_access(|state: &mut ShellState| {
            &mut state.extensions.edit
        }));
    let mut editor = col()
        .w_full()
        .px(space::S3)
        .py(space::S2)
        .gap(space::S2)
        .child(text("Extensions can read or change pages in this profile. Use a folder you trust and keep it in place.").font(type_::LABEL).color(p.t.text_secondary))
        .child(
            row()
                .w_full()
                .h(30.0)
                .gap(space::S2)
                .center_y()
                .child(
                    el().grow(1.0)
                        .h(30.0)
                        .px(space::S2)
                        .center_y()
                        .fill(p.t.bg)
                        .border(1.0, p.t.control_off)
                        .radius(radius::SM)
                        .child(input),
                )
                .child(button("extensions-install", "Install unpacked extension", p).on_press(install)),
        );
    if !manager.message.is_empty() {
        editor = editor.child(
            text(manager.message.clone())
                .font(type_::LABEL)
                .color(p.t.text_secondary),
        );
    }
    let mut rows = vec![heading, editor];
    for extension in &manager.installed {
        let id = extension.id.clone();
        rows.push(
            row()
                .key(format!("extensions-row-{id}"))
                .w_full()
                .h(36.0)
                .px(space::S3)
                .center_y()
                .justify_between()
                .child(
                    text(extension.name.clone())
                        .font(type_::BODY)
                        .color(p.t.text)
                        .truncate(),
                )
                .child(
                    button(format!("extension-remove-{id}"), "Remove", p)
                        .label(format!("Remove {}", extension.name))
                        .on_press(move |state: &mut ShellState| {
                            if state.extensions.target.as_deref()
                                == Some(state.active().id.as_str())
                            {
                                apply(state, Request::Remove(id.clone()));
                            }
                        }),
                ),
        );
    }
    card(rows)
}

fn note(label: &str, p: Paint) -> El {
    el().w_full().px(space::S3).py(space::S2).child(
        text(label.to_owned())
            .font(type_::LABEL)
            .color(p.t.text_secondary),
    )
}

fn button(key: impl Into<String>, label: &str, p: Paint) -> El {
    el().key(key)
        .role("button")
        .label(label.to_owned())
        .focusable(true)
        .h(26.0)
        .px(space::S2)
        .center()
        .radius(radius::SM)
        .fill(p.t.control_track)
        .hover(p.t.hover)
        .press(p.t.press)
        .child(
            text(label.to_owned())
                .font(type_::LABEL)
                .color(p.t.text)
                .a11y_hidden(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_an_explicit_unpacked_folder() {
        let folder = crate::tests_icons::temp_cache("unpacked-extension");
        std::fs::create_dir_all(&folder).unwrap();
        assert!(local_extension_folder("relative").is_err());
        assert!(local_extension_folder(folder.to_str().unwrap()).is_err());
        std::fs::write(folder.join("manifest.json"), "{}").unwrap();
        let actual = local_extension_folder(folder.to_str().unwrap()).unwrap();
        assert_eq!(actual, folder);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn profile_results_do_not_cross_over_and_operations_serialize() {
        let mut manager = Manager::default();
        let first = manager.begin("first").unwrap();
        assert!(manager.begin("second").is_none());
        first.send(Ok(Update::Installed(Extension {
            id: "one".to_owned(),
            name: "First extension".to_owned(),
        })));
        assert!(manager.poll());
        assert_eq!(manager.installed.len(), 1);
        let second = manager.begin("second").unwrap();
        assert!(manager.installed.is_empty());
        second.send(Ok(Update::List(Vec::new())));
        assert!(manager.poll());
        assert_eq!(manager.message, "No extensions installed.");
    }

    #[test]
    fn remove_keeps_other_extensions_and_handles_a_dropped_callback() {
        let mut manager = Manager::default();
        manager.begin("app").unwrap().send(Ok(Update::List(vec![
            Extension {
                id: "first".to_owned(),
                name: "First".to_owned(),
            },
            Extension {
                id: "second".to_owned(),
                name: "Second".to_owned(),
            },
        ])));
        assert!(manager.poll());
        manager
            .begin("app")
            .unwrap()
            .send(Ok(Update::Removed("first".to_owned())));
        assert!(manager.poll());
        assert_eq!(manager.installed[0].id, "second");
        drop(manager.begin("app").unwrap());
        assert!(manager.poll());
        assert!(manager.message.contains("closed"));
    }
}
