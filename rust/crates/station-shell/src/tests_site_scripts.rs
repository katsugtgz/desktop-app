//! Slice 23 tests: per-site injected scripts and snooze fan-out to hosted pages.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use appstore_service::write_commands::{ApplicationConfigData, InstalledApplication};
use appstore_service::{ApplicationService, TabRecord};
use session_store::{SessionState, SessionStore};

use crate::notifications::INIT_SCRIPT;
use crate::site_scripts::{self, SUPPORTED_SERVICES};
use crate::state::{AppTab, ShellState};
use crate::webview::fake::{Event, FakeWebview};
use crate::webview::{self, PageEnvironment, PaneRect};

const RECT: PaneRect = PaneRect::new(0.0, 44.0, 1136.0, 756.0);
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn session_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "station-shell-site-scripts-{name}-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn seeded_session(path: &std::path::Path, application_id: &str) {
    SessionStore::new(path)
        .save(&SessionState {
            active_application: Some(application_id.to_owned()),
            installed: vec![InstalledApplication {
                application_id: application_id.to_owned(),
                manifest_url: format!("station-manifest://{application_id}"),
                install_context: None,
                home_tab_url: Some("https://example.com/".to_owned()),
                in_dock: true,
                config_data: ApplicationConfigData::default(),
            }],
            dock: vec![application_id.to_owned()],
            tabs: vec![TabRecord {
                tab_id: format!("tab-{application_id}"),
                application_id: application_id.to_owned(),
                url: "https://example.com/".to_owned(),
                home: true,
            }],
            ..SessionState::default()
        })
        .expect("seed");
}

#[test]
fn all_14_legacy_service_ids_resolve_to_static_scripts() {
    for service in SUPPORTED_SERVICES {
        let script = site_scripts::for_service(service)
            .unwrap_or_else(|| panic!("missing script for {service}"));
        assert!(
            script.starts_with("(() => {"),
            "{service} script should be wrapped in an IIFE"
        );
        assert!(!script.contains("eval("));
        assert!(!script.contains("new Function("));

        let combined = site_scripts::combined_init_script(Some(service));
        assert!(combined.starts_with(INIT_SCRIPT));
        assert!(combined.contains(site_scripts::CHROME_COMPAT_SCRIPT));
        assert!(combined.contains(script));
    }
    assert_eq!(site_scripts::for_service("unknown-app"), None);
    let base_combined = format!("{INIT_SCRIPT}\n{}", site_scripts::CHROME_COMPAT_SCRIPT);
    assert_eq!(site_scripts::combined_init_script(None), base_combined);
    assert_eq!(
        site_scripts::combined_init_script(Some("unknown-app")),
        base_combined
    );
}

#[test]
fn chrome_compat_shims_match_preload_inventory_and_stay_safe() {
    let shims = site_scripts::CHROME_COMPAT_SCRIPT;
    assert!(shims.starts_with("(() => {"));
    assert!(!shims.contains("eval("));
    assert!(!shims.contains("new Function("));
    for needle in [
        "webstore: true",
        "!window.chrome.runtime",
        "sendMessage",
        "sendNativeMessage",
        "delete window.chrome.runtime.lastError",
        "_docs_chrome_extension_exists",
        "_docs_chrome_extension_features_version",
        "_docs_chrome_extension_permissions",
    ] {
        assert!(shims.contains(needle), "missing {needle} in chrome shims");
    }
}

#[test]
fn bundled_manifests_propagate_legacy_service_id_into_page_environment() {
    for (manifest_id, expected_service) in [
        ("14", "gmail"),
        ("21", "slack"),
        ("34", "asana"),
        ("254", "office-365"),
        ("299", "meet"),
    ] {
        let path = session_path(manifest_id);
        seeded_session(&path, manifest_id);
        let state = ShellState::launch_in(
            ApplicationService::for_tests(&format!("site-{manifest_id}")),
            &path,
        );
        let tab = state
            .tabs()
            .iter()
            .find(|t| t.id == manifest_id)
            .unwrap_or_else(|| panic!("missing tab {manifest_id}"));
        assert_eq!(tab.legacy_service_id.as_deref(), Some(expected_service));
        let env = PageEnvironment::for_tab(tab);
        assert_eq!(env.legacy_service_id.as_deref(), Some(expected_service));
        assert!(site_scripts::for_service(expected_service).is_some());
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn snooze_changes_fan_out_to_all_attached_pages_and_newly_attached_pages() {
    let _serial = crate::test_support::lock();
    let (host, log) = FakeWebview::new();
    webview::install(Box::new(host));

    let mut state = ShellState::new(ApplicationService::for_tests("snooze-fanout"));
    let mut whatsapp = AppTab::web("whatsapp", "WhatsApp", "https://web.whatsapp.com");
    whatsapp.legacy_service_id = Some("whatsapp".to_owned());
    let mut messenger = AppTab::web("messenger", "Messenger", "https://www.messenger.com");
    messenger.legacy_service_id = Some("facebook-messenger".to_owned());
    state.tabs_mut_for_tests().push(whatsapp);
    state.tabs_mut_for_tests().push(messenger);

    // Attach WhatsApp while unsnoozed -> no snooze eval needed yet.
    state.select("whatsapp");
    webview::sync(&state, Some(RECT), 1.0);
    webview::poll_at(&mut state, 1.0);
    assert_eq!(
        log.environments()[0].legacy_service_id.as_deref(),
        Some("whatsapp")
    );
    assert!(!log.events().iter().any(|e| matches!(e, Event::Eval { .. })));

    // Enable 15 min snooze -> poll fans out to attached WhatsApp page.
    log.clear();
    state.snooze_notifications(Some("15min"));
    webview::poll_at(&mut state, 2.0);
    assert_eq!(
        log.events(),
        vec![Event::Eval {
            tab: "whatsapp".to_owned(),
            script: "window.__stationSnoozeUpdate(900000);".to_owned(),
        }]
    );

    // Polling again with unchanged snooze sends nothing.
    log.clear();
    webview::poll_at(&mut state, 3.0);
    assert!(log.events().is_empty());

    // Attach Messenger while snoozed -> Messenger immediately receives active snooze!
    log.clear();
    state.select("messenger");
    webview::sync(&state, Some(RECT), 1.0);
    assert!(log.events().iter().any(|e| {
        matches!(
            e,
            Event::Eval { tab, script }
            if tab == "messenger" && script == "window.__stationSnoozeUpdate(900000);"
        )
    }));

    // Clear snooze -> both attached pages receive `null` deterministically.
    log.clear();
    state.snooze_notifications(None);
    webview::poll_at(&mut state, 4.0);
    assert_eq!(
        log.events(),
        vec![
            Event::Eval {
                tab: "messenger".to_owned(),
                script: "window.__stationSnoozeUpdate(null);".to_owned(),
            },
            Event::Eval {
                tab: "whatsapp".to_owned(),
                script: "window.__stationSnoozeUpdate(null);".to_owned(),
            },
        ]
    );
}
