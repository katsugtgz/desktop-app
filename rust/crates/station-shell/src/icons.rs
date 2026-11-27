//! App icons: the manifest's remote icon URL, fetched once, cached on disk, drawn by Rune.
//!
//! The store's tiles and the rail's rows read [`ShellState::icon_pixels`] — an in-memory
//! [`rune::Pixels`] handle the view draws synchronously. Nothing in a view touches the network:
//!
//! - The view asks for the app's URL; the store hands back the handle it has (empty until the
//!   bytes arrive, then the decoded image; after a failure the handle stays empty and the
//!   letter avatar shows — the fallback is the design, not a stub).
//! - One worker thread owns every fetch. [`Fetcher`] is the seam tests replace, the way
//!   `WebviewHost`/`FakeWebview` replaces the web view; the fake delivers canned bytes through
//!   the same queue the HTTP fetcher reports through.
//! - The disk cache sits under the same config-dir convention `manifest-registry`
//!   (`private-manifests.json`) and `session-store` (`station/session.json`) use:
//!   `<config dir>/station/icons/<sha256(url)>.bin`. Writes are temp-file + rename.
//!
//! Bounds (the "Robustness" rows): only `http`/`https`, at most [`MAX_ICON_BYTES`] per body, a
//! [`FETCH_TIMEOUT_S`] overall timeout, at most [`MAX_ICON_SIDE`] pixels a side after decode,
//! at most [`MAX_CACHED_ICONS`] files on disk, and at most [`MAX_INFLIGHT`] fetches in flight.
//! Every failure path lands on the letter avatar; nothing panics on remote data.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use rune::tok::ACCENT_NAMES;
use rune::{el, Color, El, Pixels};

use crate::shell::Paint;
use sha2::{Digest, Sha256};

/// Only `http` and `https` URLs are fetched; everything else (and every relative or `data:`
/// URL) keeps the letter avatar. Compared case-insensitively, ASCII-only.
pub fn is_fetchable(url: &str) -> bool {
    let bytes = url.as_bytes();
    [b"http://".as_slice(), b"https://".as_slice()]
        .into_iter()
        .any(|scheme| {
            bytes
                .get(..scheme.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
        })
}

/// The most bytes one icon fetch may download. Past it the body is abandoned and the letter
/// avatar stays (these icons are a few KiB; a MiB is generous).
pub const MAX_ICON_BYTES: usize = 1 << 20; // 1 MiB

/// The overall wall-clock budget of one fetch, connect through body.
pub const FETCH_TIMEOUT_S: u64 = 10;

/// The largest side, in pixels, an icon may decode to. Rune's decoder refuses anything wider
/// anyway; declaring it here means a hostile image fails the check before its mips are built.
pub const MAX_ICON_SIDE: u32 = 8192;

/// The most icon files the disk cache keeps, pruned least-recently-modified when a fetch stores
/// past the cap — the bundled set is 637 icons, so the cap covers it with room for custom apps.
pub const MAX_CACHED_ICONS: usize = 1_500;

/// The most fetches in flight at once. One worker drains them one at a time; the channel is
/// what bounds the queue a first frame can hand over (a full 637-icon store asking at once
/// fits under it). Not a runtime knob: the worker's `recv` loop is the enforcement point.
pub const MAX_INFLIGHT: usize = 128;

/// Where the bytes came from: the network, or the disk cache a previous run filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Network,
    Disk,
}

/// One downloaded, decoded icon: straight-alpha sRGB RGBA, rows top first — what
/// [`rune::Pixels::set`] takes.
pub struct DecodedIcon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub from: Source,
}

/// The error text a fetch or decode failed with (logged once; the view just falls back).
pub type IconError = String;

/// What gets the bytes. Tests install a fake; the app builds [`HttpFetcher`].
pub trait Fetcher: Send + 'static {
    /// Downloads `url`'s bytes, or says why not. Implementations own their timeouts and byte
    /// caps; the queue checks nothing twice.
    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, IconError>;
}

/// Decode downloaded bytes into the RGBA the view draws. Separated from [`Fetcher`] so tests
/// feed bytes without HTTP: a PNG goes through the `image` crate (the one rune itself decodes
/// with), anything unsupported is the letter avatar.
pub fn decode(bytes: &[u8]) -> Result<DecodedIcon, IconError> {
    use image::ImageDecoder as _;
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_ICON_SIDE);
    limits.max_image_height = Some(MAX_ICON_SIDE);
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    limits
        .reserve(decoder.total_bytes())
        .map_err(|e| e.to_string())?;
    decoder.set_limits(limits).map_err(|e| e.to_string())?;
    let icon = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    let icon = icon.into_rgba8();
    Ok(DecodedIcon {
        width: icon.width(),
        height: icon.height(),
        rgba: icon.into_raw(),
        from: Source::Network,
    })
}

