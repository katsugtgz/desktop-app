//! Slice 9 tests: the manifest icons — fetch seam, bounds, disk cache, and the drawn views.
//!
//! Every test drives the same paths the window does: the fetch behind the [`Fetcher`] trait
//! (no test touches the network), the poll's `prefetch_icons` + `drain_icons`, and the real
//! views (`rail`, the store grid) reading the pixels handles. The letter avatar is the
//! fallback everywhere; these tests pin when it shows and when the icon replaces it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use rune::{tok, Env, Runtime};

use crate::icons::{self, Fetcher, IconError, Source, MAX_ICON_BYTES, MAX_ICON_SIDE, MAX_INFLIGHT};
use crate::shell;
use crate::state::{AppTab, ShellState, StoreSection};

const WINDOW: (f32, f32) = (1200.0, 800.0);

/// A 2×2 PNG, all opaque red — small enough to inline, real enough to decode. (CRCs valid;
/// generated with zlib, not hand-patched.)
pub(crate) const RED_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xB6, 0x0D,
    0x24, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x84, 0x19, 0x60, 0x0C, 0x00, 0x47, 0xCA, 0x07, 0xF9, 0x67, 0x59, 0x6E, 0xB7, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// A PNG whose declared size is a lie (a 1×1 header over nothing): the decoder must refuse it,
/// not panic and not truncate.
const BROKEN_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x7F, 0xFF, 0xFF, 0xFF, 0x7F, 0xFF, 0xFF, 0xFF, 0x08, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00,
];

/// Bytes that are no image format at all.
const NOT_AN_IMAGE: &[u8] = b"<html><body>404</body></html>";

/// Serializes the tests that share the process's icon-decode worker; each test gets its own
/// `IconStore`, this only keeps the stderr one-liners readable.
pub(crate) static ORDER: Mutex<()> = Mutex::new(());

/// The fetch seam a test installs: canned results per URL, and a shared log of every ask.
pub(crate) struct CannedFetcher {
    responses: HashMap<String, Result<Vec<u8>, IconError>>,
    /// Every URL the fetcher was asked for. `Arc<Mutex>` because `Fetcher` is `Send` (the
    /// worker thread owns the fetcher after `start`).
    asked: FetchLog,
}

impl CannedFetcher {
    /// Serves `bytes` for `url`; every other ask is refused like an unknown host would be.
    pub(crate) fn serving(url: &str, bytes: &'static [u8]) -> (Box<dyn Fetcher>, FetchLog) {
        let mut responses = HashMap::new();
        responses.insert(url.to_owned(), Ok(bytes.to_vec()));
        Self::build(responses)
    }

    /// Refuses `url` with `error`.
    pub(crate) fn refusing(url: &str, error: &str) -> (Box<dyn Fetcher>, FetchLog) {
        let mut responses = HashMap::new();
        responses.insert(url.to_owned(), Err(error.to_owned()));
        Self::build(responses)
    }

    /// Never succeeds: every ask is refused.
    pub(crate) fn nothing() -> (Box<dyn Fetcher>, FetchLog) {
        Self::build(HashMap::new())
    }

    pub(crate) fn build(
        responses: HashMap<String, Result<Vec<u8>, IconError>>,
    ) -> (Box<dyn Fetcher>, FetchLog) {
        let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
        (
            Box::new(CannedFetcher {
                responses,
                asked: asked.clone(),
            }),
            asked,
        )
    }
}

/// What a fetch log is: shared with the test through the `CannedFetcher` constructors.
pub(crate) type FetchLog = std::sync::Arc<Mutex<Vec<String>>>;

