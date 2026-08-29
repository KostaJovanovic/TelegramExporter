//! The window: nav bar, chat list, settings, queue, log.
//!
//! The panels live in the submodules beside this one. They are child modules
//! rather than siblings on purpose — a child can see this struct's private
//! fields, so splitting the file costs no encapsulation, whereas a sibling
//! module would have forced every piece of shell state to `pub(crate)`.
//!
//! **The one rule this file exists to hold: a worker event repaints the
//! window.** `pump()` used to be called only from inside `render`, and every
//! `cx.notify()` sat in a click handler — so a chat list, a login advancing
//! from Phone to Code, or an export's progress all sat in the queue until some
//! unrelated input happened to cause a frame. Which the user experienced as
//! moving the mouse to make the app work. The bridge was never the problem; the
//! consumer was never scheduled. It is scheduled here, in [`Shell::start_pump`],
//! and nothing else in this crate may reintroduce a poll-on-render.

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
use gpui::prelude::*;
use gpui::{
    div, px, Context, Entity, ScrollHandle, SharedString, Subscription, Task,
    UniformListScrollHandle, Window,
};
use gpui_component::input::{InputEvent, InputState};
use tgx_tg::cancel::Cancel;
use tgx_tg::client::ChatInfo;
use tgx_tg::config::Settings;
use tgx_ui::components::{eyebrow, rule, vrule, NavCell};
use tgx_ui::tokens::{metrics, Palette};

/// The settings and queue column.
///
/// **Sized from the queue, not from the settings.** The queue is a five-column
/// table and the settings are a stack of rows that will fit anything; get this
/// wrong and it is the table that breaks, silently, by pushing the column that
/// names the chat off the panel. 400px against the 900px minimum leaves the
/// chat list 480, which is the proportion this figure is chosen to hold.
pub const RIGHT_COLUMN_W: gpui::Pixels = px(400.0);

/// One row as the list paints it.
///
/// Owned, not borrowed, because it is a **cache**: `uniform_list` asks for its
/// visible range on every frame, and re-filtering and re-sorting the whole
/// account inside that callback is work done sixty times a second for an answer
/// that changed once. Rebuilt by [`Shell::rebuild_rows`] when the chats, the
/// filter, the sort, the grouping or a fold changes — and nowhere else.
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
    /// The task draining the bridge into this view. Held rather than detached,
    /// so it dies with the window instead of outliving it holding a weak handle
    /// to something that is gone.
    _pump: Option<Task<()>>,
    /// Input subscriptions. A dropped `Subscription` unsubscribes, so these
    /// have to be kept for as long as the fields they watch.
    _subscriptions: Vec<Subscription>,

    palette: Palette,
    settings: Settings,
    /// The editable settings' text fields. `None` in the headless shell the
    /// interaction tests drive: an `InputState` needs a `Window` to exist.
    form: Option<SettingsForm>,
    search: Option<Entity<InputState>>,

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
    /// Whether the sort menu is open. A single value, so "is a menu open?" and
    /// "which one?" cannot disagree — the same shape as `login` below.
    sort_open: bool,

    status: SharedString,
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

    chat_scroll: UniformListScrollHandle,
    settings_scroll: ScrollHandle,
    queue_scroll: ScrollHandle,
    log_scroll: ScrollHandle,
    /// Whether the log's Copy control has been used since the last line landed.
    ///
    /// Cleared by every new line, because the thing on the clipboard is then no
    /// longer the thing on screen, and a control still reading "Copied" over a
    /// log that has moved on is a claim that has quietly become false.
    log_copied: bool,
    /// Set when a committed setting has to be written back into its field, so a
    /// clamped entry is visible. Applied at the top of the next frame, which is
    /// the first moment a `&mut Window` is in hand.
    needs_field_sync: bool,
}

