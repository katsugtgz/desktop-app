use url_router::{normalize_url, strip_hash};

// `normalizeURL` is `new NodeURL(url).toString()`: scheme and host are
// lowercased, an empty path becomes `/`, the default port disappears, and
// path case, query and fragment survive verbatim.
#[test]
fn normalize_adds_trailing_slash_and_lowercases_scheme_and_host() {
    assert_eq!(
        normalize_url("HTTPS://ExAmPle.COM").as_deref(),
        Some("https://example.com/")
    );
}

#[test]
fn normalize_keeps_path_case_query_and_fragment() {
    assert_eq!(
        normalize_url("https://Ex.com/Path/To?Q=1&x=Y#Frag").as_deref(),
        Some("https://ex.com/Path/To?Q=1&x=Y#Frag")
    );
}

#[test]
fn normalize_drops_the_default_port() {
    assert_eq!(
        normalize_url("https://ex.com:443/a").as_deref(),
        Some("https://ex.com/a")
    );
    assert_eq!(
        normalize_url("http://ex.com:8080/a").as_deref(),
        Some("http://ex.com:8080/a")
    );
}

#[test]
fn normalize_rejects_values_that_are_not_urls() {
    assert_eq!(normalize_url("not a url"), None);
    assert_eq!(normalize_url(""), None);
}

#[test]
fn strip_hash_matches_split_on_hash() {
    assert_eq!(strip_hash("https://ex.com/#/inbox"), "https://ex.com/");
    assert_eq!(strip_hash("https://ex.com/a#b#c"), "https://ex.com/a");
    assert_eq!(strip_hash("https://ex.com/a"), "https://ex.com/a");
}

use url_router::{parse_url, search_scopes, url_in_scope};

// `parseUrl`: `hostname.split('.')` with exactly three labels becomes
// `subdomain` + `domain`; any other length keeps the whole host as `domain`.
#[test]
fn parse_splits_exactly_three_label_hosts() {
    let parsed = parse_url("https://www.example.com:8443/app?q=1#frag").expect("valid url");
    assert_eq!(parsed.protocol, "https:");
    assert_eq!(parsed.subdomain.as_deref(), Some("www"));
    assert_eq!(parsed.domain, "example.com");
    assert_eq!(parsed.port, Some(8443));
    assert_eq!(parsed.pathname, "/app");
}

#[test]
fn parse_keeps_deeper_hosts_as_one_domain() {
    // TS quirk: only exactly three labels are split, so a scope on
    // `example.com` can never cover `a.b.example.com`.
    let parsed = parse_url("https://a.b.example.com/x").expect("valid url");
    assert_eq!(parsed.subdomain, None);
    assert_eq!(parsed.domain, "a.b.example.com");
    assert_eq!(parsed.pathname, "/x");
}

#[test]
fn parse_rejects_garbage_and_empty_input() {
    assert_eq!(parse_url("not a url"), None);
    assert_eq!(parse_url(""), None);
}

#[test]
fn root_scope_matches_every_path_on_its_origin() {
    // scope pathname '/' → `url.pathname.startsWith('/')`
    assert!(url_in_scope(
        "https://example.com/a/b?q=1",
        "https://example.com"
    ));
    assert!(url_in_scope("https://example.com/", "https://example.com/"));
}

#[test]
fn scope_path_is_a_raw_string_prefix_not_a_path_segment() {
    // `url.pathname.startsWith(scope.pathname)` — "/application" starts
    // with "/app".
    assert!(url_in_scope(
        "https://example.com/application",
        "https://example.com/app"
    ));
    assert!(url_in_scope(
        "https://example.com/app/x",
        "https://example.com/app"
    ));
    assert!(!url_in_scope(
        "https://example.com/other",
        "https://example.com/app"
    ));
}

