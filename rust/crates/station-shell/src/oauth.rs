//! Native Google OAuth sign-in (Option B from `docs/oauth-config-options.md`):
//! system browser + RFC 7636 PKCE (S256) + single-shot `127.0.0.1` loopback listener.
//!
//! Security & privacy invariants:
//! - Google refuses embedded webview OAuth (`disallowed_useragent`), so authorization
//!   always opens in the OS browser via `routing::open_external`.
//! - Binds `127.0.0.1:0` only (never `0.0.0.0`), accepts a single valid callback then
//!   closes, verifies CSRF `state` in constant time, caps request size at 8 KiB, and
//!   times out after 180 s (or immediately on user cancellation).
//! - Callback HTML responses are completely static (zero reflection of query strings)
//!   with `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'`.
//! - Stores **zero** access or refresh tokens: only the ID token's profile claims
//!   (`sub`, `email`, `name`, `given_name`, `family_name`, `picture`) become a
//!   `session_store::Identity`, sidestepping encrypted-token-at-rest requirements.
//! - Authorization codes, PKCE verifiers, and tokens are never logged.

use std::fmt;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::Deserialize;
use session_store::{Identity, IdentityProfile, MAX_IDENTITY_FIELD_CHARS};
use sha2::{Digest, Sha256};
use url::Url;

/// Google's OAuth 2.0 authorization endpoint.
pub const GOOGLE_AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Google's OAuth 2.0 token endpoint.
pub const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Scopes requested for identity metadata only (`docs/oauth-config-options.md` B3).
pub const GOOGLE_SCOPES: &str = "openid email profile";

/// Callback path on the ephemeral loopback listener.
pub const CALLBACK_PATH: &str = "/callback";

/// Maximum bytes read from a single loopback HTTP request.
pub const MAX_REQUEST_BYTES: usize = 8192;

/// Maximum bytes read from the token endpoint response.
pub const MAX_TOKEN_RESPONSE_BYTES: u64 = 256 * 1024;

/// Maximum bytes allowed in a decoded JWT payload.
pub const MAX_JWT_PAYLOAD_BYTES: usize = 32 * 1024;

/// Maximum authorization code length accepted from the loopback query.
pub const MAX_CODE_CHARS: usize = 2048;

/// Socket read/write timeout on a single loopback connection.
pub const SOCKET_IO_TIMEOUT: Duration = Duration::from_secs(2);

/// Overall sign-in timeout before the loopback server stops listening.
pub const FLOW_TIMEOUT: Duration = Duration::from_secs(180);

/// Token exchange HTTP timeout.
pub const TOKEN_HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Client configuration read from environment (`GOOGLE_CLIENT_ID`, optional `GOOGLE_CLIENT_SECRET`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
}

impl OAuthConfig {
    /// Reads Google OAuth client configuration from environment variables.
    /// Returns `None` when `GOOGLE_CLIENT_ID` is unset or blank.
    pub fn from_env() -> Option<Self> {
        let client_id = std::env::var("GOOGLE_CLIENT_ID")
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())?;
        let client_secret = std::env::var("GOOGLE_CLIENT_SECRET")
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        Some(Self {
            client_id,
            client_secret,
        })
    }
}

/// RFC 7636 PKCE pair (`S256`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
}

