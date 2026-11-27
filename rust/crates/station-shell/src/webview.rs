//! Hosting each app's web content over the Rune window.
//!
//! Rune draws the rail and the top bar; an app's page is an OS child web view positioned over the
//! `content` element (Windows: a WebView2 child `HWND`). That is the seam this module owns, for
//! the reasons in `docs/rust-rune-port.md` (D4/D5):
//!
//! - `ShellState` stays pure and `Clone + PartialEq`: no platform handle enters it. The host and
//!   the bookkeeping of what is attached live in a per-thread registry here, which is also where
//!   `wry::WebView` (neither `Send` nor `Clone`) may live.
//! - The pane's rectangle comes from Rune's presented element tree, so the layout stays the one
//!   source of truth: a 44 pt top bar or a resized window moves the web view without this module
//!   knowing either number.
//! - One web view per tab: created on first activation, hidden when another tab shows, kept for
//!   the tab's next activation, dropped when the window closes or the app quits.
//!
//! Page environment (slice 21): every new page is built with the Electron session's user agent
//! ([`DEFAULT_USER_AGENT`], or the manifest's `bx_override_user_agent`) and its app's WebView2
//! profile ([`PageProfile`]): `persist:<applicationId>` becomes one user-data folder per app,
//! while `bx_use_default_session` apps share the default one. Both are fixed at build; wry exposes
//! no header interception, so the TS `onBeforeSendHeaders` rules (the `accounts.google.com`
//! UA override, Referer stripping, CSP deletion) are not reproduced. Downloads are allowed and
//! logged through [`download_line`], bounded by [`MAX_DOWNLOAD_LINES`].

use crate::state::{AppTab, PageError, ShellState};
use rune::Runtime;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// The element whose box the app's page fills. The rail and top bar stay outside it, drawn by
/// Rune, so this key is the only geometry contract between layout and the web view.
pub const CONTENT_KEY: &str = "content";

/// A rectangle in logical points, window-relative: the units Rune lays out in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl PaneRect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
}

/// A rectangle in device pixels: what a native child window is placed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanePixels {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl PaneRect {
    /// The rectangle if it is usable, clamped into the window: a non-finite or empty rect is
    /// `None` (the page is hidden rather than placed at NaN), and negative origins move to 0.
    pub fn clamped(self) -> Option<PaneRect> {
        let finite = [self.x, self.y, self.w, self.h]
            .iter()
            .all(|value| value.is_finite());
        if !finite || self.w <= 0.0 || self.h <= 0.0 {
            return None;
        }
        Some(PaneRect {
            x: self.x.clamp(0.0, MAX_PANE),
            y: self.y.clamp(0.0, MAX_PANE),
            w: self.w.min(MAX_PANE),
            h: self.h.min(MAX_PANE),
        })
    }

    /// This rectangle in device pixels at `scale` (Rune's `Env::scale`), rounded to whole pixels
    /// with a minimum of one so a collapsing pane never asks the OS for a zero-sized window.
    pub fn pixels(self, scale: f32) -> PanePixels {
        PanePixels {
            x: (self.x * scale).round() as i32,
            y: (self.y * scale).round() as i32,
            w: ((self.w * scale).round() as u32).max(1),
            h: ((self.h * scale).round() as u32).max(1),
        }
    }
}

/// Why a host could not do what the shell asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebviewError {
    /// This platform has no web view host yet. Built only by the non-Windows host.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    Unsupported(&'static str),
    /// The platform's web view failed to be created or changed.
    Failed(String),
}

impl fmt::Display for WebviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebviewError::Unsupported(why) => write!(f, "web views are unsupported here: {why}"),
            WebviewError::Failed(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for WebviewError {}

/// The Electron session's default UA (`packages/app/src/session.ts`'s `defaultUserAgent`, passed
/// to `session.setUserAgent`): every hosted page saw this string unless its manifest set
/// `bx_override_user_agent`, or the request went to `https://accounts.google.com` (that
/// per-request override needs header interception, which wry does not expose — the module sets
/// the webview's UA instead).
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36";

/// The most characters one manifest-declared UA keeps. A UA is short; an over-long one is capped,
/// never sent whole.
pub const MAX_USER_AGENT_CHARS: usize = 512;

/// How many download log lines one run writes. Every download is still allowed; only the log is
/// bounded.
pub const MAX_DOWNLOAD_LINES: u32 = 32;

/// The most URL/file-path characters one download line keeps.
pub const MAX_DOWNLOAD_TEXT_CHARS: usize = 256;

/// The profile an app's pages share. Electron named these `persist:<applicationId>` partitions;
/// the native equivalent is one WebView2 user-data folder per key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageProfile {
    /// Electron's default session: apps with `bx_use_default_session`.
    Shared,
    /// Electron's `persist:<applicationId>` partition: one app's own cookies and storage.
    App(String),
}

impl PageProfile {
    /// The directory name under the webview data root. A safe application id maps to `app-<id>`;
    /// anything else (separators, dots, control characters, over 64 bytes) maps to
    /// `app-<sha256 prefix>` so the id can never choose a path.
    pub fn directory_name(&self) -> String {
        match self {
            PageProfile::Shared => "default".to_owned(),
            PageProfile::App(id) => match safe_profile_id(id) {
                Some(id) => format!("app-{id}"),
                None => format!("app-{}", short_hash(id)),
            },
        }
    }
}

