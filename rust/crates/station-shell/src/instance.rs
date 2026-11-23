//! One shell instance per user session: a second launch hands its deep link to the primary and
//! exits.
//!
//! The OS protocol registration (`docs/rust-packaging-plan.md` §7) launches a whole second
//! `station-shell.exe "station://…"` for every link. Without a lock each of those became its own
//! window; since slice 14 the decision is:
//!
//! - **Windows.** A named mutex in the logon-session namespace (`Local\…`, created-or-exists —
//!   the documented Win32 single-instance check) is acquired before Rune opens anything. The
//!   first process keeps it for its lifetime and is the *primary*: it owns a loopback TCP
//!   listener, and `<config dir>/station/instance.json` names its port and a per-run token.
//!   A second process sees `ERROR_ALREADY_EXISTS`, sends `token\nurl\n` to that port — or just
//!   `token\n\n`, a focus-only ping, when its launch carried no `station:` argument — and exits
//!   without a window. A stale `instance.json` (a crashed primary) is ignored when the connect
//!   fails; the second process then simply exits.
//! - **Other platforms.** No lock, the documented `Unsupported` shape the web view already
//!   uses — no half-lock is invented.
//!
//! The transport is loopback TCP rather than `WM_COPYDATA`: the listener, the framing, the
//! token gate and the queue are plain-library code testable here without a window, and the
//! primary's end of it is one thread the waker already reaches. The window-side
//! `SetForegroundWindow` is the only part that needs the real window
//! (`webview::platform::bring_to_front`).
//!
//! Bounds (the "Robustness" rows): a request is at most [`MAX_REQUEST_BYTES`], answered with an
//! [`IO_TIMEOUT_S`] read/write timeout; the token is a fresh [`TOKEN_CHARS`]-hex-char secret
//! per run, compared constant-length; only `station:` URLs pass the listener (the *decision* is
//! [`crate::deep_link`]'s, shared with argv, made later on the UI thread); at most
//! [`MAX_ACCEPTED_PER_RUN`] accepted requests per run and [`MAX_ACCEPTED_PER_WINDOW`] in any
//! rolling [`ACCEPT_WINDOW_S`] seconds — links and focus pings together, so a local process
//! cannot flood the frame with focus changes either. The listener binds `127.0.0.1` only, so
//! the port is unreachable from off-machine; the token is a gate against other local processes,
//! not a secret from the user's own account.

#[cfg(target_os = "windows")]
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(any(target_os = "windows", test))]
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
#[cfg(target_os = "windows")]
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;

use crate::deep_link::{self, Outcome};
use crate::session;
use crate::state::ShellState;

/// The name of the mutex, in the logon-session namespace: one shell per Windows session. The
/// namespace itself separates fast-user-switching sessions; the fixed name is the per-session
/// identity.
#[cfg(target_os = "windows")]
const MUTEX_NAME: &str = "Local\\station-shell-singleton";

/// The largest request the listener reads at all, in bytes — a longer one is abandoned before
/// parsing, so a hostile writer cannot feed the frame an unbounded string.
pub const MAX_REQUEST_BYTES: usize = 4096;

/// The read/write timeout on one connection, both sides: a client that opens and never speaks
/// costs the listener two seconds, once.
pub const IO_TIMEOUT_S: u64 = 2;

/// The instance file's read cap (and parse cap): the file names a port and a token and nothing
/// else; anything larger is refused unread.
pub const MAX_FILE_BYTES: usize = 4096;

/// The token's length, in lowercase hex characters (16 random bytes). Fresh every run: a token
/// left over in an old `instance.json` cannot answer for today's listener.
pub const TOKEN_CHARS: usize = 32;

/// The most requests (links and focus pings together) one run accepts, whatever the pace: past
/// it a local process is refused until the primary restarts.
pub const MAX_ACCEPTED_PER_RUN: usize = 8;

