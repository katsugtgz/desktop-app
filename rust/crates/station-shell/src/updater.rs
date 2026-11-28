//! Real GitHub Releases update checker with SemVer 2.0 comparison and beta-channel filtering.
//!
//! Replaces the v1 browser-shortcut-only check with an asynchronous `ureq` (rustls) query to
//! GitHub Releases while preserving strict trust boundaries:
//! - Only reads up to [`MAX_RESPONSE_BYTES`] (512 KiB) with a [`FETCH_TIMEOUT`] (10 s) timeout.
//! - Parses versions strictly with SemVer 2.0 precedence (stable > prerelease for equal core).
//! - Skips drafts, skips prereleases when `include_beta` is false, and refuses any `html_url`
//!   outside `https://github.com/katsugtgz/desktop-app/releases/`.
//! - Never downloads or executes binaries automatically (full staged auto-updater remains
//!   human/product-gated per `docs/full-rust-port-program.md`).

use std::cmp::Ordering;
use std::fmt;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use url::Url;

/// GitHub Releases REST API endpoint queried for newer builds.
pub const GITHUB_RELEASES_API: &str =
    "https://api.github.com/repos/katsugtgz/desktop-app/releases?per_page=10";

/// Trusted path prefix on `https://github.com` for release download pages.
pub const RELEASE_PATH_PREFIX: &str = "/katsugtgz/desktop-app/releases/";

/// Maximum bytes read from the GitHub Releases API response (512 KiB).
pub const MAX_RESPONSE_BYTES: u64 = 512 * 1024;

/// Maximum characters allowed in a release display name or version label.
pub const MAX_RELEASE_TEXT_CHARS: usize = 256;

/// Maximum characters allowed in a release page URL.
pub const MAX_RELEASE_URL_CHARS: usize = 2048;

/// Overall HTTP timeout for the release-check request.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// One prerelease identifier in SemVer 2.0 (`1.2.3-beta.10`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreId {
    Numeric(u64),
    AlphaNum(String),
}

impl Ord for PreId {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (PreId::Numeric(a), PreId::Numeric(b)) => a.cmp(b),
            (PreId::AlphaNum(a), PreId::AlphaNum(b)) => a.cmp(b),
            // SemVer 2.0 §11.4.3: numeric identifiers always have lower precedence than non-numeric.
            (PreId::Numeric(_), PreId::AlphaNum(_)) => Ordering::Less,
            (PreId::AlphaNum(_), PreId::Numeric(_)) => Ordering::Greater,
        }
    }
}

impl PartialOrd for PreId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// SemVer 2.0 version (`major.minor.patch[-pre.1]`, build metadata ignored for precedence).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Vec<PreId>,
}

impl SemVer {
    /// Parses a version string such as `"0.1.0"`, `"v2.3.4"`, or `"2.3.4-beta.2+build.9"`.
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        let without_v = trimmed
            .strip_prefix('v')
            .or_else(|| trimmed.strip_prefix('V'))
            .unwrap_or(trimmed);
        // Strip optional `+build` metadata (ignored for precedence per SemVer 2.0 §10).
        let without_build = without_v.split('+').next().unwrap_or(without_v);
        if without_build.is_empty() {
            return None;
        }
        let (core, pre_str) = match without_build.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (without_build, None),
        };
        let mut core_parts = core.split('.');
        let major = parse_numeric_part(core_parts.next()?)?;
        let minor = parse_numeric_part(core_parts.next()?)?;
        let patch = parse_numeric_part(core_parts.next()?)?;
        if core_parts.next().is_some() {
            return None;
        }

        let mut prerelease = Vec::new();
        if let Some(pre) = pre_str {
            if pre.is_empty() {
                return None;
            }
            for part in pre.split('.') {
                if part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                {
                    return None;
                }
                if part.bytes().all(|b| b.is_ascii_digit()) {
                    prerelease.push(PreId::Numeric(part.parse().ok()?));
                } else {
                    prerelease.push(PreId::AlphaNum(part.to_owned()));
                }
            }
        }

        Some(Self {
            major,
            minor,
            patch,
            prerelease,
        })
    }

    /// True when this version carries any SemVer prerelease suffix (`-beta.1`, `-rc.2`, etc.).
    pub fn is_prerelease(&self) -> bool {
        !self.prerelease.is_empty()
    }
}