impl PkceChallenge {
    /// Generates a fresh 256-bit PKCE verifier and its `S256` challenge.
    pub fn generate() -> Result<Self, OAuthError> {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes)
            .map_err(|_| OAuthError::Random("OS RNG unavailable".to_owned()))?;
        Ok(Self::from_verifier(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// Computes the `S256` challenge from an existing verifier (deterministic for tests).
    pub fn from_verifier(verifier: String) -> Self {
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(digest);
        Self {
            verifier,
            challenge,
        }
    }
}

/// Generates a fresh 192-bit CSRF `state` token (base64url, 32 chars).
pub fn generate_state() -> Result<String, OAuthError> {
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| OAuthError::Random("OS RNG unavailable".to_owned()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Builds the Google OAuth 2.0 authorization URL with proper query encoding.
pub fn authorize_url(
    config: &OAuthConfig,
    redirect_uri: &str,
    pkce: &PkceChallenge,
    state: &str,
) -> Result<String, OAuthError> {
    let mut url = Url::parse(GOOGLE_AUTH_ENDPOINT)
        .map_err(|why| OAuthError::Config(format!("invalid auth endpoint: {why}")))?;
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", GOOGLE_SCOPES)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("prompt", "select_account");
    Ok(url.into())
}

/// Failure modes of the OAuth sign-in flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthError {
    NotConfigured,
    AlreadyInProgress,
    Cancelled,
    TimedOut,
    StateMismatch,
    MissingCode,
    ProviderError(String),
    Loopback(String),
    TokenExchange(String),
    InvalidIdToken(String),
    Random(String),
    Config(String),
}

impl fmt::Display for OAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OAuthError::NotConfigured => write!(
                f,
                "Google sign-in requires GOOGLE_CLIENT_ID in the environment"
            ),
            OAuthError::AlreadyInProgress => write!(f, "Sign-in is already in progress"),
            OAuthError::Cancelled => write!(f, "Sign-in was cancelled"),
            OAuthError::TimedOut => write!(f, "Sign-in timed out waiting for the browser callback"),
            OAuthError::StateMismatch => write!(f, "Sign-in rejected: OAuth state mismatch"),
            OAuthError::MissingCode => write!(f, "Sign-in callback did not include a code"),
            OAuthError::ProviderError(code) => write!(f, "Google sign-in declined ({code})"),
            OAuthError::Loopback(why) => write!(f, "Local OAuth callback failed: {why}"),
            OAuthError::TokenExchange(why) => write!(f, "Token exchange failed: {why}"),
            OAuthError::InvalidIdToken(why) => write!(f, "Invalid identity token: {why}"),
            OAuthError::Random(why) => write!(f, "Randomness error: {why}"),
            OAuthError::Config(why) => write!(f, "OAuth configuration error: {why}"),
        }
    }
}

impl std::error::Error for OAuthError {}

/// Single-shot `127.0.0.1` loopback HTTP listener for the OAuth redirect callback.
pub struct LoopbackServer {
    listener: TcpListener,
    port: u16,
}

