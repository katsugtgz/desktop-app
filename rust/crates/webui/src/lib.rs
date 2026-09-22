//! WebUI `station://` protocol-handler resolution (port of
//! `packages/app/src/webui/webUIHandler.ts`): mapping a `station://`
//! request URL to the local file the Electron `protocol.handle` callback
//! should serve.
//!
//! Electron registration stays Electron — `registerSchemesAsPrivileged`
//! and `protocol.handle` live in the TS shell. What is ported is the
//! per-request decision logic, so s14 can call
//! [`resolve_webui_request`] through napi inside the handler callback
//! and `net.fetch()` the returned file URL.
//!
//! Semantics ported, not lines:
//!
//! - `handlers.find(h => h.hostname === parsedUrl.hostname)` — exact
//!   hostname match over the registered handlers; miss = 404 with body
//!   `Handler not found for {url}`.
//! - `if (!parsedUrl.pathname)` — reachable: `station` is a WHATWG
//!   non-special scheme, so `new URL('station://appstore').pathname`
//!   is `''` (not `/` like http URLs); miss = 404 with body `Empty
//!   path in {url}`.
//! - `pathname !== '/'` — subpath serves `dirname(filePath) +
//!   pathname.substring(1)` (path joins, then file URL).
//! - `pathname === '/'` — serves `filePath` itself.
//!
//! The TS [`Response`] bodies (404 text, the fetched file stream) are
//! the Electron side's job; [`WebuiResolution`] returns the file URL or
//! the 404 body so the callback can build them.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use url::Url;

/// `BX_PROTOCOL` (`packages/app/src/webui/const.ts`) — `station`. If
/// edited here, the TS const, `app/plugins/webview-preload.js` and
/// `app/webui/preload.js` must be edited too (see that file's comment).
pub const BX_PROTOCOL: &str = "station";

/// Port of `ProtocolHandler` (`packages/app/src/webui/types.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolHandler {
    /// Hostname this handler answers, e.g. `appstore`.
    pub hostname: String,
    /// Absolute path of the entry HTML file served at `/`.
    pub file_path: PathBuf,
}

/// What the Electron `protocol.handle` callback should do for a request.
///
/// `Serve` mirrors the `net.fetch(pathToFileURL(...))` returns; `NotFound`
/// mirrors the two 404 `Response` branches (body text included verbatim).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WebuiResolution {
    /// Fetch this `file://` URL and return the response.
    Serve { file_url: String },
    /// 404 with the ported body text.
    NotFound { body: String },
}

/// Resolve one `station://` request against the registered handlers.
///
/// Port of the `protocol.handle` callback body in `webUIHandler.ts`
/// `start()`. `handlers` is the `handlers.ts` list (multi-instance
/// configurator + appstore at time of port); `req_url` is `req.url`.
pub fn resolve_webui_request(handlers: &[ProtocolHandler], req_url: &str) -> WebuiResolution {
    let Ok(parsed) = Url::parse(req_url) else {
        return WebuiResolution::NotFound {
            body: format!("Handler not found for {req_url}"),
        };
    };

    let Some(handler) = handlers
        .iter()
        .find(|h| parsed.host_str() == Some(h.hostname.as_str()))
    else {
        return WebuiResolution::NotFound {
            body: format!("Handler not found for {req_url}"),
        };
    };

    // WHATWG non-special scheme: 'station://appstore' has empty
    // pathname (only http-like special schemes coerce it to '/'), so
    // the TS `!parsedUrl.pathname` branch is genuinely reachable.
    if parsed.path().is_empty() {
        return WebuiResolution::NotFound {
            body: format!("Empty path in {req_url}"),
        };
    }

    if parsed.path() != "/" {
        // `${dirname(handler.filePath)}/${pathname.substring(1)}`:
        // strip the leading '/', join onto the entry file's directory.
        let sub = &parsed.path()[1..];
        let file_path = join_subpath(&handler.file_path, sub);
        return WebuiResolution::Serve {
            file_url: path_to_file_url(&file_path),
        };
    }

    WebuiResolution::Serve {
        file_url: path_to_file_url(&handler.file_path),
    }
}