/// The rolling budget: at most this many accepted requests in any [`ACCEPT_WINDOW_S`] seconds.
pub const MAX_ACCEPTED_PER_WINDOW: usize = 10;

/// The rolling window, in seconds.
pub const ACCEPT_WINDOW_S: f64 = 60.0;

/// The queued requests accepted but not yet drained by a frame. The per-run acceptance cap
/// already bounds this; the queue cap restates the bound at the other end.
const MAX_QUEUED: usize = MAX_ACCEPTED_PER_RUN;

/// Where the instance file lives: beside the session file, under the same
/// `STATION_SESSION_PATH` override, so tests and smoke runs stay off the user profile.
fn instance_path() -> PathBuf {
    session::path().with_file_name("instance.json")
}

/// One `instance.json`: the listener's port and the per-run token.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct InstanceFile {
    pub port: u16,
    pub token: String,
}

/// Whether `token` has the shape this run generates: exactly [`TOKEN_CHARS`] lowercase hex
/// characters. The shape check is the parse's; the secret match is [`token_matches`].
pub fn is_token(token: &str) -> bool {
    token.len() == TOKEN_CHARS
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Parses the instance file's text: malformed JSON, a wrong shape (port 0, a token that is not
/// [`TOKEN_CHARS`] lowercase hex characters) or an oversized document is `None` — never a
/// panic, never a truncation.
pub fn parse_instance_file(text: &str) -> Option<InstanceFile> {
    if text.len() > MAX_FILE_BYTES {
        return None;
    }
    let file: InstanceFile = serde_json::from_str(text).ok()?;
    if file.port == 0 || !is_token(&file.token) {
        return None;
    }
    Some(file)
}

/// The constant-length token comparison: equal length first (the lengths are public), then one
/// fold over the bytes so the work does not depend on how far into the token a mismatch sits.
pub fn token_matches(given: &str, expected: &str) -> bool {
    if given.len() != expected.len() {
        return false;
    }
    given
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

/// A request that failed its framing or its gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// More than [`MAX_REQUEST_BYTES`] bytes.
    TooLong,
    /// Not two lines, not UTF-8, or data past the optional trailing newline.
    Malformed,
    /// The first line is not this run's token.
    WrongToken,
    /// The URL is not a `station:` candidate (the listener's only own URL rule).
    NotStation,
}

/// Parses one request: `token\nurl\n` (the trailing newline optional). `Ok(None)` is the
/// focus-only ping — a second launch with no deep link. Everything else that survives is a
/// `station:` candidate string for [`crate::deep_link`], which owns the *decision* (bounds,
/// parsing, serve-or-not) exactly as it does for argv.
pub fn parse_request(bytes: &[u8], expected_token: &str) -> Result<Option<String>, RequestError> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(RequestError::TooLong);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| RequestError::Malformed)?;
    let Some((token_line, rest)) = text.split_once('\n') else {
        return Err(RequestError::Malformed);
    };
    let url_line = rest.strip_suffix('\n').unwrap_or(rest);
    if url_line.contains('\n') {
        return Err(RequestError::Malformed);
    }
    if !token_matches(token_line, expected_token) {
        return Err(RequestError::WrongToken);
    }
    if url_line.is_empty() {
        return Ok(None);
    }
    if !deep_link::starts_with_scheme(url_line) {
        return Err(RequestError::NotStation);
    }
    Ok(Some(url_line.to_owned()))
}

/// The acceptance gate: the per-run counter and the rolling window, on the clock the caller
/// reads. `now` is a parameter so the window is testable without sleeping.
#[derive(Default)]
pub struct Gate {
    accepted_run: usize,
    times: Vec<Instant>,
}

impl Gate {
    const fn new() -> Self {
        Self {
            accepted_run: 0,
            times: Vec::new(),
        }
    }

