//! Station manifest registry: typed [`BxAppManifest`] model (port of
//! `packages/app/src/applications/manifest-provider/bxAppManifest.d.ts`),
//! the bundled definitions, copied from `packages/app/manifests/definitions`
//! into `rust/assets/definitions` (TS loads the originals via webpack
//! `require.context`; here the copy is embedded at compile time with
//! `include_dir!`, so the Rust build does not read inside `packages/`), and the
//! `manifestToMinimalApplication` projection from `packages/app/manifests/index.ts`.
//!
//! Icons: the TS build injects each bundled icon as a webpack url-loader *data* URL
//! (`require.context('!url-loader!./icons')` over `packages/app/manifests/icons`). There is no
//! webpack here, so a bundled `Manifest.icon` carries the manifest's own remote icon URL
//! (`icons[0].src`, a `https://cdn.filestackcontent.com/…` address) and the shell fetches and
//! caches it (`station-shell::icons`); private manifests already carry theirs the same way. An
//! app with no icon keeps an empty `icon` and draws its letter avatar.

use std::path::{Path, PathBuf};

pub mod config_rules;

pub use config_rules::{
    application_label, get_presets, is_configuration_required, is_multi_instance_configurator,
    ApplicationConfigState, ConfigRulesError,
};

use include_dir::{include_dir, Dir};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use serde::{Deserialize, Serialize};

/// JSON schema for a BxApp manifest file.
///
/// Copy/pasted from `getstation/api`; every field is optional and
/// `recommendedPosition` / `doNotList` are tolerated as strings because
/// several bundled definitions ship them that way.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BxAppManifest {
    pub name: Option<String>,
    pub category: Option<String>,
    pub start_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub icons: Vec<ImageResource>,
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extended_scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub import: Vec<ExternalApplicationResource>,
    pub theme_color: Option<String>,
    pub bx_override_user_agent: Option<String>,
    pub bx_keep_always_loaded: Option<bool>,
    pub bx_not_use_native_window_open_on_host: Option<bool>,
    pub bx_no_dock: Option<bool>,
    pub bx_use_default_session: Option<bool>,
    pub bx_single_page: Option<bool>,
    pub bx_multi_instance_config: Option<MultiInstanceConfig>,
    pub bx_legacy_service_id: Option<String>,
    pub main: Option<String>,
    pub renderer: Option<String>,
    /// In BrowserX, the URL to be used when adding a new page.
    pub bx_new_page_url: Option<String>,
    #[serde(
        rename = "recommendedPosition",
        default,
        with = "lenient_number_or_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub recommended_position: Option<f64>,
    #[serde(
        rename = "doNotList",
        default,
        with = "lenient_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub do_not_list: Option<bool>,
}

impl BxAppManifest {
    /// `recommendedPosition` in TS: numeric when present, else `0`
    /// (`manifest.recommendedPosition ? Number(...) : 0`).
    pub fn recommended_position(&self) -> f64 {
        self.recommended_position.unwrap_or(0.0)
    }
}

/// `true` / `false` / `"true"` / `"false"` / absent.
mod lenient_bool {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum V {
            B(bool),
            S(String),
        }
        match Option::<V>::deserialize(d)? {
            None => Ok(None),
            Some(V::B(b)) => Ok(Some(b)),
            Some(V::S(s)) if s.eq_ignore_ascii_case("true") => Ok(Some(true)),
            Some(V::S(s)) if s.eq_ignore_ascii_case("false") => Ok(Some(false)),
            Some(V::S(s)) => Err(serde::de::Error::custom(format!("invalid boolean: {s}"))),
        }
    }

    pub fn serialize<S: Serializer>(v: &Option<bool>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(b) => s.serialize_bool(*b),
        }
    }
}

