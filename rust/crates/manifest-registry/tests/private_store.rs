//! Robustness of the on-disk private-manifest store: atomic writes (a failure leaves the old
//! file and no temp), size and count caps for a hostile file, and an id counter that cannot
//! overflow into a panic.

use manifest_registry::{
    NewPrivateApplication, PrivateStore, MAX_PRIVATE_MANIFESTS, MAX_PRIVATE_MANIFEST_BYTES,
};

/// A scratch directory unique per test process, removed on drop.
struct Scratch {
    root: std::path::PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "manifest-registry-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).expect("scratch dir");
        Self { root }
    }

    fn path(&self) -> std::path::PathBuf {
        self.root.join("private-manifests.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn recipe(name: &str) -> NewPrivateApplication {
    NewPrivateApplication {
        name: name.to_owned(),
        theme_color: "#112233".to_owned(),
        icon_url: "https://icon.test/i.png".to_owned(),
        start_url: "https://app.test/".to_owned(),
        scope: "https://app.test".to_owned(),
    }
}

#[test]
fn a_save_and_delete_round_trip_through_the_atomic_write() {
    let scratch = Scratch::new("roundtrip");
    let mut store = PrivateStore::from_manifests(scratch.path()).expect("store");
    let id = store.save_new_application(recipe("Acme")).expect("save");

    let reloaded = PrivateStore::from_manifests(scratch.path()).expect("reload");
    assert_eq!(reloaded.get_private_manifests().len(), 1);
    assert_eq!(
        reloaded.get_private_manifests()[0].inner.name.as_deref(),
        Some("Acme")
    );

    let mut store = reloaded;
    store.delete_manifest(id).expect("delete");
    let reloaded = PrivateStore::from_manifests(scratch.path()).expect("reload");
    assert!(reloaded.get_private_manifests().is_empty());
}

#[test]
fn a_failed_write_keeps_the_old_file_and_leaves_no_temp() {
    let scratch = Scratch::new("failed-write");
    // The store path is a directory: the rename over it fails, like a locked or read-only target.
    std::fs::create_dir_all(scratch.path()).expect("directory at the store path");
    let mut store = PrivateStore::from_manifests(scratch.path()).expect("store over a directory");

    let result = store.save_new_application(recipe("Nope"));

    assert!(result.is_err(), "a write into a directory reported success");
    assert!(
        scratch.path().is_dir(),
        "the failed write replaced what was there"
    );
    let temps: Vec<String> = std::fs::read_dir(&scratch.root)
        .expect("read scratch")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(temps.is_empty(), "temp files left behind: {temps:?}");
    // The in-memory list was rolled back with the failed write.
    assert!(store.get_private_manifests().is_empty());
}

#[test]
fn an_oversized_private_file_is_ignored() {
    let scratch = Scratch::new("oversized");
    let path = scratch.path();
    let mut file = std::fs::File::create(&path).expect("create");
    use std::io::Write;
    file.write_all(br#"{"data":[]}"#).expect("write");
    file.set_len(MAX_PRIVATE_MANIFEST_BYTES + 1)
        .expect("extend");
    drop(file);

    let store = PrivateStore::from_manifests(&path).expect("a hostile file is not fatal");

    assert!(store.get_private_manifests().is_empty());
}

#[test]
fn a_file_with_too_many_records_is_ignored() {
    let scratch = Scratch::new("too-many");
    let path = scratch.path();
    let records: Vec<serde_json::Value> = (0..=MAX_PRIVATE_MANIFESTS)
        .map(|index| {
            serde_json::json!({
                "name": format!("App {index}"),
                "scope": "https://app.test",
                "icons": [],
                "start_url": "https://app.test/",
                "category": "Miscellaneous",
                "theme_color": "#000",
                "id": 1_000_000 + index,
            })
        })
        .collect();
    let document = serde_json::json!({ "data": records });
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).expect("write");

    let store = PrivateStore::from_manifests(&path).expect("a hostile count is not fatal");

    assert!(store.get_private_manifests().is_empty());
}

#[test]
fn the_highest_possible_id_does_not_overflow_on_save() {
    let scratch = Scratch::new("max-id");
    std::fs::write(
        scratch.path(),
        br##"{"data":[{"name":"Big","scope":"https://big.test","icons":[],"start_url":"https://big.test/","category":"Miscellaneous","theme_color":"#000","id":18446744073709551615}]}"##,
    )
    .expect("seed");
    let mut store = PrivateStore::from_manifests(scratch.path()).expect("store");

    // Saturating, not wrapping: the counter is a `u64` where the TS original is a float that
    // simply keeps growing. No panic, no duplicated allocation of an id the store already has.
    let id = store
        .save_new_application(recipe("Next"))
        .expect("an exhausted id space still saves");
    assert_eq!(id, u64::MAX);
    assert_eq!(store.get_private_manifests().len(), 2);
    let reloaded = PrivateStore::from_manifests(scratch.path()).expect("reload");
    assert_eq!(reloaded.get_private_manifests().len(), 2);
}