    /// Whether one more request is accepted at `now`; records it when it is. A refused request
    /// is dropped, never deferred.
    pub fn allows(&mut self, now: Instant) -> bool {
        self.times
            .retain(|t| now.duration_since(*t).as_secs_f64() < ACCEPT_WINDOW_S);
        if self.accepted_run >= MAX_ACCEPTED_PER_RUN || self.times.len() >= MAX_ACCEPTED_PER_WINDOW
        {
            return false;
        }
        self.accepted_run += 1;
        self.times.push(now);
        true
    }
}

/// The accepted requests waiting for the frame's poll: `Some(url)` for a link, `None` for a
/// focus-only ping. Poisoning still yields its requests, like every queue in this crate.
static PENDING: Mutex<Vec<Option<String>>> = Mutex::new(Vec::new());

/// The acceptance gate, shared by the listener thread and (in tests) the request path.
static GATE: Mutex<Gate> = Mutex::new(Gate::new());

/// Set by `shutdown`: the listener stops taking connections.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// The listener's bound port, so `shutdown` knows where the accept loop is.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
static LISTEN_PORT: OnceLock<u16> = OnceLock::new();

/// The per-test freeze: `poll_at` runs in other tests' frames; a frozen module hands back an
/// empty drain so a concurrent frame cannot consume this module's queues. The freeze owner
/// (the test thread that froze, see `tests_instance`) is exempt: its own drains go through.
#[cfg(test)]
static FROZEN: AtomicBool = AtomicBool::new(false);

/// Marks the calling thread as the freeze's owner: its `drain` runs while frozen.
#[cfg(test)]
pub(crate) fn own_freeze() {
    EXEMPT.with(|exempt| exempt.set(true));
}

#[cfg(test)]
thread_local! {
    static EXEMPT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Freezes/unfreezes the module's queues for one test (tests only).
#[cfg(test)]
pub(crate) fn freeze(on: bool) {
    FROZEN.store(on, Ordering::SeqCst);
}

/// Empties the gate and the pending queue (tests only): the module statics are process-global,
/// and a test that asserts exact acceptance counts must start from a fresh module.
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    match GATE.lock() {
        Ok(mut gate) => *gate = Gate::new(),
        Err(poisoned) => *poisoned.into_inner() = Gate::new(),
    }
    match PENDING.lock() {
        Ok(mut pending) => pending.clear(),
        Err(poisoned) => poisoned.into_inner().clear(),
    }
}

/// Validates and queues one request body (the listener's whole decision, minus I/O): framing
/// and token first, then the gate, then the queue. True when accepted. Not freeze-gated: only
/// `drain` runs on the UI thread, so only it can be disturbed by another test's frame.
pub fn handle_request(bytes: &[u8], expected_token: &str) -> bool {
    let url = match parse_request(bytes, expected_token) {
        Ok(url) => url,
        Err(_) => return false,
    };
    let mut gate = match GATE.lock() {
        Ok(gate) => gate,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !gate.allows(Instant::now()) {
        return false;
    }
    let mut pending = match PENDING.lock() {
        Ok(pending) => pending,
        Err(poisoned) => poisoned.into_inner(),
    };
    if pending.len() >= MAX_QUEUED {
        return false;
    }
    pending.push(url);
    true
}

/// What one drain did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drain {
    /// A link reached the state (the same "state changed" signal `poll` returns, so Rune
    /// rebuilds the view the frame after a link lands).
    pub changed: bool,
    /// Accepted requests carried by this drain, links and focus pings together: any of them is
    /// a reason to bring the window forward.
    pub arrived: usize,
}

/// Takes the accepted requests (the frame's poll, on the UI thread). Each link goes through the
/// one decision [`crate::deep_link`] makes for argv — there is no second copy of the rules.
pub fn drain(state: &mut ShellState) -> Drain {
    #[cfg(test)]
    if FROZEN.load(Ordering::SeqCst) && !EXEMPT.with(|exempt| exempt.get()) {
        return Drain::default();
    }
    let urls = {
        let mut pending = match PENDING.lock() {
            Ok(pending) => pending,
            Err(poisoned) => poisoned.into_inner(),
        };
        if pending.is_empty() {
            return Drain::default();
        }
        std::mem::take(&mut *pending)
    };
    let mut out = Drain {
        changed: false,
        arrived: urls.len(),
    };
    for url in urls.into_iter().flatten() {
        out.changed |= deep_link::act(state, url) == Outcome::Store;
    }
    out
}

