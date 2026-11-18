use tab_model::{Badge, TabAction, TabModel};

#[test]
fn add_stores_all_fields_with_ts_defaults() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://example.com/"));

    assert_eq!(model.len(), 1);
    let tab = model.tab("tab-1").expect("tab stored under its id");
    assert_eq!(tab.tab_id, "tab-1");
    assert_eq!(tab.application_id, "app-1");
    assert_eq!(tab.url, "https://example.com/");
    assert_eq!(tab.title, None);
    assert_eq!(tab.badge, None);
    assert!(tab.favicons.is_empty());
    assert!(!tab.is_application_home);
    assert!(!tab.is_loading);
    assert!(!tab.can_go_back);
    assert!(!tab.can_go_forward);
    assert_eq!(tab.last_put_to_sleep_at, None);
    assert_eq!(tab.last_activity_at, None);
    assert!(!tab.ignored_for_back_history);
    assert_eq!(model.tab("missing"), None);
}

#[test]
fn add_pushes_non_home_tab_to_front_of_application_order() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));
    model.apply(TabAction::add("app-1", "tab-2", "https://two/"));
    model.apply(TabAction::add("app-2", "tab-3", "https://three/"));

    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-2", "tab-1"]);
    assert_eq!(model.ordered_tab_ids("app-2"), ["tab-3"]);
    assert!(model.ordered_tab_ids("missing").is_empty());

    let app_1_ids: Vec<&str> = model
        .tabs_for_application("app-1")
        .into_iter()
        .map(|tab| tab.tab_id.as_str())
        .collect();
    assert_eq!(app_1_ids, ["tab-2", "tab-1"]);
    assert!(model.tabs_for_application("app-3").is_empty());
    assert_eq!(model.applications_with_tabs(), ["app-1", "app-2"]);
}

#[test]
fn home_tab_is_stored_but_not_pushed_to_order() {
    let mut model = TabModel::new();
    model.apply(TabAction::Add {
        application_id: "app-1".to_string(),
        tab_id: "home".to_string(),
        url: "https://home/".to_string(),
        is_application_home: true,
    });

    assert_eq!(model.len(), 1);
    assert!(
        model
            .tab("home")
            .expect("home tab stored")
            .is_application_home
    );
    assert!(model.ordered_tab_ids("app-1").is_empty());
    assert!(model.tabs_for_application("app-1").is_empty());
    assert!(model.applications_with_tabs().is_empty());
}

#[test]
fn update_actions_mutate_only_the_target_tab() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));
    model.apply(TabAction::add("app-2", "tab-2", "https://two/"));

    model.apply(TabAction::UpdateTitle {
        tab_id: "tab-1".to_string(),
        title: "One".to_string(),
    });
    model.apply(TabAction::UpdateUrl {
        tab_id: "tab-1".to_string(),
        url: "https://one/changed".to_string(),
    });
    model.apply(TabAction::UpdateFavicons {
        tab_id: "tab-1".to_string(),
        favicons: vec!["a.png".to_string(), "b.png".to_string()],
    });
    model.apply(TabAction::UpdateLoading {
        tab_id: "tab-1".to_string(),
        is_loading: true,
    });
    model.apply(TabAction::UpdateBackForward {
        tab_id: "tab-1".to_string(),
        can_go_back: true,
        can_go_forward: true,
    });
    model.apply(TabAction::UpdateLastActivity {
        tab_id: "tab-1".to_string(),
        at_ms: 111,
    });
    model.apply(TabAction::UpdateLastPutToSleepAt {
        tab_id: "tab-1".to_string(),
        at_ms: 222,
    });
    model.apply(TabAction::IgnoreForBackHistory {
        tab_id: "tab-1".to_string(),
    });

    let tab = model.tab("tab-1").expect("tab-1 still stored");
    assert_eq!(tab.title.as_deref(), Some("One"));
    assert_eq!(tab.url, "https://one/changed");
    assert_eq!(tab.favicons, ["a.png", "b.png"]);
    assert!(tab.is_loading);
    assert!(tab.can_go_back);
    assert!(tab.can_go_forward);
    assert_eq!(tab.last_activity_at, Some(111));
    assert_eq!(tab.last_put_to_sleep_at, Some(222));
    assert!(tab.ignored_for_back_history);

    let untouched = model.tab("tab-2").expect("tab-2 still stored");
    assert_eq!(untouched.title, None);
    assert_eq!(untouched.url, "https://two/");
    assert!(untouched.favicons.is_empty());
    assert!(!untouched.is_loading);
    assert!(!untouched.can_go_back);
    assert!(!untouched.can_go_forward);
    assert_eq!(untouched.last_activity_at, None);
    assert_eq!(untouched.last_put_to_sleep_at, None);
    assert!(!untouched.ignored_for_back_history);
}

