//! The export queue, as the panel titled QUEUE actually shows it.
//!
//! It used to render the log. The two are not interchangeable: the log is a
//! transcript, oldest at the top and unbounded, while the queue is a fixed set
//! of rows — one per chat the run was started with — each of which changes
//! state in place. Showing the log under the queue's heading meant the empty
//! state read "Nothing queued" when what was empty was the log, and the queue
//! `start_export` built was never on screen at all.
//!
//! **A row is created for every chat the run starts with, before any of them
//! runs.** That is what makes the panel answer "what is this run going to do?"
//! rather than only "what has it done?".

use std::path::PathBuf;

/// Where one chat's job has got to.
///
/// `Stopped` is deliberately distinct from `Failed`. A chat abandoned because
/// the user pressed Stop has nothing wrong with it, and reporting it as a
/// failure is how someone concludes the export is broken; a chat that genuinely
/// refused needs its reason kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Exporting,
    Done,
    Failed(String),
    Stopped,
}

impl JobState {
    /// The word the STATUS column shows.
    pub fn label(&self) -> &str {
        match self {
            JobState::Queued => "Queued",
            JobState::Exporting => "Exporting",
            JobState::Done => "Done",
            JobState::Stopped => "Stopped",
            JobState::Failed(why) => why,
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::Stopped | JobState::Failed(_)
        )
    }
}

/// One row of the queue.
#[derive(Debug, Clone)]
pub struct Job {
    pub chat_id: i64,
    pub title: String,
    pub state: JobState,
    /// Messages written so far. Counted up as the export reports progress.
    pub messages: usize,
    /// What Telegram said the chat held. `None` until the export looks it up —
    /// and **not** zero, which would make an unknown total read as an empty
    /// chat and a progress bar sit at 100% from the first frame.
    pub expected: Option<i64>,
    /// Topic folders this chat produces. `None` for a chat that is not a forum,
    /// which is why the column paints an em dash rather than `0`.
    pub topics: Option<usize>,
    /// `(downloaded, failed)`, once the media pass has run.
    pub media: Option<(usize, usize)>,
    /// Where it was written, so a finished row can be opened.
    pub root: Option<PathBuf>,
}

impl Job {
    fn new(chat_id: i64, title: String) -> Self {
        Self {
            chat_id,
            title,
            state: JobState::Queued,
            messages: 0,
            expected: None,
            topics: None,
            media: None,
            root: None,
        }
    }

    /// The MEDIA column's text. Empty until there is something to say — a
    /// column reading `0/0` before the media pass claims a fact it does not
    /// have.
    pub fn media_text(&self) -> String {
        match self.media {
            None => String::new(),
            Some((done, 0)) => format!("{done}"),
            Some((done, failed)) => format!("{done} ({failed} failed)"),
        }
    }

    /// The TOPICS column's text. **An em dash, not `0`**: a private chat has no
    /// topics, which is not the same as a forum that produced none.
    pub fn topics_text(&self) -> String {
        match self.topics {
            None => "\u{2014}".into(),
            Some(n) => n.to_string(),
        }
    }
}

/// Every job of the current run, in queue order.
#[derive(Debug, Default)]
pub struct Queue {
    jobs: Vec<Job>,
}

impl Queue {
    /// Start a run. Replaces the previous one wholesale — the panel shows *this*
    /// run, not a history, and mixing the two is how a row from an hour ago is
    /// read as part of what is happening now.
    pub fn start(&mut self, chats: impl IntoIterator<Item = (i64, String)>) {
        self.jobs = chats
            .into_iter()
            .map(|(id, title)| Job::new(id, title))
            .collect();
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    fn job_mut(&mut self, chat_id: i64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.chat_id == chat_id)
    }

    pub fn began(&mut self, chat_id: i64) {
        if let Some(job) = self.job_mut(chat_id) {
            job.state = JobState::Exporting;
        }
    }

    pub fn set_expected(&mut self, chat_id: i64, total: i64) {
        if let Some(job) = self.job_mut(chat_id) {
            job.expected = Some(total);
        }
    }

    pub fn set_topics(&mut self, chat_id: i64, topics: usize) {
        if let Some(job) = self.job_mut(chat_id) {
            job.topics = Some(topics);
        }
    }

