//! napi surface for the `window.bxApi.applications` / `bxApi.manifest`
//! perform channels (`docs/rust-napi-bridge.md` §applications + §manifest).
//!
//! Only the methods with EXISTS backends are exported (search, most popular,
//! categories, by-category, manifest-by-url, private apps, install,
//! uninstall, request-private). The `planned` backends (uninstallByManifest,
//! setConfigData, theme/identities watchers, notification passthrough) land
//! with their own slices.
//!
//! Envelope semantics copied from
//! `packages/app/src/services/services/sdkv2/worker.ts` `callAction`: a
//! truthy saga return resolves `{ body: ... }`, a falsy one (or a plain
//! action dispatch) resolves `undefined`. `install_application` therefore
//! returns `{ body: applicationId }` (the `addApplicationRequest` saga's
//! return value) while `uninstall_application` returns `null`.
//!
//! Service state is one process-global [`ApplicationService`] behind a
//! `Mutex`, initialized lazily from the embedded manifest definitions plus
//! the user config dir's `private-manifests.json` (same rooting as
//! `manifest_registry::PrivateStore::new`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use appstore_service::{ApplicationError, ApplicationService, InstallContext, PopularApps};
use manifest_registry::{Manifest, MinimalApplication, NewPrivateApplication};
use napi::{Error, Result, Status};
use napi_derive::napi;
use serde_json::{json, Map, Value};

/// Process-global service state, initialized on first use.
static SERVICE: Mutex<Option<ApplicationService>> = Mutex::new(None);

fn with_service<T>(f: impl FnOnce(&mut ApplicationService) -> Result<T>) -> Result<T> {
    let mut guard = SERVICE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        let service = ApplicationService::new().map_err(|e| {
            Error::new(
                Status::GenericFailure,
                format!("private manifest store init failed: {e}"),
            )
        })?;
        *guard = Some(service);
    }
    f(guard.as_mut().expect("just initialized"))
}

/// Swap the global service for one rooted at an explicit
/// `private-manifests.json` (tests; also usable from Node to point the
/// bridge at a scratch store).
#[doc(hidden)]
#[napi]
pub fn set_manifests_path_for_tests(path: String) -> Result<()> {
    set_service_from_path(path)
}

fn set_service_from_path(path: String) -> Result<()> {
    let service = ApplicationService::from_manifests_path(path).map_err(|e| {
        Error::new(
            Status::GenericFailure,
            format!("private manifest store init failed: {e}"),
        )
    })?;
    let mut guard = SERVICE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(service);
    Ok(())
}

/// `ApplicationError` -> napi error carrying the TS saga's message verbatim
/// (`throw new Error('Add Application : given manifest URL is blank')` etc).
fn app_err(e: ApplicationError) -> Error {
    Error::new(Status::GenericFailure, e.to_string())
}

/// `MinimalApplication` with the TS field spellings
/// (`packages/app/src/applications/graphql/withApplications.ts`).
fn minimal_to_json(m: &MinimalApplication) -> Value {
    json!({
        "id": m.id,
        "name": m.name,
        "bxAppManifestURL": m.bx_app_manifest_url,
        "iconURL": m.icon_url,
        "themeColor": m.theme_color,
        "isChromeExtension": m.is_chrome_extension,
        "recommendedPosition": m.recommended_position,
    })
}

fn minimal_vec_to_json(apps: &[MinimalApplication]) -> Value {
    Value::Array(apps.iter().map(minimal_to_json).collect())
}

/// `PopularApps` with the `packages/app/manifests/index.ts` key names.
fn popular_to_json(p: &PopularApps) -> Value {
    json!({
        "creamOfTheCropApps": minimal_vec_to_json(&p.cream_of_the_crop_apps),
        "runnerUps": minimal_vec_to_json(&p.runner_ups),
        "noteworthy": minimal_vec_to_json(&p.noteworthy),
    })
}

