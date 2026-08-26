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
mod journal;
mod list;
mod login;
mod queue;
mod settings_form;
mod shell;
mod theme;

use gpui::{
    px, size, App, AppContext, Application, Bounds, ParentElement, Styled, TitlebarOptions,
    WindowBounds, WindowOptions,
};

/// Record a failure somewhere a user can actually find it.
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

/// Whether the window ever opened.
///
/// **A panic twenty minutes into an export is not a startup failure**, and
/// telling someone their machine needs DirectX drivers when the app has been
/// running all afternoon sends them to fix something that is not broken. The
/// hook says which kind of failure it is, and it can only know that by being
/// told.
static WINDOW_OPENED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The two ways a panic gets reported, chosen by whether the window ever
/// opened.
///
/// Pulled out of the hook itself so the choice can be tested without staging
/// a real panic or a real window: this crate has no `PanicHookInfo` it can
/// construct in a test, so `panic` here is already the formatted description,
/// which is all the branch below ever looks at.
///
/// **This is the fix for the bug the flag above was written to prevent, and
/// did not.** `WINDOW_OPENED` existed and this branch existed, but nothing
/// ever called `.store(true, ..)` on it — so the hook always took the `else`
/// below, and a panic inside a two-hour-old export was reported as needing a
/// GPU driver update, overwriting `startup-error.log` with a diagnosis that
/// sent the user to fix a graphics card that was never broken.
fn panic_message(window_opened: bool, panic: &str) -> String {
    if window_opened {
        // The renderer plainly works — it has been drawing. Naming the GPU
        // here would send someone to update a driver that is fine, and the
        // thing that actually matters is what was on screen when it went.
        format!(
            "TelegramExporter stopped unexpectedly: {panic}\n\
             An export already written to disk is unaffected; the JSON is \
             closed as each chat finishes."
        )
    } else {
        format!(
            "TelegramExporter could not start: {panic}\n\
             This build needs a GPU with working DirectX drivers."
        )
    }
}

/// One element between `Root` and the shell, carrying nothing but the typeface.
///
/// `Root` already sets `Theme::font_family` on its own div, which is why
/// `theme::apply` is where the *family* is chosen. But it sets the family
/// alone, and the design also wants Geist's `tnum`: the face's default figures
/// are proportional — nine distinct advances across ten digits — so a count
/// ticking from 199 to 200 shifts everything beside it sideways. Only
/// `Styled::font` carries [`gpui::FontFeatures`], and the outermost element the
/// shell owns is in `shell/mod.rs`, so the refinement is inserted here instead.
struct Typeface(gpui::AnyView);

impl gpui::Render for Typeface {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
            .size_full()
            .font(tgx_ui::fonts::sans())
            .child(self.0.clone())
    }
}

fn main() {
    // **First, because until this line every `log::` call in the program was a
    // no-op.** Nothing installed a `log` implementation, so our own warnings —
    // and grammers' `connecting...`, `generating new authorization key...`,
    // `authorization key generated successfully`, which between them are the
    // whole timing of a sign-in — went nowhere. A GUI-subsystem binary has no
    // stderr, so the file is the only channel that survives a double-click.
    //
    // A failure to open it is recorded and stepped over: refusing to start
    // because the *diagnostic* could not be written would cost someone an
    // export over a file they never asked for.
    if let Err(e) = tgx_tg::logging::init() {
        eprintln!("could not open the log: {e}");
    }

    std::panic::set_hook(Box::new(|info| {
        use std::sync::atomic::Ordering;
        let message = panic_message(WINDOW_OPENED.load(Ordering::Relaxed), &info.to_string());
        report_startup_failure(&message);
    }));

    Application::new().run(|cx: &mut App| {
        // **Before `gpui_component::init`**, because `theme::apply` below hands
        // the library `tgx_ui::fonts::SANS` and a family that is not registered
        // yet resolves to the system face and is then cached under that name.
        // A failure here is reported and stepped over: the window is set in the
        // wrong typeface, which is ugly, while refusing to start would cost the
        // user an export over a font.
        if let Err(e) = tgx_ui::fonts::register(cx) {
            report_startup_failure(&format!("{e}"));
        }
        // Must run before any gpui-component feature is used.
        gpui_component::init(cx);
        // And immediately after it, before a borrowed component can paint once
        // in the library's own colours. See `theme.rs` for why the order is
        // load-bearing in both directions.
        let settings = tgx_tg::config::Settings::load();
        theme::apply(&tgx_ui::tokens::Palette::named(&settings.theme), cx);

        let (w, h) = tgx_ui::components::min_window();
        let bounds = Bounds::centered(None, size(px(w + 320.0), px(h + 160.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // **The floor the layout was measured at.** Without it the
                // window drags below 900x620, where the queue's Chat column —
                // the only thing naming which row is which — is squeezed to
                // 11px, i.e. gone, while the count columns beside it keep their
                // full width. `tgx_ui::components::min_window()` has held this
                // number all along and nothing called it.
                window_min_size: Some(size(px(w), px(h))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Telegram Exporter".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| shell::Shell::new(window, cx));
                let typeface = cx.new(|_| Typeface(gpui::AnyView::from(view)));
                // The first level in the window has to be a Root, so the
                // component library's overlays have somewhere to go. `Typeface`
                // goes *inside* it rather than around it for that reason.
                cx.new(|cx| gpui_component::Root::new(gpui::AnyView::from(typeface), window, cx))
            },
        );
        if let Err(e) = opened {
            report_startup_failure(&format!("could not open a window: {e}"));
            return;
        }
        // Past this point a panic is not a startup failure — see
        // `panic_message`. `open_window` returning `Ok` is the earliest point
        // that is true, which is why this sits beside the error check above
        // rather than inside the window's own content closure.
        WINDOW_OPENED.store(true, std::sync::atomic::Ordering::Relaxed);
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_before_the_window_opens_blames_the_gpu() {
        let message = panic_message(false, "boom");
        assert!(message.contains("could not start"));
        assert!(message.contains("DirectX"));
    }

    #[test]
    fn a_panic_after_the_window_opened_does_not_blame_the_gpu() {
        // This is the branch that was unreachable: nothing ever set
        // `WINDOW_OPENED`, so every panic — including one deep into an export
        // — took the branch above and reported a driver problem that did not
        // exist.
        let message = panic_message(true, "boom");
        assert!(!message.contains("DirectX"));
        assert!(message.contains("stopped unexpectedly"));
        assert!(message.contains("unaffected"));
    }

    #[test]
    fn the_flag_starts_false_so_a_startup_panic_gets_the_gpu_message() {
        // Guards the other half of the fix: a fresh process must not
        // accidentally start in the "window opened" state, or the very panics
        // this flag exists to diagnose — a bad driver before a frame is ever
        // drawn — would get the wrong message too.
        assert!(!WINDOW_OPENED.load(std::sync::atomic::Ordering::Relaxed));
    }
}