impl Shell {
    /// Build the shell.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::with_settings(Settings::load());
        let form = SettingsForm::new(&this.settings, window, cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter chats…"));

        // The filter is read from the field, not typed into a mirror of it: a
        // second copy of the same string is how the empty state ends up quoting
        // something other than what is in the box.
        this._subscriptions.push(cx.subscribe_in(
            &search,
            window,
            |this, state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.view.filter = state.read(cx).value().to_string();
                    // On the filter changing, and only then: a search opens
                    // what it found, but the user may still fold a category
                    // while the search is running. See `list::reopen_matched`.
                    list::reopen_matched(&this.chats, &mut this.view);
                    this.rebuild_rows();
                    cx.notify();
                }
            },
        ));

        // Committing on blur rather than on every keystroke: a half-typed
        // number is not a decision, and writing settings.json per character
        // would save `5`, `50`, `500` on the way to `5000`.
        for state in [
            &form.output_dir,
            &form.page_size,
            &form.size_limit,
            &form.downloads,
            &form.member_limit,
        ] {
            this._subscriptions.push(cx.subscribe_in(
                state,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_settings(window, cx);
                    }
                },
            ));
        }

        this.form = Some(form);
        this.search = Some(search);
        this.start_pump(cx);
        // The one caller `config::lockdown_error` was written for. Until now
        // nothing asked, and an ACL failure on the folder holding a bearer
        // credential reached only `log::warn!`.
        //
        // **Submitted, not called.** The Windows implementation shells out to
        // `icacls`, and this is on the path of building the window — a
        // subprocess on a slow or network volume would hold the first frame
        // with nothing on screen to say why.
        let tx = this.bridge.sender();
        this.bridge.spawn(async move {
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
        if this.settings.api_id != 0
            && !this.settings.api_hash.is_empty()
            && tgx_tg::config::session_file().exists()
        {
            let tx = this.bridge.sender();
            let settings = this.settings.clone();
            this.bridge
                .spawn(async move { crate::actions::sign_in(settings, tx).await });
        }
        this
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
        // their GPU needs working DirectX drivers — for a failure to spawn two
        // worker threads, which has nothing to do with the renderer and would
        // send them to fix a graphics card that is fine. `report_startup_failure`
        // is the path every other startup failure already takes, and it reaches
        // `startup-error.log`, which is the only channel that survives a
        // double-click.
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
            _pump: None,
            _subscriptions: Vec::new(),
            palette,
            settings,
            form: None,
            search: None,
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
            sort_open: false,
            status: "Not signed in".into(),
            failure: None,
            journal: Journal::default(),
            queue: Queue::default(),
            count_progress: None,
            login: None,
            chat_scroll: UniformListScrollHandle::default(),
            settings_scroll: ScrollHandle::default(),
            queue_scroll: ScrollHandle::default(),
            log_scroll: ScrollHandle::default(),
            log_copied: false,
            needs_field_sync: false,
        }
    }

    // -- the wake-up -------------------------------------------------------

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
        // — but the rebuild waits for the end of the batch. See `start_pump`.
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

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A clamped or rejected entry is written back here, at the first moment
        // a `&mut Window` is in hand — not in the handler that clamped it.
        if self.needs_field_sync {
            self.needs_field_sync = false;
            if let Some(form) = self.form.take() {
                form.sync(&self.settings, window, cx);
                self.form = Some(form);
            }
        }

        let p = self.palette;
        let body = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(p.bg)
            .text_color(p.fg)
            // **No backdrop.** A drifting grid was built here to the design's
            // specification and then removed: it asked gpui for a frame
            // continuously, and gpui has no partial invalidation, so the whole
            // element tree was laid out again at display rate for as long as
            // the window was open. It also drew hairlines across every panel,
            // because the panels are unfilled by design and the grid therefore
            // ran through the type rather than behind it. Do not add it back
            // without solving both.
            .child(self.nav_bar(cx))
            .child(rule(&p))
            .child(
                div()
                    .flex()
                    .flex_1()
                    // Without this the row's children are free to grow past it
                    // and the panels below never scroll — they just get taller
                    // than the window.
                    .min_h_0()
                    .child(self.chat_panel(cx))
                    .child(vrule(&p))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .w(RIGHT_COLUMN_W)
                            .h_full()
                            // The right column used to declare 100% height plus
                            // 181px of children, so flex squeezed both panels
                            // and the last settings row was the first thing cut
                            // on a short window. Settings takes the slack and
                            // scrolls; the run panel keeps its measured height.
                            .child(self.settings_panel(cx))
                            .child(rule(&p))
                            .child(self.run_panel(cx)),
                    ),
            )
            .child(rule(&p))
            .child(self.status_bar());

        match self.login_panel(cx) {
            Some(dialog) => body.child(dialog),
            None => body,
        }
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
impl Drop for Shell {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.bridge.shutdown(std::time::Duration::from_secs(10));
    }
}

#[cfg(test)]
mod tests;