/// The production fetcher: one agent per call (they are cheap; the timeout rides the request),
/// a 10 s overall budget, and a hard read cap that a lying `Content-Length` cannot dodge.
pub struct HttpFetcher;

impl Fetcher for HttpFetcher {
    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, IconError> {
        let response = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_S))
            .user_agent("station-shell")
            .build()
            .get(url)
            .call()
            .map_err(|e| e.to_string())?;
        // The header may lie; `take` is the hard cap either way.
        if let Some(length) = response
            .header("Content-Length")
            .and_then(|length| length.parse::<usize>().ok())
        {
            if length > MAX_ICON_BYTES {
                return Err(format!(
                    "icon body {length} B over the {MAX_ICON_BYTES} B cap"
                ));
            }
        }
        let mut body = Vec::new();
        response
            .into_reader()
            .take((MAX_ICON_BYTES + 1) as u64)
            .read_to_end(&mut body)
            .map_err(|e| e.to_string())?;
        if body.len() > MAX_ICON_BYTES {
            return Err(format!("icon body over the {MAX_ICON_BYTES} B cap"));
        }
        Ok(body)
    }
}

/// The cache directory convention: `<config dir>/station/icons`, beside
/// `<config dir>/station/session.json`.
pub fn cache_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("station")
        .join("icons")
}

/// The cache file for `url`: the hex SHA-256 of the URL — content-addressed by source, so the
/// same URL never stores twice and no URL character can escape the directory.
pub fn cache_path(dir: &Path, url: &str) -> PathBuf {
    let digest = Sha256::digest(url.as_bytes());
    let mut name = String::with_capacity(68);
    for byte in digest {
        name.push_str(&format!("{byte:02x}"));
    }
    name.push_str(".bin");
    dir.join(name)
}

/// Writes one icon's bytes atomically: a sibling temp file, fsync, rename — the same pattern
/// `manifest-registry` and `session-store` use. Written only by the fetch thread, so no lock
/// beyond the rename. A failure leaves the old file (or nothing) and is not retried here.
fn store_bytes(path: &Path, bytes: &[u8]) {
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut temp_name = path.as_os_str().to_owned();
    temp_name.push(".tmp");
    let temp = PathBuf::from(temp_name);
    let written = std::fs::File::create(&temp).ok().and_then(|mut file| {
        use std::io::Write;
        file.write_all(bytes).ok()?;
        file.sync_all().ok()?;
        Some(())
    });
    match written {
        Some(()) => {
            if std::fs::rename(&temp, path).is_err() {
                let _ = std::fs::remove_file(&temp);
            }
        }
        None => {
            let _ = std::fs::remove_file(&temp);
        }
    }
}

/// Keeps the directory under [`MAX_CACHED_ICONS`] files, oldest-modified first. `keep` is the
/// file just written. A failure to prune is not a failure to store; the next fetch tries again.
fn prune(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path != keep)
        .filter_map(|path| {
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
            Some((modified, path))
        })
        .collect();
    if files.len() < MAX_CACHED_ICONS {
        return;
    }
    files.sort();
    let excess = files.len() + 1 - MAX_CACHED_ICONS;
    for (_, path) in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

/// A finished fetch job, handed from the fetch thread to the frame poll.
pub struct Job {
    pub url: String,
    pub result: Result<DecodedIcon, IconError>,
}

/// The store views read: one pixels handle per URL, the finished-job queue, and the fetch
/// thread. Lives in `ShellState` (plain data, no platform handle) — the fetch thread talks to
/// it only through the job queue and the waker.
pub struct IconStore {
    /// The pixels each URL draws, ready or not (empty pixels draw nothing: the view puts the
    /// letter avatar underneath).
    handles: HashMap<String, Pixels>,
    /// URLs this run already asked for — never re-queued after a failure, so a broken icon
    /// costs one attempt per run, not one per frame.
    attempted: std::collections::HashSet<String>,
    /// Requests the worker has not picked up yet; bounded by [`MAX_INFLIGHT`].
    requests: Option<mpsc::Sender<String>>,
    /// Requests sent and not yet picked up (the sender side of the worker's `recv` loop).
    inflight: Arc<AtomicU64>,
    queue: Arc<Mutex<Vec<Job>>>,
    /// Bumps whenever a job lands (ready or failed), so `drain` knows to run and the poll can
    /// say "redraw" when pixels changed.
    jobs_generation: Arc<AtomicU64>,
    seen_generation: u64,
    cache: PathBuf,
}

