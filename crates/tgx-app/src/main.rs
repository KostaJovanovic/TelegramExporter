//! The Telegram exporter.
//!
//! The layout is a port; the *interaction rules* are behaviour, hard-won, and
//! survive the toolkit change unchanged. Each one below is a bug that was found
//! the expensive way, and each carries the reason it exists.

// **A release build is a GUI binary, so double-clicking it does not open a
// console window behind the app.** Without this the exe defaults to the console
// subsystem and Windows allocates one, which looks like the program started
// something else.
//
// Debug builds keep the console, because that is where `cargo run` lives and
// the startup diagnostics below are worth more than the tidiness.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod bridge;
mod login;
mod shell;

use gpui::{px, size, App, AppContext, Application, Bounds, WindowBounds, WindowOptions};
use tgx_ui::metrics;

/// Record a startup failure somewhere a user can actually find it.
///
/// A GUI-subsystem binary has no stderr unless it was launched from a terminal,
/// and **a blank window is the worst possible failure precisely because it says
/// nothing**. So the message goes three places: stderr, which reaches a
/// developer running `cargo run`; the log, which reaches anything attached to
/// it; and a file beside the executable, which is the only one that survives a
/// double-click.
fn report_startup_failure(message: &str) {
    eprintln!("{message}");
    log::error!("{message}");
    if let Ok(dir) = tgx_tg::config::ensure_data_dir() {
        let _ = std::fs::write(dir.join("startup-error.log"), message);
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        report_startup_failure(&format!(
            "TelegramExporter could not start: {info}\n\
             This build needs a GPU with working DirectX drivers."
        ));
    }));

    Application::new().run(|cx: &mut App| {
        // Must run before any gpui-component feature is used.
        gpui_component::init(cx);

        let (w, h) = metrics::MIN_WINDOW;
        let bounds = Bounds::centered(None, size(px(w + 260.0), px(h + 120.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| shell::Shell::new(window, cx));
                // The first level in the window has to be a Root, so the
                // component library's overlays have somewhere to go.
                cx.new(|cx| gpui_component::Root::new(gpui::AnyView::from(view), window, cx))
            },
        );
        if let Err(e) = opened {
            report_startup_failure(&format!("could not open a window: {e}"));
            return;
        }
        cx.activate(true);
    });
}