/// Number or numeric string, for `recommendedPosition`.
mod lenient_number_or_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum V {
            N(f64),
            S(String),
        }
        match Option::<V>::deserialize(d)? {
            None => Ok(None),
            Some(V::N(n)) => Ok(Some(n)),
            Some(V::S(s)) => s
                .trim()
                .parse::<f64>()
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }

    pub fn serialize<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(n) => s.serialize_f64(*n),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageResource {
    pub src: String,
    pub sizes: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub purpose: Option<String>,
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExternalApplicationResource {
    pub platform: String,
    pub id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MultiInstanceConfig {
    pub preset: Option<Preset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<Preset>,
    pub instance_wording: Option<String>,
    pub instance_label_tpl: Option<String>,
    pub start_url_tpl: Option<String>,
    /// In BrowserX, the URL to be used when adding a new page, templated
    /// with multi-instance configuration variables.
    pub new_page_url_tpl: Option<String>,
    pub subdomain_title: Option<String>,
    pub subdomain_ui_help: Option<String>,
    pub subdomain_ui_suffix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    #[serde(rename = "subdomain")]
    Subdomain,
    #[serde(rename = "google-account")]
    GoogleAccount,
    #[serde(rename = "on-premise")]
    OnPremise,
}

/// The bundled application definitions, copied byte-for-byte from
/// `packages/app/manifests/definitions` into `rust/assets/definitions` so the Rust build no longer
/// reads inside `packages/`. The original stays until the TS tree is deleted; the drift test
/// below fails if the two diverge.
static DEFINITIONS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/definitions");

/// A manifest with its registry id and resolved icon URL attached
/// (TS `Manifest = Omit<BxAppManifest, 'icons'> & { id, icon }`).
///
/// TS's `icon` is a webpack url-loader *data* URL for a bundled icon file; the port keeps the
/// manifest's own remote icon URL (`icons[0].src`) instead and lets the shell fetch it. Empty
/// when the manifest lists no icon (the shell then draws the letter avatar).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(flatten)]
    pub inner: BxAppManifest,
    pub id: String,
    pub icon: String,
}

/// Port of `MinimalApplication` from
/// `packages/app/src/applications/graphql/withApplications.ts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MinimalApplication {
    pub id: String,
    pub name: String,
    pub bx_app_manifest_url: String,
    pub icon_url: String,
    pub theme_color: String,
    pub is_chrome_extension: bool,
    pub recommended_position: f64,
}

/// Port of `getBxAppManifestURL`.
pub fn get_bx_app_manifest_url(id: &str) -> String {
    format!("station-manifest://{id}")
}

/// Port of `getChromeExtensionId`: the id of the first `import` entry
/// whose platform is `chrome`, if any.
pub fn get_chrome_extension_id(manifest: &BxAppManifest) -> Option<&str> {
    manifest
        .import
        .iter()
        .find(|i| i.platform == "chrome")
        .and_then(|i| i.id.as_deref())
}

