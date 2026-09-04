//! What a control does when it is pressed: sign in, refresh, count, export,
//! stop, and the settings write-back.
//!
//! **One writer per fact.** A chat's message count has three sources -- the
//! Count button, the total an export looks up, and the number an export wrote
//! -- and one setter. A finished export once left the row showing one number
//! and sorting on another.

use super::*;

impl Shell {
    // -- settings ----------------------------------------------------------

    /// Read the fields back, persist, and write the stored values back in.
    pub(super) fn commit_settings(&mut self) {
        self.form.collect(&mut self.settings);
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
        // **Written back at once**, not queued for the next frame. Under GPUI
        // this had to wait for the first moment a `&mut Window` was in hand, so
        // the shell carried a `needs_field_sync` flag and the render read it.
        // The form is plain data now, so a clamped entry is visible on the same
        // frame that clamped it.
        self.form.sync(&self.settings);
    }

    /// A checkbox changed. Same path as a text field, minus the parsing.
    pub(super) fn toggle_setting(&mut self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.settings);
        self.commit_settings();
    }

    // -- actions -----------------------------------------------------------

    /// Open the sign-in dialog, or **raise the existing one**.
    ///
    /// Making a second dialog is what put two modals on top of each other,
    /// which the user experienced as the app freezing the moment it logged
    /// them in.
    pub(super) fn open_login(&mut self) {
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
        self.login = Some(LoginDialog::new(stage, &api_id, &hash, &phone));
    }

    pub(super) fn start_sign_in(&mut self) {
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
            self.open_login();
        }
    }

    pub(super) fn start_refresh(&mut self) {
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
    pub(super) fn start_count(&mut self) {
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
    pub(super) fn begin_run(&mut self) {
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

    /// Start a run, **or add to the one already going**.
    ///
    /// One button, two questions, and which is being asked is decided here and
    /// nowhere else. The bar's third cell says which — "Start export" idle,
    /// "Add to queue" mid-run — so the branch below is not a hidden mode.
    ///
    /// **A finished run is cleared before the next one begins.** Not doing so
    /// left an hour-old row sitting above the run that was actually happening,
    /// with its own counts, its own folder and a state that read as current.
    /// Both lists go: [`Queue::start`] replaces the table, and
    /// [`Pending::reset`] replaces the worker's work list.
    pub(super) fn start_export(&mut self) {
        // Whatever is in the fields is what the run should use, so the fields
        // are read here rather than trusted to have been committed already.
        self.commit_settings();
        if !self.settings.writes_anything() {
            self.journal
                .warn("Nothing to write: pick HTML or JSON under Classic, or choose Database.");
            self.status = "No output format selected".into();
            return;
        }
        let picked: Vec<ChatInfo> = self
            .chats
            .iter()
            .filter(|c| self.selected.contains(&c.id))
            .cloned()
            .collect();
        if picked.is_empty() {
            return;
        }

        if self.exporting {
            self.add_to_run(picked);
            return;
        }

        // An export is the longer job and it claims the progress bar here.
        // While `exporting` is set, no other handler may write to it.
        self.begin_run();
        // **And it claims the window.** Pressing Start used to leave the user on
        // the chat list, where the only sign anything had happened was a
        // sentence in the status bar; everything that actually reports the run
        // was in a 400pt column on the right. A run that has just been asked for
        // is what the window should be showing. `suggest`, not `show`, so
        // looking at Settings mid-export is not undone on the next event.
        self.body_pinned = false;
        self.suggest(View::Queue);
        self.queue
            .start(picked.iter().map(|c| (c.id, c.title.clone())));
        self.pending.reset(picked.clone());
        self.status = format!("Exporting {} chats…", picked.len());
        self.journal.push(format!(
            "Starting {} export(s) into {}",
            picked.len(),
            self.settings.output_dir
        ));
        self.spawn_export();
    }

    /// Put more chats into a run that is already going.
    ///
    /// **Both lists, and only the ones the queue actually took.**
    /// [`Queue::append`] refuses a chat that is already waiting or in flight
    /// and re-queues one whose row has finished; pushing the whole selection
    /// into `pending` regardless would export the chats in flight a second
    /// time, with one row between them.
    fn add_to_run(&mut self, picked: Vec<ChatInfo>) {
        let queued = self
            .queue
            .append(picked.iter().map(|c| (c.id, c.title.clone())));
        if queued.is_empty() {
            self.status = "Already queued".into();
            return;
        }
        self.pending
            .extend(picked.into_iter().filter(|c| queued.contains(&c.id)));
        let n = queued.len();
        let chats = if n == 1 { "chat" } else { "chats" };
        self.status = format!("Added {n} {chats} to the queue");
        self.journal
            .push(format!("Added {n} {chats} to the run in progress"));
        self.log_copied = false;
    }

    /// Hand the work list to a worker.
    ///
    /// Split out because [`Shell::resume_if_more_queued`] needs the same three
    /// lines: a run whose worker has finished while chats were being added is
    /// restarted rather than left with rows nothing will ever reach.
    pub(super) fn spawn_export(&mut self) {
        let tx = self.bridge.sender();
        let settings = self.settings.clone();
        let cancel = self.cancel.clone();
        let pending = self.pending.clone();
        self.bridge
            .spawn(async move { crate::actions::export(settings, pending, cancel, tx).await });
    }

    /// Ask the run to stop, and **wait for it to say it has**.
    ///
    /// It does not clear `exporting` here. That was the whole of the old Stop:
    /// it set a flag the worker never read, cleared the progress, wrote
    /// "Stopped", and the export ran to completion writing files — then fired
    /// its own `Finished` over the top of the message. The run is over when the
    /// worker says so, and until then the interface says "Stopping".
    pub(super) fn stop(&mut self) {
        self.cancel.cancel();
        if self.exporting {
            self.status = "Stopping…".into();
        }
        if self.counting {
            self.status = "Stopping the count…".into();
        }
    }
}
