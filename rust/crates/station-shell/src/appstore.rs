//! The App Store, drawn by Rune inside the shell's content area.
//!
//! Station's store is two columns: a 220 pt sidebar (its name, a search field, and the DISCOVER
//! sections) and the list the sidebar selects. Tiles are a 4-column grid of letter avatars;
//! pressing an uninstalled tile installs the app and switches to it, pressing an installed one
//! removes it (the rail row goes, and the caller closes its page).
//!
//! Everything here reads cached lists from [`ShellState`]; the only queries run are the ones a
//! user action triggers (install/uninstall, and the search field's own refresh).

use crate::shell::Paint;
use crate::state::{ShellState, StoreSection, TileAction};
use manifest_registry::MinimalApplication;
use rune::color;
use rune::tok::{self, radius, space, type_};
use rune::{col, edit_access, el, row, text, text_input, Color, El, ScrollSpec};

/// The store's secondary sidebar, in points: Station's own is 220 pt wide.
pub const SIDEBAR_WIDTH: f32 = 220.0;

/// Tiles per grid row.
const COLUMNS: usize = 4;

/// The store's pane: sidebar beside the selected list, filling the shell's content box.
pub fn view(state: &ShellState, p: Paint) -> El {
    col()
        .key("content")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .fill(p.t.bg)
        .child(
            row()
                .key("store")
                .grow(1.0)
                .w_full()
                .min_h(0.0)
                .child(sidebar(state, p))
                .child(main(state, p)),
        )
}

fn sidebar(state: &ShellState, p: Paint) -> El {
    let mut column = col()
        .key("store-sidebar")
        .w(SIDEBAR_WIDTH)
        .h_full()
        .min_h(0.0)
        .py(space::S3)
        .px(space::S2)
        .gap(space::S1)
        .fill(p.t.card)
        .border_sides([0.0, 1.0, 0.0, 0.0], p.t.separator);
    column = column.child(
        row()
            .key("store-title")
            .h(28.0)
            .px(space::S2)
            .center_y()
            .child(
                text("Station App Store")
                    .key("store-title-text")
                    .font(type_::TITLE)
                    .color(p.t.text),
            ),
    );
    column = column.child(search_field(state, p));
    column = column.child(
        text("DISCOVER")
            .key("store-discover")
            .font(type_::SECTION)
            .color(p.t.text_secondary)
            .ml(space::S2)
            .mt(space::S3)
            .a11y_hidden(),
    );
    for section in StoreSection::ALL {
        column = column.child(section_row(section, state.section(), p));
    }
    column.child(rune::spacer())
}

fn search_field(state: &ShellState, p: Paint) -> El {
    el().key("store-search-box")
        .w_full()
        .h(30.0)
        .px(space::S2)
        .center_y()
        .radius(radius::MD)
        .fill(p.t.card)
        .border(1.0, p.t.control_off)
        .child(
            text_input(state.search(), type_::BODY)
                .key("store-search")
                .w_full()
                .single_line()
                .label("Search an app")
                .placeholder("Search an app", p.t.text_secondary)
                .color(p.t.text)
                .on_edit(edit_access(|state: &mut ShellState| state.search_mut()))
                .on_input_change(|state: &mut ShellState, _| state.refresh_search()),
        )
}

fn section_row(section: StoreSection, current: StoreSection, p: Paint) -> El {
    let selected = section == current;
    el().key(section.key())
        .role("tab")
        .label(section.label())
        .checked(selected)
        .focusable(true)
        .w_full()
        .h(30.0)
        .px(space::S2)
        .center_y()
        .radius(radius::SM)
        .fill(if selected {
            p.t.control_track
        } else {
            p.t.card
        })
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press(move |state: &mut ShellState| state.show_section(section))
        .child(
            text(section.label())
                .font(type_::BODY)
                .color(if selected {
                    p.t.text
                } else {
                    p.t.text_secondary
                })
                .a11y_hidden(),
        )
}

fn main(state: &ShellState, p: Paint) -> El {
    let mut area = col()
        .key("store-main")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .py(space::S6)
        .px(space::S6)
        .gap(space::S6)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    area = area.child(heading(state, p));
    if !state.query().trim().is_empty() {
        area = area.child(grid(state.results(), state, p));
    } else {
        match state.section() {
            StoreSection::MostPopular => {
                let popular = state.popular();
                for (title, apps) in [
                    ("Cream of the crop", &popular.cream_of_the_crop_apps),
                    ("Runner ups", &popular.runner_ups),
                    ("Noteworthy", &popular.noteworthy),
                ] {
                    if apps.is_empty() {
                        continue;
                    }
                    area = area.child(group(title, apps, state, p));
                }
            }
            StoreSection::AllApps => {
                for (category, apps) in state.categories() {
                    if apps.is_empty() {
                        continue;
                    }
                    area = area.child(group(category, apps, state, p));
                }
            }
            StoreSection::MyCustomApps => {
                let custom = state.custom();
                if custom.is_empty() {
                    area = area.child(
                        text("No custom apps yet.")
                            .key("store-custom-empty")
                            .font(type_::BODY)
                            .color(p.t.text_secondary),
                    );
                } else {
                    area = area.child(grid(custom, state, p));
                }
            }
        }
    }
    area
}