/// All bundled definition ids (basename of each JSON file, no extension),
/// sorted for deterministic iteration — TS order comes from
/// `require.context().keys()` and is unspecified here.
pub fn get_all_application_ids() -> Vec<String> {
    let mut ids: Vec<String> = DEFINITIONS
        .files()
        .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
        .map(|f| {
            f.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    ids.sort();
    ids
}

/// Parse one embedded definition by id; `None` when the id is unknown or
/// the JSON does not fit the schema.
pub fn get_application_by_id(id: &str) -> Option<Manifest> {
    let file = DEFINITIONS.get_file(format!("{id}.json"))?;
    let inner: BxAppManifest = serde_json::from_str(file.contents_utf8()?).ok()?;
    let icon = inner
        .icons
        .first()
        .map(|resource| resource.src.clone())
        .unwrap_or_default();
    Some(Manifest {
        inner,
        id: id.to_owned(),
        icon,
    })
}

/// Port of `manifestToMinimalApplication`.
pub fn manifest_to_minimal_application(manifest: &Manifest) -> MinimalApplication {
    MinimalApplication {
        id: manifest.id.clone(),
        name: manifest.inner.name.clone().unwrap_or_default(),
        bx_app_manifest_url: get_bx_app_manifest_url(&manifest.id),
        icon_url: manifest.icon.clone(),
        theme_color: manifest.inner.theme_color.clone().unwrap_or_default(),
        is_chrome_extension: get_chrome_extension_id(&manifest.inner).is_some(),
        recommended_position: manifest.inner.recommended_position(),
    }
}

/// Port of `listAllApplications`: every bundled manifest plus private ones,
/// minus `doNotList`.
///
/// Private manifests come last (TS appends them after the bundled ids).
pub fn list_all_applications(private: &[Manifest]) -> Vec<Manifest> {
    // The bundled definitions are static; parse them once instead of once
    // per search call (the TS module parses them a single time at import).
    let mut out: Vec<Manifest> = bundled_applications().clone();
    out.reserve(private.len());
    for m in private {
        if m.inner.do_not_list != Some(true) {
            out.push(m.clone());
        }
    }
    out
}

/// Bundled, non-`doNotList` manifests, parsed once per process.
fn bundled_applications() -> &'static Vec<Manifest> {
    static BUNDLED: std::sync::LazyLock<Vec<Manifest>> = std::sync::LazyLock::new(|| {
        get_all_application_ids()
            .into_iter()
            .filter_map(|id| get_application_by_id(&id))
            .filter(|m| m.inner.do_not_list != Some(true))
            .collect()
    });
    &BUNDLED
}

/// How well `haystack` matches the lowercased `query` word-by-word.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
enum WordHit {
    /// No word-level match.
    None,
    /// Every query word matched as a substring of some haystack word
    /// (Fuse finds "calendar" inside "Google Calendar").
    Substring,
    /// Every query word is a prefix of some haystack word ("git" →
    /// "GitHub", "code" → "Codecov").
    Prefix,
}

fn word_hit(haystack_lower: &str, words: &[&str]) -> WordHit {
    let mut best = WordHit::Prefix;
    // Split once: re-scanning the haystack per query word is quadratic on
    // word-heavy queries.
    let haystack_words: Vec<&str> = haystack_lower
        .split(|c: char| !c.is_alphanumeric())
        .collect();
    for q in words {
        let mut word_best = WordHit::None;
        for w in &haystack_words {
            if w.starts_with(q) {
                word_best = WordHit::Prefix;
            } else if w.contains(q) {
                word_best = word_best.max(WordHit::Substring);
            }
        }
        best = best.min(word_best);
    }
    best
}

/// Default score for fuzzy-only matches, below any real nucleo score.
const NO_WORD_HIT_SCORE: i64 = -1;

/// Search all application manifests by name (port of `manifests.search`).
///
/// TS runs Fuse.js with `threshold: 0.4, keys: ['name']` over
/// `MinimalApplication`s. The port keeps the semantics Fuse gives there —
/// case-insensitive, subsequence matching on the whole name, word-boundary
/// matches ranked ahead, one-typo queries still hitting — with nucleo
/// scoring instead of Fuse's bitap distance. Ranking rule:
/// 1. query-word prefix match ("git" → "GitHub"),
/// 2. query-word substring match ("calendar" → "Google Calendar"),
/// 3. nucleo fuzzy score (descending),
///    then name, then id, for determinism.
pub fn search(query: &str, private: &[Manifest]) -> Vec<MinimalApplication> {
    let apps = list_all_applications(private);
    // Empty pattern: Fuse.search('') returns no results; match that.
    if query.trim().is_empty() {
        return Vec::new();
    }

    let query_lower = query.trim().to_lowercase();
    let words: Vec<&str> = query_lower.split_whitespace().collect();

    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut hits: Vec<(WordHit, i64, MinimalApplication)> = Vec::new();
    let mut buf = Vec::new();
    for app in &apps {
        let name = app.inner.name.clone().unwrap_or_default();
        // Exact match (whole name equals the query, like Fuse's
        // `exactMatch` bonus): always wins.
        if name.to_lowercase() == query_lower {
            return vec![manifest_to_minimal_application(app)];
        }

        let hit = word_hit(&name.to_lowercase(), &words);

        if let WordHit::None = hit {
            // Allow one typo per query word (Fuse bitap distance <= 1 at
            // threshold 0.4 finds "gmial" → "Gmail"): retry each word
            // against each name word with edit distance 1.
            if !words.iter().all(|q| {
                name.to_lowercase()
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|w| within_one_edit(q, w))
            }) {
                continue;
            }
        }

        buf.clear();
        let score = Pattern::new(
            &query_lower,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        )
        .score(nucleo_matcher::Utf32Str::new(&name, &mut buf), &mut matcher)
        .map(|s| s as i64)
        .unwrap_or(NO_WORD_HIT_SCORE);
        hits.push((hit, score, manifest_to_minimal_application(app)));
    }
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then_with(|| a.2.name.cmp(&b.2.name))
            .then_with(|| a.2.id.cmp(&b.2.id))
    });
    hits.into_iter().map(|(_, _, m)| m).collect()
}