fn safe_profile_id(id: &str) -> Option<&str> {
    (!id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then_some(id)
}

fn short_hash(id: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let digest = Sha256::digest(id.as_bytes());
    let mut out = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// What a hosted page is created with. Both facts are fixed from install-time data when the web
/// view is built; no page message can change either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageEnvironment {
    pub user_agent: String,
    pub profile: PageProfile,
    pub legacy_service_id: Option<String>,
}

impl PageEnvironment {
    /// The app's page environment: the manifest UA override when one exists (Electron's
    /// `<webview useragent>` attribute), else the session's default; and the default session for
    /// `bx_use_default_session` apps, else the app's own profile.
    pub fn for_tab(tab: &AppTab) -> Self {
        let user_agent = tab
            .user_agent
            .as_deref()
            .map(bounded_user_agent)
            .filter(|user_agent| !user_agent.is_empty())
            .unwrap_or_else(|| DEFAULT_USER_AGENT.to_owned());
        let profile = if tab.use_default_session {
            PageProfile::Shared
        } else {
            PageProfile::App(tab.id.clone())
        };
        Self {
            user_agent,
            profile,
            legacy_service_id: tab.legacy_service_id.clone(),
        }
    }
}

fn bounded_user_agent(user_agent: &str) -> String {
    user_agent
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_USER_AGENT_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The directory one profile's WebView2 user data lives in: `<root>/<directory_name>`.
pub fn profile_dir(root: &Path, profile: &PageProfile) -> PathBuf {
    root.join(profile.directory_name())
}

/// The webview data root: beside the session document, so `STATION_SESSION_PATH` scratch runs
/// (and their pages' cookies and cache) stay off the user profile; production is
/// `<config dir>/station/webview`.
pub fn profile_root() -> PathBuf {
    crate::session::path()
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("webview")
}

/// A download event wry reports; [`download_line`] is the bounded log shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadPhase {
    Started,
    Completed { success: bool },
}

/// One bounded, control-character-free download log line. The URL and the destination are both
/// outside the shell's control (page content and WebView2's download folder), so each is capped
/// and stripped of control characters; the line never spans more than one log line.
pub fn download_line(phase: DownloadPhase, url: &str, path: Option<&Path>) -> String {
    let url = bounded_text(url);
    match phase {
        DownloadPhase::Started => format!("station-shell: download started: {url}"),
        DownloadPhase::Completed { success } => {
            let event = if success {
                "download complete"
            } else {
                "download failed"
            };
            match path {
                Some(path) => format!(
                    "station-shell: {event}: {url} -> {}",
                    bounded_text(&path.display().to_string())
                ),
                None => format!("station-shell: {event}: {url}"),
            }
        }
    }
}

fn bounded_text(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .take(MAX_DOWNLOAD_TEXT_CHARS)
        .collect()
}

pub(crate) fn log_download(line: String) {
    static LINES: AtomicU32 = AtomicU32::new(0);
    let line_number = LINES.fetch_add(1, Ordering::Relaxed);
    if line_number < MAX_DOWNLOAD_LINES {
        eprintln!("{line}");
    } else if line_number == MAX_DOWNLOAD_LINES {
        eprintln!("station-shell: download log stopped after {MAX_DOWNLOAD_LINES} lines this run");
    }
}

/// What the shell asks of whatever renders an app's page.
pub trait WebviewHost {
    /// Create the page for `tab_id` at `url` with `environment` (UA and profile), filling `rect`
    /// (points), hidden.
    fn attach(
        &mut self,
        tab_id: &str,
        url: &str,
        environment: &PageEnvironment,
        rect: PaneRect,
    ) -> Result<(), WebviewError>;
    /// Move and resize an attached page.
    fn set_bounds(&mut self, tab_id: &str, rect: PaneRect) -> Result<(), WebviewError>;
    /// Navigate an attached page (a routed link that belongs to this tab).
    fn load(&mut self, tab_id: &str, url: &str) -> Result<(), WebviewError>;
    /// Deliver one notification click to the page behind `tab_id`: evaluate the script that
    /// invokes the page's registered click listeners with `notification_id`.
    fn click(&mut self, tab_id: &str, notification_id: &str) -> Result<(), WebviewError>;
    /// Evaluate an arbitrary shell-generated script inside an attached page (snooze fan-out,
    /// history navigation, in-tab search).
    fn eval_script(&mut self, tab_id: &str, script: &str) -> Result<(), WebviewError>;
    /// Reload an attached page in place.
    fn reload(&mut self, tab_id: &str) -> Result<(), WebviewError>;
    /// Apply a zoom scale factor (`0.5..=3.0`) to an attached page.
    fn set_zoom(&mut self, tab_id: &str, factor: f64) -> Result<(), WebviewError>;
    /// The window's scale changed: later bounds are converted with it.
    fn set_scale(&mut self, scale: f32);
    /// Show or hide an attached page without destroying it.
    fn set_visible(&mut self, tab_id: &str, visible: bool);
    /// Destroy an attached page.
    fn detach(&mut self, tab_id: &str);
}

/// How many attach attempts a tab gets before the shell stops retrying by itself: the first try
/// plus three.
pub const MAX_ATTEMPTS: u32 = 4;

/// The waits between automatic attempts: 1 s, 5 s, 30 s. A permanently broken WebView2 is asked
/// four times in 36 s and then left to the user's Retry.
pub const RETRY_DELAYS_S: [f64; 3] = [1.0, 5.0, 30.0];

/// The most pages kept alive at once: beyond this the least-recently-shown page is closed.
pub const MAX_PAGES: usize = 5;

/// The largest pane a page is ever given, in points; anything larger is clamped.
pub const MAX_PANE: f32 = 16_384.0;

/// The display scales the shell accepts as real, inclusive: Windows' per-monitor factors run
/// from 1.0 (100%) through 4.0 (400%); a read outside this — a broken `GetDpiForWindow`, a
/// test override gone wrong — is refused and logged, never applied to a page's bounds.
const SANE_SCALE: std::ops::RangeInclusive<f32> = 0.5..=4.0;

fn is_sane_scale(scale: f32) -> bool {
    scale.is_finite() && SANE_SCALE.contains(&scale)
}

/// The seconds between log lines for the same recurring failure.
const LOG_EVERY_S: f64 = 10.0;

/// One tab's attach failure: what was attempted, how often, and when to try again.
struct Failure {
    /// The URL the attempt used: a retargeted page is a new page and starts over.
    url: String,
    detail: String,
    attempts: u32,
    /// Engine clock (seconds) before which no automatic attempt happens.
    next_at: f64,
}

/// What the host was last asked, per tab, so a frame only asks for what changed.
#[derive(Default)]
struct Session {
    bounds: HashMap<String, PaneRect>,
    visible: Option<String>,
    failures: HashMap<String, Failure>,
    /// Recency for the page cap: a counter bumped on every show.
    shown_at: HashMap<String, u64>,
    shown_seq: u64,
    /// The tab and surface the previous `sync` saw: a switch is a reason to retry a failed page
    /// without waiting out its backoff.
    last_active: Option<String>,
    last_hosted: bool,
}

struct Engine {
    host: Box<dyn WebviewHost>,
    session: Session,
    /// Seconds from Rune's frame clock (`AppSpec::poll`), used for retry backoff and logging.
    now: f64,
    /// The scale the host was built with; a change re-applies every page's bounds.
    scale: f32,
    /// Set by `close_all`: nothing may attach into a window that is going.
    closed: bool,
    /// The last log line and when it was printed, so a failure every frame logs once in a while.
    last_log: Option<(String, f64)>,
    /// The last snooze duration broadcast to attached pages (`__stationSnoozeUpdate`).
    last_snooze: Option<u64>,
}

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
    /// Whether `close_all` ran: a frame after it must not bring a page back.
    static CLOSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The Rune window's raw handle, read in `App::on_window`; the host is built from it on the
    /// first drawn frame, where the window's scale is known too.
    #[cfg(target_os = "windows")]
    static PARENT: RefCell<Option<rune::raw_window_handle::RawWindowHandle>> = const { RefCell::new(None) };
}

/// Installs `host` for this thread, forgetting any host before it. Called once per window; the
/// previous host's pages are dropped with it.
pub fn install(host: Box<dyn WebviewHost>) {
    CLOSED.with(|closed| closed.set(false));
    ENGINE.with(|engine| {
        *engine.borrow_mut() = Some(Engine {
            host,
            session: Session::default(),
            now: 0.0,
            scale: 0.0,
            closed: false,
            last_log: None,
            last_snooze: None,
        });
    });
}