fn heading(state: &ShellState, p: Paint) -> El {
    let query = state.query().trim();
    let (title, subtitle) = if query.is_empty() {
        let subtitle = match state.section() {
            StoreSection::MostPopular => "The most installed apps by the Station community",
            StoreSection::AllApps => "Every app the Station app store lists",
            StoreSection::MyCustomApps => "Apps you added yourself",
        };
        (state.section().main_title(), subtitle.to_owned())
    } else {
        (
            "Search results",
            format!("{} matching “{query}”", state.results().len()),
        )
    };
    col()
        .key("store-heading")
        .w_full()
        .gap(space::S1)
        .child(
            text(title)
                .key("store-heading-title")
                .font(type_::TITLE)
                .color(p.t.text),
        )
        .child(
            text(subtitle)
                .key("store-heading-subtitle")
                .font(type_::LABEL)
                .color(p.t.text_secondary),
        )
        .child(
            el().key("store-heading-rule")
                .w_full()
                .h(1.0)
                .mt(space::S2)
                .fill(p.t.separator)
                .a11y_hidden(),
        )
}

/// One titled band of tiles: the store's "Cream of the crop", a category, or a search's results.
fn group(title: &str, apps: &[MinimalApplication], state: &ShellState, p: Paint) -> El {
    col()
        .key(format!("store-group-{}", slug(title)))
        .w_full()
        .gap(space::S3)
        .child(
            col()
                .gap(space::S1)
                .child(
                    text(title)
                        .key("store-group-title")
                        .font(type_::SECTION)
                        .color(p.t.text_secondary),
                )
                .child(
                    el().key("store-group-underline")
                        .w(40.0)
                        .h(2.0)
                        .radius(1.0)
                        .fill(tok::accent(0, p.dark))
                        .a11y_hidden(),
                ),
        )
        .child(grid(apps, state, p))
}

fn grid(apps: &[MinimalApplication], state: &ShellState, p: Paint) -> El {
    let mut grid = col().key("store-grid").w_full().gap(space::S3);
    for chunk in apps.chunks(COLUMNS) {
        let mut line = row().w_full().gap(space::S3);
        for app in chunk {
            line = line.child(tile(app, state, p));
        }
        // Keep the last row's tiles the width of a full row's.
        for _ in chunk.len()..COLUMNS {
            line = line.child(rune::spacer());
        }
        grid = grid.child(line);
    }
    grid
}

fn tile(app: &MinimalApplication, state: &ShellState, p: Paint) -> El {
    let installed = state.is_installed(&app.bx_app_manifest_url);
    let tint = tint_for(app, p);
    let pressed = app.clone();
    let mut tile = el()
        .key(format!("store-tile-{}", app.id))
        .role("button")
        .label(app.name.clone())
        .focusable(true)
        .grow(1.0)
        .h(40.0)
        .gap(space::S3)
        .center_y()
        .radius(radius::SM)
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press(move |state: &mut ShellState| {
            if let TileAction::Uninstalled { application_id } = state.activate_tile(&pressed) {
                crate::webview::close_tab(&application_id);
            }
        })
        .child(avatar(&app.name, tint, p));
    if installed {
        tile = tile.child(
            col()
                .grow(1.0)
                .gap(2.0)
                .child(
                    text(app.name.clone())
                        .font(type_::BODY)
                        .color(p.t.text)
                        .truncate(),
                )
                .child(
                    text("Installed")
                        .key(format!("store-installed-{}", app.id))
                        .font(type_::LABEL)
                        .color(tok::accent(0, p.dark)),
                ),
        );
    } else {
        tile = tile.child(
            text(app.name.clone())
                .grow(1.0)
                .font(type_::BODY)
                .color(p.t.text)
                .truncate(),
        );
    }
    tile
}

fn avatar(name: &str, tint: Color, p: Paint) -> El {
    let letter = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    el().size(28.0, 28.0)
        .radius(14.0)
        .center()
        .fill(tint)
        .child(
            text(letter)
                .font(type_::LABEL)
                .color(ink_on(tint, p))
                .a11y_hidden(),
        )
        .a11y_hidden()
}

/// The avatar colour: the manifest's `theme_color` when it parses, else one of Rune's accents
/// picked from the app id, so every tile has a stable, palette-derived colour.
fn tint_for(app: &MinimalApplication, p: Paint) -> Color {
    if let Some(tint) = color::hex(&app.theme_color) {
        return tint;
    }
    let index =
        app.id.bytes().fold(0usize, |sum, byte| sum + byte as usize) % tok::ACCENT_NAMES.len();
    tok::accent(index, p.dark)
}

/// A readable foreground for text on `tint`: the appearance's own foregrounds, chosen by the
/// tint's perceived lightness.
fn ink_on(tint: Color, p: Paint) -> Color {
    let luminance = 0.2126 * tint.r + 0.7152 * tint.g + 0.0722 * tint.b;
    if luminance > 0.4 {
        p.t.bg
    } else {
        tok::theme(!p.dark).knob
    }
}

/// A stable element key from a title: lowercase ASCII alphanumerics, runs joined by dashes.
fn slug(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}
