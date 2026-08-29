//! Draining the worker channel and applying what comes back.
//!
//! **A worker event repaints the window; the window never polls for one.** The
//! bridge's channel is awaited on GPUI's foreground executor and the pump
//! calls `cx.notify()`. Polling from `render` instead means nothing is seen
//! until some unrelated input causes a frame, which the user experiences as
//! having to move the mouse to make the app work.
//!
//! Events are applied in batches: a chat list of four hundred rows arrives as
//! one event, but an export emits progress steadily, and one repaint per event
//! queues frames faster than they can be drawn.

use super::*;

impl Shell {
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
    pub(super) fn start_pump(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn apply(&mut self, event: Event) {
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
}