#[test]
fn updates_and_ignore_for_back_history_ignore_unknown_tab_ids() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));
    let before = model.tab("tab-1").cloned().expect("tab-1 stored");

    model.apply(TabAction::UpdateTitle {
        tab_id: "missing".to_string(),
        title: "nope".to_string(),
    });
    model.apply(TabAction::UpdateUrl {
        tab_id: "missing".to_string(),
        url: "nope".to_string(),
    });
    model.apply(TabAction::UpdateFavicons {
        tab_id: "missing".to_string(),
        favicons: vec!["nope.png".to_string()],
    });
    model.apply(TabAction::UpdateLoading {
        tab_id: "missing".to_string(),
        is_loading: true,
    });
    model.apply(TabAction::UpdateBackForward {
        tab_id: "missing".to_string(),
        can_go_back: true,
        can_go_forward: true,
    });
    model.apply(TabAction::UpdateLastActivity {
        tab_id: "missing".to_string(),
        at_ms: 1,
    });
    model.apply(TabAction::UpdateLastPutToSleepAt {
        tab_id: "missing".to_string(),
        at_ms: 1,
    });
    model.apply(TabAction::IgnoreForBackHistory {
        tab_id: "missing".to_string(),
    });

    assert_eq!(model.len(), 1);
    assert_eq!(model.tab("tab-1"), Some(&before));
}

#[test]
fn updating_last_activity_clears_ignored_for_back_history() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));

    model.apply(TabAction::IgnoreForBackHistory {
        tab_id: "tab-1".to_string(),
    });
    assert!(
        model
            .tab("tab-1")
            .expect("tab-1 stored")
            .ignored_for_back_history
    );

    model.apply(TabAction::UpdateLastActivity {
        tab_id: "tab-1".to_string(),
        at_ms: 10,
    });
    let tab = model.tab("tab-1").expect("tab-1 stored");
    assert!(!tab.ignored_for_back_history);
    assert_eq!(tab.last_activity_at, Some(10));
}

#[test]
fn badge_is_normalized_like_the_ts_reducer() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));

    model.apply(TabAction::UpdateBadge {
        tab_id: "tab-1".to_string(),
        badge: "3".to_string(),
    });
    assert_eq!(
        model.tab("tab-1").expect("tab-1 stored").badge,
        Some(Badge::Number(3))
    );

    // `parseInt` stops at the first non-digit.
    model.apply(TabAction::UpdateBadge {
        tab_id: "tab-1".to_string(),
        badge: "12abc".to_string(),
    });
    assert_eq!(
        model.tab("tab-1").expect("tab-1 stored").badge,
        Some(Badge::Number(12))
    );

    model.apply(TabAction::UpdateBadge {
        tab_id: "tab-1".to_string(),
        badge: "-7".to_string(),
    });
    assert_eq!(
        model.tab("tab-1").expect("tab-1 stored").badge,
        Some(Badge::Number(-7))
    );

    // Empty means "no badge", like the TS `null`.
    model.apply(TabAction::UpdateBadge {
        tab_id: "tab-1".to_string(),
        badge: String::new(),
    });
    assert_eq!(model.tab("tab-1").expect("tab-1 stored").badge, None);

    model.apply(TabAction::UpdateBadge {
        tab_id: "tab-1".to_string(),
        badge: "inbox".to_string(),
    });
    assert_eq!(
        model.tab("tab-1").expect("tab-1 stored").badge,
        Some(Badge::Text("inbox".to_string()))
    );

    // `is-empty` only treats the empty string as empty; `parseInt(" ")` is NaN.
    model.apply(TabAction::UpdateBadge {
        tab_id: "tab-1".to_string(),
        badge: " ".to_string(),
    });
    assert_eq!(
        model.tab("tab-1").expect("tab-1 stored").badge,
        Some(Badge::Text(" ".to_string()))
    );

    let before = model.tab("tab-1").cloned().expect("tab-1 stored");
    model.apply(TabAction::UpdateBadge {
        tab_id: "missing".to_string(),
        badge: "0".to_string(),
    });
    assert_eq!(model.tab("tab-1"), Some(&before));
}

