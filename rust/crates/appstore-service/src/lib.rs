//! Read-only appstore queries over the manifest registry.
//!
//! Port of the appstore read commands `packages/appstore` reaches through
//! `window.bxApi.applications` (backed by `packages/app/src/app-store/sagas.ts`):
//! `search`, `getMostPopularApps`, `getAllCategories` and
//! `getApplicationsByCategory`, plus the `listMostPopularApplications`
//! bucketing from `packages/app/manifests/index.ts`.
//!
//! Plain functions with serde-friendly return types so s14 can wire them
//! to napi-rs without touching call sites; no `#[napi]` attributes yet.
//!
//! Not ported in this slice: install/uninstall/requestPrivate (s11b) and
//! anything needing the HTTP GraphQL transport (only the local
//! manifest-registry-backed shapes the UI actually renders are modeled).

use manifest_registry::{Manifest, MinimalApplication};
use serde::{Deserialize, Serialize};

/// Port of `PopularApps` (`packages/app/manifests/index.ts`): three
/// `recommendedPosition` buckets of ten.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PopularApps {
    pub cream_of_the_crop_apps: Vec<MinimalApplication>,
    pub runner_ups: Vec<MinimalApplication>,
    pub noteworthy: Vec<MinimalApplication>,
}

/// `specialCategoriesForList` from `packages/app/src/app-store/sagas.ts`:
/// stripped from the sorted list, `Miscellaneous` re-appended last.
const SPECIAL_CATEGORIES: [&str; 3] = ["My Private Apps", "Company Apps", "Miscellaneous"];

/// Port of `searchApplication` / `bxApi.applications.search`: fuzzy search
/// over every listable application (bundled + `private`).
pub fn search_applications(query: &str, private: &[Manifest]) -> Vec<MinimalApplication> {
    manifest_registry::search(query, private)
}

/// Port of `getMostPopularApplications` /
/// `listMostPopularApplications`: apps with `recommendedPosition > 0`,
/// ascending, bucketed 0-10 / 10-20 / 20-30.
pub fn get_most_popular_apps(private: &[Manifest]) -> PopularApps {
    let mut apps: Vec<Manifest> = manifest_registry::list_all_applications(private)
        .into_iter()
        .filter(|m| m.inner.recommended_position() > 0.0)
        .collect();
    // TS `Array.sort` on numbers is lexicographic on strings for >10 values;
    // positions here are 1..30 so ascending numeric matches what the UI got.
    apps.sort_by(|a, b| {
        a.inner
            .recommended_position()
            .partial_cmp(&b.inner.recommended_position())
            .expect("recommendedPosition is never NaN (parsed finite or 0)")
    });
    let minimal = |slice: &[Manifest]| {
        slice
            .iter()
            .map(manifest_registry::manifest_to_minimal_application)
            .collect()
    };
    PopularApps {
        cream_of_the_crop_apps: minimal(&apps[0..apps.len().min(10)]),
        runner_ups: minimal(&apps[10..apps.len().min(20)]),
        noteworthy: minimal(&apps[20..apps.len().min(30)]),
    }
}

/// Port of `retrieveAllCategories`: distinct, insertion-ordered categories
/// of every listable application (TS `Object.keys` order).
fn retrieve_all_categories(private: &[Manifest]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for m in manifest_registry::list_all_applications(private) {
        if let Some(c) = m.inner.category.as_deref() {
            if !seen.iter().any(|s| s == c) {
                seen.push(c.to_owned());
            }
        }
    }
    seen
}

/// Port of `getAllCategories` / `bxApi.applications.getAllCategories`:
/// distinct categories, alphabetically, with the special ones stripped and
/// `Miscellaneous` forced last. Empty input stays empty (TS early return).
pub fn get_all_categories(private: &[Manifest]) -> Vec<String> {
    let categories = retrieve_all_categories(private);
    if categories.is_empty() {
        return categories;
    }
    let mut sorted: Vec<String> = categories
        .into_iter()
        .filter(|c| !SPECIAL_CATEGORIES.contains(&c.as_str()))
        .collect();
    sorted.sort();
    sorted.push("Miscellaneous".to_owned());
    sorted
}

