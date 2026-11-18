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

use crate::state::{PageError, ShellState};
use rune::Runtime;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;

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

/// What the shell asks of whatever renders an app's page.
pub trait WebviewHost {
    /// Create the page for `tab_id` at `url`, filling `rect` (points), hidden.
    fn attach(&mut self, tab_id: &str, url: &str, rect: PaneRect) -> Result<(), WebviewError>;
    /// Move and resize an attached page.
    fn set_bounds(&mut self, tab_id: &str, rect: PaneRect) -> Result<(), WebviewError>;
    /// Navigate an attached page (a routed link that belongs to this tab).
    fn load(&mut self, tab_id: &str, url: &str) -> Result<(), WebviewError>;
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
        });
    });
}

/// Records this window's native handle (`App::on_window`), so the platform host can parent its
/// pages to it. On platforms without a host this installs the one that reports
/// [`WebviewError::Unsupported`].
pub fn window_created(handle: &rune::WindowHandle) {
    #[cfg(target_os = "windows")]
    {
        match handle.with_raw_handle(|handle| handle.as_raw()) {
            Ok(raw) => PARENT.with(|parent| *parent.borrow_mut() = Some(raw)),
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

/// Rune's setup hook: keeps the wake handle a routed request uses to reach the next frame.
pub fn setup(_state: &mut ShellState, waker: rune::Waker) {
    crate::routing::remember_waker(waker);
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
    let mut changed = surface_page_error(state);
    changed |= route_requests(state, clock);
    changed
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
        // points are converted to pixels with the host's scale.
        if scale.is_finite() && scale > 0.0 {
            if engine.scale == 0.0 {
                // The host was built with this window's scale: adopt it without asking it to
                // re-place pages that do not exist yet.
                engine.scale = scale;
            } else if (engine.scale - scale).abs() > 0.001 {
                engine.scale = scale;
                engine.host.set_scale(scale);
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
        match engine.host.attach(&active.id, &active.url, rect) {
            Ok(()) => {
                engine.session.bounds.insert(active.id.clone(), rect);
                engine.session.failures.remove(&active.id);
                engine.last_log = None;
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

/// One page's platform implementation.
mod platform {
    use super::*;

    /// A WebView2 child window per tab, parented to the Rune window.
    #[cfg(target_os = "windows")]
    pub struct WryWebview {
        views: HashMap<String, wry::WebView>,
        parent: ParentWindow,
        scale: f32,
    }

    #[cfg(target_os = "windows")]
    impl WryWebview {
        pub fn new(parent: rune::raw_window_handle::RawWindowHandle, scale: f32) -> Self {
            Self {
                views: HashMap::new(),
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
        fn attach(&mut self, tab_id: &str, url: &str, rect: PaneRect) -> Result<(), WebviewError> {
            if self.views.contains_key(tab_id) {
                return Ok(());
            }
            let origin = tab_id.to_owned();
            let view = wry::WebViewBuilder::new()
                .with_url(url)
                // A page's `window.open`/`target=_blank` never becomes an OS window: queue it for
                // the next frame to route (to one of our tabs, or the default browser).
                .with_new_window_req_handler(move |url, _features| {
                    crate::routing::enqueue(&origin, &url);
                    wry::NewWindowResponse::Deny
                })
                .with_bounds(self.rect(rect))
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
        fn attach(&mut self, _: &str, _: &str, _: PaneRect) -> Result<(), WebviewError> {
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

        fn set_scale(&mut self, _: f32) {}

        fn set_visible(&mut self, _: &str, _: bool) {}

        fn detach(&mut self, _: &str) {}
    }
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

    /// What a [`FakeWebview`] was asked, shared with the test that installed it.
    #[derive(Clone, Debug, Default)]
    pub struct WebviewLog(Rc<RefCell<Vec<Event>>>);

    impl WebviewLog {
        pub fn events(&self) -> Vec<Event> {
            self.0.borrow().clone()
        }

        pub fn clear(&self) {
            self.0.borrow_mut().clear();
        }

        /// The pages the host currently holds: attaches minus detaches.
        pub fn live_pages(&self) -> Vec<String> {
            let mut live: Vec<String> = Vec::new();
            for event in self.0.borrow().iter() {
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
            self.0
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
        fn attach(&mut self, tab_id: &str, url: &str, rect: PaneRect) -> Result<(), WebviewError> {
            self.attempts += 1;
            if self.refuse_attach || self.attempts <= self.fail_first {
                self.log.0.borrow_mut().push(Event::AttachFailed {
                    tab: tab_id.to_owned(),
                });
                return Err(WebviewError::Failed("no runtime".into()));
            }
            self.attached.insert(tab_id.to_owned());
            self.log.0.borrow_mut().push(Event::Attach {
                tab: tab_id.to_owned(),
                url: url.to_owned(),
                rect,
            });
            Ok(())
        }

        fn set_bounds(&mut self, tab_id: &str, rect: PaneRect) -> Result<(), WebviewError> {
            self.log.0.borrow_mut().push(Event::Bounds {
                tab: tab_id.to_owned(),
                rect,
            });
            Ok(())
        }

        fn set_scale(&mut self, scale: f32) {
            self.log.0.borrow_mut().push(Event::Scale { scale });
        }

        fn load(&mut self, tab_id: &str, url: &str) -> Result<(), WebviewError> {
            if !self.attached.contains(tab_id) {
                // Like the real host: there is no page to navigate.
                return Err(WebviewError::Failed(format!("no web view for {tab_id}")));
            }
            self.log.0.borrow_mut().push(Event::Load {
                tab: tab_id.to_owned(),
                url: url.to_owned(),
            });
            Ok(())
        }

        fn set_visible(&mut self, tab_id: &str, visible: bool) {
            self.log.0.borrow_mut().push(Event::Visible {
                tab: tab_id.to_owned(),
                visible,
            });
        }

        fn detach(&mut self, tab_id: &str) {
            self.attached.remove(tab_id);
            self.log.0.borrow_mut().push(Event::Detach {
                tab: tab_id.to_owned(),
            });
        }
    }
}
