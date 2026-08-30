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

/// The settings and queue column, as it opens.
///
/// **Sized from the queue, not from the settings.** The queue is a five-column
/// table and the settings are a stack of rows that will fit anything; get this
/// wrong and it is the table that breaks by pushing the column that names the
/// chat off the panel. 400px against the 900px minimum leaves the chat list 480,
/// which is the proportion this figure is chosen to hold — and it is now a
/// default rather than a fixed width, because the panel is draggable and the
/// table defends its own columns.
pub const RIGHT_COLUMN_W: f32 = 400.0;

/// How much of that column the run panel takes, as it opens.
///
/// Queue, bar and log against five sections of settings. Also a default: the
/// split is draggable, because which half matters depends on whether an export
/// is running, and that changes several times in a session.
const RUN_PANEL_H: f32 = 340.0;

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
            status: "Not signed in".into(),
            failure: None,
            journal: Journal::default(),
            queue: Queue::default(),
            count_progress: None,
            login: None,
            log_copied: false,
            started: false,
            theme_stale: true,
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

impl eframe::App for Shell {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // **Before anything is drained.** Senders handed out before this cannot
        // repaint, and the startup jobs below are handed one immediately.
        self.bridge.wake_with(ctx);
        if !self.started {
            self.started = true;
            self.start_up();
        }
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

        egui::TopBottomPanel::top("nav")
            .frame(bare)
            .show_separator_line(false)
            .show(ctx, |ui| self.nav_bar(ui));

        egui::TopBottomPanel::bottom("status")
            .frame(bare)
            .show_separator_line(false)
            .show(ctx, |ui| self.status_bar(ui));

        egui::SidePanel::right("right")
            .frame(bare)
            .default_width(RIGHT_COLUMN_W)
            // **Draggable, within a range the queue table can survive.** It was
            // pinned to exactly 400 because the GPUI column was, and 400 is a
            // reasonable default rather than the only width that works: the
            // table's own floor is what decides that, and `egui_extras` enforces
            // it per column now instead of a comment asking the next reader to
            // re-check a sum. The floor here is the width below which the Chat
            // column is all that is left of the table.
            .width_range(340.0..=560.0)
            .resizable(true)
            .show_separator_line(false)
            .show(ctx, |ui| {
                // Settings takes the slack and scrolls; the run panel is
                // measured off the bottom and draggable. The GPUI version
                // declared 100% height plus 181px of children, so flex squeezed
                // both panels and the last settings row was the first thing cut
                // on a short window.
                egui::TopBottomPanel::bottom("run")
                    .frame(bare)
                    .default_height(RUN_PANEL_H)
                    .height_range(200.0..=560.0)
                    .resizable(true)
                    .show_separator_line(false)
                    .show_inside(ui, |ui| self.run_panel(ui));
                self.settings_panel(ui);
            });

        egui::CentralPanel::default()
            .frame(bare)
            .show(ctx, |ui| self.chat_panel(ui));

        // Last, so it takes its clicks before anything under it does.
        self.login_panel(ctx);
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
