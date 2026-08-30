//! The Telegram exporter.
//!
//! The layout is a port; the *interaction rules* are behaviour, hard-won, and
//! survive the toolkit change unchanged. Each one carries the reason it exists
//! at the place it is enforced.

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

/// What the window and its failures call themselves.
pub const TITLE: &str = "Telegram Exporter";

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
/// telling someone their machine needs working graphics drivers when the app
/// has been running all afternoon sends them to fix something that is not
/// broken. The hook says which kind of failure it is, and it can only know that
/// by being told.
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
/// driver update, overwriting `startup-error.log` with a diagnosis that sent
/// the user to fix a graphics card that was never broken.
fn panic_message(window_opened: bool, panic: &str) -> String {
    if window_opened {
        // The renderer plainly works — it has been drawing. Naming the GPU
        // here would send someone to update a driver that is fine, and the
        // thing that actually matters is what was on screen when it went.
        format!(
            "{TITLE} stopped unexpectedly: {panic}\n\
             An export already written to disk is unaffected; the JSON is \
             closed as each chat finishes."
        )
    } else {
        format!(
            "{TITLE} could not start: {panic}\n\
             This build draws with OpenGL and needs working graphics drivers."
        )
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

    let (w, h) = tgx_ui::components::min_window();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([w + 320.0, h + 160.0])
            // **The floor the layout was measured at.** Without it the window
            // drags below 900x620, where the queue's Chat column — the only
            // thing naming which row is which — is squeezed to 11px, i.e. gone,
            // while the count columns beside it keep their full width.
            .with_min_inner_size([w, h]),
        ..Default::default()
    };

    let started = eframe::run_native(
        TITLE,
        options,
        Box::new(|cc| {
            // The earliest point at which "the window opened" is true — past
            // here a panic is not a startup failure. See `panic_message`. The
            // GPUI version had this flag and never set it, so every panic took
            // the other branch.
            WINDOW_OPENED.store(true, std::sync::atomic::Ordering::Relaxed);
            // The design, before the first frame draws anything in egui's own
            // rounded, shadowed defaults. `Shell::update` reinstalls it when the
            // appearance changes and not otherwise — see `theme_stale`.
            let settings = tgx_tg::config::Settings::load();
            tgx_ui::theme::install(
                &cc.egui_ctx,
                &tgx_ui::tokens::Palette::named(&settings.theme),
            );
            Ok(Box::new(shell::Shell::new()))
        }),
    );
    if let Err(e) = started {
        report_startup_failure(&format!("could not open a window: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_before_the_window_opens_blames_the_graphics_stack() {
        let message = panic_message(false, "boom");
        assert!(message.contains("could not start"));
        assert!(message.contains("graphics drivers"));
    }

    #[test]
    fn a_panic_after_the_window_opened_does_not_blame_the_graphics_stack() {
        // The branch that was unreachable for a long time, because nothing ever
        // set the flag: a panic deep into an export was reported as a graphics
        // driver problem.
        let message = panic_message(true, "boom");
        assert!(!message.contains("graphics drivers"));
        assert!(message.contains("stopped unexpectedly"));
        assert!(message.contains("unaffected"));
    }

    #[test]
    fn the_flag_starts_false_so_a_startup_panic_gets_the_driver_message() {
        assert!(!WINDOW_OPENED.load(std::sync::atomic::Ordering::Relaxed));
    }
}
