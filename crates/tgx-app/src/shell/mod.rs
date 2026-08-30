//! The window: nav bar, chat list, settings, queue, log.
//!
//! The panels live in the submodules beside this one. They are child modules
//! rather than siblings on purpose — a child can see this struct's private
//! fields, so splitting the file costs no encapsulation, whereas a sibling
//! module would have forced every piece of shell state to `pub(crate)`.
//!
//! **The one rule this file exists to hold: a worker event repaints the
//! window.** Under GPUI that meant scheduling a task to *await* the channel,
//! because a poll inside `render` only ran once something else had already
//! caused a frame — which the user experienced as moving the mouse to make the
//! app work. An immediate-mode window has no await to schedule: [`Shell::update`]
//! drains the bridge at the top of every frame, and what makes that correct is
//! that `bridge::Events` calls `request_repaint` on every send, so the frame it
//! drains in is one a worker asked for. **Nothing in this crate may send an
//! event by any route that does not wake the context.**

mod chats;
mod chrome;
mod commands;
mod events;
mod run;
mod settings;
mod signin;

use crate::bridge::{Activity, Bridge, Event};
use crate::journal::Journal;
use crate::list::{self, Category, SortMode};
use crate::login::{LoginDialog, Stage};
use crate::queue::Queue;
use crate::settings_form::SettingsForm;
use eframe::egui::{self, Context};
use tgx_tg::cancel::Cancel;
use tgx_tg::client::ChatInfo;
use tgx_tg::config::Settings;
use tgx_ui::tokens::Palette;

/// What the body of the window is showing.
///
/// **The window has one job at a time, and this says which.**
///
/// Four regions used to be on screen permanently: the chat list, the settings,
/// the queue and the log. That is two different tasks shown at once and both
/// starved. Before a run the chat list *is* the work — six hundred rows, a
/// filter, a sort, a selection worth several thousand messages. During one the
/// queue and the log are the work and the list is irrelevant. Settings are
/// consulted rarely and changed rarely, and they are twenty-five controls.
///
/// Sharing one 400pt column between the three left the queue's Chat column —
/// the only cell that says which row is which — with about 110 points, the
/// settings in a scrolling tube that showed four rows of twenty-five, and the
/// log in 160. Every one of those is fixed by the same move: show one, full
/// width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// Pick what to export. The default, and where a session starts.
    #[default]
    Chats,
    /// Where it goes and what it contains.
    Settings,
    /// What a run is doing: the queue, the bar and the log.
    Run,
}

impl View {
    pub const ALL: [View; 3] = [View::Chats, View::Settings, View::Run];

    pub fn label(self) -> &'static str {
        match self {
            View::Chats => "Chats",
            View::Settings => "Settings",
            View::Run => "Run",
        }
    }
}

/// One row as the list paints it.
///
/// Owned, not borrowed, because it is a **cache**: the list is virtualised and
/// asks for its visible range on every frame, and re-filtering and re-sorting
/// the whole account inside that callback is work done sixty times a second for
/// an answer that changed once. Rebuilt by [`Shell::rebuild_rows`] when the
/// chats, the filter, the sort, the grouping or a fold changes — and nowhere
/// else.
pub enum PaintedRow {
    Heading {
        category: Category,
        total: usize,
        folded: bool,
    },
    Chat(ChatInfo),
}

pub struct Shell {
    /// The tokio side. The UI thread never blocks on it.
    bridge: Bridge,

    palette: Palette,
    settings: Settings,
    /// The editable settings' text fields.
    ///
    /// Not an `Option` any more. Under GPUI these were `Entity<InputState>`,
    /// which cannot exist without a live `Window`, so the headless shell the
    /// interaction tests drive had to carry `None` and skip them. egui binds a
    /// field to a `&mut String`, so the buffer is ordinary data and the tests
    /// reach the same one the window does.
    form: SettingsForm,
    /// The filter box's buffer. **The one copy** — `view.filter` is written
    /// from it, never typed into separately, because a second copy of the same
    /// string is how the empty state ends up quoting something other than what
    /// is in the box.
    search: String,