impl LoopbackServer {
    /// Binds an ephemeral port on `127.0.0.1` only.
    pub fn bind() -> Result<Self, OAuthError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|why| OAuthError::Loopback(format!("bind failed: {why}")))?;
        let port = listener
            .local_addr()
            .map_err(|why| OAuthError::Loopback(format!("local_addr failed: {why}")))?
            .port();
        listener
            .set_nonblocking(true)
            .map_err(|why| OAuthError::Loopback(format!("nonblocking failed: {why}")))?;
        Ok(Self { listener, port })
    }

    #[cfg(test)]
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }

    /// Waits for a single callback request carrying a valid `state`, returning the authorization
    /// `code`. Ignores `/favicon.ico` requests and exits promptly on cancellation or timeout.
    pub fn wait_for_callback(
        self,
        expected_state: &str,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<String, OAuthError> {
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(OAuthError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(OAuthError::TimedOut);
            }
            match self.listener.accept() {
                Ok((stream, _peer)) => {
                    if let Some(result) = handle_connection(stream, expected_state) {
                        return result;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(err) => {
                    return Err(OAuthError::Loopback(format!("accept failed: {err}")));
                }
            }
        }
    }
}

/// Static HTML body for a successful OAuth callback (zero reflection of user/query input).
const SUCCESS_HTML: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>Signed in to Station</title>\
<style>body{font-family:system-ui,-apple-system,sans-serif;background:#111318;color:#f3f4f6;\
display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}\
main{max-width:420px;padding:32px;border-radius:12px;background:#1b1f27;border:1px solid #2d3340;\
text-align:center}h1{font-size:20px;margin:0 0 10px}p{font-size:14px;color:#9ca3af;margin:0}</style>\
</head><body><main><h1>Signed in to Station</h1>\
<p>Authentication succeeded. You can close this tab and return to Station.</p>\
</main></body></html>";

/// Static HTML body for a failed or rejected OAuth callback (zero query reflection).
const ERROR_HTML: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>Station sign-in could not complete</title>\
<style>body{font-family:system-ui,-apple-system,sans-serif;background:#111318;color:#f3f4f6;\
display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}\
main{max-width:420px;padding:32px;border-radius:12px;background:#1b1f27;border:1px solid #2d3340;\
text-align:center}h1{font-size:20px;margin:0 0 10px}p{font-size:14px;color:#9ca3af;margin:0}</style>\
</head><body><main><h1>Sign-in did not complete</h1>\
<p>Station rejected or cancelled this callback. You can close this tab and retry in Station.</p>\
</main></body></html>";

fn write_http_response(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n\
         {body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn write_no_content(stream: &mut TcpStream) {
    let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
    let _ = stream.flush();
}

/// Handles one incoming loopback TCP connection. Returns `None` when the request was an ignored
/// auxiliary request (such as `/favicon.ico`) and the server should keep waiting.
fn handle_connection(
    mut stream: TcpStream,
    expected_state: &str,
) -> Option<Result<String, OAuthError>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(SOCKET_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(SOCKET_IO_TIMEOUT));

    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() + n > MAX_REQUEST_BYTES {
                    write_http_response(&mut stream, "413 Payload Too Large", ERROR_HTML);
                    return Some(Err(OAuthError::Loopback(
                        "callback request exceeded 8 KiB cap".to_owned(),
                    )));
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let text = match std::str::from_utf8(&buf) {
        Ok(text) => text,
        Err(_) => {
            write_http_response(&mut stream, "400 Bad Request", ERROR_HTML);
            return Some(Err(OAuthError::Loopback("non-UTF-8 request".to_owned())));
        }
    };
    let request_line = text.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();

    if method != "GET" || !target.starts_with('/') {
        write_http_response(&mut stream, "400 Bad Request", ERROR_HTML);
        return Some(Err(OAuthError::Loopback(
            "unexpected HTTP method or target".to_owned(),
        )));
    }

    let parsed = match Url::parse(&format!("http://127.0.0.1{target}")) {
        Ok(url) => url,
        Err(_) => {
            write_http_response(&mut stream, "400 Bad Request", ERROR_HTML);
            return Some(Err(OAuthError::Loopback(
                "malformed callback URL".to_owned(),
            )));
        }
    };

    if parsed.path() == "/favicon.ico" {
        write_no_content(&mut stream);
        return None;
    }

    if parsed.path() != CALLBACK_PATH && parsed.path() != "/" {
        write_http_response(&mut stream, "404 Not Found", ERROR_HTML);
        return None;
    }

    match parse_callback_query(&parsed, expected_state) {
        Ok(code) => {
            write_http_response(&mut stream, "200 OK", SUCCESS_HTML);
            Some(Ok(code))
        }
        Err(err) => {
            write_http_response(&mut stream, "400 Bad Request", ERROR_HTML);
            Some(Err(err))
        }
    }
}

/// Validates callback query parameters against `expected_state`.
pub fn parse_callback_query(url: &Url, expected_state: &str) -> Result<String, OAuthError> {
    let mut code: Option<String> = None;
    let mut state: Option<String> = None;
    let mut error: Option<String> = None;

    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" if code.is_none() => code = Some(value.into_owned()),
            "state" if state.is_none() => state = Some(value.into_owned()),
            "error" if error.is_none() => error = Some(value.into_owned()),
            _ => {}
        }
    }

    if let Some(err_code) = error {
        let clean: String = err_code
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(64)
            .collect();
        let clean = if clean.is_empty() {
            "unknown_error".to_owned()
        } else {
            clean
        };
        return Err(OAuthError::ProviderError(clean));
    }

    let given_state = state.ok_or(OAuthError::StateMismatch)?;
    if !crate::instance::token_matches(&given_state, expected_state) {
        return Err(OAuthError::StateMismatch);
    }

    let code = code
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty() && c.len() <= MAX_CODE_CHARS && !c.chars().any(char::is_control))
        .ok_or(OAuthError::MissingCode)?;

    Ok(code)
}

/// Seam for swapping the authorization code for an OIDC `id_token` (mocked in tests).
pub trait TokenExchanger: Send + Sync {
    fn exchange(
        &self,
        config: &OAuthConfig,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<String, OAuthError>;
}

/// Production Google token endpoint exchanger over `ureq` (rustls).
pub struct GoogleTokenExchanger;

#[derive(Deserialize)]
struct TokenEndpointResponse {
    id_token: Option<String>,
}

impl TokenExchanger for GoogleTokenExchanger {
    fn exchange(
        &self,
        config: &OAuthConfig,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<String, OAuthError> {
        let agent = ureq::AgentBuilder::new()
            .timeout(TOKEN_HTTP_TIMEOUT)
            .build();
        let mut form: Vec<(&str, &str)> = vec![
            ("client_id", config.client_id.as_str()),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ];
        if let Some(secret) = config.client_secret.as_deref() {
            form.push(("client_secret", secret));
        }
        let response =
            agent
                .post(GOOGLE_TOKEN_ENDPOINT)
                .send_form(&form)
                .map_err(|why| match why {
                    ureq::Error::Status(status, _) => {
                        OAuthError::TokenExchange(format!("HTTP {status} from token endpoint"))
                    }
                    ureq::Error::Transport(_) => {
                        OAuthError::TokenExchange("network transport error".to_owned())
                    }
                })?;

        let mut body = Vec::new();
        response
            .into_reader()
            .take(MAX_TOKEN_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|why| OAuthError::TokenExchange(format!("read failed: {why}")))?;
        if body.len() as u64 > MAX_TOKEN_RESPONSE_BYTES {
            return Err(OAuthError::TokenExchange(
                "token response exceeded size limit".to_owned(),
            ));
        }

        let parsed: TokenEndpointResponse = serde_json::from_slice(&body)
            .map_err(|_| OAuthError::TokenExchange("malformed JSON token response".to_owned()))?;
        parsed
            .id_token
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| OAuthError::InvalidIdToken("response omitted id_token".to_owned()))
    }
}

#[derive(Deserialize)]
struct IdTokenClaims {
    sub: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    given_name: String,
    #[serde(default)]
    family_name: String,
    #[serde(default)]
    picture: String,
}

fn sanitize_claim(value: &str) -> Result<String, OAuthError> {
    let trimmed = value.trim();
    if trimmed.chars().any(char::is_control) {
        return Err(OAuthError::InvalidIdToken(
            "claim contained control characters".to_owned(),
        ));
    }
    Ok(trimmed.chars().take(MAX_IDENTITY_FIELD_CHARS).collect())
}

/// Extracts a `session_store::Identity` from an OIDC JWT `id_token`.
///
/// The token arrived directly over TLS from `https://oauth2.googleapis.com/token` in response to
/// a PKCE-bound code exchange, so signature verification against JWKS is not needed for
/// profile-display metadata (per OIDC Core §3.1.3.7 for direct TLS token-endpoint responses).
/// Neither the raw JWT nor any access/refresh token is retained.
pub fn parse_id_token_claims(id_token: &str) -> Result<Identity, OAuthError> {
    let mut segments = id_token.split('.');
    let _header = segments
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| OAuthError::InvalidIdToken("missing JWT header".to_owned()))?;
    let payload_b64 = segments
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| OAuthError::InvalidIdToken("missing JWT payload".to_owned()))?;
    let _sig = segments
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| OAuthError::InvalidIdToken("missing JWT signature".to_owned()))?;
    if segments.next().is_some() {
        return Err(OAuthError::InvalidIdToken(
            "JWT had more than 3 segments".to_owned(),
        ));
    }

    let decoded = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| OAuthError::InvalidIdToken("invalid base64url payload".to_owned()))?;
    if decoded.len() > MAX_JWT_PAYLOAD_BYTES {
        return Err(OAuthError::InvalidIdToken(
            "JWT payload exceeded size limit".to_owned(),
        ));
    }

    let claims: IdTokenClaims = serde_json::from_slice(&decoded)
        .map_err(|_| OAuthError::InvalidIdToken("malformed JWT JSON claims".to_owned()))?;
    let sub = sanitize_claim(&claims.sub)?;
    if sub.is_empty() {
        return Err(OAuthError::InvalidIdToken("empty sub claim".to_owned()));
    }
    let identity_id = format!("google-{sub}");
    if identity_id.chars().count() > MAX_IDENTITY_FIELD_CHARS {
        return Err(OAuthError::InvalidIdToken(
            "identity id exceeded session limit".to_owned(),
        ));
    }

    let email = sanitize_claim(&claims.email)?;
    let display_name = sanitize_claim(&claims.name)?;
    let first_name = sanitize_claim(&claims.given_name)?;
    let last_name = sanitize_claim(&claims.family_name)?;
    let image_url = sanitize_claim(&claims.picture)?;

    Ok(Identity {
        identity_id,
        provider: "google".to_owned(),
        user_id: sub,
        profile_data: IdentityProfile {
            display_name,
            first_name,
            last_name,
            email,
            image_url,
        },
    })
}