/// Records this window's native handle (`App::on_window`), so the platform host can parent its
/// pages to it, and gives the window Station's icon (taskbar and title bar) — the one place a
/// raw handle exists, like the web view's parenting. On platforms without a host this installs
/// the one that reports [`WebviewError::Unsupported`].
pub fn window_created(handle: &rune::WindowHandle) {
    #[cfg(target_os = "windows")]
    {
        match handle.with_raw_handle(|handle| handle.as_raw()) {
            Ok(raw) => {
                PARENT.with(|parent| *parent.borrow_mut() = Some(raw));
                crate::platform_icon::apply(raw);
            }
            Err(why) => eprintln!("station-shell: window has no native handle: {why}"),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = handle;
        install(Box::new(platform::WryWebview::unsupported()));
    }
}

/// The box the app's page fills, as Rune presented it last frame, or `None` while the pane is
/// not drawn (no content element, or nothing of it visible).
pub fn pane_rect(rt: &Runtime<ShellState>) -> Option<PaneRect> {
    let ([x, y, w, h], opacity) = rt.presented(CONTENT_KEY)?;
    (w > 0.0 && h > 0.0 && opacity > 0.0).then_some(PaneRect::new(x, y, w, h))
}

/// Runs after each drawn frame (`App::on_frame_drawn`): makes the pages match the shell — the
/// active tab attached, shown and on the pane's rectangle; other tabs hidden.
pub fn after_frame(rt: &Runtime<ShellState>) {
    #[cfg(target_os = "windows")]
    platform::install_for_window(rt);
    sync(&rt.state, pane_rect(rt), rt.env.scale);
}

/// Rune's setup hook: keeps the wake handle a routed request uses to reach the next frame, and
/// starts the icons worker on the same waker (a fetched icon wakes the next frame itself).
pub fn setup(state: &mut ShellState, waker: rune::Waker) {
    crate::routing::remember_waker(waker.clone());
    crate::downloads::remember_waker(waker.clone());
    state.start_icons(waker);
}

/// Rune's poll hook: stamps the engine's clock, surfaces a failed page to the view, routes the
/// new-window requests pages queued, and flushes a dirty session at its coalesced pace. Returns
/// whether the shell's state changed, so Rune rebuilds the view.
pub fn poll(state: &mut ShellState, clock: f64, _tx: &mut rune::Tx) -> bool {
    poll_at(state, clock)
}

/// `poll` without Rune's transaction: the seam tests drive frames and time themselves.
pub fn poll_at(state: &mut ShellState, clock: f64) -> bool {
    tick(clock);
    publish_tabs(state);
    adopt_live_scale();
    let mut changed = surface_page_error(state);
    changed |= route_requests(state, clock);
    // A link a second launch handed over arrives the same way: the listener thread only
    // validated and queued it; this poll applies the deep-link decision on the UI thread and
    // brings the window forward when anything arrived.
    let links = crate::instance::drain(state);
    if links.arrived > 0 {
        focus_window();
    }
    changed |= links.changed;
    // A page's notification messages (`bxApi.notificationCenter`), a finished snooze, OS DND
    // state for `Snooze::SyncWithOs`, download lifecycle notices, and freshly fetched icons are
    // frame work.
    changed |= crate::notifications::poll(state, clock);
    sync_snooze(state.effective_snooze_duration_ms());
    changed |= state.apply_download_events(crate::downloads::drain_events());
    changed |= state.poll_oauth();
    changed |= state.poll_updates();
    state.prefetch_icons();
    changed |= state.drain_icons();
    changed
}

/// Compares the window's live scale ([`platform::live_scale`], the frame poll's read of the
/// real DPI) with the engine's, and on a sane change does what a scale change asks: the host
/// learns it (`set_scale`) and every attached page's bounds are re-applied — points are
/// converted with the new scale inside the host, so the child window lands pixel-correct.
/// Rune's `Env::scale` stays frozen at the windowed app's build-time 1.0, so without this a
/// monitor change would leave every page at its old scale. `sync` consults the same live read,
/// so the two cannot fight over the engine's scale within one frame.
fn adopt_live_scale() {
    let Some(live) = platform::live_scale() else {
        return;
    };
    if !is_sane_scale(live) {
        ENGINE.with(|engine| {
            if let Ok(mut engine) = engine.try_borrow_mut() {
                if let Some(engine) = engine.as_mut() {
                    log_throttled(
                        engine,
                        &format!("implausible display scale {live}, ignored"),
                    );
                }
            }
        });
        return;
    }
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Some(engine) = engine.as_mut() else {
            return;
        };
        if (engine.scale - live).abs() <= 0.001 {
            return;
        }
        if engine.scale == 0.0 {
            // Nothing is placed yet (`sync` adopts the same way): take it without host calls.
            engine.scale = live;
            return;
        }
        engine.scale = live;
        engine.host.set_scale(live);
        for (tab, bounds) in engine.session.bounds.clone() {
            let _ = engine.host.set_bounds(&tab, bounds);
        }
    });
}

/// The rail's tabs as the IPC thread may read them: published on every frame by `poll_at`, so
/// `attach`'s ipc handler can tell which tab id sent a message without borrowing the
/// `ShellState` (which is not `Sync`). `sync`'s attaches read the same snapshot after it.
fn publish_tabs(state: &ShellState) {
    let tabs: Vec<crate::state::AppTab> = state.tabs().to_vec();
    TAB_SNAPSHOT.with(|slot| *slot.borrow_mut() = Some(std::rc::Rc::new(tabs)));
}

/// The most recent tab list, for the web-view thread. Empty before the first frame.
fn tab_snapshot() -> std::rc::Rc<Vec<crate::state::AppTab>> {
    TAB_SNAPSHOT
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| std::rc::Rc::new(Vec::new()))
}

thread_local! {
    /// The last polled rail tabs, shared with the attach path's ipc handler on this thread.
    static TAB_SNAPSHOT: RefCell<Option<std::rc::Rc<Vec<crate::state::AppTab>>>> =
        const { RefCell::new(None) };
}

/// Sets the clock retry backoff and log throttling read (seconds since the app started).
pub fn tick(now: f64) {
    ENGINE.with(|engine| {
        if let Ok(mut engine) = engine.try_borrow_mut() {
            if let Some(engine) = engine.as_mut() {
                engine.now = now;
            }
        }
    });
}

/// The tests' stand-in for the OS's live DPI read: `Some(scale)` makes the window report that
/// scale, `None` restores the real read. Lives in `platform` next to [`platform::live_scale`].
#[cfg(test)]
pub(crate) fn set_test_live_scale(scale: Option<f32>) {
    platform::set_live_scale(scale);
}

/// Copies the active page's failure into `ShellState`, or clears a stale one. True when the
/// view-visible error changed.
pub fn surface_page_error(state: &mut ShellState) -> bool {
    let error = ENGINE.with(|engine| {
        let engine = engine.borrow();
        let engine = engine.as_ref()?;
        let active = state.active();
        let failure = engine.session.failures.get(&active.id)?;
        Some(PageError {
            application: active.label.clone(),
            detail: failure.detail.clone(),
        })
    });
    match error {
        Some(error) => state.set_page_error(error),
        None => state.clear_page_error(),
    }
}

/// Retries a failed page now, however much backoff was left: the view's Retry control.
pub fn retry(tab_id: &str) {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Some(engine) = engine.as_mut() else {
            return;
        };
        engine.session.failures.remove(tab_id);
        engine.last_log = None;
    });
}

/// Carries out every queued request: select the tab that answers to the URL, load it there, or
/// hand it to the OS browser (within its rolling budget). `now` is the frame clock, in seconds.
/// True when the selection moved.
pub fn route_requests(state: &mut ShellState, now: f64) -> bool {
    // The queue and the rolling budget are process state, and any test's frame poll drains
    // them: serialized so a test that enqueued cannot have its request routed (and, for an
    // external URL, actually opened) by another test's poll.
    #[cfg(test)]
    let _serial = crate::test_support::lock();
    let requests = crate::routing::take();
    if requests.is_empty() {
        return false;
    }
    let apps = state.routing_apps();
    let tabs = state.routing_tabs();
    let mut changed = false;
    let mut externals = 0;
    let mut opened: HashSet<String> = HashSet::new();
    for request in requests {
        match crate::routing::decide(&request.url, &apps, &tabs, Some(&request.origin_tab)) {
            crate::routing::Routed::Ignore => {}
            crate::routing::Routed::External => {
                // A page can queue a flood of windows: cap the browsers one frame opens and open
                // each distinct URL once.
                if externals >= crate::routing::MAX_EXTERNAL_PER_FRAME
                    || !opened.insert(request.url.clone())
                {
                    continue;
                }
                externals += 1;
                // The per-frame cap is not enough: a page loops across frames. The rolling budget
                // is what bounds browser launches per minute.
                if crate::routing::budget_allows(now) {
                    crate::routing::open_external(&request.url);
                }
            }
            crate::routing::Routed::Focus { tab_id } => {
                changed |= state.select(&tab_id);
            }
            crate::routing::Routed::FocusAndLoad { tab_id, url }
            | crate::routing::Routed::Load { tab_id, url } => {
                // The tab's own URL is what a page not yet open attaches with; an open page is
                // navigated directly.
                state.retarget(&tab_id, &url);
                changed |= state.select(&tab_id);
                // A page that is not open yet attaches with the retargeted URL on the next frame.
                if is_attached(&tab_id) {
                    load_in(&tab_id, &url);
                }
            }
        }
    }
    changed
}

