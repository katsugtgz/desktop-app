//! The single-instance slice's pure half: instance-file parsing, request framing, the
//! acceptance gate, and the shared decision (a link over the IPC acts exactly as the same
//! string passed as a launch argument would).
//!
//! The seams are `instance::{parse_instance_file, parse_request, Gate, handle_request, drain}`
//! and `deep_link::{open, act, first_candidate}` — plain-library code, no socket bound and no
//! mutex created. What is **not** verifiable here, and why: `handoff_if_secondary`'s
//! `CreateMutexW`/`ERROR_ALREADY_EXISTS` branch and `std::process::exit` (a test that took it
//! would end the test process), the real `TcpListener` accept loop in `start_primary` (it binds
//! a real port and spawns a real thread; `handle_request` is tested directly instead, and it is
//! the listener's whole decision minus I/O), and the windowed focus path
//! (`webview::platform::bring_to_front` — it needs the live window's `HWND`, the same seam the
//! DPI override stands in for). Those three are the slice's Unverified notes.

use std::ffi::OsString;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use appstore_service::ApplicationService;

use crate::deep_link::{self, Outcome};
use crate::instance::{
    self, Drain, Gate, RequestError, ACCEPT_WINDOW_S, MAX_ACCEPTED_PER_RUN,
    MAX_ACCEPTED_PER_WINDOW, MAX_FILE_BYTES, MAX_REQUEST_BYTES, TOKEN_CHARS,
};
use crate::state::ShellState;

static NEXT_STORE: AtomicUsize = AtomicUsize::new(0);

/// A shell over a fresh private-manifest store in the system temp dir.
fn shell_state() -> ShellState {
    let tag = format!(
        "station-shell-instance-{}",
        NEXT_STORE.fetch_add(1, Ordering::Relaxed)
    );
    ShellState::new(ApplicationService::for_tests(&tag))
}

/// The module's gate and queue are process-global; other tests' frames drain the queue, and
/// this module's own tests run in parallel against one another. One test at a time holds the
/// global test lock, freezes the module (a concurrent frame's `poll_at` drain hands back
/// nothing) and resets the statics — the guard unfreezes on drop, so a panic cannot leave the
/// frame polls starved.
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Session;

impl Session {
    fn take() -> (std::sync::MutexGuard<'static, ()>, Self) {
        let guard = match TEST_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        instance::freeze(true);
        instance::own_freeze();
        instance::reset_for_tests();
        (guard, Session)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        instance::freeze(false);
    }
}

/// A well-formed link request for `token`.
fn link_request(token: &str, url: &str) -> Vec<u8> {
    let mut body = token.as_bytes().to_vec();
    body.push(b'\n');
    body.extend_from_slice(url.as_bytes());
    body.push(b'\n');
    body
}

/// The focus-only ping: `token\n\n`.
fn ping_request(token: &str) -> Vec<u8> {
    link_request(token, "")
}

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

// ── instance.json parsing ───────────────────────────────────────────────────────────────────