/// Asynchronous sign-in worker state shared between the UI poll and the background thread.
struct FlowRuntime {
    in_progress: bool,
    cancel: Option<Arc<AtomicBool>>,
    completed: Option<Result<Identity, OAuthError>>,
}

static FLOW: Mutex<FlowRuntime> = Mutex::new(FlowRuntime {
    in_progress: false,
    cancel: None,
    completed: None,
});

/// True while a Google OAuth flow is actively waiting on the browser callback or token exchange.
pub fn is_in_progress() -> bool {
    FLOW.lock().unwrap_or_else(|p| p.into_inner()).in_progress
}

/// Cancels any active OAuth sign-in flow; the loopback listener exits within ~25 ms.
pub fn cancel_sign_in() -> bool {
    let mut guard = FLOW.lock().unwrap_or_else(|p| p.into_inner());
    if !guard.in_progress {
        return false;
    }
    if let Some(flag) = &guard.cancel {
        flag.store(true, Ordering::Relaxed);
    }
    guard.in_progress = false;
    guard.cancel = None;
    true
}

/// Drains a finished OAuth result from the background worker (called from `webview::poll_at`).
pub fn take_completed() -> Option<Result<Identity, OAuthError>> {
    let mut guard = FLOW.lock().unwrap_or_else(|p| p.into_inner());
    guard.completed.take()
}

