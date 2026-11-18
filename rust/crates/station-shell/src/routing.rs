//! Routing a page's new-window request: whether the shell can show it, or the OS browser must.
//!
//! A page's `window.open` / `target=_blank` arrives on the web view's new-window handler. The
//! shell never lets WebView2 open a window — the app has exactly one — so the request is queued
//! and denied, then routed on the next frame by [`url_router`] over the installed applications and
//! the rail's tabs. [`decide`] is the whole decision and takes no platform types, so it is tested
//! directly; [`super::webview`] carries it out.

use std::cell::RefCell;
use std::sync::Mutex;

use url_router::{InstalledApp, OpenTab, Origin, Route, RouteRequest};

/// What the shell will do with a requested URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Routed {
    /// Nothing to do: no tab the shell owns answers to it.
    Ignore,
    /// Bring an existing tab forward; its page is already there.
    Focus { tab_id: String },
    /// Bring a tab forward and navigate it (a hash-only match, or the origin app's own scope).
    FocusAndLoad { tab_id: String, url: String },
    /// Navigate an app's tab (it may not be open yet) and bring it forward.
    Load { tab_id: String, url: String },
    /// Hand the URL to the OS default browser. Never a second web view.
    External,
}

/// One queued request: which tab asked, and where it wanted to go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub origin_tab: String,
    pub url: String,
}

static QUEUE: Mutex<Vec<Request>> = Mutex::new(Vec::new());

/// The most requests a page can have queued at once: a `window.open` loop cannot grow memory.
pub const MAX_QUEUED: usize = 64;

/// The most external browser launches one frame will perform, and the most distinct URLs a single
/// frame will hand over: a page cannot spawn an unbounded number of browsers. Requests beyond the
/// cap are dropped for that frame, not deferred: a trickle across frames would still be
/// unbounded.
pub const MAX_EXTERNAL_PER_FRAME: usize = 5;

/// The rolling budget: at most this many browser launches in any [`EXTERNAL_WINDOW_S`] seconds,
/// whatever the frame rate. The per-frame cap alone let a page loop across frames.
pub const MAX_EXTERNAL_PER_WINDOW: usize = 10;

/// The rolling window, in seconds.
pub const EXTERNAL_WINDOW_S: f64 = 60.0;

/// When each allowed launch happened, on the frame clock; older entries are pruned lazily.
static EXTERNAL_BUDGET: Mutex<Vec<f64>> = Mutex::new(Vec::new());

/// Whether the rolling budget has room for one more launch at `now`; records it when it does.
/// A request refused here is dropped, not deferred.
pub fn budget_allows(now: f64) -> bool {
    let now = if now.is_finite() { now } else { 0.0 };
    let mut opened = match EXTERNAL_BUDGET.lock() {
        Ok(opened) => opened,
        Err(poisoned) => poisoned.into_inner(),
    };
    opened.retain(|at| now - at < EXTERNAL_WINDOW_S);
    if opened.len() >= MAX_EXTERNAL_PER_WINDOW {
        return false;
    }
    opened.push(now);
    true
}

/// Forgets the rolling budget (tests).
#[cfg(test)]
pub fn reset_external_budget() {
    match EXTERNAL_BUDGET.lock() {
        Ok(mut opened) => opened.clear(),
        Err(poisoned) => poisoned.into_inner().clear(),
    }
}

/// The schemes a page may hand to the OS browser. Everything else — `file:`, `javascript:`,
/// `data:`, custom app schemes — is refused: a page does not get to launch anything locally.
pub const EXTERNAL_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Whether `url` may be opened by the OS browser.
pub fn is_external_scheme(url: &str) -> bool {
    url_router::parse_url(url).is_some_and(|parsed| {
        let scheme = parsed.protocol.trim_end_matches(':');
        EXTERNAL_SCHEMES.contains(&scheme)
    })
}

thread_local! {
    /// Rune's wake handle: a queued request must reach the next frame even though the click that
    /// caused it happened inside the page's own window, where Rune draws nothing.
    static WAKER: RefCell<Option<rune::Waker>> = const { RefCell::new(None) };
}

/// Queues a new-window request and wakes the shell so its next frame routes it.
///
/// A duplicate of a request already queued is dropped, and a full queue refuses the request:
/// false in both cases. A poisoned queue refuses rather than panicking.
pub fn enqueue(origin_tab: &str, url: &str) -> bool {
    let queued = {
        // A poisoned queue is recovered, not abandoned: a panic elsewhere must not stop routing.
        let mut queue = match QUEUE.lock() {
            Ok(queue) => queue,
            Err(poisoned) => poisoned.into_inner(),
        };
        enqueue_locked(&mut queue, origin_tab, url)
    };
    if queued {
        WAKER.with(|waker| {
            // A re-entrant or poisoned borrow must not take the shell down mid-frame.
            if let Ok(slot) = waker.try_borrow() {
                if let Some(wake) = slot.as_ref() {
                    wake();
                }
            }
        });
    }
    queued
}

fn enqueue_locked(queue: &mut Vec<Request>, origin_tab: &str, url: &str) -> bool {
    if queue.len() >= MAX_QUEUED {
        return false;
    }
    if queue
        .iter()
        .any(|queued| queued.origin_tab == origin_tab && queued.url == url)
    {
        return false;
    }
    queue.push(Request {
        origin_tab: origin_tab.to_owned(),
        url: url.to_owned(),
    });
    true
}

/// Takes the queued requests (called once per frame by the poll hook). A poisoned lock still
/// yields its queued requests: a panic elsewhere must not strand them.
pub fn take() -> Vec<Request> {
    match QUEUE.lock() {
        Ok(mut queue) => std::mem::take(&mut *queue),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    }
}