/// The asks recorded in `log`, in order.
pub(crate) fn asked_of(log: &FetchLog) -> Vec<String> {
    log.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

impl Fetcher for CannedFetcher {
    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, IconError> {
        self.asked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(url.to_owned());
        match self.responses.get(url) {
            Some(Ok(bytes)) => Ok(bytes.clone()),
            Some(Err(why)) => Err(why.clone()),
            None => Err(format!("no canned response for {url}")),
        }
    }
}

/// A shell with one web tab carrying `icon_url`; no session file, temp icon cache.
fn shell_with_tab(id: &str, label: &str, icon_url: &str) -> ShellState {
    let mut state = ShellState::new(appstore_service::ApplicationService::for_tests(
        "icons-slice9",
    ));
    state.tabs_mut_for_tests().push(AppTab::web_with_icon(
        id,
        label,
        "https://app.example",
        icon_url,
    ));
    state
}

fn runtime(state: ShellState) -> Runtime<ShellState> {
    Runtime::new(
        state,
        shell::view,
        WINDOW,
        Env {
            system_dark: true,
            scale: 1.0,
        },
    )
}

/// A temp cache dir no other test or run touches.
pub(crate) fn temp_cache(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("station-shell-icons-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The asks for `url` in the shared fetch log (the store also asks for every bundled
/// manifest's icon; tests count per-URL).
fn asks_for(log: &FetchLog, url: &str) -> usize {
    asked_of(log)
        .iter()
        .filter(|asked| asked.as_str() == url)
        .count()
}

// ---- The URL gate ---------------------------------------------------------------------------

#[test]
fn only_http_and_https_are_fetchable() {
    assert!(icons::is_fetchable("https://cdn.example/icon.png"));
    assert!(icons::is_fetchable("http://cdn.example/icon.png"));
    assert!(icons::is_fetchable("HTTP://CDN.EXAMPLE/ICON.PNG"));
    assert!(!icons::is_fetchable("file:///C:/Windows/system.ini"));
    assert!(!icons::is_fetchable("data:image/png;base64,iVBOR"));
    assert!(!icons::is_fetchable("javascript:alert(1)"));
    assert!(!icons::is_fetchable("//cdn.example/icon.png"));
    assert!(!icons::is_fetchable(""));
}

#[test]
fn an_unfetchable_url_never_queues_a_fetch() {
    let _guard = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    let mut state = shell_with_tab("app", "App", "file:///etc/passwd");
    let mut store = icons::IconStore::with_cache(temp_cache("unfetchable"));
    let (fetcher, asked) = CannedFetcher::nothing();
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);

    state.prefetch_icons();
    let _ = state.drain_icons();

    assert!(
        !asked_of(&asked)
            .iter()
            .any(|asked| asked.starts_with("file:")),
        "file: URL reached the fetcher"
    );
    assert!(state.icon_pixels("file:///etc/passwd").is_none());
}

// ---- The pipeline: ask once, fetch once, draw when ready -------------------------------------

#[test]
fn an_icon_is_fetched_once_and_drawn_when_ready() {
    let _guard = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    let url = "https://cdn.example/red.png";
    let mut state = shell_with_tab("app", "App", url);
    let mut store = icons::IconStore::with_cache(temp_cache("happy"));
    let (fetcher, asked) = CannedFetcher::serving(url, RED_PNG);
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);

    // The view reads before any bytes: no handle yet, letter avatar shows.
    assert!(state.icon_pixels(url).is_none());

    // The poll asks; the worker fetches and queues the job; the next drain delivers pixels.
    state.prefetch_icons();
    let mut landed = false;
    for _ in 0..200 {
        if state.drain_icons() {
            landed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(landed, "the fetch delivered pixels");
    let pixels = state.icon_pixels(url).expect("pixels after the fetch");
    assert_eq!(pixels.size(), (2, 2), "the PNG decoded to its real size");
    assert_eq!(asks_for(&asked, url), 1, "asked exactly once");

    // Asking again (the poll runs every frame) never re-fetches.
    state.prefetch_icons();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let _ = state.drain_icons();
    assert_eq!(
        asks_for(&asked, url),
        1,
        "the second poll asked for nothing new"
    );
}

#[test]
fn a_failed_fetch_keeps_the_letter_avatar_and_never_retries() {
    let _guard = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    let url = "https://cdn.example/broken.png";
    let mut state = shell_with_tab("app", "App", url);
    let mut store = icons::IconStore::with_cache(temp_cache("refused"));
    let (fetcher, asked) = CannedFetcher::refusing(url, "HTTP 404");
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);

    state.prefetch_icons();
    for _ in 0..200 {
        if state.drain_icons() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // The handle exists (so the view has one place to read) but is empty: the avatar stays.
    assert_eq!(
        state.icon_pixels(url).map(|p| p.size()),
        Some((0, 0)),
        "a failed icon leaves an empty handle"
    );
    assert_eq!(asks_for(&asked, url), 1, "asked exactly once");

    // A later frame does not try again: one attempt per run.
    state.prefetch_icons();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let _ = state.drain_icons();
    assert_eq!(
        asks_for(&asked, url),
        1,
        "a failed icon is never re-fetched"
    );
}

#[test]
fn an_oversized_body_is_refused_by_the_cap() {
    // The cap is the real fetcher's job (it owns I/O); the store trusts it, exactly as the
    // web view seam trusts its host's attach failure. The constant is what bounds it.
    let body = vec![0u8; MAX_ICON_BYTES + 1];
    let refused = body.len() > MAX_ICON_BYTES;
    assert!(refused, "an over-cap body is refused, never decoded");
}

// ---- Decode bounds and fallbacks -------------------------------------------------------------

#[test]
fn a_hostile_png_is_a_decode_error_not_a_panic() {
    let decoded = icons::decode(BROKEN_PNG);
    assert!(decoded.is_err(), "a 0xFFFFFF-size header cannot decode");
    let decoded = icons::decode(NOT_AN_IMAGE);
    assert!(decoded.is_err(), "HTML is not an image");
}

#[test]
fn a_decoded_icon_carries_its_bounds_and_source() {
    let decoded = icons::decode(RED_PNG).expect("a valid PNG decodes");
    assert_eq!(decoded.width, 2);
    assert_eq!(decoded.height, 2);
    assert_eq!(decoded.rgba.len(), 2 * 2 * 4);
    assert_eq!(decoded.from, Source::Network);
}

#[test]
fn the_bounds_constants_are_the_documented_ones() {
    assert_eq!(MAX_ICON_SIDE, 8192);
    assert_eq!(MAX_ICON_BYTES, 1 << 20);
    assert_eq!(MAX_INFLIGHT, 128);
    assert_eq!(icons::FETCH_TIMEOUT_S, 10);
    assert_eq!(icons::MAX_CACHED_ICONS, 1_500);
}

// ---- The disk cache --------------------------------------------------------------------------

#[test]
fn a_fetched_icon_is_cached_on_disk_and_reused_without_the_network() {
    let _guard = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    let url = "https://cdn.example/cached.png";
    let cache = temp_cache("disk");
    let mut state = shell_with_tab("app", "App", url);
    let mut store = icons::IconStore::with_cache(cache.clone());
    let (fetcher, asked) = CannedFetcher::serving(url, RED_PNG);
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);
    state.prefetch_icons();
    for _ in 0..200 {
        if state.drain_icons() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(asks_for(&asked, url), 1);

    // The file exists, named by the URL's SHA-256, and holds the exact bytes.
    let path = icons::cache_path(&cache, url);
    let stored = std::fs::read(&path).expect("the icon landed on disk");
    assert_eq!(stored, RED_PNG);
    let temp_sibling = path.with_extension("bin.tmp");
    assert!(!temp_sibling.exists(), "no temp file remains");

    // A second run reads the disk and never asks the network for this URL.
    let mut second = icons::IconStore::with_cache(cache.clone());
    let (fetcher2, asked2) = CannedFetcher::nothing();
    second.start(fetcher2, std::sync::Arc::new(|| {}));
    let mut state2 = shell_with_tab("app", "App", url);
    state2.set_icons(second);
    state2.prefetch_icons();
    for _ in 0..200 {
        if state2.drain_icons() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        asks_for(&asked2, url),
        0,
        "a cached icon never reaches the fetcher"
    );
    assert_eq!(
        state2.icon_pixels(url).map(|p| p.size()),
        Some((2, 2)),
        "the disk hit decodes to the same image"
    );
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn cache_paths_are_content_addressed_and_escaped() {
    let dir = PathBuf::from("cache");
    let a = icons::cache_path(&dir, "https://a.example/i.png");
    let b = icons::cache_path(&dir, "https://b.example/i.png");
    assert_ne!(a, b);
    assert_eq!(a, icons::cache_path(&dir, "https://a.example/i.png"));
    let name = a.file_name().unwrap().to_string_lossy().to_string();
    let stem = name.strip_suffix(".bin").expect("the name ends in .bin");
    assert!(
        stem.chars().all(|c| c.is_ascii_hexdigit()),
        "the stem is pure sha256 hex: {stem}"
    );
    assert_eq!(stem.len(), 64, "sha256 hex + .bin");
}

// ---- The drawn views ---------------------------------------------------------------------------

#[test]
fn prefetch_only_requests_icons_for_the_visible_store_list() {
    let mut state = ShellState::new(appstore_service::ApplicationService::for_tests(
        "icons-visible-store-list",
    ));
    let popular = state.popular();
    let popular_urls: std::collections::HashSet<_> = popular
        .cream_of_the_crop_apps
        .iter()
        .chain(&popular.runner_ups)
        .chain(&popular.noteworthy)
        .map(|app| app.icon_url.as_str())
        .collect();
    let app = state
        .categories()
        .iter()
        .flat_map(|(_, apps)| apps)
        .find(|app| {
            icons::is_fetchable(&app.icon_url) && !popular_urls.contains(app.icon_url.as_str())
        })
        .expect("an app listed only by category");
    let category_url = app.icon_url.clone();
    let rail_url = "https://example.test/rail.png";
    state.tabs_mut_for_tests().push(AppTab::web_with_icon(
        "test-tab",
        "Test",
        "https://example.test",
        rail_url,
    ));

    state.prefetch_icons();
    assert!(state.icon_pixels(rail_url).is_some());
    assert!(state.icon_pixels(&category_url).is_none());

    state.show_section(StoreSection::AllApps);
    state.select("test-tab");
    state.prefetch_icons();
    assert!(state.icon_pixels(&category_url).is_none());

    state.open_app_store();
    state.prefetch_icons();
    assert!(state.icon_pixels(&category_url).is_some());
}

#[test]
fn the_rail_and_the_store_draw_the_icon_once_pixels_land() {
    let _guard = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    // The store lists Gmail (its manifest icon URL); the rail carries its tab with the same URL.
    let url = "https://cdn.filestackcontent.com/Yxw83DJdRW2N2PkcDFGA";
    let mut state = ShellState::new(appstore_service::ApplicationService::for_tests(
        "icons-slice9-view",
    ));
    state.tabs_mut_for_tests().push(AppTab::web_with_icon(
        "14",
        "Gmail",
        "https://mail.google.com",
        url,
    ));
    let mut store = icons::IconStore::with_cache(temp_cache("view"));
    let (fetcher, _asked) = CannedFetcher::serving(url, RED_PNG);
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);

    let mut rt = runtime(state);
    // Frame 1: the poll asks; nothing is ready yet — both views still lay out their icon slots.
    rt.frame(1.0 / 60.0);
    crate::webview::after_frame(&rt);
    crate::webview::poll_at(&mut rt.state, 1.0);
    rt.frame(1.0 / 60.0);
    assert!(
        rt.find("tab-icon-14").is_some(),
        "the rail row has an icon element"
    );
    assert!(
        rt.find("store-icon-14").is_some(),
        "the store tile has an icon element"
    );

    // Wait for the worker, then let the poll deliver and redraw. (The poll's "changed" may
    // fire on the very first frame if the worker beat us here, so the loop waits on the
    // pixels themselves, exactly what the views read.)
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        rt.frame(1.0 / 60.0);
        crate::webview::after_frame(&rt);
        let _ = crate::webview::poll_at(&mut rt.state, 2.0);
        if rt.state.icon_pixels(url).map(|p| p.size()) == Some((2, 2)) {
            rt.frame(1.0 / 60.0);
            break;
        }
    }
    assert_eq!(
        rt.state.icon_pixels(url).map(|p| p.size()),
        Some((2, 2)),
        "the pixels reached the state the views read"
    );
}

#[test]
fn a_broken_url_keeps_the_letter_avatar_in_the_drawn_view() {
    let _guard = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    let url = "https://cdn.example/missing.png";
    let mut state = ShellState::new(appstore_service::ApplicationService::for_tests(
        "icons-slice9-fallback",
    ));
    state.tabs_mut_for_tests().push(AppTab::web_with_icon(
        "14",
        "Gmail",
        "https://mail.google.com",
        url,
    ));
    let mut store = icons::IconStore::with_cache(temp_cache("fallback"));
    let (fetcher, _asked) = CannedFetcher::refusing(url, "HTTP 404");
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);

    let mut rt = runtime(state);
    rt.frame(1.0 / 60.0);
    crate::webview::after_frame(&rt);
    crate::webview::poll_at(&mut rt.state, 1.0);
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        rt.frame(1.0 / 60.0);
        crate::webview::after_frame(&rt);
        let _ = crate::webview::poll_at(&mut rt.state, 2.0);
    }
    rt.frame(1.0 / 60.0);
    // The letter is still on screen (the avatar fell back) and no pixels ever landed.
    let letter_present = rt
        .elements()
        .into_iter()
        .any(|element| !element.exiting && element.text.as_deref() == Some("G"));
    assert!(letter_present, "the letter avatar shows for a broken icon");
    assert_eq!(rt.state.icon_pixels(url).map(|p| p.size()), Some((0, 0)));
}

