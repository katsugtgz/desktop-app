//! Slice 19 tests: `new Notification(...)` reaches the panel through the same four members.
//!
//! **No JavaScript engine runs here.** These assertions are over `INIT_SCRIPT`'s reviewed source
//! plus the real Rust parser: for a hostile title, the wire JSON is built with `serde_json` (the
//! same bytes `JSON.stringify` produces for the object the script sends) and fed through
//! `bridge::parse`, proving the field names, escaping and caps on the receiving side. A windowed
//! page is the only thing that can prove the construction actually runs; see the slice entry.

use serde_json::json;

use super::bridge::{parse, PageMessage, Sender, INIT_SCRIPT};

fn sender() -> Sender {
    Sender::App {
        tab_id: "gmail".to_owned(),
        application_id: "gmail".to_owned(),
    }
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let from = text.find(start).expect("start marker") + start.len();
    let to = text[from..].find(end).expect("end marker") + from;
    &text[from..to]
}

/// The JSON the constructor path produces for `title`: the script sends
/// `{ timestamp, title, body, icon }` to `sendNotification`, which stringifies `{ kind, id,
/// notification }`.
fn send_json(id: &str, title: &str) -> String {
    json!({
        "kind": "send",
        "id": id,
        "notification": {
            "timestamp": 1_700_000_000_000i64,
            "title": title,
            "body": "body",
            "icon": "https://cdn.example/i.png",
        }
    })
    .to_string()
}

