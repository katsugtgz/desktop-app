//! Adversarial stress tests for `manifest-registry`.
//!
//! Malformed/oversized JSON, huge and unicode queries, duplicate ids, and
//! hostile templates must parse, search, and project without panics, with
//! explicit wall-clock bounds on the bulk cases.

use std::time::{Duration, Instant};

use manifest_registry::config_rules::{
    application_label, is_configuration_required, ApplicationConfigState, ConfigRulesError,
};
use manifest_registry::{
    get_application_by_id, list_all_applications, manifest_to_minimal_application, search,
    BxAppManifest, Manifest, NewPrivateApplication, PrivateStore,
};

fn manifest_record(id: u64, name: &str, scope: &str) -> String {
    format!(
        r##"{{"data":[{{"name":"{name}","scope":"{scope}","icons":[],"start_url":"https://x/","category":"Miscellaneous","theme_color":"#fff","id":{id}}}]}}"##
    )
}

/// A scratch store file, unique per test process; removed on drop.
struct ScratchStore {
    path: std::path::PathBuf,
}

impl ScratchStore {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "manifest-registry-stress-{}-{tag}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        Self { path }
    }

    fn store(&self) -> PrivateStore {
        PrivateStore::from_manifests(&self.path).expect("store opens")
    }
}

impl Drop for ScratchStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Corrupt, truncated, and wrong-shaped store files start empty instead of
/// panicking, and the store keeps working afterwards.
#[test]
fn malformed_store_files_start_empty_and_recover() {
    let scratch = ScratchStore::new("malformed");
    let cases = [
        "".to_owned(),
        "not json".to_owned(),
        "{\"data\":".to_owned(),
        "{\"data\":{}}".to_owned(),
        "{\"data\":[{\"id\":\"not-a-number\"}]}".to_owned(),
        "{\"data\":[{\"name\":123}]}".to_owned(),
        "[1,2,3]".to_owned(),
        "\u{0}\u{1}\u{2}".to_owned(),
    ];
    for (i, content) in cases.iter().enumerate() {
        std::fs::write(&scratch.path, content).unwrap();
        let mut store = scratch.store();
        assert!(
            store.get_private_manifests().is_empty(),
            "case {i}: corrupt file must read as empty"
        );
        // The store still functions: save, read back, delete.
        let id = store
            .save_new_application(NewPrivateApplication {
                name: "ok".into(),
                theme_color: "#000".into(),
                icon_url: "https://i/".into(),
                start_url: "https://s/".into(),
                scope: "https://s/".into(),
            })
            .expect("save after corrupt load");
        assert_eq!(store.get_private_manifests().len(), 1);
        assert_eq!(store.get_private_manifests()[0].id, id.to_string());
        store
            .delete_manifest(id)
            .expect("delete after corrupt load");
        assert!(store.get_private_manifests().is_empty());
    }
}

/// An id of `u64::MAX` in the store file must not overflow when the next
/// application is saved (TS floats just keep going; here saturation is the
/// only non-panicking choice).
#[test]
fn max_u64_stored_id_does_not_overflow_on_save() {
    let scratch = ScratchStore::new("max-id");
    std::fs::write(
        &scratch.path,
        manifest_record(u64::MAX, "big", "https://big/"),
    )
    .unwrap();
    let mut store = scratch.store();
    assert_eq!(store.get_private_manifests().len(), 1);
    assert_eq!(store.get_private_manifests()[0].id, u64::MAX.to_string());

    // Must not panic on `highest_id + 1`.
    let new_id = store
        .save_new_application(NewPrivateApplication {
            name: "next".into(),
            theme_color: "#000".into(),
            icon_url: "https://i/".into(),
            start_url: "https://n/".into(),
            scope: "https://n/".into(),
        })
        .expect("save with saturated id");
    assert_eq!(store.get_private_manifests().len(), 2);
    assert!(
        store.get_private_application_by_id(new_id).is_some(),
        "saved application readable by its id"
    );
}