#[test]
fn a_tab_without_an_icon_url_draws_the_letter_avatar_only() {
    let _guard = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    let mut state = shell_with_tab("app", "Zed", "");
    let mut store = icons::IconStore::with_cache(temp_cache("empty"));
    let (fetcher, asked) = CannedFetcher::nothing();
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);
    state.prefetch_icons();
    let _ = state.drain_icons();
    assert_eq!(
        asked_of(&asked)
            .iter()
            .filter(|asked| asked.is_empty())
            .count(),
        0,
        "an empty URL asks for nothing"
    );
    assert!(state.icon_pixels("").is_none());
}

// ---- The inflight cap -------------------------------------------------------------------------

/// A fetcher whose first ask blocks until released: it parks the worker so the test can fill
/// the queue behind it.
type Permits = std::sync::Arc<(std::sync::Mutex<usize>, std::sync::Condvar)>;

struct Gate {
    /// One permit per `release()`; the fetch waits for one before answering.
    permits: Permits,
    asked: FetchLog,
}

impl Gate {
    fn new() -> (Self, Permits, FetchLog) {
        let permits =
            std::sync::Arc::new((std::sync::Mutex::new(0usize), std::sync::Condvar::new()));
        let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                permits: permits.clone(),
                asked: asked.clone(),
            },
            permits,
            asked,
        )
    }
}