#[test]
fn remove_deletes_tab_from_map_and_application_order() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));
    model.apply(TabAction::add("app-1", "tab-2", "https://two/"));
    model.apply(TabAction::add("app-2", "tab-3", "https://three/"));

    model.apply(TabAction::Remove {
        application_id: "app-1".to_string(),
        tab_id: "tab-1".to_string(),
    });
    assert!(model.tab("tab-1").is_none());
    assert_eq!(model.len(), 2);
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-2"]);
    assert_eq!(model.ordered_tab_ids("app-2"), ["tab-3"]);

    // Unknown ids are a no-op, as in the TS reducer.
    model.apply(TabAction::Remove {
        application_id: "app-1".to_string(),
        tab_id: "missing".to_string(),
    });
    assert_eq!(model.len(), 2);
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-2"]);
    assert_eq!(model.applications_with_tabs(), ["app-1", "app-2"]);

    // The last ordered tab of an application drops its order entry.
    model.apply(TabAction::Remove {
        application_id: "app-1".to_string(),
        tab_id: "tab-2".to_string(),
    });
    assert!(model.ordered_tab_ids("app-1").is_empty());
    assert_eq!(model.applications_with_tabs(), ["app-2"]);
}

#[test]
fn remove_clears_the_tab_id_from_every_application_order() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));
    model.apply(TabAction::PushOrderedTab {
        application_id: "app-2".to_string(),
        tab_id: "tab-1".to_string(),
    });
    // `getOrderedTabsForApplicationId` only resolves tabs of the application.
    assert!(model.tabs_for_application("app-2").is_empty());

    model.apply(TabAction::Remove {
        application_id: "app-1".to_string(),
        tab_id: "tab-1".to_string(),
    });
    assert!(model.ordered_tab_ids("app-1").is_empty());
    assert!(model.ordered_tab_ids("app-2").is_empty());
    assert!(model.applications_with_tabs().is_empty());
}

#[test]
fn close_removes_tab_like_remove() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));
    model.apply(TabAction::add("app-1", "tab-2", "https://two/"));

    model.apply(TabAction::Close {
        tab_id: "tab-1".to_string(),
    });
    assert!(model.tab("tab-1").is_none());
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-2"]);

    model.apply(TabAction::Close {
        tab_id: "missing".to_string(),
    });
    assert_eq!(model.len(), 1);
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-2"]);
}

#[test]
fn close_all_in_app_removes_its_tabs_and_order_entry() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));
    model.apply(TabAction::add("app-1", "tab-2", "https://two/"));
    model.apply(TabAction::add("app-2", "tab-3", "https://three/"));
    model.apply(TabAction::Add {
        application_id: "app-1".to_string(),
        tab_id: "home-1".to_string(),
        url: "https://home/".to_string(),
        is_application_home: true,
    });

    model.apply(TabAction::CloseAllInApp {
        application_id: "app-1".to_string(),
    });
    assert_eq!(model.len(), 1);
    assert!(model.tab("tab-3").is_some());
    assert!(model.tab("home-1").is_none());
    assert!(model.ordered_tab_ids("app-1").is_empty());
    assert_eq!(model.applications_with_tabs(), ["app-2"]);

    model.apply(TabAction::CloseAllInApp {
        application_id: "missing".to_string(),
    });
    assert_eq!(model.len(), 1);
    assert_eq!(model.applications_with_tabs(), ["app-2"]);
}