/// Whether the host holds a page for `tab_id`.
fn is_attached(tab_id: &str) -> bool {
    ENGINE.with(|engine| {
        engine
            .borrow()
            .as_ref()
            .is_some_and(|engine| engine.session.bounds.contains_key(tab_id))
    })
}

/// Navigates an already-attached page to `url` — the seam a configuration change uses: the tab's
/// own URL is what a page not yet open attaches with (`sync`), an open page follows now. False
/// when nothing is attached (no host, or the tab's page was evicted).
pub fn load_if_attached(tab_id: &str, url: &str) -> bool {
    if !is_attached(tab_id) {
        return false;
    }
    load_in(tab_id, url);
    true
}

/// Navigates one attached page, if the host has it (one that is not open yet is attached with the
/// retargeted URL by the next frame's `sync`).
fn load_in(tab_id: &str, url: &str) {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Some(Engine { host, .. }) = engine.as_mut() else {
            return;
        };
        if let Err(why) = host.load(tab_id, url) {
            eprintln!("station-shell: {why}");
        }
    });
}

/// The script one click message evaluates: the injected script's dispatch hook called with the
/// notification id as a **JSON string literal** (`serde_json::to_string`), never interpolated
/// raw. A page controls the id, so quotes, backslashes, newlines and `</script>` stay data
/// inside the literal and cannot end the expression. `None` means the id could not be encoded
/// and the message must be refused.
pub(crate) fn click_script(notification_id: &str) -> Option<String> {
    let encoded = serde_json::to_string(notification_id).ok()?;
    Some(format!("window.__stationNotificationClick({encoded});"))
}

/// The script that pushes a snooze duration change to a page's registered snooze listeners.
pub(crate) fn snooze_script(duration_ms: Option<u64>) -> String {
    match duration_ms {
        Some(ms) => format!("window.__stationSnoozeUpdate({ms});"),
        None => "window.__stationSnoozeUpdate(null);".to_owned(),
    }
}

/// Broadcasts `duration_ms` to every attached page whenever the effective snooze state changes.
pub fn sync_snooze(duration_ms: Option<u64>) {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Some(engine) = engine.as_mut() else {
            return;
        };
        if engine.closed || engine.last_snooze == duration_ms {
            return;
        }
        engine.last_snooze = duration_ms;
        let script = snooze_script(duration_ms);
        let mut tabs: Vec<String> = engine.session.bounds.keys().cloned().collect();
        tabs.sort();
        for tab in tabs {
            let _ = engine.host.eval_script(&tab, &script);
        }
    });
}

/// Evaluates a shell-generated script inside `tab_id` if it is attached.
pub fn eval_in_tab(tab_id: &str, script: &str) {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Some(engine) = engine.as_mut() else {
            return;
        };
        if engine.closed || !engine.session.bounds.contains_key(tab_id) {
            return;
        }
        let _ = engine.host.eval_script(tab_id, script);
    });
}

/// Reloads an attached web tab in place.
pub fn reload(tab_id: &str) {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Some(engine) = engine.as_mut() else {
            return;
        };
        if engine.closed || !engine.session.bounds.contains_key(tab_id) {
            return;
        }
        let _ = engine.host.reload(tab_id);
    });
}

/// Navigates back in the attached page's history.
pub fn go_back(tab_id: &str) {
    eval_in_tab(tab_id, "window.history.back();");
}

/// Navigates forward in the attached page's history.
pub fn go_forward(tab_id: &str) {
    eval_in_tab(tab_id, "window.history.forward();");
}

/// Sets the zoom factor (`0.5..=3.0`) on an attached page.
pub fn set_zoom(tab_id: &str, factor: f64) {
    let clamped = if factor.is_finite() {
        factor.clamp(0.5, 3.0)
    } else {
        1.0
    };
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Some(engine) = engine.as_mut() else {
            return;
        };
        if engine.closed || !engine.session.bounds.contains_key(tab_id) {
            return;
        }
        let _ = engine.host.set_zoom(tab_id, clamped);
    });
}

/// Delivers one notification click to the page behind `tab_id` — `sagaNotificationClick`'s
/// `triggerClick`, asked by the view action after `ShellState` marked the notification read and
/// routed the tab (D5: state never holds the host). An unattached page, a missing host and a
/// failed evaluation all drop the message silently: one click per press, never a retry.
pub fn click(tab_id: &str, notification_id: &str) {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Some(engine) = engine.as_mut() else {
            return;
        };
        if engine.closed {
            return;
        }
        let _ = engine.host.click(tab_id, notification_id);
    });
}

/// Makes the host match `state`: `rect` holds the active tab's page; every other attached page
/// is hidden. A tab Rune draws itself (the App Store), or a Rune surface over the pane (Settings,
/// the notification panel), gets no page on screen — the visible page is hidden like any other
/// tab's. Attaching, bounding and showing are each asked for once per change, so a steady window
/// asks the host for nothing.
pub fn sync(state: &ShellState, rect: Option<PaneRect>, scale: f32) {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Some(engine) = engine.as_mut() else {
            return;
        };
        if engine.closed {
            return;
        }
        // A display-scale change (another monitor, a DPI change) re-applies every page's bounds:
        // points are converted to pixels with the host's scale. The live DPI read is the
        // authority when the OS reports one — `adopt_live_scale` (frame poll) uses it too, so
        // this branch never reverts the poll's adoption within the same frame; `scale` (Rune's
        // `Env::scale`, fixed at build time in the windowed app) is the fallback tests drive.
        let target = platform::live_scale()
            .filter(|live| is_sane_scale(*live))
            .unwrap_or(scale);
        if target.is_finite() && target > 0.0 {
            if engine.scale == 0.0 {
                // The host was built with this window's scale: adopt it without asking it to
                // re-place pages that do not exist yet.
                engine.scale = target;
            } else if (engine.scale - target).abs() > 0.001 {
                engine.scale = target;
                engine.host.set_scale(target);
                for (tab, bounds) in engine.session.bounds.clone() {
                    let _ = engine.host.set_bounds(&tab, bounds);
                }
            }
        }
        let rect = match rect.and_then(PaneRect::clamped) {
            Some(rect) => rect,
            // No pane to sit on (a collapsed layout, a non-finite rect): the page must not float
            // over whatever is there.
            None => {
                hide_visible(engine);
                return;
            }
        };
        let active = state.active();
        let hosted = state.shows_page();
        let switched = engine.session.last_active.as_deref() != Some(active.id.as_str())
            || !engine.session.last_hosted;
        engine.session.last_active = Some(active.id.clone());
        engine.session.last_hosted = hosted;
        if !hosted {
            hide_visible(engine);
            return;
        }
        // A retargeted page is a different page: its failure no longer applies.
        if engine
            .session
            .failures
            .get(&active.id)
            .is_some_and(|failure| failure.url != active.url)
        {
            engine.session.failures.remove(&active.id);
        }
        if engine.session.visible.as_deref() == Some(active.id.as_str()) {
            update_bounds(engine, &active.id, rect);
            return;
        }
        // The tab (or the surface) changed: the old page goes off screen before the new one.
        hide_visible(engine);
        if engine.session.bounds.contains_key(&active.id) {
            show(engine, &active.id);
            update_bounds(engine, &active.id, rect);
            return;
        }
        // A tab that failed waits out its backoff — except when the user has just switched to
        // it (a rail click, Settings closing, the panel closing), which asks again at once. A
        // permanently broken host is left alone until the user asks through Retry.
        if let Some(failure) = engine.session.failures.get(&active.id) {
            if failure.attempts >= MAX_ATTEMPTS || (!switched && engine.now < failure.next_at) {
                return;
            }
        }
        evict_for_capacity(engine, &active.id);
        let environment = PageEnvironment::for_tab(active);
        match engine
            .host
            .attach(&active.id, &active.url, &environment, rect)
        {
            Ok(()) => {
                engine.session.bounds.insert(active.id.clone(), rect);
                engine.session.failures.remove(&active.id);
                engine.last_log = None;
                if engine.last_snooze.is_some() {
                    let script = snooze_script(engine.last_snooze);
                    let _ = engine.host.eval_script(&active.id, &script);
                }
                show(engine, &active.id);
            }
            Err(why) => {
                let attempts = engine
                    .session
                    .failures
                    .get(&active.id)
                    .map_or(1, |failure| failure.attempts + 1);
                let delay = RETRY_DELAYS_S
                    .get(attempts.saturating_sub(1) as usize)
                    .copied()
                    .unwrap_or(0.0);
                engine.session.failures.insert(
                    active.id.clone(),
                    Failure {
                        url: active.url.clone(),
                        detail: why.to_string(),
                        attempts,
                        next_at: engine.now + delay,
                    },
                );
                // The page is off screen either way; the view shows it through `page_error`.
                engine.session.visible = None;
                let message = format!("no web view for {}: {why}", active.id);
                log_throttled(engine, &message);
            }
        }
    });
}