fn parse_numeric_part(part: &str) -> Option<u64> {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(
                || match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
                    (true, true) => Ordering::Equal,
                    // SemVer 2.0 §11.3: a normal version has higher precedence than a pre-release.
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (false, false) => self.prerelease.cmp(&other.prerelease),
                },
            )
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.prerelease.is_empty() {
            f.write_str("-")?;
            for (i, id) in self.prerelease.iter().enumerate() {
                if i > 0 {
                    f.write_str(".")?;
                }
                match id {
                    PreId::Numeric(n) => write!(f, "{n}")?,
                    PreId::AlphaNum(s) => f.write_str(s)?,
                }
            }
        }
        Ok(())
    }
}

/// One GitHub release entry relevant to update selection.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ReleaseInfo {
    #[serde(default)]
    pub tag_name: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
}

/// Result of comparing the current version against fetched releases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateCheckOutcome {
    UpToDate,
    Available {
        release_name: String,
        html_url: String,
    },
}

/// Failure modes of an update check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateError {
    AlreadyChecking,
    Network(String),
    Parse(String),
    InvalidCurrentVersion(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::AlreadyChecking => write!(f, "Update check is already in progress"),
            UpdateError::Network(why) => write!(f, "{why}"),
            UpdateError::Parse(why) => write!(f, "{why}"),
            UpdateError::InvalidCurrentVersion(ver) => {
                write!(f, "Invalid current version: {ver}")
            }
        }
    }
}

impl std::error::Error for UpdateError {}

/// True only for `https://github.com/katsugtgz/desktop-app/releases/...` with no credentials,
/// custom port, or control characters (and normalized by WHATWG `Url::parse` so `..` cannot escape).
pub fn is_trusted_release_url(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_RELEASE_URL_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return false;
    }
    let Ok(parsed) = Url::parse(trimmed) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed.host_str() == Some("github.com")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.port().is_none()
        && parsed.path().starts_with(RELEASE_PATH_PREFIX)
}

fn sanitize_label(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_RELEASE_TEXT_CHARS)
        .collect()
}

/// Selects the highest newer trusted release from `releases` relative to `current_version`.
pub fn select_update(
    current_version: &str,
    releases: &[ReleaseInfo],
    include_beta: bool,
) -> Result<UpdateCheckOutcome, UpdateError> {
    let current = SemVer::parse(current_version)
        .ok_or_else(|| UpdateError::InvalidCurrentVersion(current_version.to_owned()))?;

    let mut best: Option<(SemVer, &ReleaseInfo)> = None;
    for release in releases {
        if release.draft {
            continue;
        }
        let Some(version) = SemVer::parse(&release.tag_name) else {
            continue;
        };
        if !include_beta && (release.prerelease || version.is_prerelease()) {
            continue;
        }
        if version <= current {
            continue;
        }
        if !is_trusted_release_url(&release.html_url) {
            continue;
        }
        let replace = match &best {
            Some((best_ver, _)) => version > *best_ver,
            None => true,
        };
        if replace {
            best = Some((version, release));
        }
    }

    let Some((version, release)) = best else {
        return Ok(UpdateCheckOutcome::UpToDate);
    };

    let clean_name = sanitize_label(&release.name);
    let release_name = if clean_name.is_empty() {
        format!("v{version}")
    } else {
        clean_name
    };
    Ok(UpdateCheckOutcome::Available {
        release_name,
        html_url: release.html_url.trim().to_owned(),
    })
}

/// Testable seam for fetching release metadata (mocked in unit tests).
pub trait UpdateFetcher: Send + Sync {
    fn fetch_releases(&self) -> Result<Vec<ReleaseInfo>, UpdateError>;
}

/// Production GitHub Releases REST API client over `ureq` (rustls).
pub struct GitHubReleaseFetcher {
    endpoint: String,
}

impl Default for GitHubReleaseFetcher {
    fn default() -> Self {
        Self {
            endpoint: GITHUB_RELEASES_API.to_owned(),
        }
    }
}

impl GitHubReleaseFetcher {
    #[cfg(test)]
    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }
}