/// This run's token: 16 bytes hashed from two independently seeded `RandomState`s over the
/// clock, the process id and a salt. The seeds come from the OS once, and each
/// `RandomState::new` derives a new key from them — for a loopback gate against other processes
/// of the same user this is the right budget; it is not claimed to be a CSPRNG.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn generate_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mix = |salt: u64| -> u64 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u64(nanos);
        hasher.write_u64(std::process::id() as u64);
        hasher.finish()
    };
    let mut bytes = [0u8; TOKEN_CHARS / 2];
    bytes[..8].copy_from_slice(&mix(0x0246_8ace_1357_9bdf).to_ne_bytes());
    bytes[8..].copy_from_slice(&mix(0x1a2b_3c4d_5e6f_7081).to_ne_bytes());
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Writes `instance.json` atomically (temp + `sync_all` + rename, the pattern every file this
/// shell writes uses): a reader never sees a half file.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn write_instance_file(file: &InstanceFile) {
    let Ok(body) = serde_json::to_vec(file) else {
        return;
    };
    let path = instance_path();
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
        file.write_all(&body).ok()?;
        file.sync_all().ok()?;
        Some(())
    });
    match written {
        Some(()) => {
            if std::fs::rename(&temp, &path).is_err() {
                let _ = std::fs::remove_file(&temp);
            }
        }
        None => {
            let _ = std::fs::remove_file(&temp);
        }
    }
}

/// What the second process sends: the body of `token\nurl\n`, focus-only when `url` is `None`.
#[cfg(target_os = "windows")]
fn request_body(token: &str, url: Option<&str>) -> Vec<u8> {
    let mut body = Vec::with_capacity(TOKEN_CHARS + url.map_or(0, str::len) + 2);
    body.extend_from_slice(token.as_bytes());
    body.push(b'\n');
    body.extend_from_slice(url.unwrap_or("").as_bytes());
    body.push(b'\n');
    body
}

/// Sends `url` (or the focus-only ping) to the primary named by `instance.json`. A stale or
/// unreadable file, a refused connect and a timeout are all the same "no" — the second process
/// exits either way.
#[cfg(target_os = "windows")]
fn send_to_primary(url: Option<&str>) -> bool {
    let bytes = match std::fs::read(instance_path()) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    if bytes.len() > MAX_FILE_BYTES {
        return false;
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let Some(file) = parse_instance_file(&text) else {
        return false;
    };
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), file.port);
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_secs(IO_TIMEOUT_S))
    else {
        return false;
    };
    use std::io::Write;
    if stream
        .set_write_timeout(Some(Duration::from_secs(IO_TIMEOUT_S)))
        .is_err()
    {
        return false;
    }
    stream.write_all(&request_body(&file.token, url)).is_ok()
}

/// The mutex and every Windows call this module makes, in one cfg block beside their feature
/// line in `Cargo.toml`.
#[cfg(target_os = "windows")]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    /// Creates-or-finds the single-instance mutex. Returns the handle and whether this process
    /// is the primary (the mutex did not exist yet).
    pub(crate) fn create_or_existing(name: &[u16]) -> (HANDLE, bool) {
        // SAFETY: `name` is a NUL-terminated UTF-16 buffer built by the caller; no attributes
        // are requested (default security, no inheritance) and the handle is closed or kept for
        // the process lifetime.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
        let primary = handle.is_null() || unsafe { GetLastError() } != ERROR_ALREADY_EXISTS;
        (handle, primary)
    }

    pub(crate) fn close(handle: HANDLE) {
        // SAFETY: the handle came from `create_or_existing` and is closed exactly once, in
        // `shutdown`, after which nothing uses it.
        unsafe { CloseHandle(handle) };
    }

    /// A NUL-terminated UTF-16 buffer for the wide APIs.
    pub(crate) fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