#[test]
fn a_well_formed_instance_file_parses() {
    let text = format!(r#"{{"port": 51234, "token": "{TOKEN}"}}"#);
    let file = instance::parse_instance_file(&text).expect("parses");
    assert_eq!(file.port, 51234);
    assert_eq!(file.token, TOKEN);
}

#[test]
fn a_malformed_instance_file_is_refused_without_panicking() {
    assert!(instance::parse_instance_file("not json").is_none());
    assert!(instance::parse_instance_file("").is_none());
    assert!(instance::parse_instance_file(r#"{"port": 51234}"#).is_none());
    assert!(instance::parse_instance_file(r#"{"token": "abc"}"#).is_none());
}

#[test]
fn an_oversized_instance_file_is_refused_unread() {
    let pad = "x".repeat(MAX_FILE_BYTES + 1);
    assert!(instance::parse_instance_file(&pad).is_none());
    // Exactly the cap is not refused on size (it then fails the shape check, which is fine —
    // the point is the bound sits at MAX_FILE_BYTES, exclusive).
    assert!(instance::parse_instance_file(&"x".repeat(MAX_FILE_BYTES)).is_none());
}

#[test]
fn a_token_of_the_wrong_shape_is_refused() {
    // Right length, not hex.
    let text = r#"{"port": 1, "token": ""#.to_owned() + &"z".repeat(TOKEN_CHARS) + r#""}"#;
    assert!(instance::parse_instance_file(&text).is_none());
    // Hex, wrong length (a leftover token from an older format).
    let text = r#"{"port": 1, "token": "abcd"}"#;
    assert!(instance::parse_instance_file(text).is_none());
    // Uppercase hex is not the shape this run generates.
    let upper = TOKEN.to_uppercase();
    let text = format!(r#"{{"port": 1, "token": "{upper}"}}"#);
    assert!(instance::parse_instance_file(&text).is_none());
}

#[test]
fn port_zero_is_refused() {
    let text = format!(r#"{{"port": 0, "token": "{TOKEN}"}}"#);
    assert!(instance::parse_instance_file(&text).is_none());
}

// ── the token ────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_token_comparison_is_exact_and_length_gated() {
    assert!(instance::token_matches(TOKEN, TOKEN));
    assert!(!instance::token_matches(
        TOKEN,
        "ffffffffffffffffffffffffffffffff"
    ));
    // Wrong length never compares byte-wise.
    assert!(!instance::token_matches("short", TOKEN));
    assert!(!instance::token_matches(&TOKEN[..31], TOKEN));
}

#[test]
fn the_generated_token_has_the_instance_file_shape() {
    // Two calls in the same process differ (each run's token is fresh); the public shape
    // predicate accepts both.
    let text = format!(r#"{{"port": 1, "token": "{TOKEN}"}}"#);
    assert!(instance::parse_instance_file(&text).is_some());
    assert!(instance::is_token(&"a".repeat(TOKEN_CHARS)));
    assert!(!instance::is_token(&"a".repeat(TOKEN_CHARS - 1)));
}

// ── request framing ─────────────────────────────────────────────────────────────────────────

#[test]
fn a_link_request_parses_to_its_url() {
    assert_eq!(
        instance::parse_request(&link_request(TOKEN, "station://appstore"), TOKEN),
        Ok(Some("station://appstore".to_owned()))
    );
    // The trailing newline is optional (a writer that stops after the URL still parses).
    let mut no_trailing = link_request(TOKEN, "station://appstore");
    no_trailing.pop();
    assert_eq!(
        instance::parse_request(&no_trailing, TOKEN),
        Ok(Some("station://appstore".to_owned()))
    );
}

#[test]
fn an_empty_url_line_is_the_focus_ping() {
    assert_eq!(
        instance::parse_request(&ping_request(TOKEN), TOKEN),
        Ok(None),
        "a second launch with no deep link is a focus-only request"
    );
}

#[test]
fn a_wrong_token_is_refused() {
    assert_eq!(
        instance::parse_request(
            &link_request("ffffffffffffffffffffffffffffffff", "station://appstore"),
            TOKEN
        ),
        Err(RequestError::WrongToken)
    );
}

#[test]
fn a_malformed_request_is_refused() {
    // No newline at all: the token line never ends.
    assert_eq!(
        instance::parse_request(b"token", TOKEN),
        Err(RequestError::Malformed)
    );
    // Three lines: data past the URL.
    assert_eq!(
        instance::parse_request(b"token\nstation://appstore\nextra", TOKEN),
        Err(RequestError::Malformed)
    );
    // Not UTF-8.
    assert_eq!(
        instance::parse_request(&[0xff, b'\n', b'a', b'\n'], TOKEN),
        Err(RequestError::Malformed)
    );
}

#[test]
fn an_oversized_request_is_refused_whole() {
    let url = format!("station://appstore/{}", "a".repeat(MAX_REQUEST_BYTES));
    assert_eq!(
        instance::parse_request(&link_request(TOKEN, &url), TOKEN),
        Err(RequestError::TooLong)
    );
}

#[test]
fn a_non_station_url_never_reaches_the_decision() {
    // The listener's only own URL rule; http(s) is *routing's* business, never the IPC's.
    assert_eq!(
        instance::parse_request(&link_request(TOKEN, "https://example.com"), TOKEN),
        Err(RequestError::NotStation)
    );
    assert_eq!(
        instance::parse_request(&link_request(TOKEN, "file:///etc/passwd"), TOKEN),
        Err(RequestError::NotStation)
    );
}

// ── the gate ────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_gate_caps_the_run_and_the_rolling_window() {
    let mut gate = Gate::default();
    let mut now = Instant::now();
    for _ in 0..MAX_ACCEPTED_PER_RUN.min(MAX_ACCEPTED_PER_WINDOW) {
        assert!(gate.allows(now));
    }
    // The run cap is the lower of the two at t=0: one more is refused.
    assert!(!gate.allows(now));
    // Sliding past the rolling window frees the window budget, but never the run budget.
    now += Duration::from_secs_f64(ACCEPT_WINDOW_S);
    assert!(
        !gate.allows(now),
        "the per-run cap outlives the rolling window"
    );
}

#[test]
fn a_fresh_gate_fills_its_rolling_window_again_after_it_passes() {
    // Fewer requests than either cap: after the window passes, room again — this is what
    // keeps the test above's run-cap assertion honest rather than an accident of ordering.
    let mut gate = Gate::default();
    let mut now = Instant::now();
    for _ in 0..(MAX_ACCEPTED_PER_RUN.min(MAX_ACCEPTED_PER_WINDOW) - 1) {
        assert!(gate.allows(now));
    }
    now += Duration::from_secs_f64(ACCEPT_WINDOW_S);
    assert!(
        gate.allows(now),
        "the rolling window refills after it passes"
    );
}

// ── handle + drain: the whole listener decision, minus I/O ──────────────────────────────────

#[test]
fn a_valid_request_is_accepted_and_drained_as_a_link() {
    let (_guard, _session) = Session::take();
    assert!(instance::handle_request(
        &link_request(TOKEN, "station://appstore"),
        TOKEN
    ));
    let mut state = shell_state();
    let drain = instance::drain(&mut state);
    assert_eq!(
        drain,
        Drain {
            changed: true,
            arrived: 1
        }
    );
    assert!(
        state.is_active("appstore"),
        "the IPC link selects the store tab, the same path a rail press takes"
    );
}

#[test]
fn a_focus_ping_is_accepted_drains_without_state_change_and_still_counts_as_arrival() {
    let (_guard, _session) = Session::take();
    let mut state = shell_state();
    state.select("appstore");
    let before = state.active().id.clone();
    assert!(instance::handle_request(&ping_request(TOKEN), TOKEN));
    let drain = instance::drain(&mut state);
    assert_eq!(
        drain,
        Drain {
            changed: false,
            arrived: 1
        }
    );
    assert_eq!(state.active().id, before, "no URL: no state change");
}

#[test]
fn a_refused_request_leaves_the_queue_empty() {
    let (_guard, _session) = Session::take();
    assert!(!instance::handle_request(
        &link_request("bad", "station://appstore"),
        TOKEN
    ));
    assert!(!instance::handle_request(
        &link_request(TOKEN, "https://example.com"),
        TOKEN
    ));
    let mut state = shell_state();
    assert_eq!(instance::drain(&mut state), Drain::default());
}

#[test]
fn a_flood_is_bounded_and_the_queue_never_grows_past_the_run_cap() {
    let (_guard, _session) = Session::take();
    let mut accepted = 0;
    for i in 0..(MAX_ACCEPTED_PER_RUN * 4) {
        let url = format!("station://appstore/?flood={i}");
        if instance::handle_request(&link_request(TOKEN, &url), TOKEN) {
            accepted += 1;
        }
    }
    assert!(
        accepted <= MAX_ACCEPTED_PER_RUN,
        "at most {MAX_ACCEPTED_PER_RUN} accepted per run, got {accepted}"
    );
    let mut state = shell_state();
    let drain = instance::drain(&mut state);
    assert_eq!(drain.arrived, accepted);
    // Drained: the next one is accepted again only if the run budget allows (here it does not —
    // the flood used it up, which is the point).
    assert!(!instance::handle_request(&ping_request(TOKEN), TOKEN));
}

#[test]
fn the_decision_is_the_same_whether_the_link_came_from_argv_or_the_ipc() {
    // The exact strings the OS hands a launched station:// link, through both arrival paths.
    for url in [
        "station://appstore",
        "station://appstore/",
        "STATION://appstore",
        "station://configurator/settings",
        "station://appstore?query=1",
    ] {
        let mut by_argv = shell_state();
        let argv_outcome = deep_link::open(&mut by_argv, &[OsString::from(url)]);
        let mut by_ipc = shell_state();
        let ipc_outcome = deep_link::act(&mut by_ipc, url.to_owned());
        assert_eq!(
            argv_outcome, ipc_outcome,
            "argv and IPC must decide {url} identically"
        );
        assert_eq!(
            by_argv.is_active("appstore"),
            by_ipc.is_active("appstore"),
            "argv and IPC must leave {url}'s state identical"
        );
    }
}

#[test]
fn argv_and_the_ipc_agree_on_the_unserved_hosts_too() {
    // Both paths name-and-ignore the same non-store hosts; neither touches the state.
    let opener_argv = crate::routing::record_external();
    let mut by_argv = shell_state();
    let argv = deep_link::open(&mut by_argv, &[OsString::from("station://other-host")]);
    let opener_ipc = crate::routing::record_external();
    let mut by_ipc = shell_state();
    let ipc = deep_link::act(&mut by_ipc, "station://other-host".to_owned());
    assert_eq!(argv, ipc);
    assert!(matches!(argv, Outcome::NotServed(_)));
    assert_eq!(by_argv.active().id, "appstore");
    assert_eq!(by_ipc.active().id, "appstore");
    assert!(crate::routing::recorded_external(&opener_argv).is_empty());
    assert!(crate::routing::recorded_external(&opener_ipc).is_empty());
}

#[test]
fn the_second_launch_forwards_only_a_real_candidate() {
    // What `handoff_if_secondary` sends: the first `station:` argument, or nothing — never the
    // program name, never a flag, never a non-UTF-8 byte string.
    assert_eq!(
        deep_link::first_candidate(&[
            OsString::from("station-shell.exe"),
            OsString::from("--flag")
        ]),
        None,
        "no URL: the second launch is a focus-only ping"
    );
    assert_eq!(
        deep_link::first_candidate(&[
            OsString::from("station-shell.exe"),
            OsString::from("station://appstore")
        ]),
        Some("station://appstore".to_owned())
    );
    assert_eq!(
        deep_link::first_candidate(&[
            OsString::from("--flag"),
            OsString::from("station://first"),
            OsString::from("station://second")
        ]),
        Some("station://first".to_owned()),
        "the first candidate only, like argv handling"
    );
}

#[test]
fn a_second_launch_with_no_url_changes_no_state_even_when_a_link_is_already_queued() {
    // A no-URL launch's ping (empty URL line) drains to `arrived` without state change, even
    // beside a real link queued before it — each queued request keeps its own shape.
    let (_guard, _session) = Session::take();
    assert!(instance::handle_request(
        &link_request(TOKEN, "station://appstore"),
        TOKEN
    ));
    assert!(instance::handle_request(&ping_request(TOKEN), TOKEN));
    let mut state = shell_state();
    let drain = instance::drain(&mut state);
    assert_eq!(drain.arrived, 2);
    assert!(drain.changed, "the link acted; the ping did not");
    assert!(state.is_active("appstore"));
}