    signed_in: bool,
    /// Tracked separately from `chats.is_empty()`: the list is empty both
    /// before a sign-in and after one that found nothing, and those two need
    /// opposite instructions.
    loaded: bool,
    exporting: bool,
    counting: bool,
    /// **The stop signal, shared with the worker.** Setting `exporting = false`
    /// and writing "Stopped" is not stopping: the export went on writing files
    /// and then overwrote the message with its own success. This is the flag
    /// the read loop actually reads.
    cancel: Cancel,

    chats: Vec<ChatInfo>,
    /// Ticks are held **by chat id**, so they survive re-sorting, regrouping
    /// and filtering.
    selected: std::collections::HashSet<i64>,
    view: list::View,
    rows: Vec<PaintedRow>,
    /// Something changed what the rows would be. Rebuilt once at the end of an
    /// event batch rather than on every event that invalidates them.
    rows_stale: bool,
    /// How many chats the filter leaves, cached with the rows. **Not**
    /// `rows.len()`: a folded category hides its rows but its chats still count
    /// as visible, because folding is a way of looking at the list rather than
    /// a second filter.
    visible_count: usize,

    /// Which of the three the body is showing. See [`View`].
    ///
    /// **Named `body` and not `view`**, because `view: list::View` beside it is
    /// how the chat list is filtered and sorted, and two fields called `view`
    /// meaning different things in one struct is a bug waiting for a hurried
    /// afternoon.
    body: View,
    /// The user changed the view by hand, so a run must not change it back.
    ///
    /// Starting an export switches to Run once — that is where the answer to
    /// "what is it doing?" now lives, and leaving someone on the chat list
    /// watching nothing happen is the failure this avoids. But if they then go
    /// and look at Settings mid-run, the next `Progress` event must not yank
    /// them away again.
    body_pinned: bool,

    status: String,
    /// Why the run could not proceed, if something said so.
    ///
    /// Held only until the run ends, because a run that ends with a stated
    /// cause should say the cause and not just the tally — and the two arrive
    /// in the same event batch, so without this the tally simply wins.
    failure: Option<String>,
    journal: Journal,
    queue: Queue,
    /// Chats counted so far, and how many there are. Separate from the export's
    /// progress because **one bar has two claimants** and the export owns it.
    count_progress: Option<(usize, usize)>,

    /// **One dialog, ever.** An `Option`, not a flag plus a stage, so
    /// "is one open?" and "which one?" cannot disagree.
    login: Option<LoginDialog>,

    /// Whether the log's Copy control has been used since the last line landed.
    ///
    /// Cleared by every new line, because the thing on the clipboard is then no
    /// longer the thing on screen, and a control still reading "Copied" over a
    /// log that has moved on is a claim that has quietly become false.
    log_copied: bool,
    /// Whether the startup work has been submitted. The bridge cannot wake a
    /// window that does not exist yet, so the two jobs `Shell::new` used to
    /// spawn are submitted on the first frame instead — see [`Shell::update`].
    started: bool,
    /// The palette changed and the context has not been told.
    ///
    /// `theme::install` re-uploads the font definitions, so calling it on every
    /// frame would rebuild the glyph atlas sixty times a second for an answer
    /// that changes when someone clicks the chip. Starts `true` so the first
    /// frame installs it.
    theme_stale: bool,
    /// Frames drawn. Only [`shot`] reads it, to know when the layout has
    /// settled enough to be worth photographing.
    frame_no: u32,
}

impl Shell {
    /// Build the shell.
    ///
    /// Takes no context: everything here is data. The startup work waits for
    /// the first frame, which is the first moment there is a window to wake.
    pub fn new() -> Self {
        Self::with_settings(Settings::load())
    }