#[test]
fn scope_ignores_query_and_fragment_but_not_subdomain_or_protocol() {
    assert!(url_in_scope(
        "https://example.com/app?x=1#y",
        "https://example.com/app"
    ));
    assert!(!url_in_scope(
        "https://sub.example.com/app",
        "https://example.com"
    ));
    assert!(!url_in_scope(
        "https://example.com/app",
        "https://www.example.com"
    ));
    assert!(!url_in_scope(
        "http://example.com/app",
        "https://example.com"
    ));
    assert!(!url_in_scope(
        "https://example.com:8443/app",
        "https://example.com/app"
    ));
}

#[test]
fn search_scopes_is_any_match_and_skips_unparseable_scopes() {
    let scopes = vec![
        "not a scope".to_string(),
        "https://other.example/x".to_string(),
        "https://example.com/app".to_string(),
    ];
    assert!(search_scopes("https://example.com/app/1", &scopes));
    assert!(!search_scopes("https://example.com/elsewhere", &scopes));
    assert!(!search_scopes("https://example.com/app", &[]));
    assert!(!search_scopes("", &scopes));
}

#[test]
fn wildcard_subdomains_match_either_direction() {
    // `matchScope`'s two `'*'` branches: a wildcard scope covers any
    // non-empty URL subdomain, and a wildcard URL is covered by any
    // concrete scope subdomain. A WHATWG host may contain `*`.
    assert!(url_in_scope(
        "https://x.example.com/a",
        "https://*.example.com/a"
    ));
    assert!(url_in_scope(
        "https://*.example.com/a",
        "https://x.example.com/a"
    ));
    // Apex hosts have no subdomain, so a wildcard scope does not cover them.
    assert!(!url_in_scope(
        "https://example.com/a",
        "https://*.example.com/a"
    ));
    // Domain, protocol and port still have to agree.
    assert!(!url_in_scope(
        "https://x.example.com:8443/a",
        "https://*.example.com/a"
    ));
    assert!(!url_in_scope(
        "https://x.other.com/a",
        "https://*.example.com/a"
    ));
}

use url_router::{route, InstalledApp, OpenTab, Origin, Route, RouteRequest};

fn app(application_id: &str, scope: &str) -> InstalledApp {
    InstalledApp {
        application_id: application_id.to_string(),
        manifest_url: format!("https://manifests.test/{application_id}.json"),
        scope: Some(scope.to_string()),
        extended_scopes: Vec::new(),
        custom_urls: Vec::new(),
        start_url: None,
        single_page: false,
    }
}

fn tab(tab_id: &str, application_id: &str, url: &str) -> OpenTab {
    OpenTab {
        tab_id: tab_id.to_string(),
        application_id: application_id.to_string(),
        url: url.to_string(),
        is_home: false,
    }
}

#[test]
fn exact_tab_match_focuses_before_opening_a_new_tab() {
    let apps = [app("app-1", "https://example.com")];
    let tabs = [tab("app-1/t1", "app-1", "https://example.com/inbox")];

    assert_eq!(
        route("https://example.com/inbox", &apps, &tabs),
        Route::FocusTab {
            tab_id: "app-1/t1".into()
        }
    );
}

#[test]
fn exact_match_on_the_origin_tab_reloads_it() {
    let apps = [app("app-1", "https://example.com")];
    let tabs = [tab("app-1/t1", "app-1", "https://example.com/inbox")];

    let request =
        RouteRequest::new("https://example.com/inbox", &apps, &tabs).with_origin(Origin {
            tab_id: Some("app-1/t1"),
            application_id: Some("app-1"),
        });

    assert_eq!(
        request.route(),
        Route::Reload {
            tab_id: "app-1/t1".into()
        }
    );
}

#[test]
fn hash_stripped_match_pushes_and_focuses_after_exact_misses() {
    let apps = [app("app-1", "https://example.com")];
    let tabs = [tab("app-1/t1", "app-1", "https://example.com/#/inbox")];

    assert_eq!(
        route("https://example.com/#/settings", &apps, &tabs),
        Route::PushAndFocusTab {
            tab_id: "app-1/t1".into(),
            url: "https://example.com/#/settings".into(),
        }
    );
}