impl Default for IconStore {
    fn default() -> Self {
        Self::new()
    }
}

impl IconStore {
    /// The store rooted at [`cache_dir`].
    pub fn new() -> Self {
        Self::with_cache(cache_dir())
    }

    /// The store a test drives: `cache` is usually a temp dir.
    pub fn with_cache(cache: PathBuf) -> Self {
        Self {
            handles: HashMap::new(),
            attempted: std::collections::HashSet::new(),
            requests: None,
            inflight: Arc::new(AtomicU64::new(0)),
            queue: Arc::new(Mutex::new(Vec::new())),
            jobs_generation: Arc::new(AtomicU64::new(0)),
            seen_generation: 0,
            cache,
        }
    }

    /// Starts the fetch thread with `fetcher` and Rune's waker (`AppSpec::setup` hands the
    /// waker; it is what makes the next frame happen after bytes land). One worker for the
    /// process; a second `start` replaces the previous thread's channel — the old worker sees
    /// a closed channel and exits.
    pub fn start(&mut self, fetcher: Box<dyn Fetcher>, waker: rune::Waker) {
        let (send, receive) = mpsc::channel::<String>();
        self.requests = Some(send);
        let inflight = Arc::clone(&self.inflight);
        let queue = Arc::clone(&self.queue);
        let cache = self.cache.clone();
        let generation = Arc::clone(&self.jobs_generation);
        std::thread::Builder::new()
            .name("station-icons".into())
            .spawn(move || {
                let mut fetcher = fetcher;
                while let Ok(url) = receive.recv() {
                    inflight.fetch_sub(1, Ordering::AcqRel);
                    let result = Self::obtain(&mut fetcher, &cache, &url);
                    let finished = Job { url, result };
                    {
                        let mut pending = match queue.lock() {
                            Ok(queue) => queue,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        pending.push(finished);
                    }
                    generation.fetch_add(1, Ordering::AcqRel);
                    waker();
                }
            })
            .expect("icon worker");
    }

    /// Bytes for `url`: disk cache first (a hit is mtime-touched so pruning keeps the warm
    /// set), then the fetcher, then the decode. Disk bytes skip the fetcher entirely.
    fn obtain(
        fetcher: &mut Box<dyn Fetcher>,
        cache: &Path,
        url: &str,
    ) -> Result<DecodedIcon, IconError> {
        let path = cache_path(cache, url);
        if let Ok(bytes) = std::fs::read(&path) {
            // Touched so LRU pruning keeps recently drawn icons.
            let now = std::fs::File::options().append(true).open(&path);
            if let Ok(file) = now {
                let _ = file.set_modified(std::time::SystemTime::now());
            }
            return decode(&bytes).map(|mut icon| {
                icon.from = Source::Disk;
                icon
            });
        }
        let bytes = fetcher.fetch(url)?;
        let icon = decode(&bytes)?;
        if icon.from == Source::Disk {
            return Ok(icon);
        }
        store_bytes(&path, &bytes);
        prune(cache, &path);
        Ok(icon)
    }

    /// The pixels for `url`, asking for it once per run when missing. An unfetchable URL, an
    /// empty URL, or a failed one is `None` — the letter avatar shows.
    pub fn pixels(&mut self, url: &str) -> Option<&Pixels> {
        self.ensure(url);
        self.handles.get(url)
    }

    /// The read-only half: what a view calls. Only an already-known fetchable URL gives its
    /// handle (the poll's `prefetch_icons` did the asking); anything else is `None`.
    pub fn pixels_read(&self, url: &str) -> Option<&Pixels> {
        if !is_fetchable(url) {
            return None;
        }
        self.handles.get(url)
    }

    /// Creates the handle for `url` and queues its fetch, once per run.
    fn ensure(&mut self, url: &str) {
        if !is_fetchable(url) || self.handles.contains_key(url) {
            return;
        }
        if !self.attempted.insert(url.to_owned()) {
            return;
        }
        self.handles.insert(url.to_owned(), Pixels::new());
        self.send(url.to_owned());
    }

    /// Hands `url` to the worker. No worker (`start` not called — headless runs without a
    /// fetcher) means the handle stays empty and the letter avatar shows. [`MAX_INFLIGHT`]
    /// caps the un-picked-up queue: beyond it the request is dropped, never deferred, so a
    /// first frame cannot queue without end.
    fn send(&self, url: String) {
        let Some(requests) = &self.requests else {
            return;
        };
        if self.inflight.load(Ordering::Acquire) >= MAX_INFLIGHT as u64 {
            return;
        }
        if requests.send(url).is_ok() {
            self.inflight.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// True when jobs landed since the last `drain` — the poll takes them.
    pub fn has_jobs(&self) -> bool {
        self.jobs_generation.load(Ordering::Acquire) != self.seen_generation
    }

    /// Takes the finished jobs into the handles: bytes → pixels. Returns whether anything
    /// became drawable (a failure keeps its handle empty; the view never changes).
    pub fn drain(&mut self) -> bool {
        let jobs: Vec<Job> = {
            let mut pending = match self.queue.lock() {
                Ok(queue) => queue,
                Err(poisoned) => poisoned.into_inner(),
            };
            if pending.is_empty() {
                return false;
            }
            std::mem::take(&mut *pending)
        };
        self.seen_generation = self.jobs_generation.load(Ordering::Acquire);
        let mut ready = false;
        for job in jobs {
            match job.result {
                Ok(icon) => {
                    if let Some(pixels) = self.handles.get(&job.url) {
                        pixels.set(icon.width, icon.height, icon.rgba);
                        ready = true;
                    }
                }
                Err(why) => {
                    eprintln!("station-shell: no icon for {}: {why}", job.url);
                }
            }
        }
        ready
    }
}

/// The icon avatar a rail row or a store tile draws, `size_pt` square: the decoded manifest
/// icon when `pixels` is `Some` (an empty handle draws nothing over the letter beneath), the
/// first letter of `name` on the manifest's tint otherwise — today's avatar, unchanged.
///
/// One builder for both callers so the fallback and the tint rule stay identical (the tint is
/// the manifest's `theme_color`, else a stable accent from the id).
pub fn avatar(
    name: &str,
    id: &str,
    theme_color: &str,
    size_pt: f32,
    p: Paint,
    pixels: Option<&Pixels>,
) -> El {
    let tint = tint_for(id, theme_color, p);
    let mut base = el()
        .size(size_pt, size_pt)
        .radius(size_pt / 2.0)
        .center()
        .fill(tint);
    let ready = pixels.filter(|handle| handle.size().0 > 0);
    if let Some(handle) = ready {
        base = base.child(
            el().size(size_pt, size_pt)
                .radius(size_pt / 2.0)
                .pixels_fit(handle, rune::SurfaceFit::Fill)
                .a11y_hidden(),
        );
    } else {
        base = base.child(
            rune::text(first_letter(name))
                .font(rune::tok::type_::LABEL)
                .color(ink_on(tint, p))
                .a11y_hidden(),
        );
    }
    base.a11y_hidden()
}

/// The avatar colour: the manifest's `theme_color` when it parses, else one of Rune's accents
/// picked from the app id, so every tile has a stable, palette-derived colour.
fn tint_for(id: &str, theme_color: &str, p: Paint) -> Color {
    if let Some(tint) = rune::color::hex(theme_color) {
        return tint;
    }
    let index = id.bytes().fold(0usize, |sum, byte| sum + byte as usize) % ACCENT_NAMES.len();
    rune::tok::accent(index, p.dark)
}

/// A readable foreground for text on `tint`: the appearance's own foregrounds, chosen by the
/// tint's perceived lightness.
fn ink_on(tint: Color, p: Paint) -> Color {
    let luminance = 0.2126 * tint.r + 0.7152 * tint.g + 0.0722 * tint.b;
    if luminance > 0.4 {
        p.t.bg
    } else {
        rune::tok::theme(!p.dark).knob
    }
}

/// The first letter of a name, uppercased — the letter-avatar fallback.
fn first_letter(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn tint_for_test(theme_color: &str, id: &str, p: Paint) -> Color {
    tint_for(id, theme_color, p)
}

#[cfg(test)]
pub(crate) fn ink_on_test(tint: Color, p: Paint) -> Color {
    ink_on(tint, p)
}