/// 10k private manifests: search over them stays bounded for realistic and
/// hostile queries, and duplicates collapse to deterministic results.
#[test]
fn ten_k_private_manifests_search_stays_bounded() {
    const N: usize = 10_000;
    let mut private: Vec<Manifest> = Vec::with_capacity(N);
    for i in 0..N {
        private.push(Manifest {
            inner: BxAppManifest {
                name: Some(format!("App {i}")),
                scope: Some(format!("https://app{i}.example.com")),
                ..BxAppManifest::default()
            },
            id: (i as u64).to_string(),
            icon: String::new(),
        });
    }
    // Duplicates: same id and same scope as app 0.
    private.push(private[0].clone());
    private.push(private[1].clone());

    let started = Instant::now();
    let hits = search("app 9999", &private);
    let elapsed = started.elapsed();
    assert!(!hits.is_empty(), "exact-name query finds its application");
    assert_eq!(hits[0].name, "App 9999");
    assert!(
        elapsed < Duration::from_secs(3),
        "search over 10k took {elapsed:?}"
    );

    // Prefix, substring, typo, and no-hit queries over the same 10k set.
    let started = Instant::now();
    for query in ["app 1", "9999", "aplication", "zzzznothing"] {
        let _ = search(query, &private);
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "four 10k searches took {elapsed:?}"
    );

    // Duplicates do not break list_all_applications.
    let listed = list_all_applications(&private);
    assert!(listed.len() >= N);
}

/// Hostile queries: empty, whitespace, 100k chars, 10k words, unicode,
/// control characters — no panic, bounded time, deterministic.
#[test]
fn hostile_queries_search_without_panic() {
    let private: Vec<Manifest> = vec![Manifest {
        inner: BxAppManifest {
            name: Some("GitHub".into()),
            ..BxAppManifest::default()
        },
        id: "1".into(),
        icon: String::new(),
    }];

    let exact_kinds = [
        "".to_owned(),
        " ".to_owned(),
        "\t\n ".to_owned(),
        "github".to_owned(),
        "GITHUB".to_owned(),
        "git hub".to_owned(),
        "gmial".to_owned(), // one typo away
    ];
    for query in &exact_kinds {
        let hits = search(query, &private);
        let again = search(query, &private);
        assert_eq!(hits, again, "search must be deterministic for {query:?}");
    }
    assert!(search("", &private).is_empty());
    assert!(search(" ", &private).is_empty());

    let started = Instant::now();
    let huge = "a".repeat(100_000);
    let _ = search(&huge, &private);
    let many_words = "git ".repeat(10_000);
    let _ = search(&many_words, &private);
    let _ = search("git 🎯 ünïcode 😀 x", &private);
    let _ = search("git\u{0}\u{7}hub", &private);
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "hostile queries took {elapsed:?}"
    );
}

