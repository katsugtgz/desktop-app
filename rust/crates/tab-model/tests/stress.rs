//! Adversarial stress tests for `tab-model`.
//!
//! Bounds: every wall-clock assertion is explicit (`assert!(elapsed < ...)`)
//! and generous enough to hold on slow CI machines while still failing on
//! the quadratic pathologies these tests exist to catch.

use std::time::{Duration, Instant};

use tab_model::{Badge, TabAction, TabModel};

fn tab(tab_id: &str) -> TabAction {
    TabAction::add("app", tab_id, format!("https://example.com/{tab_id}"))
}

/// 100k adds followed by 100k removals on one application must stay linear.
/// `push_ordered` prepends to the newest-first order, so a `Vec`-prepend
/// implementation is quadratic (regression guard).
#[test]
fn hundred_k_adds_and_removals_on_one_app_stay_bounded() {
    const N: u64 = 100_000;
    let mut model = TabModel::new();

    let started = Instant::now();
    for i in 0..N {
        model.apply(tab(&format!("t{i}")));
    }
    let added = started.elapsed();

    assert_eq!(model.len() as u64, N);
    assert_eq!(model.ordered_tab_ids("app").len() as u64, N);
    // Order invariant: newest first.
    assert_eq!(model.ordered_tab_ids("app")[0], format!("t{}", N - 1));
    assert_eq!(
        model.ordered_tab_ids("app").last().map(String::as_str),
        Some("t0")
    );

    let removal_started = Instant::now();
    for i in 0..N {
        model.apply(TabAction::Remove {
            application_id: "app".into(),
            tab_id: format!("t{i}"),
        });
    }
    let removed = removal_started.elapsed();

    assert!(model.is_empty());
    assert!(model.ordered_tab_ids("app").is_empty());
    assert!(
        added < Duration::from_secs(3),
        "100k adds took {added:?}; quadratic prepend path regression"
    );
    assert!(
        removed < Duration::from_secs(3),
        "100k removals took {removed:?}"
    );
}

/// `REORDER_TAB` with extreme positions must clamp, never panic or wrap.
#[test]
fn reorder_extreme_positions_stay_in_bounds() {
    let mut model = TabModel::new();
    for id in ["a", "b", "c", "d"] {
        model.apply(TabAction::add("app", id, format!("https://x/{id}")));
    }

    for position in [i64::MIN, -2, -1, 0, 1, 3, 4, 5, i64::MAX] {
        model.apply(TabAction::ReorderTab {
            tab_id: "a".into(),
            new_position: position,
        });
        let ids = model.ordered_tab_ids("app");
        assert_eq!(ids.len(), 4, "reorder to {position} changed the count");
        let mut sorted: Vec<&str> = ids.iter().map(String::as_str).collect();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            ["a", "b", "c", "d"],
            "reorder to {position} lost ids"
        );
    }

    // Adds prepend: the last-added tab is first.
    let ids = model.ordered_tab_ids("app");
    assert_eq!(ids, ["d", "c", "b", "a"]);

    // Reordering to position 0 moves the tab to the front.
    model.apply(TabAction::ReorderTab {
        tab_id: "c".into(),
        new_position: 0,
    });
    assert_eq!(model.ordered_tab_ids("app"), ["c", "d", "b", "a"]);

    // Unknown tab and duplicate reorder are no-ops.
    model.apply(TabAction::ReorderTab {
        tab_id: "missing".into(),
        new_position: 0,
    });
    model.apply(TabAction::ReorderTab {
        tab_id: "a".into(),
        new_position: i64::MIN,
    });
    assert_eq!(model.ordered_tab_ids("app").len(), 4);
}