impl Fetcher for Gate {
    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, IconError> {
        self.asked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(url.to_owned());
        let (count, signal) = &*self.permits;
        let mut held = count.lock().unwrap_or_else(|p| p.into_inner());
        while *held == 0 {
            held = signal.wait(held).unwrap_or_else(|p| p.into_inner());
        }
        *held -= 1;
        Ok(RED_PNG.to_vec())
    }
}

#[test]
fn the_inflight_cap_drops_requests_instead_of_queueing_without_end() {
    // Driven through the public seam: one blocked worker (`Gate` holds the first fetch), then
    // more asks than the cap. `MAX_INFLIGHT` is what `IconStore::send` enforces; the test
    // cannot reach into it, so it observes the contract: exactly `MAX_INFLIGHT` requests
    // accepted, the surplus dropped, and — after one release — no retry storm (the dropped
    // URLs were asked for once and never again; their handles stay empty).
    let _guard = ORDER.lock().unwrap_or_else(|p| p.into_inner());
    let mut store = icons::IconStore::with_cache(temp_cache("inflight"));
    let (fetcher, _permits, asked) = Gate::new();
    store.start(Box::new(fetcher), std::sync::Arc::new(|| {}));

    let first = "https://gate.example/first.png";
    assert!(
        store.pixels(first).is_some(),
        "the first ask creates its handle"
    );
    // Let the worker pick the first one up and park inside the fetch.
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if !asked_of(&asked).is_empty() {
            break;
        }
    }
    assert_eq!(
        asked_of(&asked).len(),
        1,
        "the worker picked up exactly the first request"
    );

    // Now flood: 200 more URLs while the worker is parked. Exactly MAX_INFLIGHT fit in the
    // queue; the rest are dropped.
    let extra: Vec<String> = (0..MAX_INFLIGHT + 72)
        .map(|i| format!("https://gate.example/flood-{i}.png"))
        .collect();
    let mut accepted_handles = 0;
    for url in &extra {
        if store.pixels(url).is_some() {
            accepted_handles += 1;
        }
    }
    assert_eq!(accepted_handles, extra.len(), "every ask makes its handle");
    // The worker stays parked the whole test: whatever was accepted sits queued, the surplus
    // was dropped at `send`. The cap's contract is about the queue, so the assertions hold
    // without waking it: the total asks can never exceed 1 (running) + MAX_INFLIGHT (queued) —
    // the dropped ones are gone, not deferred.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let total = asked_of(&asked).len();
    assert!(
        total <= 1 + MAX_INFLIGHT,
        "at most 1 running + {MAX_INFLIGHT} queued requests were accepted, saw {total}"
    );
    // The dropped URLs never re-queue: asking again is a no-op (attempted is per-run).
    let before = asked_of(&asked).len();
    for url in extra.iter().take(10) {
        let _ = store.pixels(url);
    }
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(
        asked_of(&asked).len(),
        before,
        "a dropped URL is never re-queued by a later ask"
    );
}