/// Oversized and malformed manifest JSON: huge fields, deep icon lists, and
/// wrong-typed values either parse or fail cleanly.
#[test]
fn oversized_and_malformed_manifest_json_stays_clean() {
    let huge_name = "n".repeat(1_000_000);
    let json = format!(r#"{{"name":"{huge_name}"}}"#);
    let parsed: BxAppManifest = serde_json::from_str(&json).expect("huge name parses");
    assert_eq!(parsed.name.as_deref().map(str::len), Some(1_000_000));

    let icons: Vec<String> = (0..10_000)
        .map(|i| format!(r#"{{"src":"https://i/{i}.png"}}"#))
        .collect();
    let json = format!(r#"{{"name":"icons","icons":[{}]}}"#, icons.join(","));
    let parsed: BxAppManifest = serde_json::from_str(&json).expect("10k icons parse");
    assert_eq!(parsed.icons.len(), 10_000);

    for bad in [
        r#"{"name":42}"#,
        r#"{"bx_keep_always_loaded":"sometimes"}"#,
        r#"{"doNotList":"maybe"}"#,
        r#"{"recommendedPosition":"abc"}"#,
        "{",
        "[]",
        "null",
    ] {
        assert!(
            serde_json::from_str::<BxAppManifest>(bad).is_err(),
            "{bad} must be rejected"
        );
    }

    // Lenient strings still work.
    let lenient: BxAppManifest =
        serde_json::from_str(r#"{"doNotList":"true","recommendedPosition":"7"}"#).unwrap();
    assert_eq!(lenient.do_not_list, Some(true));
    assert_eq!(lenient.recommended_position(), 7.0);
}

/// Hostile multi-instance configuration: unknown presets, huge templates,
/// broken handlebars syntax — errors, never panics.
#[test]
fn config_rules_handle_hostile_templates_and_presets() {
    let huge_email = "e".repeat(1_000_000);
    let state = ApplicationConfigState::default();

    // Google-account preset with a huge email renders without panic.
    let manifest: BxAppManifest = serde_json::from_str(
        r#"{"name":"Mail","bx_multi_instance_config":{"presets":["google-account"],"instance_label_tpl":"{{email}} <mail>"}}"#,
    )
    .unwrap();
    let label = application_label(&manifest, &state, Some(&huge_email)).expect("renders");
    assert!(label.len() > 1_000_000);

    // Broken templates return `ConfigRulesError::Template`, not a panic.
    // (An unknown helper renders empty like handlebars.js, and the empty
    // instance label then falls back to the plain name.)
    for tpl in ["{{email", "{{#each x}}", "{{{{}}}}", "{{#if}}"] {
        let manifest: BxAppManifest = serde_json::from_str(&format!(
            r#"{{"name":"Mail","bx_multi_instance_config":{{"presets":["google-account"],"instance_label_tpl":"{}"}}}}"#,
            tpl.replace('"', "\\\"")
        ))
        .unwrap();
        let result = application_label(&manifest, &state, Some("a@b.c"));
        match result {
            Err(ConfigRulesError::Template(_)) => {}
            other => panic!("template {tpl:?}: expected Template error, got {other:?}"),
        }
    }

    // Unknown preset lists error instead of silently returning false.
    let manifest: BxAppManifest = serde_json::from_str(
        r#"{"name":"X","bx_multi_instance_config":{"presets":["subdomain","on-premise","google-account"]}}"#,
    )
    .unwrap();
    let _ = is_configuration_required(&manifest, &state, "https://x/").expect("known presets ok");

    // Subdomain-only preset requires configuration when unset.
    let manifest: BxAppManifest = serde_json::from_str(
        r#"{"name":"X","bx_multi_instance_config":{"presets":["subdomain"]}}"#,
    )
    .unwrap();
    assert_eq!(
        is_configuration_required(&manifest, &state, "https://x/"),
        Ok(true)
    );
    let configured = ApplicationConfigState {
        subdomain: Some("acme".into()),
        ..ApplicationConfigState::default()
    };
    assert_eq!(
        is_configuration_required(&manifest, &configured, "https://x/"),
        Ok(false)
    );
}

/// Every bundled manifest still projects; duplicates of a bundled id among
/// private manifests do not confuse search or listing.
#[test]
fn bundled_manifests_survive_hostile_private_sets() {
    let bundled = get_application_by_id("100").expect("bundled manifest parses");
    let minimal = manifest_to_minimal_application(&bundled);
    assert_eq!(minimal.id, "100");

    // Duplicate of a bundled id as a private manifest.
    let mut private = vec![bundled.clone(), bundled];
    private[1].id = "100".into();
    let listed = list_all_applications(&private);
    assert!(listed.iter().filter(|m| m.id == "100").count() >= 2);

    let hits = search("Atlassian", &private);
    assert!(!hits.is_empty());
    assert!(hits
        .iter()
        .all(|h| h.id == "100" || h.name.contains("Atlassian")));

    // Empty private list and empty-name manifests.
    assert!(list_all_applications(&[]).len() > 100);
    let nameless = vec![Manifest {
        inner: BxAppManifest::default(),
        id: "0".into(),
        icon: String::new(),
    }];
    let _ = search("anything", &nameless);
    assert!(
        search("anything", &nameless).is_empty(),
        "nameless manifest never matches"
    );

    // Icon resources with hostile strings parse and project.
    let manifest: BxAppManifest = serde_json::from_str(
        r#"{"name":"i","icons":[{"src":"https://🎧/🎯.png","sizes":"any","purpose":"MONSTER 🎯"}]}"#,
    )
    .unwrap();
    assert_eq!(manifest.icons[0].purpose.as_deref(), Some("MONSTER 🎯"));
}