/// `dirname(filePath)` + `/${sub}` as the TS template string builds it:
/// plain string concatenation — path separators are normalized to the
/// platform's, like Node's `path` would see when the file is opened.
fn join_subpath(entry: &Path, sub: &str) -> PathBuf {
    // Query/fragment never reach file resolution in the TS handler
    // (`pathname` excludes them); percent-decoding is `net.fetch`'s job
    // on the TS side and the file URL round-trip's job here.
    let mut dir = entry.parent().map(Path::to_path_buf).unwrap_or_default();
    // `a//b` collapses empty segments the same way a URL pathname join
    // would.
    dir.extend(sub.split('/').filter(|s| !s.is_empty()));
    dir
}

/// `pathToFileURL(path).toString()` for absolute local paths: forward
/// slashes, percent-encoded special characters, always a trailing
/// authority (`file:///`).
fn path_to_file_url(path: &Path) -> String {
    let mut url = Url::from_file_path(path).expect("absolute path");
    url.set_fragment(None);
    url.to_string()
}

/// Default handler list, port of `handlers.ts`: the two registrations
/// that exist at time of port. `file_path` values are runtime build
/// outputs (`__dirname` / `__webpack_public_path__` dependent), so the
/// Electron side fills them; this returns the hostnames' shape only.
pub fn default_handler_hostnames() -> [&'static str; 2] {
    ["multi-instance-configurator", "appstore"]
}

