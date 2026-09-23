//! Integration flow over the real exported functions, driving the global
//! `SERVICE` through a scratch `private-manifests.json` (same pattern as
//! `ApplicationService::for_tests`, but through the exported
//! `set_manifests_path_for_tests` so the napi layer's laziness is exercised).
//!
//! One serial `#[test]` per envelope family; they share `SERVICE` and would
//! race as parallel tests.

#![cfg(not(target_arch = "wasm32"))]

use serde_json::{json, Value};

fn scratch(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("station-w01-bridge-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("private-manifests.json")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn bridge_full_flow_envelopes_and_state() {
    station_bridge::set_manifests_path_for_tests(scratch("flow")).unwrap();

    // search: { body: MinimalApplication[] } — exact-name short circuit.
    let hits = station_bridge::search_applications("Gmail".into()).unwrap();
    assert_eq!(hits["body"][0]["id"], "14");
    assert_eq!(hits["body"][0]["bxAppManifestURL"], "station-manifest://14");
    assert_eq!(hits["body"][0]["isChromeExtension"], false);

    // getMostPopularApps: three ten-buckets with TS key names.
    let popular = station_bridge::get_most_popular_applications().unwrap();
    assert_eq!(
        popular["body"]["creamOfTheCropApps"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    assert_eq!(popular["body"]["runnerUps"].as_array().unwrap().len(), 10);
    assert_eq!(popular["body"]["noteworthy"].as_array().unwrap().len(), 10);
    assert_eq!(popular["body"]["creamOfTheCropApps"][0]["id"], "14");

    // getAllCategories: alphabetical, Miscellaneous forced last.
    let categories = station_bridge::get_all_categories().unwrap();
    let cats = categories["body"].as_array().unwrap();
    assert_eq!(cats.last().unwrap(), "Miscellaneous");
    assert!(cats.contains(&serde_json::json!("Developer Tools")));

    // getApplicationsByCategory: object map keyed by category.
    let grouped = station_bridge::get_applications_by_category().unwrap();
    let comm = &grouped["body"]["Communication & Collaboration"];
    assert!(comm.as_array().unwrap().iter().any(|a| a["id"] == "14"));

    // getManifestByURL: manifest body; unknown URL -> null body.
    let manifest = station_bridge::get_manifest_by_url("station-manifest://14".into()).unwrap();
    assert_eq!(manifest["body"]["id"], "14");
    assert!(manifest["body"]["start_url"]
        .as_str()
        .unwrap()
        .starts_with("https://accounts.google.com"));
    let missing = station_bridge::get_manifest_by_url("station-manifest://999999".into()).unwrap();
    assert_eq!(missing["body"], Value::Null);

    // install: { body: applicationId } — the addApplicationRequest return.
    let installed = station_bridge::install_application(
        "station-manifest://14".into(),
        Some(json!({ "id": "14", "platform": "appstore" })),
        Some(true),
    )
    .unwrap();
    let app_id = installed["body"].as_str().unwrap().to_owned();
    assert!(!app_id.is_empty());

    // Blank-URL guard: napi error with the TS saga message verbatim.
    let err = station_bridge::install_application("  ".into(), None, None).unwrap_err();
    assert!(err
        .to_string()
        .contains("Add Application : given manifest URL is blank"));

    // uninstall: action channel — resolves null; then unknown id errors.
    station_bridge::uninstall_application(app_id.clone()).unwrap();
    let err = station_bridge::uninstall_application(app_id).unwrap_err();
    assert!(err.to_string().contains("not installed"));

    // requestPrivate: { body: { id, bxAppManifestURL } } from bxApi recipe.
    let created = station_bridge::request_private_application(json!({
        "name": "Acme Corp Intranet",
        "themeColor": "#112233",
        "bxIconURL": "https://acme.test/logo.png",
        "startURL": "https://acme.test/login",
        "scope": "https://acme.test",
    }))
    .unwrap();
    assert_eq!(created["body"]["id"], "1000001");
    assert_eq!(
        created["body"]["bxAppManifestURL"],
        "station-manifest://1000001"
    );

    // Missing required recipe field: preload's message verbatim.
    let err = station_bridge::request_private_application(json!({
        "name": "X", "themeColor": "#000", "startURL": "https://x.test", "scope": "https://x.test"
    }))
    .unwrap_err();
    assert!(err.to_string().contains("bxIconURL value is missing"));

    // getPrivateApps: snake_case manifest body the appstore HOC reads.
    let private = station_bridge::get_private_applications().unwrap();
    assert_eq!(private["body"].as_array().unwrap().len(), 1);
    assert_eq!(private["body"][0]["id"], "1000001");
    assert_eq!(private["body"][0]["name"], "Acme Corp Intranet");
    assert_eq!(private["body"][0]["theme_color"], "#112233");
    assert_eq!(private["body"][0]["start_url"], "https://acme.test/login");
    assert_eq!(private["body"][0]["category"], "Miscellaneous");

    // Private app now visible to search and getManifestByURL.
    let hits = station_bridge::search_applications("acme".into()).unwrap();
    assert_eq!(hits["body"].as_array().unwrap().len(), 1);
    assert_eq!(hits["body"][0]["id"], "1000001");
    let manifest =
        station_bridge::get_manifest_by_url("station-manifest://1000001".into()).unwrap();
    assert_eq!(manifest["body"]["name"], "Acme Corp Intranet");

    // install with a malformed context: InvalidArg surfaces.
    let err = station_bridge::install_application(
        "station-manifest://14".into(),
        Some(json!({ "platform": 7 })),
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("invalid context"));
}