/// Starts a background Google OAuth sign-in flow using the system browser.
/// Returns the authorization URL opened, or an immediate configuration/concurrency error.
pub fn start_google_sign_in(
    config: OAuthConfig,
    exchanger: Arc<dyn TokenExchanger>,
    open_browser: impl FnOnce(&str),
    waker: Option<rune::Waker>,
) -> Result<String, OAuthError> {
    let pkce = PkceChallenge::generate()?;
    let state = generate_state()?;
    let server = LoopbackServer::bind()?;
    let redirect_uri = server.redirect_uri();
    let url = authorize_url(&config, &redirect_uri, &pkce, &state)?;

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut guard = FLOW.lock().unwrap_or_else(|p| p.into_inner());
        if guard.in_progress {
            return Err(OAuthError::AlreadyInProgress);
        }
        guard.in_progress = true;
        guard.cancel = Some(Arc::clone(&cancel));
        guard.completed = None;
    }

    open_browser(&url);

    let thread_cancel = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("station-oauth".to_owned())
        .spawn(move || {
            let outcome = server
                .wait_for_callback(&state, FLOW_TIMEOUT, &thread_cancel)
                .and_then(|code| exchanger.exchange(&config, &code, &pkce.verifier, &redirect_uri))
                .and_then(|id_token| parse_id_token_claims(&id_token));

            let mut guard = FLOW.lock().unwrap_or_else(|p| p.into_inner());
            guard.in_progress = false;
            guard.cancel = None;
            // Suppress Cancelled errors if user explicitly cancelled (already cleared UI state).
            if !matches!(outcome, Err(OAuthError::Cancelled)) {
                guard.completed = Some(outcome);
            }
            drop(guard);

            if let Some(wake) = waker {
                wake();
            }
        })
        .map_err(|why| {
            let mut guard = FLOW.lock().unwrap_or_else(|p| p.into_inner());
            guard.in_progress = false;
            guard.cancel = None;
            OAuthError::Loopback(format!("spawn worker failed: {why}"))
        })?;

    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_jwt(payload: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
        let body = URL_SAFE_NO_PAD.encode(payload.to_string());
        let sig = URL_SAFE_NO_PAD.encode("fake-signature");
        format!("{header}.{body}.{sig}")
    }

    #[test]
    fn pkce_s256_matches_rfc7636_test_vector() {
        // RFC 7636 Appendix B test vector.
        let pkce =
            PkceChallenge::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_owned());
        assert_eq!(
            pkce.challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorize_url_carries_pkce_state_and_loopback_redirect() {
        let config = OAuthConfig {
            client_id: "123.apps.googleusercontent.com".to_owned(),
            client_secret: None,
        };
        let pkce = PkceChallenge::from_verifier("verifier-123".to_owned());
        let url =
            authorize_url(&config, "http://127.0.0.1:45678/callback", &pkce, "st-99").expect("url");
        let parsed = Url::parse(&url).expect("parse");
        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some("accounts.google.com"));
        let params: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(
            params.get("client_id").map(String::as_str),
            Some("123.apps.googleusercontent.com")
        );
        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some("http://127.0.0.1:45678/callback")
        );
        assert_eq!(
            params.get("response_type").map(String::as_str),
            Some("code")
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            params.get("code_challenge").map(String::as_str),
            Some(pkce.challenge.as_str())
        );
        assert_eq!(params.get("state").map(String::as_str), Some("st-99"));
        assert_eq!(
            params.get("scope").map(String::as_str),
            Some("openid email profile")
        );
    }

    #[test]
    fn loopback_server_accepts_valid_callback_ignores_favicon_and_rejects_bad_state() {
        // 1. Valid callback after an ignored /favicon.ico probe
        let server = LoopbackServer::bind().expect("bind");
        let port = server.port();
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = std::thread::spawn(move || {
            server.wait_for_callback("expected-state-123", Duration::from_secs(5), &cancel)
        });

        // Favicon request returns 204 and does not terminate listener.
        let mut fav = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        fav.write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let mut fav_resp = String::new();
        fav.read_to_string(&mut fav_resp).unwrap();
        assert!(fav_resp.starts_with("HTTP/1.1 204"));

        // Valid callback with XSS payload in an ignored param — must NOT reflect in HTML!
        let mut cb = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        cb.write_all(
            b"GET /callback?code=auth-code-xyz&state=expected-state-123&xss=%3Cscript%3Ealert(1)%3C/script%3E HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        )
        .unwrap();
        let mut cb_resp = String::new();
        cb.read_to_string(&mut cb_resp).unwrap();
        assert!(cb_resp.starts_with("HTTP/1.1 200 OK"));
        assert!(cb_resp.contains("Signed in to Station"));
        assert!(!cb_resp.contains("alert(1)"));
        assert!(!cb_resp.contains("auth-code-xyz"));

        assert_eq!(handle.join().unwrap(), Ok("auth-code-xyz".to_owned()));

        // 2. Wrong state returns 400 and StateMismatch
        let server2 = LoopbackServer::bind().expect("bind2");
        let port2 = server2.port();
        let cancel2 = Arc::new(AtomicBool::new(false));
        let handle2 = std::thread::spawn(move || {
            server2.wait_for_callback("real-state", Duration::from_secs(5), &cancel2)
        });
        let mut bad = TcpStream::connect((Ipv4Addr::LOCALHOST, port2)).unwrap();
        bad.write_all(b"GET /callback?code=abc&state=wrong HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let mut bad_resp = String::new();
        bad.read_to_string(&mut bad_resp).unwrap();
        assert!(bad_resp.starts_with("HTTP/1.1 400 Bad Request"));
        assert_eq!(handle2.join().unwrap(), Err(OAuthError::StateMismatch));
    }

    #[test]
    fn id_token_claims_map_to_identity_without_tokens() {
        let jwt = make_jwt(json!({
            "sub": "108234567890",
            "email": "ada@example.com",
            "name": "Ada Lovelace",
            "given_name": "Ada",
            "family_name": "Lovelace",
            "picture": "https://lh3.googleusercontent.com/ada.png"
        }));
        let id = parse_id_token_claims(&jwt).expect("identity");
        assert_eq!(id.identity_id, "google-108234567890");
        assert_eq!(id.provider, "google");
        assert_eq!(id.user_id, "108234567890");
        assert_eq!(id.profile_data.email, "ada@example.com");
        assert_eq!(id.profile_data.display_name, "Ada Lovelace");
        assert_eq!(id.profile_data.first_name, "Ada");
        assert_eq!(id.profile_data.last_name, "Lovelace");
        assert_eq!(
            id.profile_data.image_url,
            "https://lh3.googleusercontent.com/ada.png"
        );
    }

    #[test]
    fn hostile_id_token_is_refused() {
        assert!(parse_id_token_claims("not-a-jwt").is_err());
        assert!(parse_id_token_claims("a.b.c.d").is_err());
        let empty_sub = make_jwt(json!({ "sub": "", "email": "a@b.c" }));
        assert!(parse_id_token_claims(&empty_sub).is_err());
        let ctrl_email = make_jwt(json!({ "sub": "123", "email": "a@b\u{7}.c" }));
        assert!(parse_id_token_claims(&ctrl_email).is_err());
    }

    /// Live real-Chromium verification via `agent-browser` (run explicitly with `--ignored`).
    #[test]
    #[ignore]
    fn live_chrome_agent_browser_oauth_callback_verify() {
        use std::process::Command;

        fn ab(args: &[&str]) -> String {
            let out = Command::new("agent-browser")
                .arg("--session")
                .arg("station-oauth-verify")
                .args(args)
                .output()
                .expect("run agent-browser");
            assert!(
                out.status.success(),
                "agent-browser {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }

        // 1. State mismatch rejected in real Chrome
        let srv_bad = LoopbackServer::bind().expect("bind bad");
        let url_bad = format!(
            "http://127.0.0.1:{}/callback?code=bad&state=wrong",
            srv_bad.port()
        );
        let c1 = Arc::new(AtomicBool::new(false));
        let h1 = std::thread::spawn(move || {
            srv_bad.wait_for_callback("expected-live-state", Duration::from_secs(15), &c1)
        });
        ab(&["open", &url_bad]);
        let bad_title = ab(&["get", "title"]);
        assert!(
            bad_title.contains("could not complete"),
            "unexpected error title: {bad_title}"
        );
        assert_eq!(h1.join().unwrap(), Err(OAuthError::StateMismatch));

        // 2. Provider error rejected in real Chrome
        let srv_err = LoopbackServer::bind().expect("bind err");
        let url_err = format!(
            "http://127.0.0.1:{}/callback?error=access_denied&state=expected-live-state",
            srv_err.port()
        );
        let c2 = Arc::new(AtomicBool::new(false));
        let h2 = std::thread::spawn(move || {
            srv_err.wait_for_callback("expected-live-state", Duration::from_secs(15), &c2)
        });
        ab(&["open", &url_err]);
        assert_eq!(
            h2.join().unwrap(),
            Err(OAuthError::ProviderError("access_denied".to_owned()))
        );

        // 3. Valid callback with XSS probe renders confirmation page cleanly with zero XSS
        let srv_ok = LoopbackServer::bind().expect("bind ok");
        let url_ok = format!(
            "http://127.0.0.1:{}/callback?code=chrome-live-code&state=expected-live-state&xss=%3Cscript%3Ewindow.__xss=1%3C/script%3E",
            srv_ok.port()
        );
        let c3 = Arc::new(AtomicBool::new(false));
        let h3 = std::thread::spawn(move || {
            srv_ok.wait_for_callback("expected-live-state", Duration::from_secs(15), &c3)
        });
        ab(&["open", &url_ok]);
        let ok_title = ab(&["get", "title"]);
        assert!(
            ok_title.contains("Signed in to Station"),
            "title: {ok_title}"
        );
        let xss_val = ab(&["eval", "String(window.__xss)"]);
        assert!(
            xss_val.contains("undefined"),
            "XSS probe executed: {xss_val}"
        );
        let body_text = ab(&["eval", "document.body.innerText"]);
        assert!(
            body_text.contains("Signed in to Station")
                && !body_text.contains("chrome-live-code")
                && !body_text.contains("__xss"),
            "body leaked params or wrong copy: {body_text}"
        );
        if let Ok(shot_path) = std::env::var("STATION_OAUTH_SNAP") {
            ab(&["screenshot", &shot_path]);
        }
        let _ = Command::new("agent-browser")
            .args(["--session", "station-oauth-verify", "close"])
            .output();
        assert_eq!(h3.join().unwrap(), Ok("chrome-live-code".to_owned()));
    }
}
