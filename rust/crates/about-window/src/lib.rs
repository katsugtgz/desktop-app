//! About-window state backend (port of the state handling of
//! `packages/app/src/about-window`): the update/auto-launch/beta flags and
//! app metadata the about window renders, the close-on-blur/Escape rules
//! from `about.js`, and the runtime versions block from
//! `AboutWindowVersions.tsx` (Electron window itself stays Electron — napi
//! shell decision; only state is ported).
//!
//! Semantics ported, redux not: where the TS selectors read
//! `state.getIn([...], default)` over immutable maps, here
//! [`AboutWindowState`] owns plain fields with the same defaults, and the
//! reducers of `auto-update/duck.js` + `app/duck.js` (only the actions the
//! about window dispatches or reads) become [`AboutWindowState`] methods.
//!
//! Not ported in this slice: React rendering (`Presenter.js`, the two
//! `components/*.tsx`), the redux store wiring (`about.js` bootstrap), and
//! actions never surfaced in the about window (release-notes subdock
//! visibility, downloads folder, fullscreen, ...).

use serde::{Deserialize, Serialize};

/// Update-flow + preference flags the about window shows.
///
/// Field-for-field the slice of `auto_update` + `app` redux state read by
/// `about-window/Container.js`, with the selectors' `getIn` defaults as
/// field defaults (`Default` gives the initial redux `Map()`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AboutWindowState {
    /// `isCheckingUpdate` — `auto-update/duck.js` `SET_CHECKING_FOR_UPDATE`.
    pub checking_update: bool,
    /// `isUpdateAvailable` — set true (with `release_name`) by
    /// `SET_UPDATE_IS_AVAILABLE`, never reset by that duck.
    pub update_available: bool,
    /// `isDownloadingUpdate` — `SET_DOWNLOADING_AVAILABLE`.
    pub downloading_update: bool,
    /// `getReleaseName` — `Option` like the absent-by-default Map entry.
    pub release_name: Option<String>,
    /// `areBetaIncludedInUpdates` — `app` duck `includesBetaInUpdates`,
    /// selector defaults to `false`.
    #[serde(default)]
    pub beta_included_in_updates: bool,
    /// `getAppAutoLaunchEnabledStatus` — selector coerces to bool in the
    /// container (`Boolean(...)`); `autoLaunchEnabled` itself can be
    /// unset, hence `Option`.
    pub auto_launch_enabled: Option<bool>,
    /// `getAppName` — `app` duck `SET_APP_METADATA`.
    #[serde(default)]
    pub app_name: String,
    /// `getAppVersion` — `app` duck `SET_APP_METADATA`.
    #[serde(default)]
    pub app_version: String,
}

impl AboutWindowState {
    /// `SET_CHECKING_FOR_UPDATE` (`checking` payload, action defaults
    /// `true`; pass it explicitly here).
    pub fn set_checking_for_update(&mut self, checking: bool) {
        self.checking_update = checking;
    }

    /// `SET_UPDATE_IS_AVAILABLE` — mirror of the duck: clears
    /// `downloading_update`, sets `update_available` and `release_name`.
    pub fn set_update_is_available(&mut self, release_name: impl Into<String>) {
        self.downloading_update = false;
        self.update_available = true;
        self.release_name = Some(release_name.into());
    }

    /// `SET_DOWNLOADING_AVAILABLE` (`startDownloading` payload).
    pub fn set_downloading_update(&mut self, start_downloading: bool) {
        self.downloading_update = start_downloading;
    }

    /// `SET_AUTO_LAUNCH_ENABLED` / the stored half of `enableAutoLaunch`.
    pub fn set_auto_launch_enabled(&mut self, enabled: bool) {
        self.auto_launch_enabled = Some(enabled);
    }

    /// `SET_INCLUDES_BETA_IN_UPDATES` / stored half of
    /// `includeBetaInUpdates`.
    pub fn set_beta_included_in_updates(&mut self, included: bool) {
        self.beta_included_in_updates = included;
    }

    /// `SET_APP_METADATA` (`name` + `version` payload).
    pub fn set_app_metadata(&mut self, name: impl Into<String>, version: impl Into<String>) {
        self.app_name = name.into();
        self.app_version = version.into();
    }
}

/// Why/how the about window asked the shell to close — the two rules in
/// `about.js`: window blur closes, Escape closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloseReason {
    /// `currentWindow.on('blur', ...)` → `currentWindow.close()`.
    Blur,
    /// `keydown` `Escape` → `currentWindow.close()`.
    EscapePressed,
}

impl CloseReason {
    /// The keydown switch in `about.js` only handles `Escape`; every other
    /// key falls through and leaves the window open.
    pub fn from_key(key: &str) -> Option<CloseReason> {
        match key {
            "Escape" => Some(CloseReason::EscapePressed),
            _ => None,
        }
    }
}

