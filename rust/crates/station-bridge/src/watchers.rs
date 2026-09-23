//! Selector-watch state for the `bxApi` subscribe channels
//! (`docs/rust-napi-bridge.md` §theme + §identities + the snooze row).
//!
//! [`WatchHub`] is the process-global fan-out core behind each napi
//! `watch_*` export: it stores the current value (pushed by the TS glue's
//! store taps until station-shell owns the state), hands it to every new
//! subscriber immediately (matching `subscribeStore`'s "emit the first value
//! immediately" in `packages/app/src/utils/observable.ts`), and fans each
//! `emit_*` out to all subscribers. Unsubscribe is explicit by id — the
//! deliberate divergence from the renderer-local `ipcRenderer.off` leak
//! documented in the bridge doc §Transport today.
//!
//! Kept free of napi types so `cargo test` (no JS runtime) can exercise the
//! register/emit/teardown lifecycle directly.

use serde_json::Value;

type Sink = Box<dyn Fn(&Value) + Send + Sync + 'static>;

/// Fan-out registry for one selector channel.
pub(crate) struct WatchHub {
    current: Option<Value>,
    next_id: u64,
    subs: Vec<(u64, Sink)>,
}

impl Default for WatchHub {
    fn default() -> Self {
        Self::new()
    }
}

impl WatchHub {
    pub const fn new() -> Self {
        Self {
            current: None,
            next_id: 0,
            subs: Vec::new(),
        }
    }

    /// Register a sink; if a value was already emitted it is delivered to
    /// this sink immediately. Returns the subscription id used by
    /// [`WatchHub::unsubscribe`].
    pub fn subscribe(&mut self, cb: Sink) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        if let Some(current) = &self.current {
            cb(current);
        }
        self.subs.push((id, cb));
        id
    }

    /// Remove a subscription. Returns `true` when it existed.
    pub fn unsubscribe(&mut self, id: u64) -> bool {
        let before = self.subs.len();
        self.subs.retain(|(sid, _)| *sid != id);
        before != self.subs.len()
    }

    /// Store `value` as the new current value and deliver it to every
    /// subscriber. ponytail: no reentrancy guard — napi sinks only queue a
    /// tsfn call; revisit if a Rust-owned sink ever calls back into the hub.
    pub fn emit(&mut self, value: Value) {
        self.current = Some(value.clone());
        for (_, cb) in &self.subs {
            cb(&value);
        }
    }

    #[cfg(test)]
    pub fn subscriber_count(&self) -> usize {
        self.subs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Log = Arc<Mutex<Vec<Value>>>;

    fn sink(log: &Log) -> Box<dyn Fn(&Value) + Send + Sync + 'static> {
        let log = Arc::clone(log);
        Box::new(move |v: &Value| log.lock().unwrap().push(v.clone()))
    }

    fn take(log: &Log) -> Vec<Value> {
        std::mem::take(&mut *log.lock().unwrap())
    }

    #[test]
    fn subscribe_before_any_emit_receives_nothing() {
        let mut hub = WatchHub::new();
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        hub.subscribe(sink(&log));
        assert_eq!(take(&log), Vec::<Value>::new());
    }

    #[test]
    fn subscribe_after_emit_receives_current_value_immediately() {
        let mut hub = WatchHub::new();
        hub.emit(serde_json::json!(["#ffffff"]));
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        hub.subscribe(sink(&log));
        assert_eq!(take(&log), vec![serde_json::json!(["#ffffff"])]);
    }

    #[test]
    fn emit_fans_out_to_all_subscribers_and_updates_current() {
        let mut hub = WatchHub::new();
        let a: Log = Arc::new(Mutex::new(Vec::new()));
        let b: Log = Arc::new(Mutex::new(Vec::new()));
        hub.subscribe(sink(&a));
        hub.subscribe(sink(&b));
        hub.emit(serde_json::json!([{ "email": "a@b.c" }]));
        hub.emit(serde_json::json!([{ "email": "a@b.c" }, { "email": "d@e.f" }]));
        assert_eq!(take(&a).len(), 2);
        assert_eq!(take(&b).len(), 2);
        // late subscriber gets the latest, not the whole history
        let c: Log = Arc::new(Mutex::new(Vec::new()));
        hub.subscribe(sink(&c));
        assert_eq!(take(&c).len(), 1);
    }

    #[test]
    fn unsubscribe_stops_delivery_and_is_idempotent() {
        let mut hub = WatchHub::new();
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let id = hub.subscribe(sink(&log));
        assert!(hub.unsubscribe(id));
        assert!(!hub.unsubscribe(id));
        hub.emit(Value::Null);
        assert_eq!(take(&log), Vec::<Value>::new());
        assert_eq!(hub.subscriber_count(), 0);
    }

    #[test]
    fn distinct_subscriptions_get_distinct_ids() {
        let mut hub = WatchHub::new();
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let a = hub.subscribe(sink(&log));
        let b = hub.subscribe(sink(&log));
        assert_ne!(a, b);
        hub.unsubscribe(a);
        assert_eq!(hub.subscriber_count(), 1);
    }
}