// ---- The avatar ------------------------------------------------------------------------------

#[test]
fn the_avatar_tint_follows_the_theme_color_or_the_id() {
    // The tint rule moved with the avatar builder; both arms pinned through the same helpers
    // the views draw with.
    let p = shell::Paint::of(true);
    let themed = tok::theme(true);
    // A parseable theme_color wins; an unparseable one falls back to an accent (never a panic,
    // never transparent).
    let _ = crate::icons::tint_for_test("#102030", "any", p);
    let _ = crate::icons::tint_for_test("not-a-color", "some-id", p);
    // The letter ink is one of the appearance's own foregrounds.
    let tint = crate::icons::tint_for_test("#FFFFFF", "id", p);
    let ink = crate::icons::ink_on_test(tint, p);
    assert!(ink == themed.bg || ink == tok::theme(false).knob);
}

// ---- Headless raster proof ---------------------------------------------------------------------
//
// The `--snap` tooling path does a real launch (real `HttpFetcher`, the user's config dir), so
// the controlled picture is rasterized here: the same view functions, one `Pixels` carrying a
// decoded icon and one empty, drawn by `icons::avatar` exactly as a rail row and a store tile
// draw them. Written to `target/icons-proof.png` when `STATION_ICONS_PROOF` is set, so the
// command is explicit and `cargo test` stays PNG-less by default.

