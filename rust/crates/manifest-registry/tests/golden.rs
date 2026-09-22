//! Golden-query fixtures for manifest search.
//!
//! Each case fixes a query and its expected result ids so ranking changes
//! show up as diffs. Expected ids were produced by the ranking rule this
//! crate documents (exact/prefix first, then score, then name, then id);
//! the point of the fixtures is to freeze that behavior, not to bless
//! whatever today's code happens to emit.

use manifest_registry::{search, PrivateStore};

fn ids(query: &str) -> Vec<String> {
    search(query, &[]).into_iter().map(|m| m.id).collect()
}

#[test]
fn golden_exact_match_ranks_first() {
    assert_eq!(ids("slack").first().map(String::as_str), Some("21"));
}

#[test]
fn golden_case_insensitive() {
    assert_eq!(ids("SLACK").first().map(String::as_str), Some("21"));
    assert_eq!(ids("Gmail").first().map(String::as_str), Some("14"));
}

#[test]
fn golden_exact_query_returns_only_that_app() {
    assert_eq!(ids("trello"), vec!["20"]);
    assert_eq!(ids("discord"), vec!["133"]);
}

#[test]
fn golden_prefix_ranking() {
    // "Github" is a prefix match; "Gitlab" etc. only fuzzy match.
    assert_eq!(ids("github"), vec!["39"]);
    // All five "Git*" apps word-prefix-match "git" and rank ahead of
    // fuzzy-only subsequence hits ("DigitalOcean", "Bit.ly"); within the
    // prefix group nucleo score decides (shorter, tighter match first).
    assert_eq!(
        ids("git"),
        vec!["4304", "39", "123", "124", "398", "598", "289"]
    );
}

#[test]
fn golden_substring_family() {
    // Drive prefix-matches (Google Drive, OneDrive, Pipedrive) ahead of
    // fuzzy-only hits, Mailchimp-style surprises excluded by threshold.
    let got = ids("drive");
    assert_eq!(got.first().map(String::as_str), Some("16"));
    assert!(got.contains(&"342".to_string()));
    assert!(got.contains(&"72".to_string()));
}

#[test]
fn golden_word_subtoken() {
    // "Google Calendar" matches on the word "calendar".
    assert_eq!(ids("calendar").first().map(String::as_str), Some("18"));
    // "code" word-prefix-matches Codecov/Codepen/Codeship and
    // word-matches "Chromium Code Search" (word "code" equals the query,
    // still ranked as prefix-class here because starts_with covers it);
    // nucleo score puts the exact word hit first.
    let got = ids("code");
    assert_eq!(got.first().map(String::as_str), Some("8563"));
    assert!(got.contains(&"241".to_string()));
    assert!(got.contains(&"411".to_string()));
    assert!(got.contains(&"532".to_string()));
}

#[test]
fn golden_fuzzy_typo_tolerance() {
    // Transposed letters still find the app (Fuse-like fuzzy behavior).
    assert_eq!(ids("gmial").first().map(String::as_str), Some("14"));
    assert_eq!(ids("slcak").first().map(String::as_str), Some("21"));
}

#[test]
fn golden_multiword_subtoken() {
    // Word-subtoken matches the tail word of "Atlassian (Jira, Confluence..)".
    assert_eq!(ids("confluence"), vec!["100"]);
}

#[test]
fn golden_no_match_returns_empty() {
    assert!(ids("zzzzqqq").is_empty());
}

#[test]
fn golden_do_not_list_excluded() {
    // "App Store" (id 1) and "Hangouts" (id 115) are doNotList.
    assert!(!ids("app store").contains(&"1".to_string()));
    assert!(!ids("hangouts").contains(&"115".to_string()));
}

#[test]
fn golden_private_manifests_searched() {
    let mut store = PrivateStore::from_manifests(unique_path("search")).unwrap();
    store.save_new_application(manifest_registry::NewPrivateApplication {
        name: "Zephyr Internal".into(),
        scope: "https://zephyr.internal".into(),
        start_url: "https://zephyr.internal/home".into(),
        icon_url: "https://zephyr.internal/icon.png".into(),
        theme_color: "#102030".into(),
    });

    let results = manifest_registry::search_with_private("zephyr", &store);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "1000001");
    assert_eq!(results[0].name, "Zephyr Internal");
    assert_eq!(results[0].bx_app_manifest_url, "station-manifest://1000001");
    assert_eq!(results[0].icon_url, "https://zephyr.internal/icon.png");

    // Private manifests participate in ranking: exact private name beats
    // fuzzy hits on bundled apps.
    assert_eq!(
        manifest_registry::search_with_private("zephyr internal", &store)[0].id,
        "1000001"
    );
}

#[test]
fn golden_private_round_trip_and_delete() {
    let path = unique_path("roundtrip");
    let mut store = PrivateStore::from_manifests(&path).unwrap();
    let first = store.save_new_application(manifest_registry::NewPrivateApplication {
        name: "Alpha Tool".into(),
        scope: "https://alpha.tool".into(),
        start_url: "https://alpha.tool/start".into(),
        icon_url: "https://alpha.tool/i.png".into(),
        theme_color: "#aabbcc".into(),
    });
    let second = store.save_new_application(manifest_registry::NewPrivateApplication {
        name: "Beta Tool".into(),
        scope: "https://beta.tool".into(),
        start_url: "https://beta.tool/start".into(),
        icon_url: "https://beta.tool/i.png".into(),
        theme_color: "#000000".into(),
    });
    assert_eq!((first, second), (1000001, 1000002));

    // Persisted JSON is `{ "data": [...] }` like the TS store.
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(json["data"].as_array().unwrap().len(), 2);
    assert_eq!(json["data"][1]["name"], "Beta Tool");
    assert_eq!(json["data"][1]["category"], "Miscellaneous");
    assert_eq!(json["data"][0]["icons"][0]["platform"], "browserx");

    // Reload from disk keeps ids stable and bumps past the highest one.
    let mut reloaded = PrivateStore::from_manifests(&path).unwrap();
    let third = reloaded.save_new_application(manifest_registry::NewPrivateApplication {
        name: "Gamma".into(),
        scope: "https://g.amma".into(),
        start_url: "https://g.amma/s".into(),
        icon_url: "https://g.amma/i.png".into(),
        theme_color: "#111111".into(),
    });
    assert_eq!(third, 1000003);

    reloaded.delete_manifest(1000001);
    assert!(reloaded.get_private_application_by_id(1000001).is_none());
    assert!(reloaded.get_private_application_by_id(1000002).is_some());
    // Deleting an unknown id is a no-op.
    reloaded.delete_manifest(42);
    assert_eq!(reloaded.get_private_manifests().len(), 2);
}

/// Per-test scratch file under the system temp dir.
fn unique_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "manifest-registry-golden-{tag}-{}.json",
        std::process::id()
    ))
}
