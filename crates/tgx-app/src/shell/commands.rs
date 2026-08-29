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

    /// Read the fields back, persist, and queue the write-back.
    pub(super) fn commit_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn toggle_setting(
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
    pub(super) fn open_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn start_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