/// True when `a` and `b` are within one character edit (insert, delete,
/// substitute, or transpose of adjacent chars — Fuse's bitap covers
/// transpositions as two edits but still ranks them at threshold 0.4 for
/// short queries like "gmial" → "gmail") of each other.
fn within_one_edit(a: &str, b: &str) -> bool {
    // Char counts must be within one; counting without allocating keeps
    // huge query words from allocating per name word.
    let (count_a, count_b) = (a.chars().count(), b.chars().count());
    if count_a.abs_diff(count_b) > 1 {
        return false;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    match a.len().cmp(&b.len()) {
        std::cmp::Ordering::Equal => {
            let diffs = a.iter().zip(&b).filter(|(x, y)| x != y).count();
            diffs <= 1 || (diffs == 2 && one_transposition(&a, &b))
        }
        std::cmp::Ordering::Less => one_insertion(&a, &b),
        std::cmp::Ordering::Greater => one_insertion(&b, &a),
    }
}

/// Equal-length strings differing only by one adjacent swap?
fn one_transposition(a: &[char], b: &[char]) -> bool {
    let first = a.iter().zip(b).position(|(x, y)| x != y);
    let Some(i) = first else { return true };
    i + 1 < a.len() && a[i] == b[i + 1] && a[i + 1] == b[i] && a[i + 2..] == b[i + 2..]
}

/// `short` plus exactly one inserted character equals `long`?
fn one_insertion(short: &[char], long: &[char]) -> bool {
    if long.len() != short.len() + 1 {
        return false;
    }
    // Walk both; at most one char of `long` may be skipped.
    let (mut i, mut skipped) = (0, false);
    for c in long {
        if i < short.len() && short[i] == *c {
            i += 1;
        } else if !skipped {
            skipped = true;
        } else {
            return false;
        }
    }
    true
}

/// Search with a private-manifest store (port of the TS pairing of
/// `manifests.search` with `getPrivateManifests()`).
pub fn search_with_private(query: &str, store: &PrivateStore) -> Vec<MinimalApplication> {
    search(query, &store.get_private_manifests())
}

/// Payload for [`PrivateStore::save_new_application`]
/// (TS `PrivateApplicationRequest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewPrivateApplication {
    pub name: String,
    pub theme_color: String,
    pub icon_url: String,
    pub start_url: String,
    pub scope: String,
}

/// On-disk record (TS `BxAppManifestWithId` subset that `private.ts`
/// writes): id plus the manifest fields `saveNewApplication` fills.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PrivateManifestRecord {
    name: String,
    scope: String,
    icons: Vec<ImageResource>,
    start_url: String,
    category: String,
    theme_color: String,
    id: u64,
}

/// First id handed to a private manifest (TS `highestId = 1000000`).
const PRIVATE_ID_BASE: u64 = 1_000_000;

/// User-added "private" application manifests persisted as JSON in the
/// user config dir (port of `manifests/private.ts`).
///
/// The largest `private-manifests.json` this crate reads (8 MiB). Past it the file is treated
/// like a corrupt one: warned about and ignored, so a hostile file cannot cost memory.
pub const MAX_PRIVATE_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

/// The most records a private-manifest store holds (read or written).
pub const MAX_PRIVATE_MANIFESTS: usize = 10_000;

/// The TS module hardwires the path to Electron's `userData` dir and a
/// 1-second memoization cache; the port takes the directory from the
/// caller ([`PrivateStore::new`] uses `dirs::config_dir`) and always
/// reads from memory, which the 1s cache only ever approximated.
pub struct PrivateStore {
    /// `None` for an in-memory store: nothing is written, so a machine with no writable config
    /// dir can still run the shell (its custom apps live for the session only).
    path: Option<PathBuf>,
    data: Vec<PrivateManifestRecord>,
    highest_id: u64,
}

impl PrivateStore {
    /// Store rooted at `<config_dir>/private-manifests.json`, creating a
    /// new empty file if none exists.
    pub fn new() -> std::io::Result<Self> {
        let dir = dirs_config_dir();
        std::fs::create_dir_all(&dir)?;
        Self::from_manifests(dir.join("private-manifests.json"))
    }

