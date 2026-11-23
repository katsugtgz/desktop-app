//! Station's native shell: one Rune window holding a vertical app rail, a top bar naming the
//! active app, and the content pane that hosts that app's web view.
//!
//! Rune draws the window; [`webview`] places the app's page over the pane it lays out. The
//! window is all view and tokens: [`state`] owns what is selected, [`rail`] draws the tab strip,
//! [`placeholder`] draws the top bar and pane, and [`shell`] composes them into the `El` an
//! `AppSpec` renders.

// On non-Windows hosts the WebView2/tray/Win32 entry points are stubbed out, leaving helper types
// and methods exercised only by Windows or unit tests. Keep full dead-code checking on Windows.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

mod about;
mod appstore;
mod autolaunch;
mod crash;
mod deep_link;
mod dialogs;
mod downloads;
mod find;
mod ico;
mod icons;
mod instance;
mod notifications;
mod oauth;
mod permissions;
mod placeholder;
#[cfg(target_os = "windows")]
mod platform_icon;
mod quick_switch;
mod rail;
mod routing;
mod session;
mod settings;
mod shell;
mod site_scripts;
mod state;
#[cfg(test)]
mod test_support;
mod tray;
mod webview;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_pages;

#[cfg(test)]
mod tests_icons;

#[cfg(test)]
mod tests_config;

#[cfg(test)]
mod tests_rail_order;

#[cfg(test)]
mod tests_deep_link;

#[cfg(test)]
mod tests_dpi;

#[cfg(test)]
mod tests_instance;

#[cfg(test)]
mod tests_notify_click;

#[cfg(test)]
mod tests_notify_icons;

#[cfg(test)]
mod tests_tray;

#[cfg(test)]
mod tests_page_env;

#[cfg(test)]
mod tests_accounts;

#[cfg(test)]
mod tests_page_meta;

#[cfg(test)]
mod tests_downloads;

#[cfg(test)]
mod tests_site_scripts;

#[cfg(test)]
mod tests_shortcuts;

#[cfg(test)]
mod tests_find;

#[cfg(test)]
mod tests_quick_switch;

#[cfg(test)]
mod tests_os_notifications;

fn main() {
    // One shell per session: a second launch forwards its `station://` argument to the running
    // shell and exits before any window is asked for. Must precede Rune: a secondary never
    // opens the web view stack at all.
    instance::handoff_if_secondary();

    // Local, bounded, secret-free panic diagnostics under `<config>/station/crashes/`.
    crash::install_panic_hook();

    // The window opens where the session left it: the size is the spec's, the position is
    // applied once the native window exists, and maximized is the app's own launch flag.
    let path = session::path();
    let window = session::window_state(&path);
    // The session's points are already capped; this also floors a NaN or negative size, and the
    // real display scale re-checks the device-pixel ceiling in `on_window`.
    let (width, height) = session::safe_window_size(window.width, window.height, 1.0);

    let app = rune::App::new(rune::AppSpec {
        // Escape leaves Settings and the notification panel; Rune hands over keys no control took.
        on_key: Some(shell::on_key),
        // A page's new-window request is queued from the web view's thread; the wake makes the
        // next frame poll for it, and poll routes it (select/load, or the OS browser). The same
        // waker starts the icons worker, so a fetched icon wakes the next frame itself.
        // Setup is also the one deep-link read: the `station://` URL this process was launched
        // with is honoured once, here (`deep_link::open`), never again while running. The
        // single-instance listener starts here too: the waker it keeps makes the next frame
        // drain a link a second launch hands over.
        setup: Some(|state, waker| {
            webview::setup(state, waker.clone());
            instance::start_primary(waker);
            deep_link::open(state, &std::env::args_os().collect::<Vec<_>>());
        }),
        poll: Some(webview::poll),
        // Named states for snapshots: the surfaces the shell opens.
        scenes: &[
            ("settings", |state| state.open_settings()),
            ("notifications", |state| state.toggle_notifications()),
            ("about", |state| state.open_about()),
            ("quick-switch", |state| state.open_quick_switch()),
        ],
        ..rune::AppSpec::new(
            "Station",
            (width, height),
            state::ShellState::launch,
            shell::view,
        )
    })
    .maximized(window.maximized)
    .on_window(move |handle| {
        session::apply(handle, &window);
        session::remember(handle);
        webview::window_created(handle);
    })
    .on_frame_drawn(webview::after_frame)
    .on_close(|state, _| {
        state.save();
        notifications::os::shutdown();
        webview::close_all();
        instance::shutdown();
    })
    .on_quit(|state, _| {
        state.save();
        notifications::os::shutdown();
        webview::close_all();
        instance::shutdown();
    });

    // The notification-area icon runs on Rune's own tray thread; keep the Tray alive for the
    // app's whole life (dropping it removes the icon). Windows only, like the icon asset.
    #[cfg(target_os = "windows")]
    let _tray = tray::start(app.handle());

    let headless_cli = std::env::args()
        .skip(1)
        .any(|arg| matches!(arg.as_str(), "--snap" | "--tree" | "--help" | "-h"));
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run())) {
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_owned()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "Unexpected fatal error".to_owned()
        };
        if !headless_cli {
            dialogs::show_fatal_error(
                "Station stopped unexpectedly",
                &format!("Station encountered an unrecoverable error and must close.\n\n{msg}"),
            );
        } else {
            eprintln!("station-shell fatal: {msg}");
        }
        std::process::exit(101);
    }
}
