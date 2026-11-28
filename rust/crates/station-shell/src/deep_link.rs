//! Launching with a `station://` deep link: reading this process's arguments once and acting
//! on the first link among them — and, since slice 14, the one *decision* a link arriving over
//! the single-instance IPC takes too (`instance::drain` calls [`act`] on the UI thread).
//!
//! The OS protocol registration (`station://`, written by a future installer per
//! `docs/rust-packaging-plan.md` §7) launches `station-shell.exe "station://…"` — a whole second
//! process. Since slice 14 that process hands its argument to the running shell
//! (`instance::handoff_if_secondary`) and exits; the URL rules here are shared by both paths,
//! one copy.
//!
//! The shell serves exactly one WebUI surface, the App Store (`station://appstore`, D6). Any
//! other `station:` URL is named on stderr and dropped — the shell does not serve the
//! configurator or the rest of the `webui` set, and must not pretend to. A deep link is never a
//! reason to open a second window or the OS browser (this is the *user's* navigation, D9's
//! "page" role does not apply — but the shell still has one window, and argv is not a page).
//!
//! All decisions are made here on plain strings, before any parser sees more than a bounded
//! candidate: the argument length is capped ([`MAX_ARG_CHARS`]), control characters are refused,
//! and a non-UTF-8 argument cannot panic (`std::env::args` would — the caller passes
//! `args_os`). Parsing reuses [`url_router::parse_url`], the same WHATWG parser the routing
//! decisions use; nothing about a URL is judged by hand except the candidate prefix scan.

use std::ffi::OsString;

use url_router::parse_url;

use crate::state::ShellState;

/// The longest deep-link argument read at all, in characters: a longer candidate is refused
/// before it reaches the URL parser, so a hostile argv cannot feed it an unbounded string.
pub const MAX_ARG_CHARS: usize = 2048;

/// The scheme a candidate carries, checked as a case-insensitive ASCII prefix exactly like the
/// WHATWG parser treats schemes (`STATION://` parses with a lowercase `station` scheme — the
/// same acceptance `url-router` relies on, not a case rule invented here).
const SCHEME_PREFIX: &str = "station:";

/// What reading the launch arguments did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The App Store tab is now the active one (the same path a rail press takes).
    Store,
    /// A `station:` URL the shell does not serve: one stderr line, state untouched.
    NotServed(String),
    /// A `station:` candidate refused before it could act — too long, a control character,
    /// not valid UTF-8, or not a parsable URL: one stderr line, state untouched.
    Refused(String),
    /// No `station:` argument among the process arguments: a normal launch.
    None,
}

/// Reads `args` (the process arguments, `argv[0]` included or not — anything that is not a
/// `station:` candidate is skipped silently) and acts on the **first** candidate only; later
/// candidates are not examined. Returns what happened; the caller is `AppSpec::setup`, whose
/// `&mut ShellState` is the one write path into the shell.
pub fn open(state: &mut ShellState, args: &[OsString]) -> Outcome {
    for arg in args {
        let Some(text) = arg.to_str() else {
            // An argument that is not valid UTF-8 cannot be a link we honour, but it could
            // still *claim* the scheme: refuse it rather than skip it, without panicking.
            if starts_with_scheme(&arg.to_string_lossy()) {
                return refuse(arg.to_string_lossy().into_owned(), "not valid UTF-8");
            }
            continue;
        };
        if !starts_with_scheme(text) {
            continue;
        }
        return act(state, text.to_owned());
    }
    Outcome::None
}

/// The first `station:`-prefixed argument as a string, without acting on it: what a second
/// launch forwards to the primary (`instance::handoff_if_secondary`). A non-UTF-8 candidate is
/// dropped here — the primary would refuse it (`act` never sees invalid UTF-8), so forwarding
/// it would only make the primary's stderr lie about an argument it cannot show.
pub(crate) fn first_candidate(args: &[OsString]) -> Option<String> {
    args.iter()
        .filter_map(|arg| arg.to_str())
        .find(|text| starts_with_scheme(text))
        .map(|text| text.to_owned())
}

/// The one decision for a candidate that is valid UTF-8: bounds first, then the parser, then
/// the single surface the shell serves. Shared by the two arrival paths — a launch argument
/// ([`open`]) and a link handed over by a second launch (`instance::drain`) — so there is no
/// second copy of the rules.
pub(crate) fn act(state: &mut ShellState, arg: String) -> Outcome {
    if arg.chars().count() > MAX_ARG_CHARS {
        return refuse(arg, "longer than 2048 chars");
    }
    if arg.chars().any(char::is_control) {
        return refuse(arg, "a control character");
    }
    let Some(parsed) = parse_url(&arg) else {
        return refuse(arg, "not a parsable URL");
    };
    // The store link only: its exact host, with no path beyond the optional `/`. The WHATWG
    // parser preserves an opaque host's case (only schemes are lowercased), so
    // `station://AppStore` is a different URL, not the store's. A `?query` or `#fragment` makes
    // it a different URL too; after a successful parse those characters can only be separators.
    if parsed.protocol == "station:"
        && parsed.domain == "appstore"
        && (parsed.pathname.is_empty() || parsed.pathname == "/")
        && !arg.contains(['?', '#'])
    {
        state.open_app_store();
        return Outcome::Store;
    }
    not_served(arg)
}

/// One stderr line naming the URL; the state is untouched.
fn not_served(arg: String) -> Outcome {
    eprintln!(
        "station-shell: deep link not served (only station://appstore is): {}",
        loggable(&arg)
    );
    Outcome::NotServed(arg)
}

/// One stderr line naming the refusal reason; the state is untouched.
fn refuse(arg: String, reason: &str) -> Outcome {
    eprintln!(
        "station-shell: deep link refused ({}): {}",
        reason,
        loggable(&arg)
    );
    Outcome::Refused(arg)
}

/// The argument as one stderr line may show it: control characters elided and at most 256
/// characters, so a hostile argument cannot litter the log or grow it.
fn loggable(arg: &str) -> String {
    let cleaned: String = arg.chars().filter(|c| !c.is_control()).collect();
    if cleaned.chars().count() <= 256 {
        cleaned
    } else {
        let mut cut: String = cleaned.chars().take(256).collect();
        cut.push('…');
        cut
    }
}

/// The case-insensitive scheme-prefix scan that picks candidates out of the arguments. Shared
/// with `instance` (the listener's only own URL rule) so both arrival paths pick candidates the
/// same way.
pub(crate) fn starts_with_scheme(text: &str) -> bool {
    text.len() >= SCHEME_PREFIX.len()
        && text.as_bytes()[..SCHEME_PREFIX.len()].eq_ignore_ascii_case(SCHEME_PREFIX.as_bytes())
}