    /// The same state without a window, for the interaction tests.
    ///
    /// **Defaults, not `Settings::load()`.** Loading reads the settings file
    /// beside the workspace root, so a test asserting on a fold or a sort mode
    /// would pass or fail according to what the developer last did in the app —
    /// a machine-specific failure with no hint of where it came from.
    #[cfg(test)]
    fn headless() -> Self {
        Self::with_settings(Settings::default())
    }

    fn with_settings(settings: Settings) -> Self {
        let palette = Palette::named(&settings.theme);
        // **Reported, not panicked.** This runs before `WINDOW_OPENED` is set,
        // so a panic here takes the hook's other branch and tells the user
        // their machine needs working graphics drivers — for a failure to spawn
        // two worker threads, which has nothing to do with the renderer and
        // would send them to fix a graphics card that is fine.
        // `report_startup_failure` is the path every other startup failure
        // already takes, and it reaches `startup-error.log`, which is the only
        // channel that survives a double-click.
        let bridge = match crate::bridge::Bridge::new() {
            Ok(b) => b,
            Err(e) => {
                crate::report_startup_failure(&format!(
                    "TelegramExporter could not start its background worker: {e}\n\
                     Nothing that talks to Telegram can run without it. This is \
                     not a graphics problem."
                ));
                std::process::exit(1);
            }
        };
        let view = list::View {
            sort: SortMode::from_key(&settings.sort_mode),
            grouped: settings.group_by_type,
            // A name that matches no category is dropped rather than kept, so a
            // settings file from another build opens on a list that is merely
            // unfolded instead of one that will not load.
            folded: settings
                .folded_categories
                .iter()
                .filter_map(|name| Category::ALL.into_iter().find(|c| c.key() == name))
                .collect(),
            ..Default::default()
        };
        Self {
            bridge,
            palette,
            form: SettingsForm::new(&settings),
            settings,
            search: String::new(),
            signed_in: false,
            loaded: false,
            exporting: false,
            counting: false,
            cancel: Cancel::new(),
            chats: Vec::new(),
            selected: Default::default(),
            view,
            rows: Vec::new(),
            rows_stale: false,
            visible_count: 0,
            body: View::default(),
            body_pinned: false,
            status: "Not signed in".into(),
            failure: None,
            journal: Journal::default(),
            queue: Queue::default(),
            count_progress: None,
            login: None,
            log_copied: false,
            started: false,
            theme_stale: true,
            frame_no: 0,
        }
    }

    /// The work `Shell::new` used to submit, deferred to the first frame.
    ///
    /// It has to be: a sender handed out before `Bridge::wake_with` cannot
    /// repaint, and these two are the events that decide what the very first
    /// screen says.
    fn start_up(&mut self) {
        let tx = self.bridge.sender();
        // The one caller `config::lockdown_error` was written for. Until it
        // existed nothing asked, and an ACL failure on the folder holding a
        // bearer credential reached only `log::warn!`.
        //
        // **Submitted, not called.** The Windows implementation shells out to
        // `icacls`, and this is on the path of building the window — a
        // subprocess on a slow or network volume would hold the first frame
        // with nothing on screen to say why.
        self.bridge.spawn(async move {
            crate::actions::report_data_dir_protection(&tx);
            // Where to find the transcript of everything that follows. In the
            // log panel because that is where someone already is when they
            // want it, and the panel's own lines stop at the screen.
            let _ = tx.send(crate::bridge::Event::Log(format!(
                "Logging to {}",
                tgx_tg::logging::log_file().display()
            )));
        });

        // **Chats on launch, when there is a session to load them with.**
        // Nothing used to connect until the user pressed 01, so a saved
        // sign-in bought nothing on startup: the window opened onto "Not
        // signed in" and an empty list, and the two presses that fixed it were
        // the same two presses every time. A signed-in probe answers
        // `SignedIn`, which loads the list — see `apply`.
        //
        // Gated on there being something to try with, so a fresh install does
        // not open onto a failure it could have predicted. Testing for the
        // session *file* rather than reading it: whether a credential exists
        // is not the credential.
        if self.settings.api_id != 0
            && !self.settings.api_hash.is_empty()
            && tgx_tg::config::session_file().exists()
        {
            let tx = self.bridge.sender();
            let settings = self.settings.clone();
            self.bridge
                .spawn(async move { crate::actions::sign_in(settings, tx).await });
        }
    }

