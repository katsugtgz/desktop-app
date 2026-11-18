//! The left rail: one square row per seeded app, an "add app" affordance under them, and the
//! bottom items (Settings and the snooze/moon placeholder).
//!
//! Every colour and size comes from [`rune::tok`], so the rail follows the window's appearance.

use crate::shell::Paint;
use crate::state::{AppTab, ShellState, Surface};
use rune::tok::{self, radius, space, type_};
use rune::{canvas, col, el, spacer, text, El, Shape};

/// Width of the rail in points: a 64 pt strip holding 40 pt rows, as Station's own rail is.
pub const RAIL_WIDTH: f32 = 64.0;

const ROW: f32 = 40.0;

pub fn rail(state: &ShellState, p: Paint) -> El {
    let mut rows = col()
        .key("rail")
        .w(RAIL_WIDTH)
        .h_full()
        .py(space::S3)
        .gap(space::S2)
        .center()
        .fill(p.t.card);
    for tab in state.tabs() {
        rows = rows.child(tab_row(tab, state.is_active(&tab.id), p));
    }
    rows.child(add_row(p))
        .child(spacer())
        .child(item(
            "rail-settings",
            "Settings",
            p,
            gear(p),
            state.surface() == Surface::Settings,
            |state| state.open_settings(),
        ))
        .child(item(
            "rail-notifications",
            "Notifications",
            p,
            moon(p),
            state.surface() == Surface::Notifications,
            |state| state.toggle_notifications(),
        ))
}

/// One app's row. The glyph stands in for the app's icon until icons are fetched; the accessible
/// name is the app's, and `checked` says which row the pane shows.
fn tab_row(tab: &AppTab, active: bool, p: Paint) -> El {
    let id = tab.id.clone();
    el().key(tab.key())
        .role("tab")
        .label(tab.label.clone())
        .checked(active)
        .focusable(true)
        .size(ROW, ROW)
        .center()
        .radius(radius::MD)
        .fill(if active {
            tok::accent(0, p.dark)
        } else {
            p.t.card
        })
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press(move |s: &mut ShellState| {
            s.select(&id);
        })
        .child(
            text(glyph(&tab.label))
                .key(format!("tab-glyph-{}", tab.id))
                .font(type_::TITLE)
                .color(if active { p.t.knob } else { p.t.text_secondary })
                .a11y_hidden(),
        )
}

/// The "+" row: adding an app starts in the app store, which lists what can be added.
fn add_row(p: Paint) -> El {
    el().key("rail-add")
        .role("button")
        .label("Add an app")
        .focusable(true)
        .size(ROW, ROW)
        .center()
        .radius(radius::MD)
        .border(1.0, p.t.control_off)
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press(|s: &mut ShellState| s.open_app_store())
        .child(
            text("+")
                .key("rail-add-glyph")
                .font(type_::TITLE)
                .color(p.t.text_secondary)
                .a11y_hidden(),
        )
}

/// A bottom rail item: a labelled square holding a token-coloured glyph. `active` says the
/// surface it opens is showing, so the rail keeps pointing at it.
fn item(
    key: &'static str,
    label: &'static str,
    p: Paint,
    glyph: El,
    active: bool,
    on_press: fn(&mut ShellState),
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .checked(active)
        .focusable(true)
        .size(ROW, ROW)
        .center()
        .radius(radius::MD)
        .fill(if active { p.t.control_track } else { p.t.card })
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press(on_press)
        .child(glyph)
}

/// A ring with a hub: the settings glyph, drawn rather than fetched.
fn gear(p: Paint) -> El {
    let gear = Shape::new()
        .stroke(circle_points(0.0, 360.0, 24, 7.5), 1.6, p.t.text_secondary)
        .fill(circle_points(0.0, 360.0, 16, 2.0), p.t.text_secondary);
    canvas(gear).size(18.0, 18.0).a11y_hidden()
}

/// A "C" open to the right: the moon/snooze placeholder, an arc rather than a fetched icon.
fn moon(p: Paint) -> El {
    let moon = Shape::new().stroke(
        circle_points(105.0, 255.0, 12, 7.5),
        1.6,
        p.t.text_secondary,
    );
    canvas(moon).size(18.0, 18.0).a11y_hidden()
}

/// `steps` points along a circle of `radius` around the centre of the 18 pt glyph box, from
/// `from` through `to` degrees (0° at the right, 90° at the bottom: the box's y axis points down).
fn circle_points(from: f32, to: f32, steps: usize, radius: f32) -> Vec<[f32; 2]> {
    let centre = 9.0;
    (0..=steps)
        .map(|i| {
            let angle = (from + (to - from) * i as f32 / steps as f32).to_radians();
            [centre + radius * angle.cos(), centre + radius * angle.sin()]
        })
        .collect()
}

/// The first letter of an app's name, the stand-in for its icon.
fn glyph(label: &str) -> String {
    label
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}
