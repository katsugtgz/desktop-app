//! Slice 16 tests: the notification panel draws each row's `icon` URL.
//!
//! The URL travels page → `NotificationProps.icon` → the bridge's 2 048-char cap → the stored
//! `Notification.icon`; the panel's rows are the first surface that draws it. Everything here
//! goes through the same paths the window uses: the icon store's `Fetcher` seam (no test
//! touches the network), the per-frame `prefetch_icons` / `drain_icons`, and the real panel view
//! read through Rune's element tree. The letter avatar stays the fallback for a URL that is
//! refused, broken, undecodable or never asked.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use appstore_service::ApplicationService;
use manifest_registry::NewPrivateApplication;
use notification_center::{
    NewNotificationOptions, NotificationAction, NotificationCenter, NotificationProps,
};
use rune::{Env, Runtime};

use crate::icons::{self, IconStore};
use crate::shell;
use crate::state::ShellState;
use crate::tests_icons::{asked_of, temp_cache, CannedFetcher, RED_PNG};
use crate::webview;

const WINDOW: (f32, f32) = (1200.0, 800.0);

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn tag() -> String {
    format!(
        "station-shell-notify-icons-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn runtime(state: ShellState) -> Runtime<ShellState> {
    Runtime::new(
        state,
        shell::view,
        WINDOW,
        Env {
            system_dark: true,
            scale: 1.0,
        },
    )
}

/// One stored notification, as the page bridge delivers it.
fn notify(center: &mut NotificationCenter, id: &str, application: &str, icon: Option<&str>) {
    center.apply(NotificationAction::NewNotification {
        application_id: Some(application.to_owned()),
        tab_id: None,
        notification_id: id.to_owned(),
        props: NotificationProps {
            title: format!("title {id}"),
            timestamp: Some(1_700_000_000_000),
            body: None,
            icon: icon.map(str::to_owned),
        },
        options: NewNotificationOptions::default(),
    });
}

/// A shell with `center` stored and the panel open (the rail's notifications press).
fn panel_state(center: NotificationCenter) -> ShellState {
    let mut state = ShellState::new(ApplicationService::for_tests(&tag()));
    state = state.with_notifications(center);
    state.toggle_notifications();
    state
}

fn fill_of(rt: &Runtime<ShellState>, key: &str) -> Option<rune::Color> {
    rt.elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(key))
        .map(|element| element.fill)
}

/// The letter drawn inside the avatar element keyed `key`, if any: a text element whose box sits
/// inside the avatar's box. The store behind the panel draws letter avatars of its own, so the
/// assertion must stay inside the row's icon.
fn text_inside(rt: &Runtime<ShellState>, key: &str) -> Option<String> {
    let elements = rt.elements();
    let outer = elements
        .iter()
        .find(|element| element.key.as_deref() == Some(key))?
        .rect;
    elements
        .into_iter()
        .filter(|element| !element.exiting && element.rect[2] > 0.0 && element.rect[3] > 0.0)
        .find(|element| {
            element.text.is_some()
                && element.rect[0] >= outer[0] - 0.5
                && element.rect[1] >= outer[1] - 0.5
                && element.rect[0] + element.rect[2] <= outer[0] + outer[2] + 0.5
                && element.rect[1] + element.rect[3] <= outer[1] + outer[3] + 0.5
        })
        .and_then(|element| element.text)
}

/// Draws, lets the host catch up, and runs the poll (the frame work that asks and drains).
fn frame(rt: &mut Runtime<ShellState>, clock: f64) {
    rt.frame(1.0 / 60.0);
    webview::after_frame(rt);
    // The poll reports "state changed"; a host redraws on that, so the headless driver does
    // the same (`Runtime::invalidate` is the outside-a-handler seam for it).
    if webview::poll_at(&mut rt.state, clock) {
        rt.invalidate();
    }
}