impl UpdateFetcher for GitHubReleaseFetcher {
    fn fetch_releases(&self) -> Result<Vec<ReleaseInfo>, UpdateError> {
        let agent = ureq::AgentBuilder::new().timeout(FETCH_TIMEOUT).build();
        let user_agent = format!("Station/{}", env!("CARGO_PKG_VERSION"));
        let response = agent
            .get(&self.endpoint)
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", &user_agent)
            .call()
            .map_err(|why| match why {
                ureq::Error::Status(status, _) => {
                    UpdateError::Network(format!("HTTP {status} from GitHub Releases"))
                }
                ureq::Error::Transport(_) => {
                    UpdateError::Network("Network error contacting GitHub Releases".to_owned())
                }
            })?;

        let mut body = Vec::new();
        response
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|why| UpdateError::Network(format!("Failed reading response: {why}")))?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(UpdateError::Parse(
                "Releases response exceeded size limit".to_owned(),
            ));
        }

        serde_json::from_slice::<Vec<ReleaseInfo>>(&body)
            .map_err(|_| UpdateError::Parse("Malformed GitHub Releases JSON".to_owned()))
    }
}

struct UpdaterRuntime {
    in_progress: bool,
    completed: Option<Result<UpdateCheckOutcome, UpdateError>>,
}

static UPDATER: Mutex<UpdaterRuntime> = Mutex::new(UpdaterRuntime {
    in_progress: false,
    completed: None,
});

/// Drains a completed background update check (called from `webview::poll_at`).
pub fn take_completed() -> Option<Result<UpdateCheckOutcome, UpdateError>> {
    #[cfg(test)]
    let _serial = crate::test_support::lock();
    let mut guard = UPDATER.lock().unwrap_or_else(|p| p.into_inner());
    guard.completed.take()
}

/// Clears updater runtime state between unit tests.
#[cfg(test)]
pub fn reset_for_tests() {
    let _serial = crate::test_support::lock();
    let mut guard = UPDATER.lock().unwrap_or_else(|p| p.into_inner());
    guard.in_progress = false;
    guard.completed = None;
}