#[test]
fn exact_match_wins_over_a_loose_match_on_another_tab() {
    let apps = [app("app-1", "https://example.com")];
    let tabs = [
        tab("app-1/t1", "app-1", "https://example.com/#/inbox"),
        tab("app-1/t2", "app-1", "https://example.com/#/settings"),
    ];

    assert_eq!(
        route("https://example.com/#/settings", &apps, &tabs),
        Route::FocusTab {
            tab_id: "app-1/t2".into()
        }
    );
}

#[test]
fn duplicate_tab_urls_resolve_to_the_last_tab_like_immutable_map_keys() {
    // `getTabsMappedByExactURL`'s `mapKeys` keeps insertion order but lets
    // the last tab win a duplicate key.
    let apps = [app("app-1", "https://example.com")];
    let tabs = [
        tab("app-1/t1", "app-1", "https://example.com/x"),
        tab("app-1/t2", "app-1", "https://example.com/x"),
    ];

    assert_eq!(
        route("https://example.com/x", &apps, &tabs),
        Route::FocusTab {
            tab_id: "app-1/t2".into()
        }
    );
}

#[test]
fn unknown_url_without_apps_goes_to_the_external_browser() {
    assert_eq!(
        route("https://unknown.test/a", &[], &[]),
        Route::ExternalBrowser
    );
}

use url_router::{StoreApp, Target};

fn store_app(manifest_url: &str, scope: &str) -> StoreApp {
    StoreApp {
        manifest_url: manifest_url.to_string(),
        scope: Some(scope.to_string()),
        extended_scopes: Vec::new(),
        opted_out: false,
    }
}

#[test]
fn origin_scope_navigates_the_origin_tab() {
    let apps = [app("app-1", "https://example.com")];
    let request = RouteRequest::new("https://example.com/other", &apps, &[]).with_origin(Origin {
        tab_id: Some("app-1/t1"),
        application_id: Some("app-1"),
    });

    assert_eq!(
        request.route(),
        Route::OpenInTab {
            tab_id: Some("app-1/t1".into()),
            url: "https://example.com/other".into(),
        }
    );
}

#[test]
fn origin_scope_with_new_tab_target_opens_a_new_tab_of_the_origin_app() {
    let apps = [app("app-1", "https://example.com")];
    let request = RouteRequest::new("https://example.com/other", &apps, &[])
        .with_origin(Origin {
            tab_id: Some("app-1/t1"),
            application_id: Some("app-1"),
        })
        .with_target(Target::NewTab);

    assert_eq!(
        request.route(),
        Route::NewTab {
            application_id: "app-1".into(),
            url: "https://example.com/other".into(),
        }
    );
}

#[test]
fn origin_scope_with_new_window_target_opens_a_detached_instance() {
    let apps = [app("app-1", "https://example.com")];
    let request = RouteRequest::new("https://example.com/other", &apps, &[])
        .with_origin(Origin {
            tab_id: Some("app-1/t1"),
            application_id: Some("app-1"),
        })
        .with_target(Target::NewWindow);

    assert_eq!(
        request.route(),
        Route::NewInstance {
            application_id: "app-1".into(),
            url: "https://example.com/other".into(),
        }
    );
}

#[test]
fn tab_matching_beats_the_origin_scope() {
    let apps = [app("app-1", "https://example.com")];
    let tabs = [tab("app-2/t1", "app-2", "https://example.com/inbox")];
    let request =
        RouteRequest::new("https://example.com/inbox", &apps, &tabs).with_origin(Origin {
            tab_id: Some("app-1/t1"),
            application_id: Some("app-1"),
        });

    assert_eq!(
        request.route(),
        Route::FocusTab {
            tab_id: "app-2/t1".into()
        }
    );
}

#[test]
fn another_installed_app_scope_opens_a_new_tab_when_the_origin_scope_misses() {
    let apps = [
        app("app-1", "https://one.test"),
        app("app-2", "https://two.test"),
    ];
    let request = RouteRequest::new("https://two.test/x", &apps, &[]).with_origin(Origin {
        tab_id: Some("app-1/t1"),
        application_id: Some("app-1"),
    });

    assert_eq!(
        request.route(),
        Route::NewTab {
            application_id: "app-2".into(),
            url: "https://two.test/x".into(),
        }
    );
}