/// Hides the page the host is showing, if any.
fn hide_visible(engine: &mut Engine) {
    if let Some(previous) = engine.session.visible.take() {
        engine.host.set_visible(&previous, false);
    }
}

/// Shows `tab` and stamps it as the most recently shown page.
fn show(engine: &mut Engine, tab: &str) {
    engine.host.set_visible(tab, true);
    engine.session.visible = Some(tab.to_owned());
    engine.session.shown_seq += 1;
    let seq = engine.session.shown_seq;
    engine.session.shown_at.insert(tab.to_owned(), seq);
}

/// Keeps the host's rectangle in step with the pane.
fn update_bounds(engine: &mut Engine, tab: &str, rect: PaneRect) {
    if engine.session.bounds.get(tab) != Some(&rect) && engine.host.set_bounds(tab, rect).is_ok() {
        engine.session.bounds.insert(tab.to_owned(), rect);
    }
}

/// Closes the least-recently-shown page (never `keep`) while the cap is reached, so the page
/// about to attach fits.
fn evict_for_capacity(engine: &mut Engine, keep: &str) {
    while engine.session.bounds.len() >= MAX_PAGES {
        let victim = engine
            .session
            .bounds
            .keys()
            .filter(|tab| tab.as_str() != keep)
            .min_by_key(|tab| engine.session.shown_at.get(*tab).copied().unwrap_or(0))
            .cloned();
        let Some(victim) = victim else {
            return;
        };
        engine.session.bounds.remove(&victim);
        engine.session.failures.remove(&victim);
        engine.session.shown_at.remove(&victim);
        if engine.session.visible.as_deref() == Some(victim.as_str()) {
            engine.session.visible = None;
        }
        engine.host.detach(&victim);
    }
}

/// Prints a recurring failure once, then at most every [`LOG_EVERY_S`].
fn log_throttled(engine: &mut Engine, message: &str) {
    let should_log = match &engine.last_log {
        Some((last, at)) => last != message || engine.now - at >= LOG_EVERY_S,
        None => true,
    };
    if should_log {
        eprintln!("station-shell: {message}");
        engine.last_log = Some((message.to_owned(), engine.now));
    }
}

/// Closes one tab's page: the tab is gone (an uninstall), so its web view goes with it. The rest
/// of the shell is untouched; another tab's page keeps its bounds and visibility.
pub fn close_tab(tab_id: &str) {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Some(engine) = engine.as_mut() else {
            return;
        };
        let session = &mut engine.session;
        if session.visible.as_deref() == Some(tab_id) {
            session.visible = None;
        }
        session.bounds.remove(tab_id);
        session.failures.remove(tab_id);
        session.shown_at.remove(tab_id);
        engine.host.detach(tab_id);
    });
}

/// Hides whichever page is currently visible without detaching any page or closing the host.
/// Used when closing the main window hides Station to the Windows system tray (`minimize_to_tray`),
/// so pages retain their DOM/session and reappear on the next drawn frame after reopening.
pub fn hide_all() {
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        if let Some(engine) = engine.as_mut() {
            hide_visible(engine);
        }
    });
}

/// Drops every page: the window is closing or the app quitting, and each web view must go before
/// the window it is parented to. On `App::on_close` and `App::on_quit`. The host goes with them,
/// so a frame still in flight cannot attach a page to a window that is gone.
pub fn close_all() {
    CLOSED.with(|closed| closed.set(true));
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        if let Some(engine) = engine.as_mut() {
            engine.closed = true;
            for tab in engine.session.bounds.keys() {
                engine.host.detach(tab);
            }
        }
        // Dropping the host closes whatever it still held; a second call is a no-op.
        *engine = None;
    });
}

/// The live Rune window's Win32 `HWND`, if `window_created` has run on this thread.
#[cfg(target_os = "windows")]
pub(crate) fn parent_hwnd() -> Option<windows_sys::Win32::Foundation::HWND> {
    PARENT.with(|slot| match *slot.borrow() {
        Some(rune::raw_window_handle::RawWindowHandle::Win32(handle)) => {
            Some(platform::hwnd_from_isize(handle.hwnd.get()))
        }
        _ => None,
    })
}

/// One page's platform implementation.
mod platform {
    use super::*;