/// Launches one background update check against `fetcher`, waking Rune when finished.
pub fn start_check(
    current_version: String,
    include_beta: bool,
    fetcher: Arc<dyn UpdateFetcher>,
    waker: Option<rune::Waker>,
) -> Result<(), UpdateError> {
    {
        let mut guard = UPDATER.lock().unwrap_or_else(|p| p.into_inner());
        if guard.in_progress {
            return Err(UpdateError::AlreadyChecking);
        }
        guard.in_progress = true;
        guard.completed = None;
    }

    std::thread::Builder::new()
        .name("station-updater".to_owned())
        .spawn(move || {
            let outcome = fetcher
                .fetch_releases()
                .and_then(|releases| select_update(&current_version, &releases, include_beta));
            let mut guard = UPDATER.lock().unwrap_or_else(|p| p.into_inner());
            guard.in_progress = false;
            guard.completed = Some(outcome);
            drop(guard);
            if let Some(wake) = waker {
                wake();
            }
        })
        .map_err(|why| {
            let mut guard = UPDATER.lock().unwrap_or_else(|p| p.into_inner());
            guard.in_progress = false;
            UpdateError::Network(format!("Could not start update worker: {why}"))
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{Ipv4Addr, TcpListener};

    #[test]
    fn semver_ordering_matches_semver_2_0_precedence() {
        let ordered = [
            "0.1.0",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "v1.0.0",
            "1.0.0+build.42",
            "1.0.1",
            "1.2.0",
            "2.0.0",
        ];
        let parsed: Vec<SemVer> = ordered
            .iter()
            .map(|v| SemVer::parse(v).unwrap_or_else(|| panic!("parse {v}")))
            .collect();
        // `v1.0.0` and `1.0.0+build.42` compare equal (build metadata ignored).
        assert_eq!(parsed[8], parsed[9]);
        for pair in parsed.windows(2) {
            assert!(pair[0] <= pair[1], "expected {} <= {}", pair[0], pair[1]);
        }
        assert!(parsed[7] < parsed[8], "rc.1 must be lower than stable");
        assert!(parsed[0] < parsed[1]);
        assert!(SemVer::parse("not-a-version").is_none());
        assert!(SemVer::parse("1.2").is_none());
        assert!(SemVer::parse("1.2.3-").is_none());
    }

    #[test]
    fn select_update_filters_drafts_betas_and_hostile_urls() {
        let releases = vec![
            ReleaseInfo {
                tag_name: "v9.9.9".to_owned(),
                name: "Draft 9.9.9".to_owned(),
                html_url: "https://github.com/katsugtgz/desktop-app/releases/tag/v9.9.9".to_owned(),
                draft: true,
                prerelease: false,
            },
            ReleaseInfo {
                tag_name: "v8.0.0".to_owned(),
                name: "Evil Mirror".to_owned(),
                html_url: "https://evil.example/katsugtgz/desktop-app/releases/tag/v8.0.0"
                    .to_owned(),
                draft: false,
                prerelease: false,
            },
            ReleaseInfo {
                tag_name: "v7.0.0".to_owned(),
                name: "Path Traversal".to_owned(),
                html_url: "https://github.com/katsugtgz/desktop-app/releases/../../other/repo"
                    .to_owned(),
                draft: false,
                prerelease: false,
            },
            ReleaseInfo {
                tag_name: "v2.0.0-beta.1".to_owned(),
                name: "Station 2.0 Beta 1".to_owned(),
                html_url: "https://github.com/katsugtgz/desktop-app/releases/tag/v2.0.0-beta.1"
                    .to_owned(),
                draft: false,
                prerelease: true,
            },
            ReleaseInfo {
                tag_name: "v1.5.0".to_owned(),
                name: "Station 1.5.0\nSafe".to_owned(),
                html_url: "https://github.com/katsugtgz/desktop-app/releases/tag/v1.5.0".to_owned(),
                draft: false,
                prerelease: false,
            },
        ];

        // Stable channel picks 1.5.0, skipping draft, evil URL, traversal URL, and beta.
        let stable = select_update("1.0.0", &releases, false).expect("stable");
        assert_eq!(
            stable,
            UpdateCheckOutcome::Available {
                release_name: "Station 1.5.0Safe".to_owned(),
                html_url: "https://github.com/katsugtgz/desktop-app/releases/tag/v1.5.0".to_owned(),
            }
        );

        // Beta channel picks 2.0.0-beta.1 over 1.5.0.
        let beta = select_update("1.0.0", &releases, true).expect("beta");
        assert_eq!(
            beta,
            UpdateCheckOutcome::Available {
                release_name: "Station 2.0 Beta 1".to_owned(),
                html_url: "https://github.com/katsugtgz/desktop-app/releases/tag/v2.0.0-beta.1"
                    .to_owned(),
            }
        );

        // Already on 2.0.0 -> up to date on both channels.
        assert_eq!(
            select_update("2.0.0", &releases, true).expect("up to date"),
            UpdateCheckOutcome::UpToDate
        );
    }

    #[test]
    fn github_fetcher_parses_loopback_response_and_enforces_size_limit() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = listener.local_addr().unwrap().port();
        let body = r#"[{"tag_name":"v2.1.0","name":"Station 2.1.0","html_url":"https://github.com/katsugtgz/desktop-app/releases/tag/v2.1.0","draft":false,"prerelease":false}]"#;

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut req_buf = [0u8; 2048];
            let n = stream.read(&mut req_buf).expect("read request");
            let req = String::from_utf8_lossy(&req_buf[..n]);
            assert!(
                req.to_ascii_lowercase().contains("user-agent: station/"),
                "GitHub API requires User-Agent header: {req}"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).expect("write response");
        });

        let fetcher = GitHubReleaseFetcher::with_endpoint(format!("http://127.0.0.1:{port}/rel"));
        let releases = fetcher.fetch_releases().expect("fetch ok");
        server.join().expect("server join");
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag_name, "v2.1.0");

        // Oversized response is refused cleanly.
        let big_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind big");
        let big_port = big_listener.local_addr().unwrap().port();
        let big_server = std::thread::spawn(move || {
            let (mut stream, _) = big_listener.accept().expect("accept big");
            let mut req_buf = [0u8; 1024];
            let _ = stream.read(&mut req_buf);
            let payload = vec![b' '; (MAX_RESPONSE_BYTES as usize) + 64];
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&payload);
        });
        let big_fetcher =
            GitHubReleaseFetcher::with_endpoint(format!("http://127.0.0.1:{big_port}/big"));
        let err = big_fetcher.fetch_releases().expect_err("oversized refused");
        assert!(matches!(err, UpdateError::Parse(_)));
        big_server.join().expect("big server join");
    }
}