#[test]
fn remove_ordered_tab_ignores_application_id_and_unknown_tabs() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));
    model.apply(TabAction::add("app-1", "tab-2", "https://two/"));
    model.apply(TabAction::add("app-2", "tab-3", "https://three/"));

    // `REMOVE_TAB` keeps its `applicationId` only for action parity.
    model.apply(TabAction::RemoveOrderedTab {
        application_id: "app-2".to_string(),
        tab_id: "tab-1".to_string(),
    });
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-2"]);
    // Order-only removal keeps the tab itself.
    assert!(model.tab("tab-1").is_some());
    assert_eq!(model.len(), 3);

    let app_2_order = model.ordered_tab_ids("app-2").clone();
    model.apply(TabAction::RemoveOrderedTab {
        application_id: "app-2".to_string(),
        tab_id: "missing".to_string(),
    });
    assert_eq!(model.ordered_tab_ids("app-2"), app_2_order);

    // Emptying an application's order drops its entry but not its tabs.
    model.apply(TabAction::RemoveOrderedTab {
        application_id: "app-2".to_string(),
        tab_id: "tab-3".to_string(),
    });
    assert!(model.ordered_tab_ids("app-2").is_empty());
    assert!(model.tab("tab-3").is_some());
    assert_eq!(model.applications_with_tabs(), ["app-1"]);
}

fn model_with_three_ordered_tabs() -> TabModel {
    let mut model = TabModel::new();
    for tab_id in ["tab-1", "tab-2", "tab-3"] {
        model.apply(TabAction::add("app-1", tab_id, "https://example.com/"));
    }
    model
}

#[test]
fn reorder_tab_moves_up_and_down_within_its_application_order() {
    let mut model = model_with_three_ordered_tabs();
    model.apply(TabAction::add("app-2", "other", "https://other/"));
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-3", "tab-2", "tab-1"]);

    model.apply(TabAction::ReorderTab {
        tab_id: "tab-1".to_string(),
        new_position: 0,
    });
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-1", "tab-3", "tab-2"]);

    model.apply(TabAction::ReorderTab {
        tab_id: "tab-1".to_string(),
        new_position: 2,
    });
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-3", "tab-2", "tab-1"]);

    assert_eq!(model.ordered_tab_ids("app-2"), ["other"]);
}

#[test]
fn reorder_tab_clamps_positions_like_a_list_splice() {
    let mut model = model_with_three_ordered_tabs();

    model.apply(TabAction::ReorderTab {
        tab_id: "tab-2".to_string(),
        new_position: 99,
    });
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-3", "tab-1", "tab-2"]);

    model.apply(TabAction::ReorderTab {
        tab_id: "tab-2".to_string(),
        new_position: -1,
    });
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-3", "tab-2", "tab-1"]);

    model.apply(TabAction::ReorderTab {
        tab_id: "tab-2".to_string(),
        new_position: -99,
    });
    assert_eq!(model.ordered_tab_ids("app-1"), ["tab-2", "tab-3", "tab-1"]);
}

#[test]
fn reorder_tab_ignores_unknown_and_untracked_tabs() {
    let mut model = TabModel::new();
    model.apply(TabAction::add("app-1", "tab-1", "https://one/"));
    model.apply(TabAction::add("app-1", "tab-2", "https://two/"));
    model.apply(TabAction::Add {
        application_id: "app-1".to_string(),
        tab_id: "home".to_string(),
        url: "https://home/".to_string(),
        is_application_home: true,
    });
    let order = model.ordered_tab_ids("app-1").to_vec();

    model.apply(TabAction::ReorderTab {
        tab_id: "missing".to_string(),
        new_position: 0,
    });
    assert_eq!(model.ordered_tab_ids("app-1"), order);

    model.apply(TabAction::ReorderTab {
        tab_id: "home".to_string(),
        new_position: 0,
    });
    assert_eq!(model.ordered_tab_ids("app-1"), order);
}
