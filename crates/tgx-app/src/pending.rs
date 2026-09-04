//! The work list an export pulls from, shared with the window.
//!
//! **A run used to be a fixed `Vec` handed to the worker at spawn time**, which
//! meant the queue could not be changed once it had started: the only way to add
//! a chat was to stop the run and start it again, losing whatever was in flight.
//! The list lives here instead, and both ends hold it — the window pushes and
//! removes, the worker pops one at a time.
//!
//! **The window owns the lifecycle, the worker only drains.** [`Pending::reset`]
//! is called from `Shell::start_export` and nowhere else, and only when no run is
//! going; the worker never clears the list, because a worker that tidied up after
//! itself would race the window's next `reset`. The one rule that keeps this
//! honest: nothing here is emptied while `exporting` is set.
//!
//! There is no channel because a channel cannot answer the two questions the
//! queue panel asks of it — *is this chat still coming?* and *take that one back
//! out* — and a cancel button that could only mark a row as removed while the
//! worker exported it anyway is not a cancel button.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tgx_tg::client::ChatInfo;

/// The chats a run has still to reach, oldest first.
///
/// **`Clone` shares the list, it does not copy it** — the same property
/// `tgx_tg::cancel::Cancel` is built on, and for the same reason: the window
/// keeps one handle and the worker takes another, and a clone with its own
/// `VecDeque` would leave every removal doing nothing at all.
#[derive(Debug, Clone, Default)]
pub struct Pending(Arc<Mutex<VecDeque<ChatInfo>>>);

impl Pending {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the lock, recovering from a poisoned one.
    ///
    /// A panic on the worker thread must not turn every later click on the
    /// queue panel into a second panic on the UI thread. What is behind the
    /// lock is a list of chat titles; there is no invariant a half-finished
    /// write could have broken.
    fn list(&self) -> std::sync::MutexGuard<'_, VecDeque<ChatInfo>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Start a run: replace the list wholesale.
    ///
    /// Called only when nothing is exporting. See the module docs.
    pub fn reset(&self, chats: impl IntoIterator<Item = ChatInfo>) {
        let mut list = self.list();
        list.clear();
        list.extend(chats);
    }

    /// Add to a run already going. Appended, so the chats in flight keep their
    /// place — someone who queues three more has not asked to reorder the two
    /// they queued a minute ago.
    pub fn extend(&self, chats: impl IntoIterator<Item = ChatInfo>) {
        self.list().extend(chats);
    }

    /// The next chat to export, removed from the list.
    pub fn take_next(&self) -> Option<ChatInfo> {
        self.list().pop_front()
    }

    /// Take one back out. `false` if it had already been picked up.
    pub fn remove(&self, chat_id: i64) -> bool {
        let mut list = self.list();
        match list.iter().position(|c| c.id == chat_id) {
            Some(at) => {
                list.remove(at);
                true
            }
            None => false,
        }
    }

    pub fn clear(&self) {
        self.list().clear();
    }

    pub fn is_empty(&self) -> bool {
        self.list().is_empty()
    }

    /// Everything still waiting, in order. The worker resolves these to peers
    /// in one dialog sweep rather than one per chat.
    pub fn snapshot(&self) -> Vec<ChatInfo> {
        self.list().iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tgx_tg::client::ChatKind;

    fn chat(id: i64) -> ChatInfo {
        ChatInfo {
            id,
            title: format!("chat {id}"),
            kind: ChatKind::Supergroup,
            last_activity: 0,
            is_forum: false,
            public: false,
            message_count: None,
        }
    }

    #[test]
    fn a_clone_drains_the_same_list() {
        // The property the type exists for. A `Pending` that copied its list on
        // clone would pass every other test here and still leave the worker
        // exporting chats the user had removed.
        let window = Pending::new();
        let worker = window.clone();
        window.reset([chat(1), chat(2)]);
        assert_eq!(worker.take_next().map(|c| c.id), Some(1));
        assert_eq!(window.snapshot().len(), 1);
    }

    #[test]
    fn removing_a_waiting_chat_means_the_worker_never_sees_it() {
        // A cancel button that only marked the row would let the export run
        // anyway, which is the failure it exists to prevent.
        let q = Pending::new();
        q.reset([chat(1), chat(2), chat(3)]);
        assert!(q.remove(2));
        assert_eq!(
            q.snapshot().iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        // One already picked up is not there to remove, and says so rather
        // than pretending.
        assert!(!q.remove(9));
    }

    #[test]
    fn chats_added_mid_run_go_behind_the_ones_already_waiting() {
        // Queueing three more is not a request to reorder the two queued a
        // minute ago.
        let q = Pending::new();
        q.reset([chat(1), chat(2)]);
        q.extend([chat(7)]);
        assert_eq!(
            q.snapshot().iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![1, 2, 7]
        );
    }

    #[test]
    fn a_reset_replaces_rather_than_appends() {
        let q = Pending::new();
        q.reset([chat(1), chat(2)]);
        q.reset([chat(9)]);
        assert_eq!(q.snapshot().len(), 1);
        assert!(!q.is_empty());
        q.clear();
        assert!(q.is_empty());
    }
}
