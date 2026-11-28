//! Native Bang Quick Switcher (`Ctrl+K` / `Ctrl+T`), backed by `activity::frecency::Frecency`.
//!
//! Replaces Electron's `src/bang/` quick-switch overlay with a Rune surface searching:
//! 1. Pinned Favorites (`session_store::FavoriteEntry`)
//! 2. Installed Apps / Tabs (ranked by `activity::frecency::Frecency`)
//! 3. Shell Actions (App Store, Settings, Notifications, About Station)
//! 4. App Store catalog matches (when a search query is typed, up to 5 quick-install items)
//!
//! A pasted web link offers a Station or browser destination.

use rune::tok::{self, radius, space, type_};
use rune::{col, edit_access, el, row, text, text_input, El, ScrollSpec, TextEdit};

use crate::routing::Routed;
use crate::shell::Paint;
use crate::state::{ShellState, TabKind};

/// Maximum App Store install suggestions shown below installed/favorite hits.
pub const MAX_STORE_SUGGESTIONS: usize = 5;

/// State of the quick-switcher search palette.
#[derive(Clone, Debug)]
pub struct QuickSwitchState {
    pub edit: TextEdit,
    pub selected: usize,
}

impl Default for QuickSwitchState {
    fn default() -> Self {
        Self {
            edit: TextEdit::new(String::new()),
            selected: 0,
        }
    }
}

impl QuickSwitchState {
    pub fn reset(&mut self) {
        self.edit = TextEdit::new(String::new());
        self.selected = 0;
    }

    pub fn query(&self) -> &str {
        self.edit.text.trim()
    }
}

/// What activating a quick-switcher row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickSwitchAction {
    /// Focus an installed tab and optionally load a pinned favorite URL.
    OpenFavorite {
        favorite_id: String,
        application_id: String,
        url: String,
    },
    /// Focus an installed application tab.
    SelectTab { tab_id: String },
    /// Open the App Store tab.
    OpenStore,
    /// Open Settings.
    OpenSettings,
    /// Open Notifications panel.
    OpenNotifications,
    /// Open About Station.
    OpenAbout,
    /// Install an app from the App Store catalog and focus it.
    InstallApp { manifest_url: String },
    /// Open a pasted link in the location selected by the user.
    OpenLink { url: String, route: Routed },
}

/// One rendered row in the quick-switcher results list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickSwitchEntry {
    pub key: String,
    pub frecency_id: String,
    pub title: String,
    pub subtitle: String,
    pub badge: &'static str,
    pub action: QuickSwitchAction,
}