    pub fn progressed(&mut self, chat_id: i64, done: usize, total: i64) {
        if let Some(job) = self.job_mut(chat_id) {
            job.state = JobState::Exporting;
            job.messages = done;
            if total > 0 {
                job.expected = Some(total);
            }
        }
    }

    /// `topics` is `None` for a chat that has no topics at all — see
    /// [`Job::topics`]. Passing `Some(1)` for an unsplit chat is what turned
    /// every private chat's em dash into a `1`.
    #[allow(clippy::too_many_arguments)]
    pub fn finished(
        &mut self,
        chat_id: i64,
        messages: usize,
        expected: i64,
        topics: Option<usize>,
        media_downloaded: usize,
        media_failed: usize,
        root: PathBuf,
    ) {
        if let Some(job) = self.job_mut(chat_id) {
            job.state = JobState::Done;
            job.messages = messages;
            job.expected = Some(expected);
            // Only overwrite with a real answer: a chat whose topics were
            // counted at the start must not lose them to a `None` here.
            if topics.is_some() {
                job.topics = topics;
            }
            job.media = Some((media_downloaded, media_failed));
            job.root = Some(root);
        }
    }

    /// The title of a queued chat, for the status line.
    pub fn title_of(&self, chat_id: i64) -> Option<&str> {
        self.jobs
            .iter()
            .find(|j| j.chat_id == chat_id)
            .map(|j| j.title.as_str())
    }

    /// What the bar is doing, said in the present tense.
    ///
    /// [`summary`](Self::summary) is a sentence about a **finished** run, and
    /// captioning a bar that has just started with "Exported 0 of 3 chats" is
    /// a report on something that has not happened.
    pub fn running_caption(&self) -> String {
        let done = self.jobs.iter().filter(|j| j.state.is_finished()).count();
        match self.jobs.iter().find(|j| j.state == JobState::Exporting) {
            Some(job) => match job.expected {
                Some(total) if total > 0 => {
                    format!("{} — {} of {} messages", job.title, job.messages, total)
                }
                _ => format!("{} — {} messages", job.title, job.messages),
            },
            None => format!("{done} of {} chats", self.jobs.len()),
        }
    }

    pub fn failed(&mut self, chat_id: i64, why: String) {
        if let Some(job) = self.job_mut(chat_id) {
            job.state = JobState::Failed(why);
        }
    }

    /// Mark whatever never ran as stopped, once a run has ended early.
    ///
    /// Without this a queue of ten chats interrupted after three leaves seven
    /// rows reading "Queued" for the rest of the session, which says a run is
    /// still coming that never will be.
    pub fn stop_remaining(&mut self) {
        for job in &mut self.jobs {
            if !job.state.is_finished() {
                job.state = JobState::Stopped;
            }
        }
    }

    /// How far the whole run has got, as a fraction, or `None` while that is
    /// genuinely unknown.
    ///
    /// **`None` is not zero.** A bar painted at 0% says "started and got
    /// nowhere"; a run whose size nobody knows yet has to say so differently or
    /// it reads as stuck from the first frame.
    ///
    /// Measured in *chats*, not messages, because the totals arrive one chat at
    /// a time: a bar scaled to the messages of the only chat counted so far
    /// runs to the end and then jumps backwards when the second total lands.
    pub fn fraction(&self) -> Option<f32> {
        if self.jobs.is_empty() {
            return None;
        }
        let finished = self.jobs.iter().filter(|j| j.state.is_finished()).count();
        // The chat in flight contributes its own share, so a single long chat
        // still moves the bar rather than sitting at 0 until it is done.
        let running = self
            .jobs
            .iter()
            .find(|j| j.state == JobState::Exporting)
            .and_then(|j| match j.expected {
                Some(total) if total > 0 => {
                    Some((j.messages as f32 / total as f32).clamp(0.0, 1.0))
                }
                _ => None,
            })
            .unwrap_or(0.0);
        Some(((finished as f32 + running) / self.jobs.len() as f32).clamp(0.0, 1.0))
    }

