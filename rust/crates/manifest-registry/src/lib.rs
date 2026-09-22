//! Station manifest registry: typed [`BxAppManifest`] model (port of
//! `packages/app/src/applications/manifest-provider/bxAppManifest.d.ts`),
//! the bundled definitions from `packages/app/manifests/definitions`
//! (TS loads them via webpack `require.context`; here they are embedded at
//! compile time with `include_dir!`), and the
//! `manifestToMinimalApplication` projection from `packages/app/manifests/index.ts`.
//!
//! Not ported in this slice: icons (TS injects them as webpack url-loader
//! data URLs; no icon embedding here), the private runtime manifests from
//! `private.ts`, and Fuse-based fuzzy search.

use include_dir::{include_dir, Dir};
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
    #[serde(rename = "doNotList", default, with = "lenient_bool", skip_serializing_if = "Option::is_none")]
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
            Some(V::S(s)) => s.trim().parse::<f64>().map(Some).map_err(serde::de::Error::custom),
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

/// The bundled application definitions (`packages/app/manifests/definitions`).
static DEFINITIONS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../../packages/app/manifests/definitions");

/// A manifest with its registry id and resolved icon URL attached
/// (TS `Manifest = Omit<BxAppManifest, 'icons'> & { id, icon }`).
///
/// The icon stays an empty string in this slice: TS fills it with webpack
/// url-loader data URLs, which have no Rust-side equivalent yet.
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
    Some(Manifest { inner, id: id.to_owned(), icon: String::new() })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One definition file on disk == one parseable embedded manifest.
    #[test]
    fn embedded_definitions_match_files_on_disk() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../packages/app/manifests/definitions");
        let on_disk = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .count();

        let ids = get_all_application_ids();
        assert_eq!(ids.len(), on_disk, "embedded count != JSON file count");

        let mut parsed = 0;
        for id in &ids {
            let m = get_application_by_id(id).unwrap_or_else(|| panic!("failed to parse {id}.json"));
            assert_eq!(&m.id, id);
            parsed += 1;
        }
        assert_eq!(parsed, on_disk);
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
