//! Adversarial stress tests for `url-router`.
//!
//! Hostile URLs and large tab/app/catalogue sets must produce the same
//! decision table without panics and within bounded time.

use std::time::{Duration, Instant};

use url_router::{
    parse_url, route, scope_matches, search_scopes, strip_hash, url_in_scope, InstalledApp,
    OpenTab, Route, RouteRequest, StoreApp, Target,
};

fn app(id: &str, scope: &str) -> InstalledApp {
    InstalledApp {
        application_id: id.into(),
        manifest_url: format!("https://manifests/{id}.json"),
        scope: Some(scope.into()),
        extended_scopes: Vec::new(),
        custom_urls: Vec::new(),
        start_url: None,
        single_page: false,
    }
}

fn tab(id: &str, url: &str) -> OpenTab {
    OpenTab {
        tab_id: id.into(),
        application_id: "app".into(),
        url: url.into(),
        is_home: false,
    }
}

fn store_app(scope: &str) -> StoreApp {
    StoreApp {
        manifest_url: "https://manifests/store.json".into(),
        scope: Some(scope.into()),
        extended_scopes: Vec::new(),
        opted_out: false,
    }
}

/// A 1 MB URL parses, matches, and routes without panic, in bounded time.
#[test]
fn one_mb_url_routes_without_panic() {
    let huge_path = format!("https://example.com/a/{}", "b".repeat(1_000_000));
    let huge_hash = format!("{huge_path}#fragment");

    let parsed = parse_url(&huge_path).expect("1 MB URL parses");
    assert_eq!(parsed.protocol, "https:");
    assert_eq!(parsed.domain, "example.com");
    assert!(parsed.pathname.len() > 1_000_000);
    assert_eq!(strip_hash(&huge_hash), huge_path.as_str());
    assert!(url_in_scope(&huge_path, "https://example.com"));

    let apps = [app("a", "https://example.com")];
    let started = Instant::now();
    let decision = route(&huge_path, &apps, &[]);
    let elapsed = started.elapsed();
    assert_eq!(
        decision,
        Route::NewTab {
            application_id: "a".into(),
            url: huge_path.clone(),
        }
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "1 MB route took {elapsed:?}"
    );

    // Hash-stripped loose match against a tab holding the bare URL.
    let tabs = [tab("t1", &huge_path)];
    assert_eq!(
        route(&huge_hash, &[], &tabs),
        Route::PushAndFocusTab {
            tab_id: "t1".into(),
            url: huge_hash.clone(),
        }
    );
}

/// Unicode/IDN hosts, IPv6, and out-of-range ports parse to sane decisions.
#[test]
fn idn_ipv6_and_port_edges_parse_safely() {
    // IDN: the WHATWG parser punycodes the host.
    let unicode = parse_url("http://münchen.de/path").expect("unicode host parses");
    assert_eq!(unicode.domain, "xn--mnchen-3ya.de");
    assert_eq!(unicode.subdomain.as_deref(), None);

    let subdomain = parse_url("http://www.münchen.de/path").expect("3-label unicode host");
    assert_eq!(subdomain.subdomain.as_deref(), Some("www"));
    assert_eq!(subdomain.domain, "xn--mnchen-3ya.de");

    // IPv6 hosts keep their bracket form in the domain field.
    let v6 = parse_url("http://[2001:db8::1]:8443/x").expect("IPv6 parses");
    assert_eq!(v6.domain, "[2001:db8::1]");
    assert_eq!(v6.port, Some(8443));
    assert!(url_in_scope(
        "http://[2001:db8::1]:8443/x",
        "http://[2001:db8::1]:8443"
    ));

    // Port edges: valid bounds parse, anything above 65535 is rejected
    // exactly like `new URL`.
    assert_eq!(parse_url("http://a:0/x").unwrap().port, Some(0));
    assert_eq!(parse_url("http://a:65535/x").unwrap().port, Some(65535));
    assert_eq!(parse_url("http://a:65536/x"), None);
    assert_eq!(parse_url("http://a:99999999999999999999/x"), None);
    assert_eq!(parse_url("http://a:-1/x"), None);

    // Hosts with more or fewer than three labels keep no subdomain.
    assert_eq!(parse_url("http://a.b/c").unwrap().subdomain, None);
    assert_eq!(parse_url("http://a.b.c.d/c").unwrap().subdomain, None);
}