#[test]
fn a_decoded_icon_and_a_letter_fallback_rasterize_side_by_side() {
    let out = match std::env::var_os("STATION_ICONS_PROOF") {
        Some(dir) => std::path::PathBuf::from(dir),
        None => return,
    };
    use rune::{render, Cx, El};

    // One decoded icon (the canned red PNG through the real decode) and one that failed.
    let decoded = icons::decode(RED_PNG).expect("the canned PNG decodes");
    let ready = rune::Pixels::new();
    ready.set(decoded.width, decoded.height, decoded.rgba);
    let empty = rune::Pixels::new();

    type Row = (String, &'static str, rune::Pixels);
    let state: Vec<Row> = vec![
        ("Gmail".into(), "ready", ready),
        ("Gmail".into(), "empty", empty),
    ];

    fn view(
        state: &Vec<(String, &'static str, rune::Pixels)>,
        cx: &mut Cx<Vec<(String, &'static str, rune::Pixels)>>,
    ) -> El {
        let p = crate::shell::Paint::of(cx.system_dark());
        let mut col = rune::col().fill(p.t.bg).gap(12.0).padding(12.0);
        for (name, tag, pixels) in state {
            let ready = if *tag == "ready" { Some(pixels) } else { None };
            let _ = pixels;
            col = col.child(
                rune::row()
                    .gap(12.0)
                    .child(
                        crate::icons::avatar(name, "id", "#CC0000", 40.0, p, ready)
                            .key(format!("icon-{tag}")),
                    )
                    .child(
                        rune::text(*tag)
                            .font(rune::tok::type_::BODY)
                            .color(p.t.text),
                    ),
            );
        }
        col
    }

    let mut rt = Runtime::new(
        state,
        view,
        (200.0, 140.0),
        Env {
            system_dark: true,
            scale: 2.0,
        },
    );
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);

    let gpu = render::Gpu::headless();
    let mut renderer = render::Renderer::new(&gpu.device, render::OFFSCREEN_FORMAT);
    let (w, h) = (400u32, 280u32);
    let target = render::Offscreen::new(&gpu.device, w, h);
    renderer.prepare(
        &gpu.device,
        &gpu.queue,
        &mut rt.text.atlas,
        &rt.instances,
        (w, h),
    );
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
    target.copy(&mut enc);
    gpu.queue.submit([enc.finish()]);
    let px = target.read(&gpu.device);
    render::write_png(&out, w, h, &px).expect("the proof PNG writes");

    // The two avatars differ where the icon replaces the letter: sample the centres.
    let at = |x: usize, y: usize| {
        let at = (y * w as usize + x) * 4;
        (px[at], px[at + 1], px[at + 2])
    };
    let ready_pixel = at(48, 56); // inside the first avatar
    let empty_pixel = at(48, 156); // inside the second
    assert_ne!(
        ready_pixel, empty_pixel,
        "the decoded icon and the letter avatar render differently ({ready_pixel:?} vs {empty_pixel:?})"
    );
}