    /// The filter box's text became this. **One writer for the filter.**
    fn set_filter(&mut self, text: String) {
        if self.view.filter == text {
            return;
        }
        self.view.filter = text;
        // On the filter changing, and only then: a search opens what it found,
        // but the user may still fold a category while the search is running.
        // See `list::reopen_matched`.
        list::reopen_matched(&self.chats, &mut self.view);
        self.rebuild_rows();
    }

    // -- the one writer for a chat's count ---------------------------------

    /// Put a message count on a chat.
    ///
    /// **The only place that does.** Three sources reach here — the Count
    /// button, the total an export looks up for its progress bar, and the
    /// number an export actually wrote — and painting, sorting and the
    /// selection footer all read the value it sets. With more than one setter a
    /// finished export left the row showing one number and sorting on another.
    ///
    /// `None` is a legitimate argument and means *not counted*: it paints
    /// blank, sorts last, and is not zero.
    fn set_count(&mut self, chat_id: i64, count: Option<i64>) {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == chat_id) {
            chat.message_count = count;
        }
        // A count changes what "Most messages" sorts on, so the rows are stale
        // — but the rebuild waits for the end of the batch. See `update`.
        self.rows_stale = true;
    }

    fn count_of(&self, chat_id: i64) -> Option<i64> {
        self.chats
            .iter()
            .find(|c| c.id == chat_id)
            .and_then(|c| c.message_count)
    }

    /// Rebuild the rows if something has invalidated them.
    fn rebuild_rows_if_stale(&mut self) {
        if self.rows_stale {
            self.rebuild_rows();
        }
    }

    /// Recompute the painted rows. See [`PaintedRow`] for why they are cached.
    ///
    /// The visible *count* is cached with them, and for the same reason:
    /// `list::visible` sorts, and the empty state and the selection buttons
    /// both ask about it on every frame. Three sorts of the whole account per
    /// frame, sixty times a second, for a number that changes when the filter
    /// does.
    fn rebuild_rows(&mut self) {
        self.rows_stale = false;
        self.visible_count = list::visible(&self.chats, &self.view).len();
        self.rows = list::rows(&self.chats, &self.view)
            .into_iter()
            .map(|row| match row {
                list::Row::Heading {
                    category,
                    total,
                    folded,
                } => PaintedRow::Heading {
                    category,
                    total,
                    folded,
                },
                list::Row::Chat(chat) => PaintedRow::Chat(chat.clone()),
            })
            .collect();
    }

    /// The chats a filter leaves visible — what All / None / Invert act on.
    ///
    /// Sorts, so it is called from click handlers and not from painting. The
    /// two places that only need to know *how many* read the cached count.
    fn visible(&self) -> Vec<&ChatInfo> {
        list::visible(&self.chats, &self.view)
    }

    /// Show a view because the user asked for it. **Pins it.**
    pub(super) fn show(&mut self, body: View) {
        self.body = body;
        self.body_pinned = true;
    }

    /// Show a view because something happened. Yields to a hand-made choice.
    ///
    /// The one caller is the start of an export: that is the moment the answer
    /// to "what is it doing?" moves to the Run view, and leaving someone on the
    /// chat list watching nothing happen is the whole reason this exists. It
    /// steps aside afterwards, so opening Settings mid-run is not undone by the
    /// next progress event.
    pub(super) fn suggest(&mut self, body: View) {
        if !self.body_pinned {
            self.body = body;
        }
    }

    fn list_state(&self) -> tgx_ui::components::ListState {
        tgx_ui::components::ListState::decide(
            self.signed_in,
            self.loaded,
            self.chats.len(),
            self.visible_count,
        )
    }

    /// **All / None / Invert act on the *visible* rows**, so over an empty list
    /// every one is a no-op — and a button that is enabled and does nothing
    /// teaches that the interface is unreliable.
    fn selection_actions_enabled(&self) -> bool {
        self.visible_count > 0
    }

    fn selection_total(&self) -> (i64, bool) {
        let mut total = 0i64;
        let mut any_uncounted = false;
        for c in self.chats.iter().filter(|c| self.selected.contains(&c.id)) {
            match c.message_count {
                Some(n) => total += n,
                // A missing count is not a count of zero.
                None => any_uncounted = true,
            }
        }
        (total, any_uncounted)
    }
}