/// `javascript:`, `file:`, `data:` and percent-encoding edge inputs route to
/// the external browser or a deterministic scope match — never a panic.
#[test]
fn exotic_schemes_and_percent_encoding_route_deterministically() {
    let cases = [
        "javascript:alert(1)",
        "javascript://comment%0aalert(1)",
        "file:///C:/Windows/System32",
        "file://server/share",
        "data:text/html;base64,PHNjcmlwdD4=",
        "data:,hello",
        "mailto:someone@example.com",
        "https://example.com/%2e%2e/%2e%2e/etc",
        "https://example.com/a%00b",
        "https://user:pass@example.com:443/",
        "https://example.com:443/", // default port dropped
        "http://example.com./",     // trailing dot host
        "http://[::]/",
        "http://[fe80::1%25eth0]/", // zone id
        "///",
        "//",
        ":",
        "http://",
    ];
    for url in cases {
        // Neither the parse helper nor a full decision may panic.
        let _ = parse_url(url);
        let decision = route(
            url,
            &[app("a", "https://example.com")],
            &[tab("t", "https://example.com/x")],
        );
        // A URL that parses and matches the app scope opens a new tab; an
        // unparseable one falls through to the external browser.
        if parse_url(url).is_some() && url_in_scope(url, "https://example.com") {
            assert_eq!(
                decision,
                Route::NewTab {
                    application_id: "a".into(),
                    url: url.into()
                },
                "{url}"
            );
        } else {
            assert_eq!(decision, Route::ExternalBrowser, "{url}");
        }
    }
}

/// 10k open tabs: exact and loose matching still find the right tab, and
/// duplicate URLs take the last tab, in bounded time.
#[test]
fn ten_k_tabs_match_exactly_loosely_and_duplicated_bounded() {
    const N: u64 = 10_000;
    let mut tabs: Vec<OpenTab> = Vec::new();
    for i in 0..N {
        tabs.push(tab(&format!("t{i}"), &format!("https://example.com/{i}")));
    }

    let started = Instant::now();
    let decision = route("https://example.com/7", &[], &tabs);
    let elapsed = started.elapsed();
    assert_eq!(
        decision,
        Route::FocusTab {
            tab_id: "t7".into()
        }
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "10k-tab exact match took {elapsed:?}"
    );

    // Loose (hash-stripped) match: no exact tab carries `#frag`.
    let decision = route("https://example.com/42#frag", &[], &tabs);
    assert_eq!(
        decision,
        Route::PushAndFocusTab {
            tab_id: "t42".into(),
            url: "https://example.com/42#frag".into()
        }
    );

    // Duplicate URLs: the last tab with that URL wins the exact match.
    let mut dupes: Vec<OpenTab> = Vec::new();
    for i in 0..N {
        dupes.push(tab(&format!("t{i}"), "https://example.com/dup"));
    }
    let decision = route("https://example.com/dup", &[], &dupes);
    assert_eq!(
        decision,
        Route::FocusTab {
            tab_id: format!("t{}", N - 1)
        }
    );

    // Duplicates also collapse to one loose entry, last tab wins.
    let mut hashed: Vec<OpenTab> = Vec::new();
    for i in 0..N {
        hashed.push(tab(
            &format!("t{i}"),
            &format!("https://example.com/h#{}", i),
        ));
    }
    let decision = route("https://example.com/h#new", &[], &hashed);
    assert_eq!(
        decision,
        Route::PushAndFocusTab {
            tab_id: format!("t{}", N - 1),
            url: "https://example.com/h#new".into(),
        }
    );
}