#[test]
fn deeplink_with_new_window_target_opens_a_detached_instance() {
    let apps = [app("app-1", "https://example.com")];
    let request =
        RouteRequest::new("https://example.com/x", &apps, &[]).with_target(Target::NewWindow);

    assert_eq!(
        request.route(),
        Route::NewInstance {
            application_id: "app-1".into(),
            url: "https://example.com/x".into(),
        }
    );
}

#[test]
fn store_scope_offers_install_and_open() {
    let store = [store_app(
        "https://manifests.test/notion.json",
        "https://notion.so",
    )];
    let request = RouteRequest::new("https://notion.so/page", &[], &[]).with_store_apps(&store);

    assert_eq!(
        request.route(),
        Route::InstallAndOpen {
            manifest_url: "https://manifests.test/notion.json".into(),
            url: "https://notion.so/page".into(),
        }
    );
}

#[test]
fn opted_out_store_app_is_skipped() {
    let mut opted_out = store_app("https://manifests.test/notion.json", "https://notion.so");
    opted_out.opted_out = true;
    let store = [opted_out];
    let request = RouteRequest::new("https://notion.so/page", &[], &[]).with_store_apps(&store);

    assert_eq!(request.route(), Route::ExternalBrowser);
}

#[test]
fn wildcard_store_scope_matches_a_workspace_subdomain() {
    let store = [store_app(
        "https://manifests.test/slack.json",
        "https://*.slack.com",
    )];
    let request =
        RouteRequest::new("https://team.slack.com/archives/abc", &[], &[]).with_store_apps(&store);

    assert_eq!(
        request.route(),
        Route::InstallAndOpen {
            manifest_url: "https://manifests.test/slack.json".into(),
            url: "https://team.slack.com/archives/abc".into(),
        }
    );
}

#[test]
fn concrete_store_subdomain_precedes_the_wildcard_branch() {
    let store = [
        store_app("https://manifests.test/fallback.json", "https://*.a.test"),
        store_app(
            "https://manifests.test/app.json",
            "https://team.a.test/app/",
        ),
    ];
    let url = "https://team.a.test/app/nested";
    let request = RouteRequest::new(url, &[], &[]).with_store_apps(&store);
    assert_eq!(
        request.route(),
        Route::InstallAndOpen {
            manifest_url: "https://manifests.test/app.json".into(),
            url: url.into(),
        }
    );

    let request =
        RouteRequest::new("https://team.a.test/elsewhere", &[], &[]).with_store_apps(&store);
    assert_eq!(request.route(), Route::ExternalBrowser);
}

#[test]
fn store_path_scopes_match_whole_segments_only() {
    // The TS radix tree inserts `path` and `path/**`, so scope paths match
    // at segment boundaries (unlike installed-app scopes' raw prefix).
    let store = [store_app(
        "https://manifests.test/app.json",
        "https://a.test/app",
    )];
    let request = RouteRequest::new("https://a.test/app/x", &[], &[]).with_store_apps(&store);
    assert_eq!(
        request.route(),
        Route::InstallAndOpen {
            manifest_url: "https://manifests.test/app.json".into(),
            url: "https://a.test/app/x".into(),
        }
    );

    let request = RouteRequest::new("https://a.test/application", &[], &[]).with_store_apps(&store);
    assert_eq!(request.route(), Route::ExternalBrowser);
}

#[test]
fn the_most_specific_store_scope_wins() {
    let store = [
        store_app("https://manifests.test/root.json", "https://a.test"),
        store_app("https://manifests.test/app.json", "https://a.test/app"),
    ];
    let request = RouteRequest::new("https://a.test/app/x", &[], &[]).with_store_apps(&store);

    assert_eq!(
        request.route(),
        Route::InstallAndOpen {
            manifest_url: "https://manifests.test/app.json".into(),
            url: "https://a.test/app/x".into(),
        }
    );
}