/// Builds the ordered list of quick-switcher entries for `state`'s current query, ranking
/// installed tabs by `activity::frecency::Frecency` and including favorites, actions, and
/// store quick-install matches.
pub fn entries_for(state: &ShellState) -> Vec<QuickSwitchEntry> {
    let query = state.quick_switch().query();
    if query.contains("://") {
        return link_input(query)
            .map(|url| link_entries(state, url.as_str()))
            .unwrap_or_default();
    }
    let query_lower = query.to_ascii_lowercase();
    let matches_query = |haystack: &str| -> bool {
        query_lower.is_empty() || haystack.to_ascii_lowercase().contains(&query_lower)
    };

    let mut out: Vec<QuickSwitchEntry> = Vec::new();

    // 1. Pinned favorites matching the query (highest priority).
    for fav in state.favorites() {
        if matches_query(&fav.title) || matches_query(&fav.url) {
            out.push(QuickSwitchEntry {
                key: format!("qs-fav-{}", fav.id),
                frecency_id: format!("fav:{}", fav.id),
                title: fav.title.clone(),
                subtitle: fav.url.clone(),
                badge: "Favorite",
                action: QuickSwitchAction::OpenFavorite {
                    favorite_id: fav.id.clone(),
                    application_id: fav.application_id.clone(),
                    url: fav.url.clone(),
                },
            });
        }
    }

    // 2. Installed web tabs, scored by frecency (descending), tie-broken by rail order.
    let mut tab_hits: Vec<(f64, usize, QuickSwitchEntry)> = state
        .tabs()
        .iter()
        .enumerate()
        .filter(|(_, tab)| tab.kind == TabKind::Web)
        .filter(|(_, tab)| {
            matches_query(tab.display_label())
                || matches_query(&tab.label)
                || matches_query(&tab.url)
        })
        .map(|(idx, tab)| {
            let frec_id = format!("tab:{}", tab.id);
            let score = state.frecency_score(query, &frec_id);
            (
                score,
                idx,
                QuickSwitchEntry {
                    key: format!("qs-tab-{}", tab.id),
                    frecency_id: frec_id,
                    title: tab.display_label().to_owned(),
                    subtitle: tab.url.clone(),
                    badge: "App",
                    action: QuickSwitchAction::SelectTab {
                        tab_id: tab.id.clone(),
                    },
                },
            )
        })
        .collect();
    tab_hits.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    out.extend(tab_hits.into_iter().map(|(_, _, entry)| entry));

    // 3. Built-in shell actions.
    let actions = [
        (
            "qs-action-store",
            "action:store",
            "App Store",
            "Browse and install web apps",
            QuickSwitchAction::OpenStore,
        ),
        (
            "qs-action-settings",
            "action:settings",
            "Settings",
            "Appearance, accounts, and app configuration",
            QuickSwitchAction::OpenSettings,
        ),
        (
            "qs-action-notifications",
            "action:notifications",
            "Notifications",
            "View unread notifications and snooze settings",
            QuickSwitchAction::OpenNotifications,
        ),
        (
            "qs-action-about",
            "action:about",
            "About Station",
            "Version, runtime info, and updates",
            QuickSwitchAction::OpenAbout,
        ),
    ];
    for (key, frec_id, title, subtitle, action) in actions {
        if matches_query(title) || matches_query(subtitle) {
            out.push(QuickSwitchEntry {
                key: key.to_owned(),
                frecency_id: frec_id.to_owned(),
                title: title.to_owned(),
                subtitle: subtitle.to_owned(),
                badge: "Action",
                action,
            });
        }
    }

    // 4. App Store quick-install suggestions when searching.
    if !query.is_empty() {
        for app in state
            .search_catalog(query)
            .into_iter()
            .filter(|app| !state.is_installed(&app.bx_app_manifest_url))
            .take(MAX_STORE_SUGGESTIONS)
        {
            out.push(QuickSwitchEntry {
                key: format!("qs-install-{}", app.id),
                frecency_id: format!("install:{}", app.id),
                title: app.name.clone(),
                subtitle: format!("Install {}", app.name),
                badge: "Install",
                action: QuickSwitchAction::InstallApp {
                    manifest_url: app.bx_app_manifest_url,
                },
            });
        }
    }

    out
}

fn link_input(input: &str) -> Option<url::Url> {
    let (scheme, rest) = input.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        || authority.is_empty()
        || authority.contains('@')
        || input.contains('\\')
        || input.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let url = url::Url::parse(input).ok()?;
    if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url)
}

fn link_entries(state: &ShellState, url: &str) -> Vec<QuickSwitchEntry> {
    let route = state.route_quick_link(url);
    let mut entries = Vec::new();
    let station_choice = match &route {
        Routed::Ignore | Routed::External => None,
        Routed::InstallAndOpen { manifest_url, .. } => Some((
            format!(
                "Install and open {}",
                state.install_link_label(manifest_url)
            ),
            "Install",
        )),
        _ => Some(("Open in Station".to_owned(), "App")),
    };
    if let Some((title, badge)) = station_choice {
        entries.push(QuickSwitchEntry {
            key: "qs-link-station".to_owned(),
            frecency_id: String::new(),
            title,
            subtitle: url.to_owned(),
            badge,
            action: QuickSwitchAction::OpenLink {
                url: url.to_owned(),
                route,
            },
        });
    }
    entries.push(QuickSwitchEntry {
        key: "qs-link-browser".to_owned(),
        frecency_id: String::new(),
        title: "Open in browser".to_owned(),
        subtitle: url.to_owned(),
        badge: "Browser",
        action: QuickSwitchAction::OpenLink {
            url: url.to_owned(),
            route: Routed::External,
        },
    });
    entries
}