/// Save a frame to disk and quit, for looking at the window without a person.
///
/// **Reading the screen from outside does not work on this window.** eframe
/// draws with OpenGL, and an unoccluded GL surface goes to the display without
/// passing through the desktop compositor — so `CopyFromScreen` returns black,
/// `PrintWindow` returns white, and the one capture that ever succeeded did so
/// because a browser happened to be sitting on top of it. Three passes over this
/// design were made without anyone seeing it, and a glyph that rendered as `?`
/// in every category heading survived all three.
///
/// `TGX_SHOT=<path>` asks egui for the frame it just drew and writes it there as
/// raw RGBA, prefixed with `TGXS`, the width and the height as little-endian
/// `u32`s. Raw because a PNG encoder is a dependency this app has no other use
/// for; `tools/shot.ps1` turns it into an image.
///
/// `TGX_SHOT_VIEW=chats|settings|run` picks which view to draw first.
///
/// Nothing here runs unless the variable is set, and it is read once.
mod shot {
    use super::*;

    /// Frames to let the layout settle before asking. The first frame installs
    /// fonts and the style; asking on it captures the frame before the design.
    const SETTLE: u32 = 3;

    pub(super) fn path() -> Option<String> {
        std::env::var("TGX_SHOT").ok().filter(|s| !s.is_empty())
    }

    pub(super) fn view() -> Option<View> {
        match std::env::var("TGX_SHOT_VIEW").ok()?.as_str() {
            "chats" => Some(View::Chats),
            "settings" => Some(View::Settings),
            "run" => Some(View::Run),
            _ => None,
        }
    }

