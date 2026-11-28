//! Deep-link launch arguments: `deep_link::open` against a real `ShellState`, over the same
//! strings Windows hands the process (one argument per link, quoted or not — that is the
//! runner's job, the function takes raw arguments).
//!
//! Every test also records the OS-browser opener: a deep link must never leave the shell for
//! the browser, whatever it decides.

use std::ffi::OsString;
use std::sync::atomic::{AtomicUsize, Ordering};

use appstore_service::ApplicationService;

use crate::deep_link::{self, Outcome};
use crate::routing;
use crate::state::ShellState;

static NEXT_STORE: AtomicUsize = AtomicUsize::new(0);

/// A shell over a fresh private-manifest store in the system temp dir.
fn shell_state() -> ShellState {
    let tag = format!(
        "station-shell-deep-link-{}",
        NEXT_STORE.fetch_add(1, Ordering::Relaxed)
    );
    ShellState::new(ApplicationService::for_tests(&tag))
}

fn os(text: &str) -> OsString {
    OsString::from(text)
}

/// Records what would have gone to the OS browser; every test asserts this stays empty.
fn no_browser() -> std::rc::Rc<std::cell::RefCell<Vec<String>>> {
    routing::record_external()
}

#[test]
fn the_store_link_selects_the_store_tab() {
    let opener = no_browser();
    let mut state = shell_state();
    // A web tab is active, so "the store tab is now active" is observable.
    assert!(state.install("station-manifest://14"));
    assert_ne!(state.active().id, "appstore", "the test needs another tab");
    let outcome = deep_link::open(
        &mut state,
        &[os("station-shell.exe"), os("station://appstore")],
    );
    assert_eq!(outcome, Outcome::Store);
    assert_eq!(state.active().id, "appstore");
    assert!(routing::recorded_external(&opener).is_empty());
}

#[test]
fn the_trailing_slash_variant_selects_the_store_too() {
    let opener = no_browser();
    let mut state = shell_state();
    let outcome = deep_link::open(&mut state, &[os("station://appstore/")]);
    assert_eq!(outcome, Outcome::Store);
    assert_eq!(state.active().id, "appstore");
    assert!(routing::recorded_external(&opener).is_empty());
}

#[test]
fn an_uppercase_scheme_is_the_same_whatwg_url() {
    // The WHATWG parser (the one `url-router` uses) lowercases the scheme, so `STATION://` is
    // accepted exactly as `station://`; only the scheme gets this treatment.
    let mut state = shell_state();
    let outcome = deep_link::open(&mut state, &[os("STATION://appstore")]);
    assert_eq!(outcome, Outcome::Store);
    assert_eq!(state.active().id, "appstore");
}

#[test]
fn another_host_is_named_and_ignored_without_state_change() {
    let opener = no_browser();
    let mut state = shell_state();
    // A web tab is active, so "no state change" is observable: the link must not move it.
    assert!(state.install("station-manifest://14"));
    let active = state.active().id.clone();

    let outcome = deep_link::open(&mut state, &[os("station://settings")]);
    assert_eq!(outcome, Outcome::NotServed("station://settings".to_owned()));
    assert_eq!(
        state.active().id,
        active,
        "the ignored link changed the state"
    );
    assert!(routing::recorded_external(&opener).is_empty());

    // The shell can still honour a real link afterwards: the ignore consumed nothing.
    assert_eq!(
        deep_link::open(&mut state, &[os("station://appstore")]),
        Outcome::Store
    );
    assert_eq!(state.active().id, "appstore");
}

#[test]
fn a_longer_path_or_a_query_or_fragment_is_not_the_store() {
    let opener = no_browser();
    for arg in [
        "station://appstore/search",
        "station://appstore/search?q=mail",
        "station://appstore?from=rail",
        "station://appstore#installed",
        "station:///appstore",
        "station:appstore",
        "station://%zz",
        "station://",
    ] {
        let mut state = shell_state();
        assert!(state.install("station-manifest://14"));
        let active = state.active().id.clone();
        let outcome = deep_link::open(&mut state, &[os(arg)]);
        assert_eq!(outcome, Outcome::NotServed(arg.to_owned()), "{arg}");
        assert_eq!(state.active().id, active, "{arg} changed the state");
    }
    assert!(routing::recorded_external(&opener).is_empty());
}

#[test]
fn an_uppercase_host_is_not_the_store() {
    // Only the scheme is lowercased by the WHATWG parser; an opaque host keeps its case, so
    // `station://AppStore` is a different URL and is refused, not silently honoured.
    let mut state = shell_state();
    assert!(state.install("station-manifest://14"));
    let active = state.active().id.clone();
    let outcome = deep_link::open(&mut state, &[os("station://AppStore")]);
    assert_eq!(outcome, Outcome::NotServed("station://AppStore".to_owned()));
    assert_eq!(state.active().id, active);
}