/// Remembers Rune's wake handle (`AppSpec::setup`).
pub fn remember_waker(waker: rune::Waker) {
    WAKER.with(|slot| *slot.borrow_mut() = Some(waker));
}

/// The decision for `url`, from the request's origin tab: which tab shows it, or the OS browser.
///
/// The mapping from [`url_router`]'s actions to the shell's one-tab-per-app model:
/// - focus/reload a matched tab → show that app;
/// - push-and-focus or the origin app's own scope → show that app and load the URL in its page;
/// - a new tab or instance of an installed app → that app's tab loads the URL (the shell has no
///   per-app second tab yet);
/// - install-and-open (the app is not installed) and anything unmatched → the OS browser. The
///   shell does not install from a link.
pub fn decide(
    url: &str,
    apps: &[InstalledApp],
    tabs: &[OpenTab],
    origin_tab: Option<&str>,
) -> Routed {
    let mut request = RouteRequest::new(url, apps, tabs);
    if let Some(origin) = origin_tab {
        let application_id = tabs
            .iter()
            .find(|tab| tab.tab_id == origin)
            .map(|tab| tab.application_id.as_str());
        request = request.with_origin(Origin {
            tab_id: Some(origin),
            application_id,
        });
    }
    match request.route() {
        Route::Reload { tab_id } | Route::FocusTab { tab_id } => focus(&tab_id, tabs),
        Route::PushAndFocusTab { tab_id, url } => focus_and_load(&tab_id, url, tabs),
        Route::OpenInTab {
            tab_id: Some(tab_id),
            url,
        } => focus_and_load(&tab_id, url, tabs),
        Route::OpenInTab { tab_id: None, url } => {
            tabs.iter()
                .find(|tab| tab.url == url)
                .map_or(Routed::External, |tab| Routed::Focus {
                    tab_id: tab.tab_id.clone(),
                })
        }
        Route::NewTab {
            application_id,
            url,
        }
        | Route::NewInstance {
            application_id,
            url,
        } => tabs
            .iter()
            .find(|tab| tab.application_id == application_id)
            .map_or(Routed::External, |tab| Routed::Load {
                tab_id: tab.tab_id.clone(),
                url,
            }),
        Route::InstallAndOpen { .. } | Route::ExternalBrowser => Routed::External,
    }
}

/// A tab that is not one of ours (a page whose app left the rail): focus has nothing to show, so
/// the request is dropped — the URL is already in that page's history.
fn focus(tab_id: &str, tabs: &[OpenTab]) -> Routed {
    if tabs.iter().any(|tab| tab.tab_id == tab_id) {
        Routed::Focus {
            tab_id: tab_id.to_owned(),
        }
    } else {
        Routed::Ignore
    }
}

/// A URL that cannot be shown in a tab we own goes to the browser rather than being lost.
fn focus_and_load(tab_id: &str, url: String, tabs: &[OpenTab]) -> Routed {
    if tabs.iter().any(|tab| tab.tab_id == tab_id) {
        Routed::FocusAndLoad {
            tab_id: tab_id.to_owned(),
            url,
        }
    } else {
        Routed::External
    }
}

/// Hands `url` to the OS default browser. Never another web view or an OS window of ours.
///
/// Only [`EXTERNAL_SCHEMES`] reach the OS. On Windows the URL goes through `ShellExecuteW`, which
/// takes one string and no shell, so `&`, `|`, `"` and the rest are inert data rather than
/// command syntax — the old `cmd /C start` handed a page-controlled URL to a command interpreter.
pub fn open_external(url: &str) {
    if !is_external_scheme(url) {
        eprintln!("station-shell: refused to open a non-web URL: {url}");
        return;
    }
    #[cfg(test)]
    {
        let recorded = EXTERNAL.with(|slot| slot.borrow().clone());
        if let Some(recorded) = recorded {
            recorded.borrow_mut().push(url.to_owned());
            return;
        }
    }
    if let Err(why) = launch_in_browser(url) {
        eprintln!("station-shell: could not open {url} in the browser: {why}");
    }
}

#[cfg(target_os = "windows")]
fn launch_in_browser(url: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    let wide: Vec<u16> = std::ffi::OsStr::new(url)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let operation = [b'o' as u16, b'p' as u16, b'e' as u16, b'n' as u16, 0];
    // SAFETY: both strings are NUL-terminated and live for the call; `ShellExecuteW` copies what
    // it needs and reads no other argument.
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW reports values <= 32 as errors (docs: "the return value is cast as HINSTANCE,
    // and is one of these error codes").
    if (result as isize) <= 32 {
        Err(format!("ShellExecuteW returned {}", result as isize))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn launch_in_browser(url: &str) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|why| why.to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn launch_in_browser(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|why| why.to_string())
}

/// Tests record what would have gone to the browser instead of opening one.
#[cfg(test)]
type Opener = std::rc::Rc<RefCell<Vec<String>>>;

#[cfg(test)]
thread_local! {
    static EXTERNAL: RefCell<Option<Opener>> = const { RefCell::new(None) };
}

/// Poisons the request queue, so the recovery paths can be tested.
#[cfg(test)]
pub fn poison_queue() {
    let _ = std::panic::catch_unwind(|| {
        let _guard = QUEUE.lock().expect("queue lock");
        panic!("poison the queue on purpose");
    });
}

/// Collects external URLs for this test's thread, replacing any previous recorder.
#[cfg(test)]
pub fn record_external() -> Opener {
    let recorded: Opener = std::rc::Rc::new(RefCell::new(Vec::new()));
    EXTERNAL.with(|slot| *slot.borrow_mut() = Some(recorded.clone()));
    recorded
}

/// What the recorder has collected so far.
#[cfg(test)]
pub fn recorded_external(recorded: &Opener) -> Vec<String> {
    recorded.borrow().clone()
}