/// `getApplicationsByCategory`'s `Record<string, MinimalApplication[]>`.
fn by_category_to_json(grouped: &[(String, Vec<MinimalApplication>)]) -> Value {
    let mut map = Map::new();
    for (category, apps) in grouped {
        map.insert(category.clone(), minimal_vec_to_json(apps));
    }
    Value::Object(map)
}

/// The manifest JSON the TS sagas hand back (`getManifestByURL`,
/// `getPrivateApplications`): manifest fields flattened with `id` and `icon`.
fn manifest_to_json(m: &Manifest) -> Value {
    serde_json::to_value(m).unwrap_or(Value::Null)
}

/// `bxApi.applications.search(query)` -> `{ body: MinimalApplication[] }`.
#[napi]
pub fn search_applications(query: String) -> Result<Value> {
    with_service(|service| {
        let hits = appstore_service::search_applications(&query, &service.private_manifests());
        Ok(json!({ "body": minimal_vec_to_json(&hits) }))
    })
}

/// `bxApi.applications.getMostPopularApps()` ->
/// `{ body: { creamOfTheCropApps, runnerUps, noteworthy } }`.
#[napi]
pub fn get_most_popular_applications() -> Result<Value> {
    with_service(|service| {
        let popular = appstore_service::get_most_popular_apps(&service.private_manifests());
        Ok(json!({ "body": popular_to_json(&popular) }))
    })
}

/// `bxApi.applications.getAllCategories()` -> `{ body: string[] }`.
#[napi]
pub fn get_all_categories() -> Result<Value> {
    with_service(|service| {
        let categories = appstore_service::get_all_categories(&service.private_manifests());
        Ok(json!({ "body": categories }))
    })
}

/// `bxApi.applications.getApplicationsByCategory()` ->
/// `{ body: Record<string, MinimalApplication[]> }`.
#[napi]
pub fn get_applications_by_category() -> Result<Value> {
    with_service(|service| {
        let grouped = appstore_service::get_applications_by_category(&service.private_manifests());
        Ok(json!({ "body": by_category_to_json(&grouped) }))
    })
}

/// `bxApi.manifest.getManifest(manifestURL)` -> `{ body: BxAppManifest | null }`
/// (the TS `find` yields `undefined` for unknown URLs; `null` here).
#[napi]
pub fn get_manifest_by_url(manifest_url: String) -> Result<Value> {
    with_service(|service| {
        let manifest = service.get_manifest_by_url(&manifest_url);
        Ok(json!({ "body": manifest.as_ref().map_or(Value::Null, manifest_to_json) }))
    })
}

/// `bxApi.applications.getPrivateApps()` -> `{ body: AppManifest[] }` —
/// snake_case manifest fields, read by
/// `packages/appstore/src/HOC/withCustomApplications.tsx`.
#[napi]
pub fn get_private_applications() -> Result<Value> {
    with_service(|service| {
        let manifests = service.private_manifests();
        Ok(json!({
            "body": Value::Array(manifests.iter().map(manifest_to_json).collect())
        }))
    })
}

/// `bxApi.applications.install({ manifestURL, context, inBackground })` —
/// the `InstallApplication` channel routed through the
/// `addApplicationRequest` saga; resolves `{ body: applicationId }`.
///
/// Manifest resolution is local and instant (embedded registry), so the 2s
/// race budget is met with a zero elapsed fetch.
#[napi]
pub fn install_application(
    manifest_url: String,
    context: Option<Value>,
    in_background: Option<bool>,
) -> Result<Value> {
    let context = match &context {
        Some(value) => Some(
            serde_json::from_value::<InstallContext>(value.clone())
                .map_err(|e| Error::new(Status::InvalidArg, format!("invalid context: {e}")))?,
        ),
        None => None,
    };
    with_service(|service| {
        let payload = service
            .add_application_request(
                &manifest_url,
                context,
                in_background.unwrap_or(false),
                Duration::ZERO,
            )
            .map_err(app_err)?;
        Ok(json!({ "body": payload.application_id }))
    })
}

