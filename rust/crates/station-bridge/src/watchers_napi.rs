//! napi exports over [`crate::watchers::WatchHub`] for the three bxApi
//! selector channels (bridge doc §theme / §identities / snooze row):
//!
//! - `watchThemeColors(cb) -> id`
//! - `watchIdentities(cb) -> id` / `unwatchIdentities(id)`
//! - `watchSnoozeDuration(cb) -> id`
//!
//! Each `watch_*` returns a subscription id; `unwatch_identities` is the
//! explicit teardown the bridge doc mandates (deliberate divergence from the
//! TS `ipcRenderer.off`-only leak). `emit_*` fns are called by the TS glue
//! (worker.ts store taps) whenever the redux selectors produce a new value;
//! station-shell taking over the store later means only the caller of
//! `emit_*` changes.
//!
//! Callback identity is not used for matching — the glue registers once per
//! renderer and keeps the returned id, so id-based teardown cannot confuse
//! two renderers passing lookalike closures.

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use serde_json::Value;

use crate::watchers::WatchHub;

/// One hub per selector channel, process-global like `SERVICE`.
struct Hubs {
    theme_colors: WatchHub,
    identities: WatchHub,
    snooze_duration: WatchHub,
}

static HUBS: std::sync::Mutex<Hubs> = std::sync::Mutex::new(Hubs {
    theme_colors: WatchHub::new(),
    identities: WatchHub::new(),
    snooze_duration: WatchHub::new(),
});

fn with_hubs<T>(f: impl FnOnce(&mut Hubs) -> T) -> T {
    let mut hubs = HUBS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut hubs)
}

/// `CalleeHandled = false` (builder default) — the callback receives the
/// bare value, matching `obs.on(value)` in worker.ts. `Return = ()` since
/// sinks ignore JS return values.
type JsonTsfn = ThreadsafeFunction<Value, (), Value, Status, false>;

/// Wrap a tsfn into the hub's sink type. Builder default `CalleeHandled =
/// false` calls JS with the bare value (matching `obs.on(value)` in
/// worker.ts) — and that variant's `call` takes `T` directly, no `Result`.
fn tsfn_sink(tsfn: JsonTsfn) -> Box<dyn Fn(&Value) + Send + Sync + 'static> {
    Box::new(move |value: &Value| {
        tsfn.call(value.clone(), ThreadsafeFunctionCallMode::NonBlocking);
    })
}

macro_rules! watch_fns {
    ($watch:ident, $unwatch:ident, $emit:ident, $js_watch:literal, $js_unwatch:literal, $js_emit:literal, $hub:ident, $doc:literal) => {
        #[doc = concat!("`", $doc, "` — register a sink; the current value (if any) is delivered immediately. Returns a subscription id for the matching unwatch call.")]
        #[napi(js_name = $js_watch)]
        pub fn $watch(cb: Function<Value, ()>) -> Result<u32> {
            let tsfn: JsonTsfn = cb.build_threadsafe_function::<Value>().build()?;
            Ok(with_hubs(|h| h.$hub.subscribe(tsfn_sink(tsfn))) as u32)
        }

        #[doc = concat!("Explicit teardown for the subscription `id` returned by `", $js_watch, "`. Returns `true` when a live subscription was removed.")]
        #[napi(js_name = $js_unwatch)]
        pub fn $unwatch(id: u32) -> bool {
            with_hubs(|h| h.$hub.unsubscribe(u64::from(id)))
        }

        #[doc = "Internal: push a new value (already the selector's projection) to every subscriber; becomes a no-op for the glue once station-shell owns the state."]
        #[doc(hidden)]
        #[napi(js_name = $js_emit)]
        pub fn $emit(value: Value) {
            with_hubs(|h| h.$hub.emit(value));
        }
    };
}

// The JS API exposes remove* only for identities (bridge doc table), but the
// glue tears every channel down by id on `wc-destroyed-<senderId>`, so all
// three unwatch exports are live — the explicit teardown the bridge doc
// mandates over the renderer-local ipcRenderer.off leak.
watch_fns!(
    watch_theme_colors,
    unwatch_theme_colors,
    emit_theme_colors,
    "watchThemeColors",
    "unwatchThemeColors",
    "emitThemeColors",
    theme_colors,
    "bxApi.theme.addThemeColorsChangeListener"
);
watch_fns!(
    watch_identities,
    unwatch_identities,
    emit_identities,
    "watchIdentities",
    "unwatchIdentities",
    "emitIdentities",
    identities,
    "bxApi.identities.addIdentitiesChangeListener"
);
watch_fns!(
    watch_snooze_duration,
    unwatch_snooze_duration,
    emit_snooze_duration,
    "watchSnoozeDuration",
    "unwatchSnoozeDuration",
    "emitSnoozeDuration",
    snooze_duration,
    "bxApi.notificationCenter.addSnoozeDurationInMsChangeListener"
);
