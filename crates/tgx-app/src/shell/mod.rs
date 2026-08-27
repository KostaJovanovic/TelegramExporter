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
/// names the chat off the panel. The original gave its right pane 620px of an
/// 1180px window — a proportion this holds at the 900px minimum, where 400
/// leaves the chat list 480.
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

    /// Drain the bridge into this view, for the life of the window.
    ///
    /// **This is the fix for the defect the whole window was judged through.**
    /// The task awaits the channel on GPUI's foreground executor, so an event
    /// arriving on a tokio worker thread wakes it, it applies the batch, and it
    /// calls `cx.notify()` — which is what marks the window dirty. Nothing here
    /// polls and nothing waits for a frame.
    ///
    /// Events are applied in batches: a chat list of four hundred rows arrives
    /// as one event, but an export emits progress steadily, and one repaint per
    /// event would queue frames faster than they can be drawn.
    fn start_pump(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.bridge.take_events() else {
            return;
        };
        self._pump = Some(cx.spawn(async move |this, cx| {
            while let Some(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(next) = rx.try_recv() {
                    batch.push(next);
                }
                let applied = this.update(cx, |this, cx| {
                    for event in batch {
                        this.apply(event);
                    }
                    // Once per batch, not once per event. Counting a large
                    // account emits one `Counted` per chat, and re-sorting the
                    // whole list on each of them is quadratic work for a
                    // picture nobody sees until the batch is done.
                    this.rebuild_rows_if_stale();
                    cx.notify();
                });
                if applied.is_err() {
                    // The window is gone. Nothing left to notify.
                    break;
                }
            }
        }));
    }

    /// Fold one event into the view.
    ///
    /// **`exporting` decides who may write to the progress bar.** An export is
    /// the longer job and it claims the bar; while it is set, a counting
    /// handler returns without touching it — otherwise a count finishing
    /// mid-export paints "Counted 12 of 12 chats" over "6,000 of 6,643".
    fn apply(&mut self, event: Event) {
        match event {
            Event::Status(s) => self.status = s.into(),
            // A new line means what was copied is no longer what is on screen,
            // so the control stops claiming otherwise.
            Event::Log(s) => {
                self.journal.push(s);
                self.log_copied = false;
            }
            Event::Warn(s) => {
                self.journal.warn(s);
                self.log_copied = false;
            }
            Event::SignedIn(name) => {
                self.signed_in = true;
                // **The list, without being asked twice.** Signing in and then
                // pressing 02 was two steps where the second had exactly one
                // possible answer: 02 is the only thing a freshly signed-in
                // window can do, and it was being demanded rather than done.
                // Safe to fire from here because the connection is already up
                // and authorised — that is what this event means.
                self.start_refresh();
                // Success just closes the dialog: the status bar already reads
                // "Signed in: <name>", and a modal box saying it again is
                // exactly what could end up ordered behind the window.
                self.login = None;
                self.status = format!("Signed in: {name}").into();
            }
            Event::LoginStage(stage) => {
                if let Some(d) = self.login.as_mut() {
                    d.busy = false;
                    d.stage = stage;
                }
            }
            Event::LoginFailed(msg) => {
                if let Some(d) = self.login.as_mut() {
                    d.busy = false;
                    d.copied = false;
                    d.error = Some(msg.into());
                }
            }
            Event::Chats(mut chats) => {
                self.loaded = true;
                let n = chats.len();
                // **A refresh must not throw away the counts.** The dialog
                // list carries no message totals — `dialogs.rs` hard-codes
                // `None` — so replacing the list wholesale silently undid a
                // Count that had just spent one request per chat and several
                // minutes to get them. Carried across by id, like the ticks.
                let known: std::collections::HashMap<i64, i64> = self
                    .chats
                    .iter()
                    .filter_map(|c| c.message_count.map(|n| (c.id, n)))
                    .collect();
                for chat in &mut chats {
                    if chat.message_count.is_none() {
                        chat.message_count = known.get(&chat.id).copied();
                    }
                }
                self.chats = chats;
                // Ticks are held by id, so a refresh keeps any selection whose
                // chat is still there and silently drops the rest.
                let live: std::collections::HashSet<i64> =
                    self.chats.iter().map(|c| c.id).collect();
                self.selected.retain(|id| live.contains(id));
                self.rebuild_rows();
                self.status = format!("{n} chats").into();
            }

            Event::Counted { chat_id, total } => self.set_count(chat_id, total),
            Event::CountProgress { done, total } => {
                if !self.exporting {
                    self.count_progress = Some((done, total));
                    self.status = format!("Counting {done} of {total} chats").into();
                }
            }
            Event::CountFinished { counted, failed } => {
                self.counting = false;
                self.count_progress = None;
                if !self.exporting {
                    let mut msg = format!("Counted {counted} chats");
                    if failed > 0 {
                        msg.push_str(&format!(", {failed} could not be counted"));
                    }
                    self.status = msg.into();
                }
                // A new count can reorder the list under "Most messages".
                self.rebuild_rows();
            }

            Event::ChatStarted { chat_id, title } => {
                self.queue.began(chat_id);
                self.status = format!("Exporting {title}").into();
            }
            Event::ChatTotal { chat_id, total } => {
                self.queue.set_expected(chat_id, total);
                // An export of a chat nobody counted looks the number up
                // anyway. Writing it to the list is free, and without it the
                // row reads blank beside a progress bar that knows the answer.
                if self.count_of(chat_id).is_none() {
                    self.set_count(chat_id, Some(total));
                }
            }
            Event::ChatTopics { chat_id, topics } => self.queue.set_topics(chat_id, topics),
            Event::Progress {
                chat_id,
                done,
                total,
            } => {
                self.queue.progressed(chat_id, done, total);
                // **Put the status line back.** A rate-limit wait writes over
                // it, and nothing else during an export writes to it at all —
                // so one 60-second wait halfway through a chat left the bar
                // reading "Rate limited, waiting 60s" for the rest of that
                // chat while the progress advanced beside it.
                if let Some(title) = self.queue.title_of(chat_id) {
                    self.status = format!("Exporting {title}").into();
                }
            }
            Event::ChatDone {
                chat_id,
                messages,
                expected,
                topics,
                media_downloaded,
                media_failed,
                root,
            } => {
                self.queue.finished(
                    chat_id,
                    messages,
                    expected,
                    topics,
                    media_downloaded,
                    media_failed,
                    root,
                );
                // What the export actually wrote is a **measured** number, so
                // it replaces whatever the list was carrying.
                self.set_count(chat_id, Some(messages as i64));
            }
            Event::ChatFailed { chat_id, message } => {
                // **A cancelled or failed export writes no count at all**: a
                // truncated run must not leave its own length behind as the
                // size of the chat.
                self.queue.failed(chat_id, message.clone());
                // Prefixed with the chat, like every other line in the
                // transcript. Unprefixed, "no messages could be read" in a
                // twenty-chat queue named none of them — and the queue table
                // that does know is a different panel, sorted differently.
                let line = match self.queue.title_of(chat_id) {
                    Some(title) => format!("{title}: {message}"),
                    None => message,
                };
                self.journal.warn(line);
            }

            Event::FloodWait(seconds) => {
                self.status = format!("Rate limited, waiting {seconds}s").into();
            }
            Event::Finished { stopped } => {
                self.exporting = false;
                if stopped {
                    self.queue.stop_remaining();
                }
                // The queue is the one writer of what the run did; a worker
                // that composed its own sentence was the second, and the two
                // overwrote each other.
                //
                // **A stated cause outranks a tally.** `Failed` and `Finished`
                // arrive in the same batch when a run cannot start at all —
                // point the destination at a disconnected drive and the queue's
                // "Exported 0 of 3 chats, 3 not run" would land on top of the
                // one sentence that says *why*, leaving the reason only in the
                // log, which is not where the user is looking.
                self.journal.push(self.queue.summary());
                self.status = match self.failure.take() {
                    Some(why) => format!("{}: {why}", self.queue.summary()).into(),
                    None => self.queue.summary().into(),
                };
                self.rebuild_rows();
            }
            // **Only the activity that failed is switched off.** This used to
            // clear `exporting` and `counting` on every `Failed`, whoever sent
            // it — five senders share the variant — so a sign-in probe that
            // could not reach Telegram while an export was running made the
            // window believe the export had stopped: the Stop button vanished,
            // the progress bar froze, and the export went on writing files
            // until its own `Finished` put the state back.
            //
            // `failure` is likewise the export's, because the only reader is
            // the export's own summary line.
            Event::Failed { activity, message } => {
                match activity {
                    Activity::Export => {
                        self.exporting = false;
                        self.failure = Some(message.clone());
                    }
                    Activity::Count => self.counting = false,
                    Activity::SignIn | Activity::Chats => {}
                }
                self.status = format!("Failed: {message}").into();
                self.journal.warn(message);
            }
        }
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

    // -- settings ----------------------------------------------------------

    /// Read the fields back, persist, and queue the write-back.
    fn commit_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.form else { return };
        form.collect(&mut self.settings, cx);
        self.settings.sort_mode = self.view.sort.key().into();
        self.settings.group_by_type = self.view.grouped;
        // Stored in the fixed category order rather than in iteration order, so
        // two runs that folded the same categories write the same file and a
        // diff of settings.json means something changed.
        self.settings.folded_categories = Category::ALL
            .into_iter()
            .filter(|c| self.view.folded.contains(c))
            .map(|c| c.key().to_string())
            .collect();
        if let Err(e) = self.settings.save() {
            self.journal.warn(format!("could not save settings: {e}"));
        }
        self.needs_field_sync = true;
        cx.notify();
    }

    /// A checkbox changed. Same path as a text field, minus the parsing.
    fn toggle_setting(
        &mut self,
        f: impl FnOnce(&mut Settings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        f(&mut self.settings);
        self.commit_settings(window, cx);
    }

    // -- actions -----------------------------------------------------------

    /// Open the sign-in dialog, or **raise the existing one**.
    ///
    /// Making a second dialog is what put two modals on top of each other,
    /// which the user experienced as the app freezing the moment it logged
    /// them in.
    fn open_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.login.is_some() {
            return;
        }
        let stage = if self.settings.api_id == 0 || self.settings.api_hash.is_empty() {
            Stage::Credentials
        } else {
            Stage::Phone
        };
        let api_id = if self.settings.api_id == 0 {
            String::new()
        } else {
            self.settings.api_id.to_string()
        };
        let hash = self.settings.api_hash.clone();
        let phone = self.settings.phone.clone();
        self.login = Some(LoginDialog::new(stage, window, cx, &api_id, &hash, &phone));
    }

    fn start_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Probe the session on disk, and open the dialog without waiting for
        // the answer. A signed-in account answers `SignedIn`, which closes the
        // dialog again — so the cost of already being signed in is that the
        // dialog appears for as long as the connect takes, and the cost of
        // waiting instead would be a button that does nothing visible while a
        // network round trip happens. The first is the better trade, but it is
        // a trade: this does *not* skip the dialog for a good session.
        let tx = self.bridge.sender();
        let settings = self.settings.clone();
        self.bridge
            .spawn(async move { crate::actions::sign_in(settings, tx).await });
        if !self.signed_in {
            self.open_login(window, cx);
        }
    }

    fn start_refresh(&mut self) {
        let tx = self.bridge.sender();
        let settings = self.settings.clone();
        self.bridge
            .spawn(async move { crate::actions::refresh_chats(settings, tx).await });
    }

    /// Count every chat, or stop a count already running.
    ///
    /// The button has to be able to undo itself: a count can sit in a
    /// two-minute rate-limit wait, and with the button merely disabled that is
    /// indistinguishable from a hang.
    fn start_count(&mut self) {
        if self.counting {
            self.cancel.cancel();
            self.status = "Stopping the count…".into();
            return;
        }
        if self.chats.is_empty() {
            return;
        }
        self.cancel.reset();
        self.counting = true;
        let ids: Vec<i64> = self.chats.iter().map(|c| c.id).collect();
        self.count_progress = Some((0, ids.len()));
        let tx = self.bridge.sender();
        let settings = self.settings.clone();
        let cancel = self.cancel.clone();
        self.bridge
            .spawn(async move { crate::actions::count_chats(settings, ids, cancel, tx).await });
    }

    /// The per-run state a new export resets.
    ///
    /// Split out of [`Self::start_export`] because that needs a `Window` and
    /// this does not, so the reset can be tested — which matters most for
    /// `failure`, whose whole failure mode is being *left set*.
    fn begin_run(&mut self) {
        // An export is the longer job and it claims the progress bar here.
        // While `exporting` is set, no other handler may write to it.
        self.cancel.reset();
        self.exporting = true;
        // **Cleared on the way in, not on the way out.** `failure` is appended
        // to the run's summary, so a cause left over from an earlier run — a
        // sign-in that could not reach Telegram, a destination that was
        // unwritable last time — was reported as the reason *this* run ended.
        self.failure = None;
        self.count_progress = None;
    }

    fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Whatever is in the fields is what the run should use, so the fields
        // are read here rather than trusted to have been committed already.
        self.commit_settings(window, cx);
        if !(self.settings.export_html || self.settings.export_json) {
            self.journal
                .warn("Nothing to write: enable HTML, JSON or both under Format.");
            self.status = "No output format selected".into();
            return;
        }
        let queue: Vec<ChatInfo> = self
            .chats
            .iter()
            .filter(|c| self.selected.contains(&c.id))
            .cloned()
            .collect();
        if queue.is_empty() {
            return;
        }

        // An export is the longer job and it claims the progress bar here.
        // While `exporting` is set, no other handler may write to it.
        self.begin_run();
        self.queue
            .start(queue.iter().map(|c| (c.id, c.title.clone())));
        self.status = format!("Exporting {} chats…", queue.len()).into();
        self.journal.push(format!(
            "Starting {} export(s) into {}",
            queue.len(),
            self.settings.output_dir
        ));

        let tx = self.bridge.sender();
        let settings = self.settings.clone();
        let cancel = self.cancel.clone();
        self.bridge
            .spawn(async move { crate::actions::export(settings, queue, cancel, tx).await });
    }

    /// Ask the run to stop, and **wait for it to say it has**.
    ///
    /// It does not clear `exporting` here. That was the whole of the old Stop:
    /// it set a flag the worker never read, cleared the progress, wrote
    /// "Stopped", and the export ran to completion writing files — then fired
    /// its own `Finished` over the top of the message. The run is over when the
    /// worker says so, and until then the interface says "Stopping".
    fn stop(&mut self) {
        self.cancel.cancel();
        if self.exporting {
            self.status = "Stopping…".into();
        }
        if self.counting {
            self.status = "Stopping the count…".into();
        }
    }

    // -- painting ----------------------------------------------------------
    // (see the panel modules beside this one)

    fn nav_bar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.palette;
        // The bar numbers the steps and only the steps. Stop and Open output
        // folder are tools: unnumbered, and pushed right where the sequence
        // has ended.
        let busy = self.exporting || self.counting;
        let steps = [
            // Disabled while the dialog is up: a second click would spawn a
            // second `Session::connect` behind the one already running, and
            // `open_login` would raise the existing dialog while the extra
            // connection carried on in the background.
            NavCell::step(1, "Sign in").enabled(!busy && self.login.is_none()),
            NavCell::step(2, "Refresh chats").enabled(self.signed_in && !busy),
            NavCell::step(3, "Start export")
                .enabled(self.signed_in && !self.selected.is_empty() && !busy),
        ];
        let tools = [
            NavCell::tool("Stop").enabled(busy),
            NavCell::tool("Open output folder"),
        ];

        let mut bar = div()
            .flex()
            .items_center()
            .w_full()
            .flex_none()
            .h(metrics::NAV_HEIGHT)
            .bg(p.bg);

        for (i, cell) in steps.iter().enumerate() {
            let live = cell.enabled;
            let mut slot = div().id(("step", i)).px(px(16.0)).child(cell.render(p));
            if live {
                slot = slot
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        match i {
                            0 => this.start_sign_in(window, cx),
                            1 => this.start_refresh(),
                            2 => this.start_export(window, cx),
                            _ => {}
                        }
                        cx.notify();
                    }));
            }
            bar = bar.child(slot).child(vrule(p));
        }

        bar = bar.child(div().flex_1());

        for (i, cell) in tools.iter().enumerate() {
            let live = cell.enabled;
            let mut slot = div().id(("tool", i)).px(px(16.0)).child(cell.render(p));
            if live {
                slot = slot
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match i {
                            0 => this.stop(),
                            // Reported, not discarded. A silent spawn failure
                            // is what let `explorer.exe` be looked for in
                            // System32 — where it is not — for as long as
                            // nobody happened to watch the button do nothing.
                            1 => {
                                if let Err(e) =
                                    crate::actions::open_output_folder(&this.settings.output_dir)
                                {
                                    this.journal.warn(e);
                                    this.log_copied = false;
                                }
                            }
                            _ => {}
                        }
                        cx.notify();
                    }));
            }
            bar = bar.child(vrule(p)).child(slot);
        }

        // **The chip names the theme it switches *to***, like the site's, so it
        // reads as an action rather than as a label for where you already are.
        bar.child(vrule(p)).child(
            div()
                .id("theme")
                .px(px(16.0))
                .cursor_pointer()
                .child(tgx_ui::components::caps(
                    other_theme(&self.settings.theme),
                    tgx_ui::tokens::type_scale::MICRO,
                    p.muted,
                ))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_theme(window, cx))),
        )
    }

    /// Swap the appearance.
    ///
    /// **A token swap, not a rebuild.** Every colour in this window comes from
    /// the palette, so re-reading it repaints the lot; the borrowed components
    /// need one extra call because they paint from `gpui-component`'s own
    /// global rather than from ours — see [`crate::theme`].
    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.theme = other_theme(&self.settings.theme).into();
        self.palette = Palette::named(&self.settings.theme);
        crate::theme::apply(&self.palette, cx);
        self.commit_settings(window, cx);
    }

    fn status_bar(&self) -> gpui::Div {
        let p = &self.palette;
        // Counting reports in chats and an export in messages, and the export
        // owns the line whenever it is running.
        let right = if self.exporting {
            self.queue
                .fraction()
                .map(|f| format!("{:.0}%", f * 100.0))
                .unwrap_or_default()
        } else {
            match self.count_progress {
                Some((done, total)) => format!("{done} of {total}"),
                None => String::new(),
            }
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .w_full()
            .px(px(16.0))
            .py(px(10.0))
            .child(
                div()
                    .flex_1()
                    // A long status must push nothing off the bar; a flex child
                    // is allowed to shrink below its content only with this.
                    .min_w_0()
                    // **Not letterspaced, deliberately.** `eyebrow` goes
                    // through `components::tracked`, which lays out one `div`
                    // per character with `flex_none` children — its own doc
                    // says never to put a string of unknown length through it,
                    // and this line carries chat titles and raw RPC errors. It
                    // would overflow the bar rather than truncate, and rebuild
                    // a div per character on every progress event.
                    .truncate()
                    .text_size(tgx_ui::tokens::type_scale::MICRO)
                    .text_color(p.muted)
                    .child(tgx_ui::components::uppercase(self.status.clone())),
            )
            // The right-hand figure is ours and short, so it can be tracked.
            .child(eyebrow(right, p))
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