/// Runs frames until `done`, then one more frame so the view draws what the poll delivered.
fn run_until(rt: &mut Runtime<ShellState>, mut done: impl FnMut(&ShellState) -> bool) {
    for _ in 0..400 {
        frame(rt, 2.0);
        if done(&rt.state) {
            for _ in 0..3 {
                rt.frame(1.0 / 60.0);
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the frame work never reached the expected state");
}

/// The asks for notification URLs in the shared fetch log (the store also asks for bundled
/// manifest icons; tests count the notification host only).
fn notify_asks(log: &crate::tests_icons::FetchLog) -> Vec<String> {
    asked_of(log)
        .into_iter()
        .filter(|asked| asked.starts_with("https://notify.example/"))
        .collect()
}

// ---- The decoded icon replaces the letter ----------------------------------------------------

#[test]
fn a_row_draws_the_decoded_icon_once_pixels_land() {
    let _guard = crate::tests_icons::ORDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let url = "https://notify.example/red.png";
    let mut center = NotificationCenter::new();
    notify(&mut center, "n1", "slack", Some(url));
    let mut state = panel_state(center);
    let mut store = IconStore::with_cache(temp_cache("notify-decode"));
    let (fetcher, asked) = CannedFetcher::serving(url, RED_PNG);
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);

    let mut rt = runtime(state);
    rt.frame(1.0 / 60.0);
    assert!(
        rt.find("notification-icon-n1").is_some(),
        "the row lays out its icon slot"
    );
    // Nothing has landed yet: the sender app's letter avatar is what draws ("slack" has no
    // manifest in this shell, so the id is the letter source).
    assert_eq!(
        text_inside(&rt, "notification-icon-n1").as_deref(),
        Some("S"),
        "the letter avatar shows before the bytes arrive"
    );

    run_until(&mut rt, |state| {
        state.icon_pixels(url).map(|pixels| pixels.size()) == Some((2, 2))
    });

    assert!(rt.find("notification-icon-n1").is_some());
    assert_eq!(
        rt.state.icon_pixels(url).map(|pixels| pixels.size()),
        Some((2, 2)),
        "the decoded icon is the handle the row reads"
    );
    assert_eq!(
        text_inside(&rt, "notification-icon-n1"),
        None,
        "the letter is gone once the decoded icon draws"
    );
    assert_eq!(
        notify_asks(&asked),
        vec![url.to_owned()],
        "the row asked its URL exactly once"
    );
}

// ---- The fallbacks stay the fallback ---------------------------------------------------------

#[test]
fn a_broken_url_keeps_the_letter_avatar() {
    let _guard = crate::tests_icons::ORDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let url = "https://notify.example/missing.png";
    let mut center = NotificationCenter::new();
    notify(&mut center, "n1", "slack", Some(url));
    let mut state = panel_state(center);
    let mut store = IconStore::with_cache(temp_cache("notify-broken"));
    let (fetcher, asked) = CannedFetcher::refusing(url, "HTTP 404");
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);

    let mut rt = runtime(state);
    // The failure leaves an empty handle (the store's contract); the avatar keeps the letter.
    run_until(&mut rt, |state| {
        state.icon_pixels(url).map(|pixels| pixels.size()) == Some((0, 0))
    });
    // The handle exists from the ask, so wait for the worker's answer before counting asks.
    for _ in 0..200 {
        if !notify_asks(&asked).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        rt.state.icon_pixels(url).map(|pixels| pixels.size()),
        Some((0, 0))
    );
    assert_eq!(
        text_inside(&rt, "notification-icon-n1").as_deref(),
        Some("S"),
        "the letter avatar still draws for a refused icon"
    );
    assert!(rt.find("notification-icon-n1").is_some());
    assert_eq!(notify_asks(&asked), vec![url.to_owned()]);
}

#[test]
fn a_notification_whose_app_is_gone_still_draws_the_id_derived_letter() {
    let mut center = NotificationCenter::new();
    notify(&mut center, "n1", "ghost-app", None);
    let mut rt = runtime(panel_state(center));
    rt.frame(1.0 / 60.0);

    assert!(rt.find("notification-icon-n1").is_some());
    assert_eq!(
        text_inside(&rt, "notification-icon-n1").as_deref(),
        Some("G"),
        "the gone app's id is the letter source"
    );
    // No manifest means no theme colour: `icons::avatar` derives the accent from the id.
    let accent = rune::tok::accent(
        "ghost-app".bytes().map(usize::from).sum::<usize>() % rune::tok::ACCENT_NAMES.len(),
        true,
    );
    assert_eq!(fill_of(&rt, "notification-icon-n1"), Some(accent));
}

#[test]
fn the_manifest_theme_color_tints_the_sender_avatar() {
    let mut service = ApplicationService::for_tests(&tag());
    let created = service
        .request_private_application(NewPrivateApplication {
            name: "Acme".into(),
            theme_color: "#102030".into(),
            icon_url: "https://acme.example/icon.png".into(),
            start_url: "https://acme.test".into(),
            scope: "https://acme.test".into(),
        })
        .expect("the private manifest is accepted");
    let mut state = ShellState::new(service);
    assert!(state.install(&created.bx_app_manifest_url));
    let application_id = state.routing_apps()[0].application_id.clone();
    let mut center = NotificationCenter::new();
    notify(&mut center, "n1", &application_id, None);
    state = state.with_notifications(center);
    state.toggle_notifications();

    let mut rt = runtime(state);
    rt.frame(1.0 / 60.0);

    assert_eq!(
        text_inside(&rt, "notification-icon-n1").as_deref(),
        Some("A"),
        "the manifest name supplies the letter"
    );
    assert_eq!(
        fill_of(&rt, "notification-icon-n1"),
        rune::color::hex("#102030"),
        "the manifest theme colour is the tint when the panel can obtain it"
    );

    // The manifest is evicted underneath the installed app: the identity falls back to the id
    // (and the id-derived accent) without a panic or a missing row.
    let id_letter = application_id
        .chars()
        .next()
        .map(|first| first.to_uppercase().to_string())
        .unwrap();
    rt.state.private_store_remove_all_for_tests();
    rt.invalidate();
    // The avatar tint rides Rune's fill motion, so let the change settle (0.5 s of frames).
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(
        text_inside(&rt, "notification-icon-n1").as_deref(),
        Some(id_letter.as_str()),
        "a gone manifest falls back to the id's letter"
    );
    assert_eq!(
        fill_of(&rt, "notification-icon-n1"),
        Some(rune::tok::accent(
            application_id.bytes().map(usize::from).sum::<usize>() % rune::tok::ACCENT_NAMES.len(),
            true,
        )),
        "a gone manifest falls back to the id-derived accent"
    );
}

// ---- The per-frame bound ---------------------------------------------------------------------

/// URL `index` of the bound test's corpus.
fn notify_url(index: usize) -> String {
    format!("https://notify.example/{index}.png")
}

#[test]
fn forty_notifications_ask_for_at_most_32_urls() {
    let _guard = crate::tests_icons::ORDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(crate::notifications::MAX_NOTIFICATION_ICON_URLS, 32);
    let mut center = NotificationCenter::new();
    for index in 1..=40 {
        notify(
            &mut center,
            &format!("n{index}"),
            "slack",
            Some(&notify_url(index)),
        );
    }
    let mut state = panel_state(center);
    let mut store = IconStore::with_cache(temp_cache("notify-bound"));
    let (fetcher, asked) = CannedFetcher::nothing();
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);

    // One frame's ask (`poll_at` calls this on every frame).
    state.prefetch_icons();
    for _ in 0..200 {
        if notify_asks(&asked).len() >= 32 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let asked_urls = notify_asks(&asked);
    assert_eq!(
        asked_urls.len(),
        32,
        "the forty rows asked for exactly the 32-URL bound"
    );
    // Rows are newest-first, so the first 32 are indexes 40 down to 9.
    for index in 9..=40 {
        assert!(
            asked_urls.contains(&notify_url(index)),
            "row {index} is inside the first 32 and was asked"
        );
    }
    for index in 1..=8 {
        assert!(
            !asked_urls.contains(&notify_url(index)),
            "row {index} is past the bound and must not be asked"
        );
        assert!(
            state.icon_pixels(&notify_url(index)).is_none(),
            "no handle is created for an unasked row"
        );
    }
    // A later frame asks nothing new: the prefix is stable and attempted URLs are skipped.
    state.prefetch_icons();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(notify_asks(&asked).len(), 32);
}

#[test]
fn a_hidden_panel_asks_for_nothing() {
    let _guard = crate::tests_icons::ORDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut center = NotificationCenter::new();
    notify(
        &mut center,
        "n1",
        "slack",
        Some("https://notify.example/a.png"),
    );
    notify(
        &mut center,
        "n2",
        "slack",
        Some("https://notify.example/b.png"),
    );
    let mut state = ShellState::new(ApplicationService::for_tests(&tag()));
    state = state.with_notifications(center);
    let mut store = IconStore::with_cache(temp_cache("notify-hidden"));
    let (fetcher, asked) = CannedFetcher::nothing();
    store.start(fetcher, std::sync::Arc::new(|| {}));
    state.set_icons(store);

    state.prefetch_icons();
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        notify_asks(&asked).is_empty(),
        "a closed panel asks for no notification icons"
    );
    assert!(state.icon_pixels("https://notify.example/a.png").is_none());

    // Opening it is the whole difference: the next frame asks.
    state.toggle_notifications();
    state.prefetch_icons();
    for _ in 0..200 {
        if !notify_asks(&asked).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(notify_asks(&asked).len(), 2);
}

#[test]
fn the_fallbacks_come_from_the_existing_icon_rules() {
    // A `file:` icon and a `data:` icon never reach the store's fetcher (its `is_fetchable`
    // gate), so they stay on the letter avatar without spending one of the 32 asks.
    for url in [
        "file:///C:/Windows/system.ini",
        "data:image/png;base64,iVBOR",
    ] {
        assert!(!icons::is_fetchable(url));
    }
    assert!(icons::is_fetchable("https://notify.example/icon.png"));
}
