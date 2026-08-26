//! The Telegram exporter.
//!
//! The layout is a port; the *interaction rules* are behaviour, hard-won, and
//! survive the toolkit change unchanged. Each one below is a bug that was found
//! the expensive way, and each carries the reason it exists.

mod actions;
mod bridge;
mod shell;

use gpui::{px, size, App, AppContext, Application, Bounds, WindowBounds, WindowOptions};
use tgx_ui::metrics;

fn main() {
    // A blank window is the worst possible failure and it is the default one:
    // if the renderer cannot start, say so on the console at least, since there
    // is no window to say it in.
    std::panic::set_hook(Box::new(|info| {
        eprintln!(
            "TelegramExporter could not start: {info}\n\
             This build needs a GPU with working DirectX drivers."
        );
    }));

    Application::new().run(|cx: &mut App| {
        let (w, h) = metrics::MIN_WINDOW;
        let bounds = Bounds::centered(None, size(px(w + 260.0), px(h + 120.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| shell::Shell::new()),
        );
        if let Err(e) = opened {
            eprintln!("could not open a window: {e}");
            return;
        }
        cx.activate(true);
    });
}