    /// Store at an explicit `path`, loading existing records from disk
    /// when the file exists (corrupt JSON warns and starts empty, like
    /// the TS try/catch).
    pub fn from_manifests(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut data: Vec<PrivateManifestRecord> = Vec::new();
        let mut highest_id = PRIVATE_ID_BASE;
        if path.exists() {
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.len() > MAX_PRIVATE_MANIFEST_BYTES => {
                    eprintln!(
                        "private-manifests.json is {} bytes; starting empty (at most {} is read)",
                        metadata.len(),
                        MAX_PRIVATE_MANIFEST_BYTES
                    );
                }
                Ok(_) => match std::fs::read_to_string(&path) {
                    Ok(text) => match serde_json::from_str::<PrivateFile>(&text) {
                        Ok(file) if file.data.len() > MAX_PRIVATE_MANIFESTS => {
                            eprintln!(
                                "private-manifests.json has {} records; starting empty (at most {} is kept)",
                                file.data.len(),
                                MAX_PRIVATE_MANIFESTS
                            );
                        }
                        Ok(file) => {
                            for r in file.data {
                                highest_id = highest_id.max(r.id);
                                data.push(r);
                            }
                        }
                        Err(e) => {
                            // TS console.warn's and carries on with [].
                            eprintln!("private-manifests.json unreadable: {e}");
                        }
                    },
                    Err(e) => eprintln!("private-manifests.json unreadable: {e}"),
                },
                Err(e) => eprintln!("private-manifests.json unreadable: {e}"),
            }
        }
        Ok(Self {
            path: Some(path),
            data,
            highest_id,
        })
    }

    /// A store that never touches the disk: reads nothing, and writes are accepted and forgotten.
    /// The shell falls back to it when the user's config dir cannot be opened.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            data: Vec::new(),
            highest_id: PRIVATE_ID_BASE,
        }
    }

    /// Port of `saveNewApplication`: assigns the next id, persists, and returns the new id.
    ///
    /// A write failure is returned instead of panicking, and the in-memory list is rolled back:
    /// what the caller sees stays what the disk holds.
    pub fn save_new_application(&mut self, payload: NewPrivateApplication) -> std::io::Result<u64> {
        // TS numbers are floats and cannot overflow; saturate so a hostile
        // `u64::MAX` id loaded from the file cannot panic on `+ 1`.
        let id = self.highest_id.saturating_add(1);
        let manifest = PrivateManifestRecord {
            name: payload.name,
            scope: payload.scope,
            icons: vec![ImageResource {
                src: payload.icon_url,
                platform: Some("browserx".to_owned()),
                ..ImageResource::default()
            }],
            start_url: payload.start_url,
            category: "Miscellaneous".to_owned(),
            theme_color: payload.theme_color,
            id,
        };
        self.data.push(manifest);
        if let Err(error) = self.persist() {
            self.data.pop();
            return Err(error);
        }
        self.highest_id = id;
        Ok(id)
    }

    /// Port of `deleteManifest` (unknown id is a no-op, like TS). A write failure is returned and
    /// the record is put back.
    pub fn delete_manifest(&mut self, id: u64) -> std::io::Result<()> {
        let Some(index) = self.data.iter().position(|record| record.id == id) else {
            return Ok(());
        };
        let removed = self.data.remove(index);
        if let Err(error) = self.persist() {
            self.data.insert(index, removed);
            return Err(error);
        }
        Ok(())
    }

    /// Port of `getPrivateManifests` (TS `cleanIcon`: first icon src as
    /// `icon`, id as string).
    pub fn get_private_manifests(&self) -> Vec<Manifest> {
        self.data
            .iter()
            .map(|r| Manifest {
                inner: BxAppManifest {
                    name: Some(r.name.clone()),
                    scope: Some(r.scope.clone()),
                    icons: r.icons.clone(),
                    start_url: Some(r.start_url.clone()),
                    category: Some(r.category.clone()),
                    theme_color: Some(r.theme_color.clone()),
                    ..BxAppManifest::default()
                },
                id: r.id.to_string(),
                icon: r.icons.first().map(|i| i.src.clone()).unwrap_or_default(),
            })
            .collect()
    }

    /// Port of `getPrivateApplicationById`.
    pub fn get_private_application_by_id(&self, id: u64) -> Option<Manifest> {
        self.get_private_manifests()
            .into_iter()
            .find(|m| m.id == id.to_string())
    }

    /// Writes through a sibling temp file, fsyncs it, then renames it over the store: a failed or
    /// killed write leaves the previous file in place, and a failed one removes its temp file.
    /// (The same pattern `session-store` uses; one small helper each keeps the crates independent.)
    fn persist(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            // In memory: accepted and forgotten.
            return Ok(());
        };
        let json = serde_json::to_string(&PrivateFileRef { data: &self.data })?;
        write_atomically(path, json.as_bytes())
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "private manifest path has no file name",
        )
    })?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temp = directory.join(format!(
        ".{}.{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));

    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// The `{ "data": [...] }` envelope `private.ts` writes.
#[derive(Serialize)]
struct PrivateFileRef<'a> {
    data: &'a [PrivateManifestRecord],
}

#[derive(Deserialize)]
struct PrivateFile {
    data: Vec<PrivateManifestRecord>,
}

/// `dirs::config_dir()` without adding a dependency for one call.
fn dirs_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("APPDATA") {
        return PathBuf::from(dir).join("BrowserX");
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("browserx");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join("browserx");
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded definitions are the copied asset input: 638 JSON files, every one parseable,
    /// byte-identical to the embedded bytes. That makes the copy the build input (not a name
    /// coincidence) and fails if either side drifts.
    #[test]
    fn embedded_definitions_match_files_on_disk() {
        const EXPECTED_DEFINITIONS: usize = 638;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/definitions");
        let mut on_disk = 0;
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|why| panic!("{} unreadable: {why}", dir.display()))
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.extension().is_some_and(|x| x == "json") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|why| panic!("{} unreadable: {why}", path.display()));
            serde_json::from_str::<serde_json::Value>(&text)
                .unwrap_or_else(|why| panic!("{} is not JSON: {why}", path.display()));
            on_disk += 1;
        }
        assert_eq!(
            on_disk,
            EXPECTED_DEFINITIONS,
            "copied definitions count drifted: {}",
            dir.display()
        );

        let ids = get_all_application_ids();
        assert_eq!(ids.len(), on_disk, "embedded count != JSON file count");

        for id in &ids {
            let m =
                get_application_by_id(id).unwrap_or_else(|| panic!("failed to parse {id}.json"));
            assert_eq!(&m.id, id);
            let name = format!("{id}.json");
            let embedded = DEFINITIONS.get_file(name.as_str()).expect("embedded file");
            let disk = std::fs::read(dir.join(&name)).expect("copied file");
            assert_eq!(
                embedded.contents(),
                disk.as_slice(),
                "{name} differs between the embedded copy and rust/assets/definitions"
            );
        }
    }

    /// Every manifest survives a serde JSON round-trip unchanged.
    #[test]
    fn manifests_round_trip() {
        for id in get_all_application_ids() {
            let m = get_application_by_id(&id).expect("parse");
            let json = serde_json::to_string(&m).unwrap();
            let back: Manifest = serde_json::from_str(&json).unwrap();
            assert_eq!(m, back, "round-trip mismatch for {id}");
        }
    }

    #[test]
    fn minimal_application_projection() {
        let atlassian = get_application_by_id("100").unwrap();
        let minimal = manifest_to_minimal_application(&atlassian);
        assert_eq!(minimal.id, "100");
        assert_eq!(minimal.name, "Atlassian (Jira, Confluence..)");
        assert_eq!(minimal.bx_app_manifest_url, "station-manifest://100");
        assert_eq!(minimal.theme_color, "#264970");
        assert_eq!(minimal.recommended_position, 23.0);
        assert!(!minimal.is_chrome_extension);

        let mixmax = get_application_by_id("267").unwrap();
        let minimal = manifest_to_minimal_application(&mixmax);
        assert!(minimal.is_chrome_extension);
        assert_eq!(
            get_chrome_extension_id(&mixmax.inner),
            Some("ocpljaamllnldhepankaeljmeeeghnid")
        );
    }

    /// A bundled manifest's icon is its first icon resource's URL (the TS build embeds the
    /// same source as a data URL; here it stays remote for the shell to fetch and cache).
    #[test]
    fn bundled_manifests_carry_their_icon_url() {
        let gmail = get_application_by_id("14").unwrap();
        assert_eq!(
            gmail.icon,
            "https://cdn.filestackcontent.com/Yxw83DJdRW2N2PkcDFGA"
        );
        let minimal = manifest_to_minimal_application(&gmail);
        assert_eq!(minimal.icon_url, gmail.icon);
    }

    /// A manifest without an icon list keeps an empty icon: the shell's letter avatar is the
    /// fallback, not an error.
    #[test]
    fn a_manifest_without_icons_has_an_empty_icon() {
        let m = get_application_by_id("1").unwrap(); // the App Store itself
        assert!(m.icon.is_empty(), "1.json declares no icons");
        assert!(manifest_to_minimal_application(&m).icon_url.is_empty());
    }

    #[test]
    fn lenient_fields() {
        // recommendedPosition ships as a string in the bundled JSONs.
        let m = get_application_by_id("100").unwrap();
        assert_eq!(m.inner.recommended_position(), 23.0);
        // doNotList ships as a boolean; absent means None.
        let appstore = get_application_by_id("1").unwrap();
        assert_eq!(appstore.inner.do_not_list, Some(true));
        let none = serde_json::from_str::<BxAppManifest>(r#"{"name":"x"}"#).unwrap();
        assert_eq!(none.recommended_position(), 0.0);
        assert_eq!(none.do_not_list, None);
    }
}