/// The primary's mutex handle, kept for the process lifetime (a dropped handle would let a
/// third launch become a second primary). Stored as `usize` because raw handles are not `Send`
/// and a static wants to be.
#[cfg(target_os = "windows")]
static LOCK: OnceLock<usize> = OnceLock::new();

/// The entry every launch takes, before Rune is asked for anything: acquire the mutex. A
/// secondary sends its deep link (or the focus-only ping) to the primary and **exits the
/// process** — no state, no window, no listener. The primary keeps the handle and returns
/// `false` to run on. A mutex call that fails outright logs once and runs sole: the shell must
/// open even when the lock cannot.
pub fn handoff_if_secondary() -> bool {
    #[cfg(target_os = "windows")]
    {
        let name = win::wide(MUTEX_NAME);
        let (handle, primary) = win::create_or_existing(&name);
        if handle.is_null() {
            eprintln!("station-shell: single-instance lock unavailable, running without it");
            return false;
        }
        if !primary {
            win::close(handle);
            let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
            let url = deep_link::first_candidate(&args);
            send_to_primary(url.as_deref());
            std::process::exit(0);
        }
        let _ = LOCK.set(handle as usize);
        false
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// The primary's listener, started from `AppSpec::setup` once the mutex is ours. Binds an
/// ephemeral port on `127.0.0.1`, writes `instance.json`, and serves until `shutdown`. A bind
/// or write failure logs once and the shell runs without the file — second launches then open
/// their own windows, exactly the pre-slice behaviour, said out loud on stderr.
pub fn start_primary(waker: rune::Waker) {
    #[cfg(target_os = "windows")]
    {
        let token = generate_token();
        let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) {
            Ok(listener) => listener,
            Err(why) => {
                eprintln!("station-shell: single-instance listener unavailable: {why}");
                return;
            }
        };
        let Ok(port) = listener.local_addr().map(|a| a.port()) else {
            return;
        };
        write_instance_file(&InstanceFile {
            port,
            token: token.clone(),
        });
        let _ = LISTEN_PORT.set(port);
        std::thread::Builder::new()
            .name("station-instance".into())
            .spawn(move || listener_loop(listener, &token, waker))
            .expect("instance listener thread");
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = waker;
    }
}

/// Accept loop: non-blocking accept with a short sleep between misses, so `shutdown`'s flag is
/// seen within a sleep of stopping. One connection at a time (a second waits — its writer times
/// out in [`IO_TIMEOUT_S`], and the gate refuses floods anyway). Each accepted request wakes
/// the next frame, which drains the queue on the UI thread; the wake is the latency hint, the
/// poll the guarantee.
#[cfg(target_os = "windows")]
fn listener_loop(listener: TcpListener, token: &str, waker: rune::Waker) {
    use std::io::Read;
    let _ = listener.set_nonblocking(true);
    loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(IO_TIMEOUT_S)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(IO_TIMEOUT_S)));
                let mut body = Vec::new();
                {
                    let mut take = (&stream).take((MAX_REQUEST_BYTES + 1) as u64);
                    let _ = take.read_to_end(&mut body);
                }
                if handle_request(&body, token) {
                    waker();
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// The clean-exit half (`on_close` / `on_quit`): the file goes first — a link arriving between
/// the flag and the process exit must find no file and give up — then the flag, and the mutex
/// goes with the process. The accept loop is non-blocking; it sees the flag within one sleep.
pub fn shutdown() {
    #[cfg(target_os = "windows")]
    {
        let _ = std::fs::remove_file(instance_path());
        SHUTDOWN.store(true, Ordering::SeqCst);
        if let Some(handle) = LOCK.get() {
            win::close(*handle as _);
        }
    }
}