/// Renders the Quick Switcher surface in the content pane.
pub fn view(state: &ShellState, p: Paint) -> El {
    let entries = entries_for(state);
    let selected = if entries.is_empty() {
        0
    } else {
        state.quick_switch().selected.min(entries.len() - 1)
    };

    let mut list = col()
        .key("quick-switch-list")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(space::S1)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });

    if entries.is_empty() {
        list = list.child(
            text("No matching apps, favorites, or actions.")
                .key("quick-switch-empty")
                .font(type_::BODY)
                .color(p.t.text_secondary)
                .px(space::S3)
                .py(space::S3),
        );
    } else {
        for (idx, entry) in entries.iter().enumerate() {
            let is_selected = idx == selected;
            let action = entry.action.clone();
            let frec_id = entry.frecency_id.clone();
            list = list.child(
                row()
                    .key(entry.key.clone())
                    .role("button")
                    .label(entry.title.clone())
                    .checked(is_selected)
                    .focusable(true)
                    .w_full()
                    .h(44.0)
                    .px(space::S3)
                    .gap(space::S3)
                    .center_y()
                    .justify_between()
                    .radius(radius::SM)
                    .fill(if is_selected {
                        tok::accent(0, p.dark)
                    } else {
                        p.t.card
                    })
                    .hover(p.t.hover)
                    .press(p.t.press)
                    .on_press(move |state: &mut ShellState| {
                        state.activate_quick_switch_entry(&frec_id, action.clone());
                    })
                    .child(
                        col()
                            .grow(1.0)
                            .gap(2.0)
                            .child(
                                text(entry.title.clone())
                                    .font(type_::BODY)
                                    .color(if is_selected {
                                        rune::color::hex("#FFFFFF").unwrap_or(p.t.text)
                                    } else {
                                        p.t.text
                                    })
                                    .truncate()
                                    .a11y_hidden(),
                            )
                            .child(
                                text(entry.subtitle.clone())
                                    .font(type_::MONO_CAPTION)
                                    .color(if is_selected {
                                        rune::color::hex("#E5E7EB").unwrap_or(p.t.text_secondary)
                                    } else {
                                        p.t.text_secondary
                                    })
                                    .truncate()
                                    .a11y_hidden(),
                            ),
                    )
                    .child(
                        text(entry.badge)
                            .font(type_::LABEL)
                            .color(if is_selected {
                                rune::color::hex("#FFFFFF").unwrap_or(p.t.text)
                            } else {
                                p.t.text_secondary
                            })
                            .a11y_hidden(),
                    ),
            );
        }
    }

    col()
        .key("content")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .px(space::S6)
        .py(space::S6)
        .center_y()
        .fill(p.t.bg)
        .child(
            col()
                .key("quick-switch-card")
                .w_full()
                .max_w(620.0)
                .grow(1.0)
                .min_h(0.0)
                .padding(space::S3)
                .gap(space::S3)
                .radius(radius::LG)
                .fill(p.t.card)
                .border(1.0, p.t.card_border)
                .child(
                    row()
                        .w_full()
                        .center_y()
                        .justify_between()
                        .child(
                            text("Quick Switcher")
                                .key("quick-switch-title")
                                .font(type_::TITLE)
                                .color(p.t.text),
                        )
                        .child(
                            el().key("quick-switch-close")
                                .role("button")
                                .label("Close")
                                .focusable(true)
                                .h(24.0)
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
                                    text("Esc")
                                        .font(type_::MONO_CAPTION)
                                        .color(p.t.text_secondary)
                                        .a11y_hidden(),
                                ),
                        ),
                )
                .child(
                    el().key("quick-switch-input-box")
                        .w_full()
                        .h(34.0)
                        .px(space::S3)
                        .center_y()
                        .radius(radius::MD)
                        .fill(p.t.bg)
                        .border(1.0, p.t.control_off)
                        .child(
                            text_input(&state.quick_switch().edit, type_::BODY)
                                .key("quick-switch-input")
                                .w_full()
                                .single_line()
                                .autofocus()
                                .label("Find an app or paste a link")
                                .placeholder("Find an app or paste a link", p.t.text_secondary)
                                .color(p.t.text)
                                .on_edit(edit_access(|state: &mut ShellState| {
                                    state.quick_switch_edit_mut()
                                }))
                                .on_input_change(|state: &mut ShellState, _| {
                                    state.reset_quick_switch_selection();
                                })
                                .on_submit(|state: &mut ShellState, _| {
                                    state.submit_quick_switch();
                                }),
                        ),
                )
                .child(list),
        )
}