/// 10k installed apps and 10k catalogue entries: scope scans stay linear and
/// bounded, wildcards and malformed scopes included.
#[test]
fn ten_k_apps_and_store_apps_stay_bounded() {
    const N: u64 = 10_000;
    let mut apps: Vec<InstalledApp> = Vec::new();
    for i in 0..N {
        apps.push(app(
            &format!("a{i}"),
            &format!("https://app{i}.example.com"),
        ));
    }
    // The matching app is last: the scan must still find it.
    apps.push(app("winner", "https://winner.example.com"));

    let started = Instant::now();
    let decision = route("https://winner.example.com/page", &apps, &[]);
    let elapsed = started.elapsed();
    assert_eq!(
        decision,
        Route::NewTab {
            application_id: "winner".into(),
            url: "https://winner.example.com/page".into()
        }
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "10k-app scan took {elapsed:?}"
    );

    // Catalogue: 10k store apps, the match is last, malformed scopes mixed in.
    let mut store: Vec<StoreApp> = Vec::new();
    for i in 0..N {
        store.push(store_app(&format!("https://s{i}.example.com")));
    }
    store.push(store_app("https://winner2.catalogue.example.com"));
    store.push(store_app("not a url"));
    store.push(store_app("http://"));
    let request = RouteRequest::new("https://winner2.catalogue.example.com/deep/path", &[], &[])
        .with_store_apps(&store);
    let started = Instant::now();
    let decision = request.route();
    let elapsed = started.elapsed();
    assert_eq!(
        decision,
        Route::InstallAndOpen {
            manifest_url: "https://manifests/store.json".into(),
            url: "https://winner2.catalogue.example.com/deep/path".into(),
        }
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "10k-store scan took {elapsed:?}"
    );

    // An opted-out winner is not replaced by a weaker catalogue entry.
    let mut opted: Vec<StoreApp> = vec![StoreApp {
        manifest_url: "https://manifests/opted.json".into(),
        scope: Some("https://opted.example.com".into()),
        extended_scopes: Vec::new(),
        opted_out: true,
    }];
    opted.push(store_app("https://opted.example.com"));
    let request =
        RouteRequest::new("https://opted.example.com/x", &[], &[]).with_store_apps(&opted);
    assert_eq!(request.route(), Route::ExternalBrowser);
}

/// Malformed scope lists (commas, empties, garbage) degrade to no-match, and
/// wildcard subdomains match in both directions like the TS.
#[test]
fn malformed_and_wildcard_scopes_behave_sensibly() {
    // Empty scope segments and garbage never match.
    assert!(!search_scopes("https://example.com/", &[]));
    assert!(!search_scopes("https://example.com/", &["".into()]));
    assert!(!search_scopes(
        "https://example.com/",
        &[
            "https://".into(),
            "http://".into(),
            ":::".into(),
            "https://other.com".into()
        ]
    ));
    // getScopes disables an app whose manifest scope is empty/absent even
    // with extended scopes present.
    let disabled = InstalledApp {
        application_id: "d".into(),
        manifest_url: String::new(),
        scope: Some(String::new()),
        extended_scopes: vec!["https://would.example.com".into()],
        custom_urls: Vec::new(),
        start_url: None,
        single_page: false,
    };
    assert_eq!(
        route("https://would.example.com/x", &[disabled], &[]),
        Route::ExternalBrowser
    );

    // Wildcard subdomains, both directions, from plain string scopes.
    let wildcard_scope = parse_url("https://*.example.com/").unwrap();
    let concrete_url = parse_url("https://shop.example.com/").unwrap();
    assert!(scope_matches(&concrete_url, &wildcard_scope));
    // A URL whose subdomain is literally `*` is covered by any concrete scope.
    let wildcard_url = parse_url("https://*.example.com/").unwrap();
    let concrete_scope = parse_url("https://shop.example.com/").unwrap();
    assert!(scope_matches(&wildcard_url, &concrete_scope));
    // Empty subdomains do not match wildcards, and different concrete
    // subdomains do not match each other.
    let bare = parse_url("https://example.com/").unwrap();
    let mail_scope = parse_url("https://mail.example.com/").unwrap();
    assert!(!scope_matches(&bare, &wildcard_scope));
    assert!(!scope_matches(&concrete_url, &mail_scope));
    // Wildcard URL vs wildcard scope still matches.
    assert!(scope_matches(&wildcard_url, &wildcard_scope));

    // Raw `*` inside a scope string survives parsing (WHATWG hosts allow `*`).
    assert!(parse_url("https://*.example.com/").is_some());
    assert!(
        route(
            "https://shop.example.com/",
            &[app("w", "https://*.example.com")],
            &[]
        ) != Route::ExternalBrowser
    );
}

/// Empty and whitespace-only inputs route to the external browser without
/// special cases.
#[test]
fn empty_inputs_fall_through_to_external_browser() {
    for url in ["", " ", "\t\n"] {
        assert_eq!(
            route(url, &[app("a", "https://example.com")], &[]),
            Route::ExternalBrowser
        );
    }
    // A requested external-browser target short-circuits everything.
    let apps = [app("a", "https://example.com")];
    let request =
        RouteRequest::new("https://example.com", &apps, &[]).with_target(Target::DefaultBrowser);
    assert_eq!(request.route(), Route::ExternalBrowser);
}