    /// The window's scale as the OS reports it right now, or `None` when there is nothing to
    /// read (no window yet, another platform). Windows: `GetDpiForWindow` on the parent HWND
    /// stored by `window_created`, divided by the 96-DPI baseline. This is the authority the
    /// poll consults each frame, because `Runtime::env.scale` is fixed when the windowed app
    /// is built (1.0) and never follows a monitor change.
    pub(crate) fn live_scale() -> Option<f32> {
        #[cfg(test)]
        // A test's "the window is now 1.5×": the override stands in for the OS read, so the
        // detection and re-scaling seam is provable headlessly on every platform.
        if let Some(overridden) = TEST_LIVE_SCALE.with(|slot| *slot.borrow()) {
            return Some(overridden);
        }
        #[cfg(target_os = "windows")]
        if let Some(rune::raw_window_handle::RawWindowHandle::Win32(handle)) =
            PARENT.with(|parent| *parent.borrow())
        {
            // SAFETY: `hwnd` is the live Rune window's handle, read in `window_created` and
            // kept for exactly these reads while the window is open on this thread;
            // `GetDpiForWindow` only reads it.
            let dpi = unsafe {
                windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd_from_isize(handle.hwnd.get()))
            };
            if dpi > 0 {
                return Some(dpi as f32 / 96.0);
            }
        }
        None
    }

    /// The test-only stand-in for the OS read: `Some(scale)` forces [`live_scale`], `None`
    /// restores it.
    #[cfg(test)]
    pub(crate) fn set_live_scale(scale: Option<f32>) {
        TEST_LIVE_SCALE.with(|slot| *slot.borrow_mut() = scale);
    }

    #[cfg(test)]
    thread_local! {
        static TEST_LIVE_SCALE: RefCell<Option<f32>> = const { RefCell::new(None) };
    }

    /// This Rune's raw-window-handle carries `hwnd` as `NonZero<isize>`; windows-sys 0.59 takes
    /// `HWND = *mut c_void`.
    #[cfg(target_os = "windows")]
    pub(crate) fn hwnd_from_isize(hwnd: isize) -> windows_sys::Win32::Foundation::HWND {
        hwnd as windows_sys::Win32::Foundation::HWND
    }

    /// Brings the Rune window forward when a second launch hands something over. `SetForegroundWindow`
    /// alone can refuse a background process (the foreground lock), so a minimized window is
    /// restored first (`ShowWindow`) and, when another thread holds the foreground, the switch
    /// is attempted through its input queue (`AttachThreadInput`) — the documented workaround,
    /// still best-effort: Windows may refuse any background process, and that refusal is logged
    /// once and never fatal, because the deep link is acted on regardless. Runs on the window's
    /// own thread (the frame poll), which is where focus switches belong.
    #[cfg(target_os = "windows")]
    pub(crate) fn bring_to_front(hwnd: windows_sys::Win32::Foundation::HWND) {
        use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
        };
        // SAFETY: `hwnd` is the live Rune window's handle (`window_created`); read-only calls
        // plus one show/restore/foreground switch on it, on the window's own thread.
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                ShowWindow(hwnd, SW_SHOW);
            }
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            let foreground = GetForegroundWindow();
            if foreground.is_null() || foreground == hwnd {
                if SetForegroundWindow(hwnd) == 0 {
                    eprintln!("station-shell: could not bring the window forward");
                }
                return;
            }
            let this_thread = GetCurrentThreadId();
            let other_thread = GetWindowThreadProcessId(foreground, std::ptr::null_mut());
            let attached = AttachThreadInput(other_thread, this_thread, 1) != 0;
            let switched = SetForegroundWindow(hwnd) != 0;
            if attached {
                AttachThreadInput(other_thread, this_thread, 0);
            }
            if !switched {
                eprintln!("station-shell: could not bring the window forward");
            }
        }
    }

    /// A WebView2 child window per tab, parented to the Rune window.
    #[cfg(target_os = "windows")]
    pub struct WryWebview {
        views: HashMap<String, wry::WebView>,
        /// One `WebContext` (and so one WebView2 user-data folder) per profile key. The contexts
        /// outlive the views built from them, as wry's own `WebContext` warning asks.
        contexts: HashMap<String, wry::WebContext>,
        parent: ParentWindow,
        scale: f32,
    }

    #[cfg(target_os = "windows")]
    impl WryWebview {
        pub fn new(parent: rune::raw_window_handle::RawWindowHandle, scale: f32) -> Self {
            Self {
                views: HashMap::new(),
                contexts: HashMap::new(),
                parent: ParentWindow(parent),
                scale,
            }
        }

        fn rect(&self, rect: PaneRect) -> wry::Rect {
            let pixels = rect.pixels(self.scale);
            wry::Rect {
                position: wry::dpi::PhysicalPosition::new(pixels.x, pixels.y).into(),
                size: wry::dpi::PhysicalSize::new(pixels.w, pixels.h).into(),
            }
        }
    }

    /// The window handle wry borrows to parent its child web view: `on_window` read it from the
    /// live Rune window, and wry reads it only for the duration of the call that borrows it.
    #[cfg(target_os = "windows")]
    struct ParentWindow(rune::raw_window_handle::RawWindowHandle);

    #[cfg(target_os = "windows")]
    impl rune::raw_window_handle::HasWindowHandle for ParentWindow {
        fn window_handle(
            &self,
        ) -> Result<rune::raw_window_handle::WindowHandle<'_>, rune::raw_window_handle::HandleError>
        {
            // SAFETY: the raw handle came from `WindowHandle::with_raw_handle` for the window
            // that is still open on this thread; wry uses the borrow only inside
            // `build_as_child`, and nothing retains it.
            Ok(unsafe { rune::raw_window_handle::WindowHandle::borrow_raw(self.0) })
        }
    }

    #[cfg(target_os = "windows")]
    impl WebviewHost for WryWebview {
        fn attach(
            &mut self,
            tab_id: &str,
            url: &str,
            environment: &PageEnvironment,
            rect: PaneRect,
        ) -> Result<(), WebviewError> {
            if self.views.contains_key(tab_id) {
                return Ok(());
            }
            let origin = tab_id.to_owned();
            // The frame's poll publishes which tabs exist (the IPC thread cannot borrow the
            // `ShellState`, which is not `Sync`): a snapshot the handler can consult.
            let tabs_at_attach = tab_snapshot();
            let ipc_origin = origin.clone();
            let bounds = self.rect(rect);
            let profile_key = environment.profile.directory_name();
            let directory = profile_dir(&profile_root(), &environment.profile);
            let context = self
                .contexts
                .entry(profile_key)
                .or_insert_with(|| wry::WebContext::new(Some(directory)));
            let view = wry::WebViewBuilder::new_with_web_context(context)
                // Electron's session UA (or the manifest's `bx_override_user_agent`). Fixed for
                // this web view; nothing a page sends can change it.
                .with_user_agent(environment.user_agent.clone())
                .with_url(url)
                // The page's core bridge plus optional per-site script selected by the manifest's
                // `bx_legacy_service_id` (`site_scripts::combined_init_script`). Messages come back
                // through wry's ipc (`window.ipc.postMessage`) with the tab as origin.
                .with_initialization_script(crate::site_scripts::combined_init_script(
                    environment.legacy_service_id.as_deref(),
                ))
                .with_ipc_handler(move |request| {
                    // `request.body()` is the page's string. The bridge owns every limit, drops
                    // what a source at its cap sends, and never panics on page input.
                    crate::notifications::receive(&ipc_origin, &tabs_at_attach, request.body());
                })
                // A page's `window.open`/`target=_blank` never becomes an OS window: queue it for
                // the next frame to route (to one of our tabs, or the default browser).
                .with_new_window_req_handler(move |url, _features| {
                    crate::routing::enqueue(&origin, &url);
                    wry::NewWindowResponse::Deny
                })
                // Downloads are allowed, like the browser WebView2 imitates; the shell keeps a
                // bounded line per start/finish (no UI this slice).
                //
                // Slice 22: the suggested name is page-controlled, so `downloads::accept` is the
                // trust boundary — it sanitizes the name, picks a free target under the OS
                // Downloads folder (never overwriting, never trusting a page path), caps the
                // concurrent downloads and reserves the name until the completion below. wry
                // turns `true` into `SetResultFilePath` + `SetHandled(true)` and `false` into
                // `SetCancel(true)` on WebView2's own `ICoreWebView2_4::add_DownloadStarting`
                // event (`wry-0.57.2/src/webview2/mod.rs:879-951`), so no COM is touched here.
                .with_download_started_handler(|url, path| {
                    let accepted = crate::downloads::accept(&url, path);
                    log_download(download_line(DownloadPhase::Started, &url, None));
                    accepted
                })
                .with_download_completed_handler(|url, path, success| {
                    // Releases the name reserved at start; `path` is WebView2's report, matched
                    // with fallbacks (`downloads::finished`).
                    crate::downloads::finished(&url, path.as_deref(), success);
                    log_download(download_line(
                        DownloadPhase::Completed { success },
                        &url,
                        path.as_deref(),
                    ));
                })
                // Least-privilege permission policy (Slice 30): denies WebView2 Notifications
                // (routed via INIT_SCRIPT) and silent/privileged APIs; prompts interactive media.
                .with_permission_handler(crate::permissions::decide_wry)
                .with_bounds(bounds)
                .with_visible(false)
                .with_focused(false)
                .build_as_child(&self.parent)
                .map_err(|why| WebviewError::Failed(format!("WebView2 for {tab_id}: {why}")))?;
            self.views.insert(tab_id.to_owned(), view);
            Ok(())
        }

        fn set_bounds(&mut self, tab_id: &str, rect: PaneRect) -> Result<(), WebviewError> {
            let bounds = self.rect(rect);
            self.views
                .get(tab_id)
                .ok_or_else(|| WebviewError::Failed(format!("no web view for {tab_id}")))?
                .set_bounds(bounds)
                .map_err(|why| WebviewError::Failed(format!("bounds for {tab_id}: {why}")))
        }

        fn load(&mut self, tab_id: &str, url: &str) -> Result<(), WebviewError> {
            self.views
                .get(tab_id)
                .ok_or_else(|| WebviewError::Failed(format!("no web view for {tab_id}")))?
                .load_url(url)
                .map_err(|why| WebviewError::Failed(format!("load {url} in {tab_id}: {why}")))
        }

        fn click(&mut self, tab_id: &str, notification_id: &str) -> Result<(), WebviewError> {
            let view = self
                .views
                .get(tab_id)
                .ok_or_else(|| WebviewError::Failed(format!("no web view for {tab_id}")))?;
            let script = click_script(notification_id).ok_or_else(|| {
                WebviewError::Failed(format!(
                    "notification id {notification_id:?} cannot be encoded"
                ))
            })?;
            view.evaluate_script(&script).map_err(|why| {
                WebviewError::Failed(format!("click {notification_id:?} in {tab_id}: {why}"))
            })
        }

        fn eval_script(&mut self, tab_id: &str, script: &str) -> Result<(), WebviewError> {
            self.views
                .get(tab_id)
                .ok_or_else(|| WebviewError::Failed(format!("no web view for {tab_id}")))?
                .evaluate_script(script)
                .map_err(|why| WebviewError::Failed(format!("eval in {tab_id}: {why}")))
        }

        fn reload(&mut self, tab_id: &str) -> Result<(), WebviewError> {
            self.views
                .get(tab_id)
                .ok_or_else(|| WebviewError::Failed(format!("no web view for {tab_id}")))?
                .reload()
                .map_err(|why| WebviewError::Failed(format!("reload {tab_id}: {why}")))
        }

        fn set_zoom(&mut self, tab_id: &str, factor: f64) -> Result<(), WebviewError> {
            self.views
                .get(tab_id)
                .ok_or_else(|| WebviewError::Failed(format!("no web view for {tab_id}")))?
                .zoom(factor)
                .map_err(|why| WebviewError::Failed(format!("zoom {tab_id}: {why}")))
        }

        fn set_visible(&mut self, tab_id: &str, visible: bool) {
            if let Some(view) = self.views.get(tab_id) {
                if let Err(why) = view.set_visible(visible) {
                    eprintln!("station-shell: visibility for {tab_id}: {why}");
                }
            }
        }

        fn set_scale(&mut self, scale: f32) {
            if scale.is_finite() && scale > 0.0 {
                self.scale = scale;
            }
        }

        fn detach(&mut self, tab_id: &str) {
            self.views.remove(tab_id);
        }
    }

    /// Builds the platform host on the first drawn frame: `on_window` ran before the window's
    /// scale was readable, and `Runtime::env.scale` is what pixel conversion needs.
    #[cfg(target_os = "windows")]
    pub fn install_for_window(rt: &Runtime<ShellState>) {
        if CLOSED.with(|closed| closed.get()) {
            return;
        }
        let installed = ENGINE.with(|engine| engine.borrow().is_some());
        if installed {
            return;
        }
        PARENT.with(|parent| {
            if let Some(raw) = *parent.borrow() {
                install(Box::new(WryWebview::new(raw, rt.env.scale)));
            }
        });
    }

    /// Platforms without a native child web view: every call reports it, so the shell shows its
    /// placeholder pane instead of pretending a page loaded.
    #[cfg(not(target_os = "windows"))]
    pub struct WryWebview;

    #[cfg(not(target_os = "windows"))]
    impl WryWebview {
        pub fn unsupported() -> Self {
            WryWebview
        }
    }

    #[cfg(not(target_os = "windows"))]
    impl WebviewHost for WryWebview {
        fn attach(
            &mut self,
            _: &str,
            _: &str,
            _: &PageEnvironment,
            _: PaneRect,
        ) -> Result<(), WebviewError> {
            Err(WebviewError::Unsupported(
                "a native child web view is implemented on Windows (WebView2) only",
            ))
        }

        fn set_bounds(&mut self, _: &str, _: PaneRect) -> Result<(), WebviewError> {
            Err(WebviewError::Unsupported(
                "a native child web view is implemented on Windows (WebView2) only",
            ))
        }

        fn load(&mut self, _: &str, _: &str) -> Result<(), WebviewError> {
            Err(WebviewError::Unsupported(
                "a native child web view is implemented on Windows (WebView2) only",
            ))
        }

        fn click(&mut self, _: &str, _: &str) -> Result<(), WebviewError> {
            Err(WebviewError::Unsupported(
                "a native child web view is implemented on Windows (WebView2) only",
            ))
        }

        fn eval_script(&mut self, _: &str, _: &str) -> Result<(), WebviewError> {
            Err(WebviewError::Unsupported(
                "a native child web view is implemented on Windows (WebView2) only",
            ))
        }

        fn reload(&mut self, _: &str) -> Result<(), WebviewError> {
            Err(WebviewError::Unsupported(
                "a native child web view is implemented on Windows (WebView2) only",
            ))
        }

        fn set_zoom(&mut self, _: &str, _: f64) -> Result<(), WebviewError> {
            Err(WebviewError::Unsupported(
                "a native child web view is implemented on Windows (WebView2) only",
            ))
        }

        fn set_scale(&mut self, _: f32) {}

        fn set_visible(&mut self, _: &str, _: bool) {}

        fn detach(&mut self, _: &str) {}
    }
}

