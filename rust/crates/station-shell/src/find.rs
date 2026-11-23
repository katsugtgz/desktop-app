//! In-tab search (`Ctrl+F` / `Cmd+F`): a Rune bar directly above the content pane.
//!
//! Because Rune cannot draw over a child WebView2 `HWND`, the find bar sits inside the
//! `pane` column between the top bar and `content`. Opening it shrinks `content` cleanly by
//! [`FIND_BAR_HEIGHT`] pt so `webview::sync` repositions WebView2 below it with zero overlap.
//! Queries are sent to the page through `window.__stationFind(query, forward)` using
//! JSON-encoded string literals (never raw interpolation); match counts come back over the
//! single bounded IPC channel (`kind: "find"`).

use rune::tok::{radius, space, type_};
use rune::{edit_access, el, row, text, text_input, El, TextEdit};

use crate::shell::Paint;
use crate::state::ShellState;

/// Height of the in-tab search strip, in points.
pub const FIND_BAR_HEIGHT: f32 = 40.0;

/// Maximum query length forwarded to the page.
pub const MAX_QUERY_CHARS: usize = 256;

/// State of the in-tab find bar.
#[derive(Clone, Debug)]
pub struct FindState {
    pub open: bool,
    pub edit: TextEdit,
    /// 1-based index of the highlighted match (0 when there are no matches).
    pub current: u32,
    /// Total highlighted matches in the active page.
    pub total: u32,
}

impl Default for FindState {
    fn default() -> Self {
        Self {
            open: false,
            edit: TextEdit::new(String::new()),
            current: 0,
            total: 0,
        }
    }
}

impl FindState {
    pub fn query(&self) -> &str {
        &self.edit.text
    }

    pub fn set_matches(&mut self, current: u32, total: u32) -> bool {
        if self.current == current && self.total == total {
            return false;
        }
        self.current = current;
        self.total = total;
        true
    }

    pub fn reset_matches(&mut self) {
        self.current = 0;
        self.total = 0;
    }
}

/// Builds the JS invocation for `window.__stationFind(query, forward)` using JSON escaping.
/// Returns `None` if the query contains control chars or cannot be encoded.
pub fn find_script(query: &str, forward: bool) -> Option<String> {
    let trimmed: String = query
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_QUERY_CHARS)
        .collect();
    let encoded = serde_json::to_string(&trimmed).ok()?;
    let dir = if forward { "true" } else { "false" };
    Some(format!("window.__stationFind({encoded}, {dir});"))
}

/// JS invocation to remove all in-tab search highlights.
pub const CLEAR_SCRIPT: &str = "window.__stationFindClear();";

/// Human-readable counter text for the find bar.
pub fn status_label(query: &str, current: u32, total: u32) -> String {
    if query.is_empty() {
        String::new()
    } else if total == 0 {
        "No matches".to_owned()
    } else {
        format!("{current} / {total}")
    }
}

/// Draws the find bar directly below the top bar when open.
pub fn bar(state: &ShellState, p: Paint) -> El {
    let find = state.find();
    let count = status_label(find.query(), find.current, find.total);

    row()
        .key("find-bar")
        .w_full()
        .h(FIND_BAR_HEIGHT)
        .px(space::S6)
        .gap(space::S2)
        .center_y()
        .fill(p.t.card)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.t.separator)
        .child(
            el().key("find-input-box")
                .w(280.0)
                .h(26.0)
                .px(space::S2)
                .center_y()
                .radius(radius::SM)
                .fill(p.t.bg)
                .border(1.0, p.t.control_off)
                .child(
                    text_input(&find.edit, type_::BODY)
                        .key("find-input")
                        .w_full()
                        .single_line()
                        .autofocus()
                        .label("Find in page")
                        .placeholder("Find in page...", p.t.text_secondary)
                        .color(p.t.text)
                        .on_edit(edit_access(|state: &mut ShellState| state.find_edit_mut()))
                        .on_input_change(|state: &mut ShellState, _| {
                            state.dispatch_find(true);
                        })
                        .on_submit(|state: &mut ShellState, _| {
                            state.dispatch_find(true);
                        }),
                ),
        )
        .child(
            text(count)
                .key("find-count")
                .font(type_::MONO_CAPTION)
                .color(p.t.text_secondary),
        )
        .child(nav_button("find-prev", "Previous match", "Prev", p, |s| {
            s.dispatch_find(false);
        }))
        .child(nav_button("find-next", "Next match", "Next", p, |s| {
            s.dispatch_find(true);
        }))
        .child(nav_button("find-close", "Close find", "Close", p, |s| {
            s.close_find();
        }))
}

fn nav_button(
    key: &'static str,
    label: &'static str,
    glyph: &'static str,
    p: Paint,
    on_press: fn(&mut ShellState),
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
        .child(text(glyph).font(type_::LABEL).color(p.t.text).a11y_hidden())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_script_escapes_hostile_input_and_caps_length() {
        let script = find_script("a\"b\n</script>", true).expect("script");
        assert!(script.starts_with("window.__stationFind(\"a\\\"b</script>\", true);"));

        let long = "z".repeat(MAX_QUERY_CHARS + 50);
        let capped = find_script(&long, false).expect("capped");
        let expected_lit = serde_json::to_string(&"z".repeat(MAX_QUERY_CHARS)).unwrap();
        assert_eq!(
            capped,
            format!("window.__stationFind({expected_lit}, false);")
        );
    }

    #[test]
    fn status_label_formats_empty_zero_and_matches() {
        assert_eq!(status_label("", 0, 0), "");
        assert_eq!(status_label("foo", 0, 0), "No matches");
        assert_eq!(status_label("foo", 2, 5), "2 / 5");
    }
}