/// Every global the script defines: `Object.defineProperty(window, '<name>'` and
/// `window.<name> =` assignments.
fn defined_globals(script: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for line in script.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Object.defineProperty(window, '") {
            if let Some(end) = rest.find('\'') {
                names.push(rest[..end].to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("window.") {
            if let Some(eq) = rest.find(" =") {
                names.push(rest[..eq].to_owned());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

#[test]
fn the_override_is_installed_after_the_api_and_defines_no_other_global() {
    let api = INIT_SCRIPT
        .find("Object.defineProperty(window, 'bxApi'")
        .expect("the bridge is installed");
    let class = INIT_SCRIPT.find("class BxNotification").expect("the class");
    let install = INIT_SCRIPT
        .find("window.Notification = BxNotification;")
        .expect("the override is installed");
    assert!(
        api < class && class < install,
        "the class must exist before it can be installed, after the api it sends through"
    );
    assert_eq!(
        defined_globals(INIT_SCRIPT),
        [
            "Notification",
            "__stationNotificationClick",
            "__stationSnoozeUpdate",
            "bxApi",
        ],
        "the override added a global beyond the bridge, its hooks and the Notification API"
    );
    // The six members the preload declared are still the page API; nothing else of bxApi exists.
    for member in [
        "sendNotification",
        "closeNotification",
        "addNotificationClickListener",
        "removeNotificationClickListener",
        "addSnoozeDurationInMsChangeListener",
        "removeSnoozeDurationInMsChangeListener",
    ] {
        assert!(INIT_SCRIPT.contains(member), "{member} is missing");
    }
    assert!(!INIT_SCRIPT.contains("applications"));
    assert!(!INIT_SCRIPT.contains("getManifest"));
    assert!(INIT_SCRIPT.contains("Object.freeze"));
}

#[test]
fn the_constructor_sends_the_wire_fields_the_shell_parses() {
    // The TS id scheme (`webview-inject.js:87`): `notif/` plus a 21-character nanoid.
    assert!(INIT_SCRIPT.contains("this.id = 'notif/' + notificationSuffix();"));
    assert!(INIT_SCRIPT.contains("let remaining = 21;"));
    assert!(INIT_SCRIPT.contains("const NOTIFICATION_ALPHABET ="));

    // The constructor's payload is exactly the four fields the Rust `WireNotification` accepts,
    // built as data (no string concatenation into JSON).
    let payload = between(INIT_SCRIPT, "api.sendNotification(this.id, {", "});");
    for field in [
        "timestamp: this.timestamp",
        "title: this.title",
        "body: this.body",
        "icon: icon",
    ] {
        assert!(payload.contains(field), "the payload lacks {field}");
    }
    for absent in ["actions", "image", "vibrate", "renotify", "data:", "badge"] {
        assert!(
            !payload.contains(absent),
            "the payload sends {absent}, which the TS never sent"
        );
    }
    // Exactly the two notification channels already in the bridge shape; the class adds no
    // `kind`. (The page-meta reporter has its own stringify; it is asserted in
    // `tests_page_meta`.)
    assert_eq!(INIT_SCRIPT.matches("kind: 'send'").count(), 1);
    assert_eq!(INIT_SCRIPT.matches("kind: 'close'").count(), 1);
    let api = between(INIT_SCRIPT, "const api = {", "\n  };");
    assert_eq!(
        api.matches("JSON.stringify").count(),
        2,
        "one stringify per notification channel"
    );
    for forbidden in ["eval(", "new Function("] {
        assert!(
            !INIT_SCRIPT.contains(forbidden),
            "the script builds code with {forbidden}"
        );
    }
}

#[test]
fn a_hostile_title_cannot_escape_the_json_the_parser_reads() {
    let id = format!("notif/{}", "a".repeat(21));
    assert_eq!(id.len(), 27, "the TS id is 'notif/' plus 21 characters");
    // Quotes, a newline, a closing script tag and a JS line separator: all data.
    let title = "a\"b\n</script>\u{2028}end";
    let json = send_json(&id, title);

    match parse(sender(), &json) {
        Ok(PageMessage::Send {
            notification_id,
            props,
            ..
        }) => {
            assert_eq!(notification_id, id);
            assert_eq!(props.title, title, "the title did not survive as data");
            assert_eq!(props.body.as_deref(), Some("body"));
            assert_eq!(props.icon.as_deref(), Some("https://cdn.example/i.png"));
            assert_eq!(props.timestamp, Some(1_700_000_000_000));
        }
        other => panic!("the constructor's JSON was refused: {other:?}"),
    }
}

#[test]
fn close_sends_the_existing_close_kind() {
    assert!(INIT_SCRIPT.contains("close() {"));
    assert!(INIT_SCRIPT.contains("api.closeNotification(this.id)"));
    let close = json!({ "kind": "close", "id": "notif/x" }).to_string();
    match parse(sender(), &close) {
        Ok(PageMessage::Close {
            notification_id, ..
        }) => assert_eq!(notification_id, "notif/x"),
        other => panic!("closeNotification's JSON was refused: {other:?}"),
    }
}

#[test]
fn permission_is_granted_and_request_permission_resolves_it() {
    assert!(INIT_SCRIPT.contains("const NOTIFICATION_PERMISSION = 'granted';"));
    assert!(INIT_SCRIPT.contains("static requestPermission(deprecatedCallback)"));
    assert!(INIT_SCRIPT.contains("Promise.resolve(NOTIFICATION_PERMISSION)"));
    assert!(INIT_SCRIPT.contains("if (typeof deprecatedCallback === 'function')"));
    assert!(INIT_SCRIPT.contains("request.then(deprecatedCallback)"));
    // `Notification.permission` is a read-only static, as on the web platform.
    assert!(INIT_SCRIPT.contains("Object.defineProperty(BxNotification, 'permission'"));
    assert!(INIT_SCRIPT.contains("writable: false, configurable: false, enumerable: true"));
}

#[test]
fn the_click_hook_reaches_onclick_and_unregisters() {
    // One listener per instance, registered through the existing capped registry.
    assert!(INIT_SCRIPT.contains("api.addNotificationClickListener(this._clickListener);"));
    assert!(INIT_SCRIPT.contains("_handleNotificationClick(id)"));
    assert!(INIT_SCRIPT.contains("if (this.id !== id) return;"));
    assert!(INIT_SCRIPT.contains("this._unregisterIPC();"));
    assert!(INIT_SCRIPT.contains("this._emit('click');"));
    // Dispatch calls the `on<type>` handler and every listener, each in its own try/catch.
    let dispatch = between(INIT_SCRIPT, "dispatchEvent(event) {", "    _emit(type)");
    assert!(dispatch.contains("handler.call(this, event)"));
    assert!(dispatch.contains("listener.call(this, event)"));
    assert_eq!(
        dispatch.matches("try {").count(),
        2,
        "a throwing handler or listener must not stop the other"
    );
    for event in ["onclick", "onclose", "onshow", "onerror"] {
        assert!(
            INIT_SCRIPT.contains(&format!("get {event}()")),
            "no {event} surface"
        );
        assert!(
            INIT_SCRIPT.contains(&format!("set {event}(handler)")),
            "no {event} setter"
        );
    }
    // The shell's click evaluator invokes the hook with the id; the class compares it to its own.
    assert!(INIT_SCRIPT.contains("const clickListeners = [];"));
    assert!(INIT_SCRIPT.contains("try { listener(id); } catch (e) {}"));
}

#[test]
fn the_ts_blob_icon_path_and_iframe_override_are_dropped() {
    for dropped in [
        "XMLHttpRequest",
        "readAsDataURL",
        "blob:http",
        "recursiveOverride",
        "iframe",
    ] {
        assert!(
            !INIT_SCRIPT.contains(dropped),
            "{dropped} is out of this slice"
        );
    }
    // What the TS icon handling kept: Gmail's protocol-relative URL becomes https; a non-string
    // icon is dropped (the shell's letter avatar is the fallback).
    assert!(INIT_SCRIPT.contains("if (icon.startsWith('//')) icon = 'https:' + icon;"));
    assert!(INIT_SCRIPT.contains("icon = undefined;"));
}