/// One line of the versions list in `AboutWindowVersions.tsx`
/// (Electron / Chrome / Node / v8 rows).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeVersion {
    pub label: &'static str,
    pub version: String,
}

/// The `process.versions` rows the about window renders. Row order and
/// labels match the TSX (lowercase `v8` included).
pub fn runtime_versions(process_versions: &serde_json::Value) -> Vec<RuntimeVersion> {
    let get = |key: &str| -> String {
        process_versions
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned()
    };
    vec![
        RuntimeVersion {
            label: "Electron",
            version: get("electron"),
        },
        RuntimeVersion {
            label: "Chrome",
            version: get("chrome"),
        },
        RuntimeVersion {
            label: "Node",
            version: get("node"),
        },
        RuntimeVersion {
            label: "v8",
            version: get("v8"),
        },
    ]
}

/// Should the window close for this reason, per `about.js`: both wired
/// reasons close.
pub fn should_close(reason: CloseReason) -> bool {
    match reason {
        CloseReason::Blur | CloseReason::EscapePressed => true,
    }
}

/// `AboutWindowFooter.tsx` copyright line: `2019 - {currentYear}`.
pub fn copyright_year(current_year: i32) -> String {
    format!("2019 - {current_year}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_selector_defaults() {
        let s = AboutWindowState::default();
        // selector getIn defaults: false flags, Option releaseName
        assert!(!s.checking_update && !s.update_available && !s.downloading_update);
        assert_eq!(s.release_name, None);
        // areBetaIncludedInUpdates defaults false; appName/appVersion absent
        assert!(!s.beta_included_in_updates);
        assert_eq!((s.app_name.as_str(), s.app_version.as_str()), ("", ""));
        assert_eq!(s.auto_launch_enabled, None);
    }

    #[test]
    fn update_flow_reduces_like_the_duck() {
        let mut s = AboutWindowState::default();
        // componentDidMount -> checkForUpdates saga -> SET_CHECKING_FOR_UPDATE
        s.set_checking_for_update(true);
        assert!(s.checking_update);
        // update found while a previous download was running
        s.set_downloading_update(true);
        s.set_update_is_available("6.1.0-beta");
        assert!(s.update_available);
        assert_eq!(s.release_name.as_deref(), Some("6.1.0-beta"));
        // duck clears downloading when an update becomes available
        assert!(!s.downloading_update);
        // then the download starts again
        s.set_downloading_update(true);
        assert!(s.downloading_update);
        // checking flag cleared on completion
        s.set_checking_for_update(false);
        assert!(!s.checking_update);
    }

    #[test]
    fn preference_and_metadata_reduces() {
        let mut s = AboutWindowState::default();
        // enableAutoLaunch(true) -> SET_AUTO_LAUNCH_ENABLED
        s.set_auto_launch_enabled(true);
        assert_eq!(s.auto_launch_enabled, Some(true));
        // includeBetaInUpdates(false) -> SET_INCLUDES_BETA_IN_UPDATES
        s.set_beta_included_in_updates(false);
        assert!(!s.beta_included_in_updates);
        // SET_APP_METADATA
        s.set_app_metadata("Station", "6.0.0");
        assert_eq!(
            (s.app_name.as_str(), s.app_version.as_str()),
            ("Station", "6.0.0")
        );
    }

    #[test]
    fn runtime_versions_rows_match_tsx() {
        let v = runtime_versions(&json!({
            "electron": "33.0.0", "chrome": "130.0.0", "node": "20.0.0", "v8": "13.0"
        }));
        let labels: Vec<_> = v.iter().map(|r| r.label).collect();
        assert_eq!(labels, ["Electron", "Chrome", "Node", "v8"]);
        assert_eq!(v[3].version, "13.0");
        // missing keys render empty, not errors (TSX would print undefined)
        let missing = runtime_versions(&json!({}));
        assert!(missing.iter().all(|r| r.version.is_empty()));
    }

    #[test]
    fn close_rules_match_about_js() {
        // both wired reasons close the window
        assert!(should_close(CloseReason::Blur));
        assert!(should_close(CloseReason::EscapePressed));
        // only Escape maps from a keydown; other keys leave the window open
        assert_eq!(
            CloseReason::from_key("Escape"),
            Some(CloseReason::EscapePressed)
        );
        assert_eq!(CloseReason::from_key("Enter"), None);
        assert_eq!(CloseReason::from_key("a"), None);
    }

    #[test]
    fn copyright_matches_footer() {
        assert_eq!(copyright_year(2026), "2019 - 2026");
    }

    #[test]
    fn state_roundtrips_through_json() {
        let mut s = AboutWindowState::default();
        s.set_update_is_available("7.0.0");
        s.set_app_metadata("Station", "7.0.0");
        let back: AboutWindowState =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
