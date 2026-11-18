//! Station's native shell: one Rune window holding a vertical app rail, a top bar naming the
//! active app, and the content pane that hosts that app's web view.
//!
//! Rune draws the window; [`webview`] places the app's page over the pane it lays out. The
//! window is all view and tokens: [`state`] owns what is selected, [`rail`] draws the tab strip,
//! [`placeholder`] draws the top bar and pane, and [`shell`] composes them into the `El` an
//! `AppSpec` renders.

mod appstore;
mod notifications;
mod placeholder;
mod rail;
mod routing;
mod session;
mod settings;
mod shell;
mod state;
mod webview;

#[cfg(test)]
mod tests;

fn main() {
    // The window opens where the session left it: the size is the spec's, the position is
    // applied once the native window exists, and maximized is the app's own launch flag.
    let path = session::path();
    let window = session::window_state(&path);
    // The session's points are already capped; this also floors a NaN or negative size, and the
    // real display scale re-checks the device-pixel ceiling in `on_window`.
    let (width, height) = session::safe_window_size(window.width, window.height, 1.0);

    rune::App::new(rune::AppSpec {
        // Escape leaves Settings and the notification panel; Rune hands over keys no control took.
        on_key: Some(shell::on_key),
        // A page's new-window request is queued from the web view's thread; the wake makes the
        // next frame poll for it, and poll routes it (select/load, or the OS browser).
        setup: Some(webview::setup),
        poll: Some(webview::poll),
        // Named states for snapshots: the two surfaces the rail opens.
        scenes: &[
            ("settings", |state| state.open_settings()),
            ("notifications", |state| state.toggle_notifications()),
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
        webview::close_all();
    })
    .on_quit(|state, _| {
        state.save();
        webview::close_all();
    })
    .run();
}