#[test]
fn default_browser_targets_skip_every_rule() {
    let apps = [app("app-1", "https://example.com")];
    let tabs = [tab("app-1/t1", "app-1", "https://example.com/x")];
    for target in [Target::DefaultBrowser, Target::DefaultBrowserBackground] {
        let request = RouteRequest::new("https://example.com/x", &apps, &tabs).with_target(target);
        assert_eq!(request.route(), Route::ExternalBrowser);
    }
}

#[test]
fn unparseable_url_goes_to_the_external_browser() {
    let apps = [app("app-1", "https://example.com")];
    assert_eq!(
        RouteRequest::new("not a url", &apps, &[]).route(),
        Route::ExternalBrowser
    );
}

#[test]
fn home_tab_is_just_another_tab_for_routing() {
    // No `start_url` comparison: the home tab is found by the ordinary tab
    // URL match, and with no tab the scope opens a new one.
    let mut app = app("app-1", "https://example.com/app");
    app.start_url = Some("https://example.com/app".into());
    let mut home = tab("app-1/home", "app-1", "https://example.com/app");
    home.is_home = true;

    assert_eq!(
        route("https://example.com/app", &[app.clone()], &[home]),
        Route::FocusTab {
            tab_id: "app-1/home".into()
        }
    );
    assert_eq!(
        route("https://example.com/app", &[app], &[]),
        Route::NewTab {
            application_id: "app-1".into(),
            url: "https://example.com/app".into(),
        }
    );
}

#[test]
fn single_page_apps_route_like_any_other_app() {
    // `bx_single_page` only hides the "Add a new page" button; `routeURL`
    // never reads it.
    let mut app = app("app-1", "https://example.com");
    app.single_page = true;

    assert_eq!(
        route("https://example.com/x", &[app], &[]),
        Route::NewTab {
            application_id: "app-1".into(),
            url: "https://example.com/x".into(),
        }
    );
}

#[test]
fn apps_without_a_manifest_scope_are_skipped_even_with_extended_scopes() {
    let mut missing = app("app-1", "");
    missing.scope = None;
    missing.extended_scopes = vec!["https://example.com".into()];

    let mut empty = app("app-2", "");
    empty.extended_scopes = vec!["https://example.com".into()];

    for app in [missing, empty] {
        assert_eq!(
            route("https://example.com/x", &[app], &[]),
            Route::ExternalBrowser
        );
    }
}

#[test]
fn custom_url_apps_are_tried_before_plain_apps() {
    // `findApplicationInInstalledScopes` reorders: custom-URL apps first,
    // even though the plain app comes first in state order.
    let plain = app("app-plain", "https://x.test");
    let mut custom = app("app-custom", "https://other.test");
    custom.custom_urls = vec!["https://x.test".into()];

    assert_eq!(
        route("https://x.test/x", &[plain, custom], &[]),
        Route::NewTab {
            application_id: "app-custom".into(),
            url: "https://x.test/x".into(),
        }
    );
}

#[test]
fn first_matching_app_wins_without_a_longest_scope_rule() {
    // `findApplicationInInstalledScopes` returns the first app whose
    // `searchScopes` says yes; a later, more specific scope does not win.
    let apps = [
        app("app-1", "https://example.com"),
        app("app-2", "https://example.com/deep"),
    ];

    assert_eq!(
        route("https://example.com/deep/x", &apps, &[]),
        Route::NewTab {
            application_id: "app-1".into(),
            url: "https://example.com/deep/x".into(),
        }
    );
}

#[test]
fn custom_urls_are_normalized_before_matching() {
    let mut app = app("app-1", "https://elsewhere.test");
    app.custom_urls = vec!["HTTPS://Ex.COM:443/tenant".into()];

    assert_eq!(
        route("https://ex.com/tenant/inbox", &[app], &[]),
        Route::NewTab {
            application_id: "app-1".into(),
            url: "https://ex.com/tenant/inbox".into(),
        }
    );
}

#[test]
fn an_invalid_custom_url_aborts_the_whole_decision() {
    // `normalizeURL(customURL)` throws inside `getScopes`, and `routeURL`
    // catches to DEFAULT_BROWSER — the app's valid scope is never consulted.
    let mut app = app("app-1", "https://example.com");
    app.custom_urls = vec!["not a url".into()];

    assert_eq!(
        route("https://example.com/x", &[app], &[]),
        Route::ExternalBrowser
    );
}