/// Guard shared with the TS side's intent: a resolved subpath must stay
/// under the handler's directory. The TS handler does not check this
/// (Chromium blocks `..` in URL path normalization — `new URL` collapses
/// dot segments before `pathname` is read), so this is only an assert
/// helper for tests mirroring that guarantee.
pub fn subpath_stays_in_dir(entry: &Path, resolved: &Path) -> bool {
    let dir = entry.parent().unwrap_or(Path::new(""));
    resolved
        .components()
        .all(|c| !matches!(c, Component::ParentDir))
        && resolved.starts_with(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CI runs Linux; `Url::from_file_path` rejects non-absolute
    /// (there: Windows-drive) paths, so fixtures must be per-OS.
    fn entry_paths() -> (&'static str, &'static str) {
        if cfg!(windows) {
            (
                "C:/app/multi-instance-configuration.html",
                "C:/app/appstore/index.html",
            )
        } else {
            (
                "/app/multi-instance-configuration.html",
                "/app/appstore/index.html",
            )
        }
    }

    fn expected_url(win: &str, unix: &str) -> String {
        if cfg!(windows) {
            win.into()
        } else {
            unix.into()
        }
    }

    fn handlers() -> Vec<ProtocolHandler> {
        let (mic, appstore) = entry_paths();
        vec![
            ProtocolHandler {
                hostname: "multi-instance-configurator".into(),
                file_path: PathBuf::from(mic),
            },
            ProtocolHandler {
                hostname: "appstore".into(),
                file_path: PathBuf::from(appstore),
            },
        ]
    }

    #[test]
    fn root_serves_entry_file() {
        // pathname '/' → net.fetch(pathToFileURL(filePath))
        let r = resolve_webui_request(&handlers(), "station://appstore/");
        assert_eq!(
            r,
            WebuiResolution::Serve {
                file_url: expected_url(
                    "file:///C:/app/appstore/index.html",
                    "file:///app/appstore/index.html",
                )
            }
        );
    }

    #[test]
    fn root_without_slash_404s_with_empty_path_body() {
        // WHATWG non-special scheme: new URL('station://appstore')
        // .pathname === '' — the TS `!parsedUrl.pathname` branch IS
        // reachable, and this crate reproduces it.
        let r = resolve_webui_request(&handlers(), "station://appstore");
        assert_eq!(
            r,
            WebuiResolution::NotFound {
                body: "Empty path in station://appstore".into()
            }
        );
    }

    #[test]
    fn subpath_serves_from_entry_dir() {
        // constants.ts 'station://appstore/static/custom-app-icons/icon-simple-1.svg'
        let r = resolve_webui_request(
            &handlers(),
            "station://appstore/static/custom-app-icons/icon-simple-1.svg",
        );
        assert_eq!(
            r,
            WebuiResolution::Serve {
                file_url: expected_url(
                    "file:///C:/app/appstore/static/custom-app-icons/icon-simple-1.svg",
                    "file:///app/appstore/static/custom-app-icons/icon-simple-1.svg",
                )
            }
        );
    }

    #[test]
    fn multi_instance_configurator_root() {
        let r = resolve_webui_request(&handlers(), "station://multi-instance-configurator/");
        assert_eq!(
            r,
            WebuiResolution::Serve {
                file_url: expected_url(
                    "file:///C:/app/multi-instance-configuration.html",
                    "file:///app/multi-instance-configuration.html",
                )
            }
        );
    }

    #[test]
    fn unknown_hostname_404s_with_ts_body() {
        let r = resolve_webui_request(&handlers(), "station://nope/x");
        assert_eq!(
            r,
            WebuiResolution::NotFound {
                body: "Handler not found for station://nope/x".into()
            }
        );
    }

    #[test]
    fn hostname_match_is_exact() {
        // 'appstore.example' is not 'appstore'
        let r = resolve_webui_request(&handlers(), "station://appstore.example/");
        assert!(matches!(r, WebuiResolution::NotFound { .. }));
    }

    #[test]
    fn unparseable_url_404s_like_handler_not_found() {
        // Electron hands the callback valid URLs; a relative/invalid one
        // cannot match any hostname, same terminal state as the TS miss.
        let r = resolve_webui_request(&handlers(), "not a url");
        assert!(matches!(r, WebuiResolution::NotFound { .. }));
    }

    #[test]
    fn dot_segments_cannot_escape_entry_dir() {
        // Chromium's URL normalization collapses dot segments before
        // pathname is read; url crate keeps them — assert the guard.
        let hs = handlers();
        let r = resolve_webui_request(&hs, "station://appstore/../../etc/passwd");
        let WebuiResolution::Serve { file_url } = r else {
            panic!("expected Serve");
        };
        let resolved = Url::parse(&file_url).unwrap().to_file_path().unwrap();
        assert!(subpath_stays_in_dir(&hs[1].file_path, &resolved));
    }

    #[test]
    fn query_and_fragment_do_not_leak_into_path() {
        // getMultiInstanceConfiguratorURL appends ?manifestURL=...
        let r = resolve_webui_request(
            &handlers(),
            "station://multi-instance-configurator/?manifestURL=https%3A%2F%2Fx.test%2Fm.json&applicationId=org.x",
        );
        assert_eq!(
            r,
            WebuiResolution::Serve {
                file_url: expected_url(
                    "file:///C:/app/multi-instance-configuration.html",
                    "file:///app/multi-instance-configuration.html",
                )
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_become_file_urls() {
        let hs = vec![ProtocolHandler {
            hostname: "appstore".into(),
            file_path: PathBuf::from(r"C:\app\appstore\index.html"),
        }];
        let r = resolve_webui_request(&hs, "station://appstore/static/x.svg");
        assert_eq!(
            r,
            WebuiResolution::Serve {
                file_url: "file:///C:/app/appstore/static/x.svg".into()
            }
        );
    }

    #[test]
    fn default_hostnames_match_handlers_ts() {
        assert_eq!(
            default_handler_hostnames(),
            ["multi-instance-configurator", "appstore"]
        );
    }
}