    /// Ask on the settling frame, write on the frame the answer arrives.
    pub(super) fn tick(ctx: &Context, frame_no: u32, to: &str) {
        if frame_no == SETTLE {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = shot else {
            // Keep frames coming: egui repaints on demand, and a window nobody
            // is touching would otherwise sit still and never reach `SETTLE`.
            ctx.request_repaint();
            return;
        };
        let [w, h] = [image.width() as u32, image.height() as u32];
        let mut out = Vec::with_capacity(16 + (w * h * 4) as usize);
        out.extend_from_slice(b"TGXS");
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        for px in image.pixels.iter() {
            out.extend_from_slice(&px.to_srgba_unmultiplied());
        }
        match std::fs::write(to, &out) {
            Ok(()) => log::info!("wrote {w}x{h} frame to {to}"),
            Err(e) => log::error!("could not write {to}: {e}"),
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for Shell {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // **Before anything is drained.** Senders handed out before this cannot
        // repaint, and the startup jobs below are handed one immediately.
        self.bridge.wake_with(ctx);
        if !self.started {
            self.started = true;
            self.start_up();
            if let Some(view) = shot::view() {
                self.show(view);
            }
        }
        self.frame_no = self.frame_no.saturating_add(1);
        if self.theme_stale {
            self.theme_stale = false;
            tgx_ui::theme::install(ctx, &self.palette);
        }

        // The whole batch, then one rebuild. Applying an event per frame would
        // make a list of four hundred chats arrive over four hundred frames.
        for event in self.bridge.drain() {
            self.apply(event);
        }
        self.rebuild_rows_if_stale();

        let p = self.palette;
        // Every panel in this window is unfilled but for the page colour, and
        // draws its own hairlines. One frame, declared once.
        //
        // **No backdrop.** A drifting grid was built here to the design's
        // specification and then removed: it asked for a frame continuously,
        // and it drew hairlines across every panel, because the panels are
        // unfilled by design and the grid therefore ran through the type rather
        // than behind it. Do not add it back without solving both.
        let bare = egui::Frame::NONE.fill(p.bg);

        // **An exact height, because `Ui::set_height` does not give one.** The
        // bar asked for `NAV_HEIGHT` from the inside and the panel still sized
        // itself to its contents, so five 30pt buttons sat in a 34pt strip with
        // their tops against the edge of the window.
        egui::TopBottomPanel::top("nav")
            .frame(bare)
            .exact_height(tgx_ui::tokens::metrics::NAV_HEIGHT)
            .show_separator_line(false)
            .show(ctx, |ui| self.nav_bar(ui));

        egui::TopBottomPanel::top("views")
            .frame(bare)
            .show_separator_line(false)
            .show(ctx, |ui| self.view_bar(ui));

        egui::TopBottomPanel::bottom("status")
            .frame(bare)
            .show_separator_line(false)
            .show(ctx, |ui| self.status_bar(ui));

        // **One body, full width.** There is no side column any more; see
        // [`View`] for what four permanent panels cost.
        egui::CentralPanel::default()
            .frame(bare)
            .show(ctx, |ui| match self.body {
                View::Chats => self.chat_panel(ui),
                View::Settings => self.settings_panel(ui),
                View::Run => self.run_panel(ui),
            });

        // Last, so it takes its clicks before anything under it does.
        self.login_panel(ctx);

        // After everything has been drawn, so the frame egui hands back is the
        // finished one. Does nothing at all unless `TGX_SHOT` is set.
        if let Some(to) = shot::path() {
            shot::tick(ctx, self.frame_no, &to);
        }
    }

    /// **Quitting mid-export cancels, and then waits.** See the `Drop` impl,
    /// which is where the ordering is argued; this is the hook eframe gives for
    /// the same moment, and it runs before the window is torn down rather than
    /// after.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cancel.cancel();
        self.bridge.shutdown(std::time::Duration::from_secs(10));
    }
}

/// The appearance the theme chip switches *to*.
///
/// **Anything that is not `light` is dark**, matching `Palette::named`, so an
/// edited settings file cannot leave the chip promising an appearance the
/// palette will not produce — a chip reading "LIGHT" that switches to dark is
/// worse than no chip.
fn other_theme(name: &str) -> &'static str {
    if name == "light" {
        "dark"
    } else {
        "light"
    }
}

/// **Quitting mid-export cancels, and then waits.**
///
/// Both halves matter and they are in this order for a reason. Waiting without
/// cancelling means a queue of ten chats has to run to the end or be killed at
/// the timeout. Cancelling without waiting abandons the in-flight task where it
/// sits, so `Output::close()` never runs — and because the JSON is *streamed*,
/// that leaves a `result.json` that is not truncated but **zero bytes**, the
/// writes still sitting in a buffer.
///
/// `Bridge::shutdown` is what does the waiting, and it waits on the job count
/// rather than on the tokio runtime's own timeout — see the note there for why
/// those are not the same thing. What this adds is the cancel, and the
/// ordering: both calls are made here explicitly rather than left to field drop
/// order, because the export only reaches a `close_all` if it has been told to
/// stop *before* anything starts waiting for it to.
///
/// Kept as well as `App::on_exit`, because a `Shell` dropped without eframe
/// having called that — a panic, or the headless shell in the tests — must
/// still drain.
impl Drop for Shell {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.bridge.shutdown(std::time::Duration::from_secs(10));
    }
}

#[cfg(test)]
mod tests;