    /// One line summarising the run, for the status bar.
    pub fn summary(&self) -> String {
        let done = self
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Done)
            .count();
        let failed = self
            .jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::Failed(_)))
            .count();
        let stopped = self
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Stopped)
            .count();
        let mut out = format!("Exported {done} of {} chats", self.jobs.len());
        if failed > 0 {
            out.push_str(&format!(", {failed} failed"));
        }
        if stopped > 0 {
            out.push_str(&format!(", {stopped} not run"));
        }
        out
    }

    /// Where a finished job wrote its export, if it finished.
    pub fn root_of(&self, chat_id: i64) -> Option<&PathBuf> {
        self.jobs
            .iter()
            .find(|j| j.chat_id == chat_id)
            .and_then(|j| j.root.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_of(n: i64) -> Queue {
        let mut q = Queue::default();
        q.start((1..=n).map(|i| (i, format!("chat {i}"))));
        q
    }

    #[test]
    fn a_run_gets_a_row_per_chat_before_any_of_them_runs() {
        // The panel has to answer "what is this run going to do?", not only
        // "what has it done?".
        let q = queue_of(3);
        assert_eq!(q.len(), 3);
        assert!(q.jobs().iter().all(|j| j.state == JobState::Queued));
    }

    #[test]
    fn an_empty_queue_has_no_fraction_rather_than_zero() {
        // Zero percent says "started and got nowhere". Nothing has started.
        assert_eq!(Queue::default().fraction(), None);
    }

    #[test]
    fn a_running_chat_moves_the_bar_before_it_finishes() {
        // A queue of one long chat that only moved the bar on completion sat
        // at 0% for the whole export and read as hung.
        let mut q = queue_of(1);
        q.began(1);
        assert_eq!(q.fraction(), Some(0.0));
        q.progressed(1, 3000, 6000);
        assert_eq!(q.fraction(), Some(0.5));
    }

    #[test]
    fn a_chat_with_no_known_total_does_not_claim_progress() {
        let mut q = queue_of(2);
        q.began(1);
        q.progressed(1, 500, 0);
        // One of two chats, neither finished: still nothing to claim.
        assert_eq!(q.fraction(), Some(0.0));
        assert_eq!(q.jobs()[0].expected, None, "0 is not a total");
    }

    #[test]
    fn stopping_a_run_does_not_leave_rows_reading_queued() {
        // Seven rows saying "Queued" for the rest of the session promise a run
        // that is never coming.
        let mut q = queue_of(3);
        q.began(1);
        q.finished(1, 10, 10, None, 0, 0, PathBuf::from("a"));
        q.stop_remaining();
        assert_eq!(q.jobs()[0].state, JobState::Done);
        assert_eq!(q.jobs()[1].state, JobState::Stopped);
        assert_eq!(q.jobs()[2].state, JobState::Stopped);
    }

    #[test]
    fn a_stopped_chat_is_not_reported_as_a_failure() {
        // Nothing is wrong with a chat the user chose not to export.
        let mut q = queue_of(2);
        q.failed(1, "CHAT_ADMIN_REQUIRED".into());
        q.stop_remaining();
        assert_eq!(q.jobs()[0].state.label(), "CHAT_ADMIN_REQUIRED");
        assert_eq!(q.jobs()[1].state.label(), "Stopped");
        assert_eq!(q.summary(), "Exported 0 of 2 chats, 1 failed, 1 not run");
    }

    #[test]
    fn a_chat_without_topics_paints_a_dash_not_zero() {
        // A private chat has no topics; a forum that produced none has zero.
        let mut q = queue_of(2);
        assert_eq!(q.jobs()[0].topics_text(), "\u{2014}");
        q.set_topics(1, 0);
        assert_eq!(q.jobs()[0].topics_text(), "0");
    }

    #[test]
    fn the_media_column_says_nothing_until_the_media_pass_has_run() {
        let mut q = queue_of(1);
        assert_eq!(q.jobs()[0].media_text(), "");
        q.finished(1, 5, 5, Some(1), 12, 2, PathBuf::from("a"));
        assert_eq!(q.jobs()[0].media_text(), "12 (2 failed)");
    }

    #[test]
    fn a_new_run_replaces_the_previous_one_rather_than_appending() {
        // A row from an hour ago read as part of what is happening now.
        let mut q = queue_of(3);
        q.finished(1, 1, 1, None, 0, 0, PathBuf::from("a"));
        q.start([(9, "later".to_string())]);
        assert_eq!(q.len(), 1);
        assert_eq!(q.jobs()[0].state, JobState::Queued);
        assert_eq!(q.root_of(1), None);
    }
}