/// Port of `getApplicationsByCategory` /
/// `bxApi.applications.getApplicationsByCategory`: minimal applications
/// grouped by category, each bucket sorted by name (TS `localeCompare`;
/// plain string ordering here — names are ASCII).
pub fn get_applications_by_category(
    private: &[Manifest],
) -> Vec<(String, Vec<MinimalApplication>)> {
    let mut grouped: Vec<(String, Vec<MinimalApplication>)> = Vec::new();
    for m in manifest_registry::list_all_applications(private) {
        let Some(category) = m.inner.category.clone() else {
            continue;
        };
        let minimal = manifest_registry::manifest_to_minimal_application(&m);
        match grouped.iter_mut().find(|(c, _)| *c == category) {
            Some((_, apps)) => apps.push(minimal),
            None => grouped.push((category, vec![minimal])),
        }
    }
    for (_, apps) in &mut grouped {
        apps.sort_by(|a, b| a.name.cmp(&b.name));
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_ids(apps: &[MinimalApplication]) -> Vec<&str> {
        apps.iter().map(|a| a.id.as_str()).collect()
    }

    #[test]
    fn search_delegates_to_registry_semantics() {
        // Exact-name query short-circuits to the single match.
        let hits = search_applications("Gmail", &[]);
        assert_eq!(minimal_ids(&hits), vec!["14"]);

        // Query-word prefix match: "calendar" → "Google Calendar".
        let hits = search_applications("calendar", &[]);
        assert!(minimal_ids(&hits).contains(&"18"));

        // Empty query: no results (Fuse.search('') semantics).
        assert!(search_applications("", &[]).is_empty());
        assert!(search_applications("   ", &[]).is_empty());
    }

    #[test]
    fn search_includes_private_manifests() {
        let private = vec![manifest_registry::Manifest {
            inner: serde_json::from_str(
                r##"{"name":"Acme Corp Intranet","category":"Miscellaneous",
                    "start_url":"https://acme.test","scope":"https://acme.test",
                    "theme_color":"#112233","recommendedPosition":"0"}"##,
            )
            .unwrap(),
            id: "1000001".to_owned(),
            icon: String::new(),
        }];
        let hits = search_applications("acme", &private);
        assert_eq!(minimal_ids(&hits), vec!["1000001"]);
    }

    #[test]
    fn most_popular_buckets_match_ts_layout() {
        let popular = get_most_popular_apps(&[]);
        // 30 bundled apps have recommendedPosition > 0.
        assert_eq!(popular.cream_of_the_crop_apps.len(), 10);
        assert_eq!(popular.runner_ups.len(), 10);
        assert_eq!(popular.noteworthy.len(), 10);

        // Spot-check bucket edges against the definitions on disk.
        assert_eq!(popular.cream_of_the_crop_apps[0].id, "14"); // Gmail, pos 1
        assert_eq!(popular.cream_of_the_crop_apps[9].id, "126"); // Telegram, pos 10
        assert_eq!(popular.runner_ups[0].id, "388"); // Instagram, pos 11
        assert_eq!(popular.runner_ups[9].id, "138"); // Skype, pos 20
        assert_eq!(popular.noteworthy[0].id, "69"); // Dropbox, pos 21
        assert_eq!(popular.noteworthy[9].id, "157"); // Todoist, pos 30

        // Ascending positions within a bucket.
        let positions: Vec<f64> = popular
            .cream_of_the_crop_apps
            .iter()
            .map(|a| a.recommended_position)
            .collect();
        let mut sorted = positions.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(positions, sorted);
    }

    #[test]
    fn most_popular_ignores_zero_positions_and_do_not_list() {
        // Appstore manifest (id 1) is doNotList; nothing with position 0
        // leaks in: total bucketed == apps with position > 0.
        let popular = get_most_popular_apps(&[]);
        let total = popular.cream_of_the_crop_apps.len()
            + popular.runner_ups.len()
            + popular.noteworthy.len();
        assert_eq!(total, 30);
        assert!(!minimal_ids(&popular.cream_of_the_crop_apps).contains(&"1"));
    }

    #[test]
    fn all_categories_sorted_with_miscellaneous_last() {
        let categories = get_all_categories(&[]);
        assert_eq!(
            categories,
            vec![
                "Accounting & Finance",
                "Admin & Back-office",
                "Blogging & Content Creation",
                "Communication & Collaboration",
                "Curation & Sourcing",
                "Design & Creativity",
                "Developer Tools",
                "HR & Legal",
                "Marketing & Analytics",
                "Sales & CRM",
                "Social Media & Advertising",
                "Storage & File-sharing",
                "Task & Project Management",
                "User Support & Survey",
                "Miscellaneous",
            ]
        );
    }

    #[test]
    fn applications_by_category_buckets_sorted_by_name() {
        let grouped = get_applications_by_category(&[]);
        let map: std::collections::HashMap<&str, &[MinimalApplication]> = grouped
            .iter()
            .map(|(c, apps)| (c.as_str(), apps.as_slice()))
            .collect();

        // Every listable, categorized app appears exactly once.
        let total: usize = grouped.iter().map(|(_, a)| a.len()).sum();
        assert_eq!(
            total,
            manifest_registry::list_all_applications(&[])
                .iter()
                .filter(|m| m.inner.category.is_some())
                .count()
        );

        // Bucket contents sorted by name.
        for (_, apps) in &grouped {
            let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(names, sorted, "bucket not name-sorted");
        }

        // Spot-check a known bucket.
        let storage = map.get("Storage & File-sharing").unwrap();
        assert!(minimal_ids(storage).contains(&"16")); // Google Drive
        // Minimal projection carries the TS field set.
        let drive = storage.iter().find(|a| a.id == "16").unwrap();
        assert_eq!(drive.bx_app_manifest_url, "station-manifest://16");
        assert!(!drive.is_chrome_extension);
    }

    #[test]
    fn private_manifests_flow_through_all_commands() {
        let private = vec![manifest_registry::Manifest {
            inner: serde_json::from_str(
                r##"{"name":"Zeta Tools","category":"Developer Tools",
                    "start_url":"https://zeta.test","scope":"https://zeta.test",
                    "theme_color":"#000000","recommendedPosition":"5"}"##,
            )
            .unwrap(),
            id: "1000002".to_owned(),
            icon: String::new(),
        }];
        // Popular: private app at position 5 pushes into cream bucket and
        // shifts everything after it; buckets stay capped at ten and the
        // 31st app (Todoist, pos 30) falls off the noteworthy window.
        let popular = get_most_popular_apps(&private);
        assert!(minimal_ids(&popular.cream_of_the_crop_apps).contains(&"1000002"));
        assert_eq!(popular.cream_of_the_crop_apps.len(), 10);
        assert!(!minimal_ids(&popular.noteworthy).contains(&"157"));

        // Categories: no new category introduced (Developer Tools exists).
        assert_eq!(get_all_categories(&private).len(), get_all_categories(&[]).len());

        // By-category: private app lands in its bucket.
        let grouped = get_applications_by_category(&private);
        let dev = grouped
            .iter()
            .find(|(c, _)| c == "Developer Tools")
            .unwrap();
        assert!(minimal_ids(&dev.1).contains(&"1000002"));
    }
}