#[cfg(target_os = "windows")]
thread_local! {
    static APP_HANDLE: RefCell<Option<rune::AppHandle<ShellState>>> = const { RefCell::new(None) };
}

/// Remembers Rune's [`rune::AppHandle`] so single-instance wakes can reopen the window when it
/// has been hidden to the Windows system tray (`keep_running`).
#[cfg(target_os = "windows")]
pub fn remember_app_handle(handle: rune::AppHandle<ShellState>) {
    APP_HANDLE.with(|slot| *slot.borrow_mut() = Some(handle));
}

/// Brings the window forward: the primary's response to anything a second launch handed over
/// (a link, or a focus-only ping — "open Station" with no URL is still the user asking for the
/// window). The raw handle `window_created` kept is the one place this can happen from.
/// Failure is logged once and never fatal: focus is a courtesy, the link is already acted on.
pub fn focus_window() {
    #[cfg(target_os = "windows")]
    {
        APP_HANDLE.with(|slot| {
            if let Some(handle) = slot.borrow().as_ref() {
                handle.open();
                handle.focus();
            }
        });
        PARENT.with(|parent| {
            if let Some(rune::raw_window_handle::RawWindowHandle::Win32(handle)) = *parent.borrow()
            {
                platform::bring_to_front(platform::hwnd_from_isize(handle.hwnd.get()));
            }
        });
    }
    #[cfg(not(target_os = "windows"))]
    {}
}

/// A host that records what it was asked, for headless tests.
#[cfg(test)]
pub mod fake {
    use super::*;
    use std::rc::Rc;