#[test]
fn tab_urls_are_compared_verbatim_without_normalization() {
    // Tab maps are keyed by the raw tab URL; a differently-cased spelling is
    // not an exact or loose match and falls through to the app scope.
    let apps = [app("app-1", "https://example.com")];
    let tabs = [tab("app-1/t1", "app-1", "https://example.com/x")];

    assert_eq!(
        route("HTTPS://ExAmple.COM/x", &apps, &tabs),
        Route::NewTab {
            application_id: "app-1".into(),
            url: "HTTPS://ExAmple.COM/x".into(),
        }
    );
}

#[test]
fn loose_match_applies_even_when_it_is_the_origin_tab() {
    // The `RELOAD` special case only exists on the exact-match branch.
    let apps = [app("app-1", "https://example.com")];
    let tabs = [tab("app-1/t1", "app-1", "https://example.com/#/inbox")];
    let request =
        RouteRequest::new("https://example.com/#/settings", &apps, &tabs).with_origin(Origin {
            tab_id: Some("app-1/t1"),
            application_id: Some("app-1"),
        });

    assert_eq!(
        request.route(),
        Route::PushAndFocusTab {
            tab_id: "app-1/t1".into(),
            url: "https://example.com/#/settings".into(),
        }
    );
}

#[test]
fn origin_scope_without_a_tab_id_still_reports_an_in_tab_navigation() {
    // The TS passes `origin.tabId` through even when undefined.
    let apps = [app("app-1", "https://example.com")];
    let request = RouteRequest::new("https://example.com/x", &apps, &[]).with_origin(Origin {
        tab_id: None,
        application_id: Some("app-1"),
    });

    assert_eq!(
        request.route(),
        Route::OpenInTab {
            tab_id: None,
            url: "https://example.com/x".into(),
        }
    );
}

#[test]
fn unknown_origin_application_falls_through_to_installed_scopes() {
    let apps = [app("app-1", "https://example.com")];
    let request = RouteRequest::new("https://example.com/x", &apps, &[]).with_origin(Origin {
        tab_id: Some("ghost/t1"),
        application_id: Some("ghost"),
    });

    assert_eq!(
        request.route(),
        Route::NewTab {
            application_id: "app-1".into(),
            url: "https://example.com/x".into(),
        }
    );
}

#[test]
fn empty_url_goes_to_the_external_browser() {
    assert_eq!(
        route("", &[app("app-1", "https://example.com")], &[]),
        Route::ExternalBrowser
    );
}

#[test]
fn an_opted_out_store_winner_does_not_fall_back_to_a_broader_scope() {
    // `findInAllScopes` looks the URL up first and only then checks
    // `isNotInstallableApplication`; an opted-out winner yields null, it
    // does not retry a less specific catalogue scope.
    let mut specific = store_app("https://manifests.test/app.json", "https://a.test/app");
    specific.opted_out = true;
    let store = [
        store_app("https://manifests.test/root.json", "https://a.test"),
        specific,
    ];
    let request = RouteRequest::new("https://a.test/app/x", &[], &[]).with_store_apps(&store);

    assert_eq!(request.route(), Route::ExternalBrowser);
}

#[test]
fn loose_matching_follows_the_two_immutable_maps_exactly() {
    // Tabs `[x, x#1, x]` collapse to the exact map {x -> C, x#1 -> B}; the
    // loose map then re-keys that map, so B (not C) ends up as `x`.
    let apps = [app("app-1", "https://a.test")];
    let tabs = [
        tab("t-a", "app-1", "https://a.test/x"),
        tab("t-b", "app-1", "https://a.test/x#1"),
        tab("t-c", "app-1", "https://a.test/x"),
    ];

    assert_eq!(
        route("https://a.test/x#2", &apps, &tabs),
        Route::PushAndFocusTab {
            tab_id: "t-b".into(),
            url: "https://a.test/x#2".into(),
        }
    );
}