/// `bxApi.applications.uninstall(applicationId)` — the
/// `UninstallApplication` action dispatch; resolves `undefined` (worker
/// returns early for action channels), modeled as `null`.
#[napi]
pub fn uninstall_application(application_id: String) -> Result<Value> {
    with_service(|service| {
        service.uninstall(&application_id).map_err(app_err)?;
        Ok(Value::Null)
    })
}

/// `bxApi.applications.requestPrivate(recipe)` — the
/// `RequestPrivateApplication` channel; the recipe carries the bxApi field
/// names (`name`, `themeColor`, `bxIconURL`, `startURL`, `scope`, all
/// required by the preload). Resolves
/// `{ body: { id, bxAppManifestURL } }`.
#[napi]
pub fn request_private_application(recipe: Value) -> Result<Value> {
    let missing = |field: &str| Error::new(Status::InvalidArg, format!("{field} value is missing"));
    let obj = recipe.as_object().ok_or_else(|| missing("recipe"))?;
    let field = |name: &str| -> Result<String> {
        obj.get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| missing(name))
    };
    let new_private = NewPrivateApplication {
        name: field("name")?,
        theme_color: field("themeColor")?,
        icon_url: field("bxIconURL")?,
        start_url: field("startURL")?,
        scope: field("scope")?,
    };
    with_service(|service| {
        let created = service.request_private_application(new_private);
        Ok(json!({
            "body": {
                "id": created.id,
                "bxAppManifestURL": created.bx_app_manifest_url,
            }
        }))
    })
}

// Re-exported so downstream slices (the s14 TS glue generator, uninstall-by-
// manifest) can enumerate the service state without reaching into
// appstore-service types through the bridge.
pub use appstore_service::InstalledApplication;

/// Read-only snapshot of installed applications (napi-exposed shape for the
/// later `setConfigData` / `uninstallByManifest` slices; kept internal to
/// this slice's surface).
pub fn installed_snapshot() -> HashMap<String, InstalledApplication> {
    with_service(|service| Ok(service.installed_applications().clone())).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pure projection helpers: envelope shapes without touching the global
    /// service (the full-flow test lives in `tests/bridge.rs` so the two
    /// never race over `SERVICE`).
    #[test]
    fn minimal_projection_uses_ts_field_names() {
        let manifest = manifest_registry::get_application_by_id("14").unwrap();
        let minimal = manifest_registry::manifest_to_minimal_application(&manifest);
        let json = minimal_to_json(&minimal);
        assert_eq!(json["id"], "14");
        assert_eq!(json["name"], "Gmail");
        assert_eq!(json["bxAppManifestURL"], "station-manifest://14");
        assert_eq!(json["themeColor"], "#e75a4d");
        // Exact key set of the TS MinimalApplication type.
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "bxAppManifestURL",
                "iconURL",
                "id",
                "isChromeExtension",
                "name",
                "recommendedPosition",
                "themeColor"
            ]
        );
    }

    #[test]
    fn by_category_projection_is_an_object_map() {
        let grouped = appstore_service::get_applications_by_category(&[]);
        let json = by_category_to_json(&grouped);
        let map = json.as_object().unwrap();
        let comm = &map["Communication & Collaboration"];
        assert!(comm.as_array().unwrap().iter().any(|a| a["id"] == "14"));
    }

    #[test]
    fn manifest_projection_keeps_snake_case_manifest_fields() {
        let manifest = manifest_registry::get_application_by_id("14").unwrap();
        let json = manifest_to_json(&manifest);
        assert_eq!(json["id"], "14");
        assert_eq!(json["name"], "Gmail");
        assert!(json["start_url"]
            .as_str()
            .unwrap()
            .starts_with("https://accounts.google.com/AddSession"));
        assert_eq!(json["theme_color"], "#e75a4d");
        assert_eq!(json["bx_app_manifest_url"], Value::Null);
    }
}