    /// One call the shell made on a host.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Event {
        Attach {
            tab: String,
            url: String,
            rect: PaneRect,
        },
        Bounds {
            tab: String,
            rect: PaneRect,
        },
        Load {
            tab: String,
            url: String,
        },
        /// One notification click delivered to a page (the evaluated script is recorded, so a
        /// test can assert the id was encoded as a JSON string literal).
        Click {
            tab: String,
            script: String,
        },
        /// Any other shell-generated script evaluated in a page (snooze, history, find).
        Eval {
            tab: String,
            script: String,
        },
        Reload {
            tab: String,
        },
        Zoom {
            tab: String,
            factor: f64,
        },
        Scale {
            scale: f32,
        },
        /// An attach the host refused (the shell's retry policy is what the test checks).
        AttachFailed {
            tab: String,
        },
        Visible {
            tab: String,
            visible: bool,
        },
        Detach {
            tab: String,
        },
    }

    /// The UA, profile, and legacy service id an attach was asked for (fixed at build; recorded
    /// beside the events so the event stream existing tests assert stays unchanged).
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct AttachEnvironment {
        pub tab: String,
        pub user_agent: String,
        pub profile: PageProfile,
        pub legacy_service_id: Option<String>,
    }

    /// What a [`FakeWebview`] was asked, shared with the test that installed it.
    #[derive(Clone, Debug, Default)]
    pub struct WebviewLog {
        events: Rc<RefCell<Vec<Event>>>,
        environments: Rc<RefCell<Vec<AttachEnvironment>>>,
    }

    impl WebviewLog {
        pub fn events(&self) -> Vec<Event> {
            self.events.borrow().clone()
        }

        /// The environment each successful attach was given, in attach order.
        pub fn environments(&self) -> Vec<AttachEnvironment> {
            self.environments.borrow().clone()
        }

        pub fn clear(&self) {
            self.events.borrow_mut().clear();
            self.environments.borrow_mut().clear();
        }

        /// The pages the host currently holds: attaches minus detaches.
        pub fn live_pages(&self) -> Vec<String> {
            let mut live: Vec<String> = Vec::new();
            for event in self.events.borrow().iter() {
                match event {
                    Event::Attach { tab, .. } => live.push(tab.clone()),
                    Event::Detach { tab } => live.retain(|page| page != tab),
                    _ => {}
                }
            }
            live
        }

        /// The tabs that were attached, in attach order.
        pub fn attached(&self) -> Vec<String> {
            self.events
                .borrow()
                .iter()
                .filter_map(|event| match event {
                    Event::Attach { tab, .. } => Some(tab.clone()),
                    _ => None,
                })
                .collect()
        }
    }

    pub struct FakeWebview {
        log: WebviewLog,
        attached: std::collections::HashSet<String>,
        refuse_attach: bool,
        /// How many of the first attach attempts fail (0: none).
        fail_first: u32,
        attempts: u32,
    }

    impl FakeWebview {
        pub fn new() -> (Self, WebviewLog) {
            let log = WebviewLog::default();
            (
                Self {
                    log: log.clone(),
                    attached: std::collections::HashSet::new(),
                    refuse_attach: false,
                    fail_first: 0,
                    attempts: 0,
                },
                log,
            )
        }

        /// A host whose every attach fails, as a missing WebView2 runtime would.
        pub fn refusing() -> (Self, WebviewLog) {
            let (fake, log) = Self::new();
            (
                Self {
                    refuse_attach: true,
                    ..fake
                },
                log,
            )
        }

        /// A host that refuses its first `attempts` attaches and then works: the retry path.
        pub fn failing_times(fail_first: u32) -> (Self, WebviewLog) {
            let (fake, log) = Self::new();
            (Self { fail_first, ..fake }, log)
        }
    }

    impl WebviewHost for FakeWebview {
        fn attach(
            &mut self,
            tab_id: &str,
            url: &str,
            environment: &PageEnvironment,
            rect: PaneRect,
        ) -> Result<(), WebviewError> {
            self.attempts += 1;
            if self.refuse_attach || self.attempts <= self.fail_first {
                self.log.events.borrow_mut().push(Event::AttachFailed {
                    tab: tab_id.to_owned(),
                });
                return Err(WebviewError::Failed("no runtime".into()));
            }
            self.attached.insert(tab_id.to_owned());
            self.log.environments.borrow_mut().push(AttachEnvironment {
                tab: tab_id.to_owned(),
                user_agent: environment.user_agent.clone(),
                profile: environment.profile.clone(),
                legacy_service_id: environment.legacy_service_id.clone(),
            });
            self.log.events.borrow_mut().push(Event::Attach {
                tab: tab_id.to_owned(),
                url: url.to_owned(),
                rect,
            });
            Ok(())
        }

        fn set_bounds(&mut self, tab_id: &str, rect: PaneRect) -> Result<(), WebviewError> {
            self.log.events.borrow_mut().push(Event::Bounds {
                tab: tab_id.to_owned(),
                rect,
            });
            Ok(())
        }

        fn set_scale(&mut self, scale: f32) {
            self.log.events.borrow_mut().push(Event::Scale { scale });
        }

        fn load(&mut self, tab_id: &str, url: &str) -> Result<(), WebviewError> {
            if !self.attached.contains(tab_id) {
                // Like the real host: there is no page to navigate.
                return Err(WebviewError::Failed(format!("no web view for {tab_id}")));
            }
            self.log.events.borrow_mut().push(Event::Load {
                tab: tab_id.to_owned(),
                url: url.to_owned(),
            });
            Ok(())
        }

        fn set_visible(&mut self, tab_id: &str, visible: bool) {
            self.log.events.borrow_mut().push(Event::Visible {
                tab: tab_id.to_owned(),
                visible,
            });
        }

        fn click(&mut self, tab_id: &str, notification_id: &str) -> Result<(), WebviewError> {
            if !self.attached.contains(tab_id) {
                // Like the real host: there is no page to tell.
                return Err(WebviewError::Failed(format!("no web view for {tab_id}")));
            }
            let script = click_script(notification_id).ok_or_else(|| {
                WebviewError::Failed(format!(
                    "notification id {notification_id:?} cannot be encoded"
                ))
            })?;
            self.log.events.borrow_mut().push(Event::Click {
                tab: tab_id.to_owned(),
                script,
            });
            Ok(())
        }

        fn eval_script(&mut self, tab_id: &str, script: &str) -> Result<(), WebviewError> {
            if !self.attached.contains(tab_id) {
                return Err(WebviewError::Failed(format!("no web view for {tab_id}")));
            }
            self.log.events.borrow_mut().push(Event::Eval {
                tab: tab_id.to_owned(),
                script: script.to_owned(),
            });
            Ok(())
        }

        fn reload(&mut self, tab_id: &str) -> Result<(), WebviewError> {
            if !self.attached.contains(tab_id) {
                return Err(WebviewError::Failed(format!("no web view for {tab_id}")));
            }
            self.log.events.borrow_mut().push(Event::Reload {
                tab: tab_id.to_owned(),
            });
            Ok(())
        }

        fn set_zoom(&mut self, tab_id: &str, factor: f64) -> Result<(), WebviewError> {
            if !self.attached.contains(tab_id) {
                return Err(WebviewError::Failed(format!("no web view for {tab_id}")));
            }
            self.log.events.borrow_mut().push(Event::Zoom {
                tab: tab_id.to_owned(),
                factor,
            });
            Ok(())
        }

        fn detach(&mut self, tab_id: &str) {
            self.attached.remove(tab_id);
            self.log.events.borrow_mut().push(Event::Detach {
                tab: tab_id.to_owned(),
            });
        }
    }
}