/// Duplicate tab ids across applications and empty ids must not corrupt the
/// model: lookups stay keyed by id, orders keep their application.
#[test]
fn duplicate_and_empty_ids_stay_consistent() {
    let mut model = TabModel::new();

    // Empty tab id: stored, but never pushed to an order (TS saga guard).
    model.apply(TabAction::add("app-1", "", "https://empty/"));
    assert_eq!(model.len(), 1);
    assert!(model.ordered_tab_ids("app-1").is_empty());

    // Same id in two applications: second Add replaces the tab record, so
    // only the new application's order still resolves it.
    model.apply(TabAction::add("app-1", "dup", "https://one/"));
    model.apply(TabAction::add("app-2", "dup", "https://two/"));
    assert_eq!(model.len(), 2);
    assert_eq!(model.tab("dup").unwrap().application_id, "app-2");
    let app_1: Vec<&str> = model
        .tabs_for_application("app-1")
        .into_iter()
        .map(|t| t.tab_id.as_str())
        .collect();
    let app_2: Vec<&str> = model
        .tabs_for_application("app-2")
        .into_iter()
        .map(|t| t.tab_id.as_str())
        .collect();
    assert!(
        app_1.is_empty(),
        "stale order entry of the replaced tab is skipped"
    );
    assert_eq!(app_2, ["dup"]);

    // Explicit duplicate pushes are allowed (TS `PUSH_TAB` parity) and a
    // remove clears every occurrence.
    model.apply(TabAction::PushOrderedTab {
        application_id: "app-1".into(),
        tab_id: "dup".into(),
    });
    assert_eq!(model.ordered_tab_ids("app-1"), ["dup", "dup"]);
    model.apply(TabAction::RemoveOrderedTab {
        application_id: "whatever".into(),
        tab_id: "dup".into(),
    });
    assert!(model.ordered_tab_ids("app-1").is_empty());
    assert!(model.ordered_tab_ids("app-2").is_empty());
    // The tab record itself survives `REMOVE_TAB`.
    assert!(model.tab("dup").is_some());
}

/// Huge favicon lists and repeated updates must not panic, leak order, or
/// take unbounded time.
#[test]
fn huge_favicon_lists_and_repeated_updates_stay_bounded() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app", "t", "https://x/"));

    let favicons: Vec<String> = (0..100_000).map(|i| format!("https://i/{i}.png")).collect();
    model.apply(TabAction::UpdateFavicons {
        tab_id: "t".into(),
        favicons: favicons.clone(),
    });
    assert_eq!(model.tab("t").unwrap().favicons, favicons);

    let started = Instant::now();
    for i in 0..10_000u64 {
        model.apply(TabAction::UpdateTitle {
            tab_id: "t".into(),
            title: format!("title {i}"),
        });
        model.apply(TabAction::UpdateUrl {
            tab_id: "t".into(),
            url: format!("https://x/{i}"),
        });
        model.apply(TabAction::UpdateBadge {
            tab_id: "t".into(),
            badge: i.to_string(),
        });
        model.apply(TabAction::UpdateLastActivity {
            tab_id: "t".into(),
            at_ms: i64::MAX - i as i64,
        });
    }
    let elapsed = started.elapsed();

    assert_eq!(model.tab("t").unwrap().title.as_deref(), Some("title 9999"));
    assert_eq!(model.tab("t").unwrap().badge, Some(Badge::Number(9_999)));
    assert!(
        elapsed < Duration::from_secs(3),
        "40k repeated updates took {elapsed:?}"
    );
}

/// Badge normalization under hostile strings: huge digit runs, signs,
/// whitespace, garbage — never a panic, always a sane `Badge`.
#[test]
fn badge_parsing_hostile_strings_do_not_panic() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app", "t", "https://x/"));

    let cases: [(&str, Option<Badge>); 10] = [
        ("", None),
        ("9223372036854775807", Some(Badge::Number(i64::MAX))),
        (
            "-9223372036854775807",
            Some(Badge::Number(-9_223_372_036_854_775_807)),
        ),
        (
            "9223372036854775808",
            Some(Badge::Text("9223372036854775808".into())),
        ),
        (
            "99999999999999999999999999",
            Some(Badge::Text("99999999999999999999999999".into())),
        ),
        ("  42", Some(Badge::Number(42))),
        ("-7abc", Some(Badge::Number(-7))),
        ("+9x", Some(Badge::Number(9))),
        ("--5", Some(Badge::Text("--5".into()))),
        ("🎯", Some(Badge::Text("🎯".into()))),
    ];
    for (badge, expected) in cases {
        model.apply(TabAction::UpdateBadge {
            tab_id: "t".into(),
            badge: badge.into(),
        });
        assert_eq!(model.tab("t").unwrap().badge, expected, "badge {badge:?}");
    }

    // Updates against unknown tabs and extreme timestamps are no-ops.
    model.apply(TabAction::UpdateBadge {
        tab_id: "missing".into(),
        badge: "1".into(),
    });
    model.apply(TabAction::UpdateLastPutToSleepAt {
        tab_id: "t".into(),
        at_ms: i64::MIN,
    });
    assert_eq!(model.tab("t").unwrap().last_put_to_sleep_at, Some(i64::MIN));
}