#[test]
fn an_oversized_argument_is_refused_whole_and_the_cap_is_inclusive() {
    let opener = no_browser();
    // One past the cap: refused before any parser sees it.
    let long = format!("station://{}", "a".repeat(deep_link::MAX_ARG_CHARS));
    let mut state = shell_state();
    assert!(state.install("station-manifest://14"));
    let active = state.active().id.clone();
    let outcome = deep_link::open(&mut state, &[os(&long)]);
    assert_eq!(outcome, Outcome::Refused(long));
    assert_eq!(state.active().id, active);

    // Exactly at the cap: parsed (and here not the store — a long host), not refused.
    let at_cap = format!("station://{}", "a".repeat(deep_link::MAX_ARG_CHARS - 10));
    assert_eq!(at_cap.chars().count(), deep_link::MAX_ARG_CHARS);
    let outcome = deep_link::open(&mut state, &[os(&at_cap)]);
    assert_eq!(outcome, Outcome::NotServed(at_cap.clone()));
    assert_eq!(
        state.active().id,
        active,
        "a parsed non-store link changed nothing"
    );
    assert!(routing::recorded_external(&opener).is_empty());
}

#[test]
fn control_characters_are_refused_before_parsing() {
    let opener = no_browser();
    for arg in [
        "station://app\nstore",
        "station://appstore\u{0}",
        "station://app\u{7f}",
    ] {
        let mut state = shell_state();
        let outcome = deep_link::open(&mut state, &[os(arg)]);
        assert_eq!(outcome, Outcome::Refused(arg.to_owned()), "{arg:?}");
        assert_eq!(state.active().id, "appstore", "{arg:?} changed the state");
    }
    assert!(routing::recorded_external(&opener).is_empty());
}

#[cfg(windows)]
#[test]
fn an_invalid_utf8_candidate_is_refused_without_panicking() {
    use std::os::windows::ffi::OsStringExt;
    // An unpaired surrogate: `to_str` fails, and `std::env::args` would have panicked on it.
    let mut wide: Vec<u16> = "station:".encode_utf16().collect();
    wide.push(0xD800);
    let arg = OsString::from_wide(&wide);
    let mut state = shell_state();
    let outcome = deep_link::open(&mut state, std::slice::from_ref(&arg));
    assert_eq!(
        outcome,
        Outcome::Refused(arg.to_string_lossy().into_owned())
    );
    assert_eq!(state.active().id, "appstore");
}

#[test]
fn an_invalid_utf8_argument_without_the_scheme_is_skipped() {
    #[cfg(windows)]
    fn hostile() -> OsString {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[0x0041, 0xD800])
    }
    #[cfg(not(windows))]
    fn hostile() -> OsString {
        OsString::from("A\u{FFFD}")
    }
    let mut state = shell_state();
    let outcome = deep_link::open(&mut state, &[os("station-shell.exe"), hostile()]);
    assert_eq!(outcome, Outcome::None);
    assert_eq!(state.active().id, "appstore");
}

#[test]
fn arguments_without_a_station_scheme_are_a_no_op() {
    let opener = no_browser();
    let mut state = shell_state();
    assert!(state.install("station-manifest://14"));
    let active = state.active().id.clone();
    let outcome = deep_link::open(
        &mut state,
        &[
            os("station-shell.exe"),
            os("--flag"),
            os("https://example.com/page"),
        ],
    );
    assert_eq!(outcome, Outcome::None);
    assert_eq!(state.active().id, active);
    assert!(routing::recorded_external(&opener).is_empty());
}

#[test]
fn no_arguments_at_all_is_a_no_op() {
    let mut state = shell_state();
    assert_eq!(deep_link::open(&mut state, &[]), Outcome::None);
    assert_eq!(state.active().id, "appstore");
}

#[test]
fn station_scheme_garbage_that_parses_to_nothing_is_refused() {
    let opener = no_browser();
    let mut state = shell_state();
    // A space is a forbidden host character: the WHATWG parser refuses it outright. (Other
    // hostile spellings — `station://%zz`, `station://` — parse leniently into URLs the shell
    // does not serve, which the matrix test above covers as `NotServed`.)
    let arg = "station://app store";
    let outcome = deep_link::open(&mut state, &[os(arg)]);
    assert_eq!(outcome, Outcome::Refused(arg.to_owned()));
    assert_eq!(state.active().id, "appstore");
    assert!(routing::recorded_external(&opener).is_empty());
}

#[test]
fn only_the_first_station_argument_acts() {
    let mut state = shell_state();
    // The first candidate decides: served here, so the later ignored link is never examined.
    let outcome = deep_link::open(
        &mut state,
        &[os("station://appstore"), os("station://settings")],
    );
    assert_eq!(outcome, Outcome::Store);

    // And refused here, so the later store link is never honoured.
    let mut state = shell_state();
    assert!(state.install("station-manifest://14"));
    let active = state.active().id.clone();
    let outcome = deep_link::open(
        &mut state,
        &[os("station://bogus"), os("station://appstore")],
    );
    assert_eq!(outcome, Outcome::NotServed("station://bogus".to_owned()));
    assert_eq!(
        state.active().id,
        active,
        "the second candidate acted anyway"
    );
}
