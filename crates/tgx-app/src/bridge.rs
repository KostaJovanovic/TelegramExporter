//! The async bridge: **the UI thread never blocks on a future.**
//!
//! grammers requires tokio; GPUI owns the main thread and has its own executor.
//! So a tokio runtime lives on a dedicated thread for the app's lifetime, work
//! is submitted to it, and results come back over a channel that the UI drains
//! on its own schedule. This is the direct analogue of the Python
//! `worker.py::AsyncBridge`, including the part that matters most:
//!
//! **Shutdown drains, it does not stop.** Returning immediately abandons
//! in-flight tasks where they sit, so an export's cleanup — and therefore
//! `Output::close()` — never runs. The result is a **zero-byte**
//! `result.json`, not a truncated one, because the writes are still buffered.
//! (`Output` has a `Drop` backstop, so in practice the file is closed; what is
//! lost by abandoning is everything else `close_all` does — the HTML tail, the
//! empty-topic cleanup, `missing_media.txt`.)
//!
//! Draining means two things in order, and `shutdown` does both: **the caller
//! cancels, and then the bridge waits for the job count to reach zero.**
//! `Runtime::shutdown_timeout` on its own is not the second thing — see the
//! note on [`Bridge::shutdown`], which is where that distinction cost real
//! work.
//!
//! **The channel is awaitable, and that is the whole reason it is tokio's and
//! not `std::sync::mpsc`'s.** A `std` receiver can only be polled, so the
//! window had to be already painting to notice an event — which it only was
//! when the user happened to move the mouse. `tokio::sync::mpsc` is
//! runtime-agnostic (it needs a waker, not a reactor), so its receiver can be
//! awaited on GPUI's *foreground* executor while the senders live on tokio
//! worker threads. That is what lets a finished job mark the window dirty
//! instead of waiting to be found.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// The sending half, as the worker side holds it.
pub type Events = UnboundedSender<Event>;

/// Anything the worker wants the interface to know.
///
/// **Every event that carries a number about a chat carries the chat's id.**
/// A bare `Progress { done, total }` cannot be written onto the row it belongs
/// to, so the list has no way to fill a count in for a chat the user never
/// counted — which is how `message_count` stayed `None` for the life of the
/// window while the progress bar plainly knew the answer.
#[derive(Debug, Clone)]
pub enum Event {
    Status(String),
    Log(String),
    /// A log line that matters — a short export, a lost enrichment, an
    /// unprotected data folder. **The writer decides**, because sniffing a
    /// warning out of the text catches "0 failed", which is the opposite of one.
    Warn(String),
    SignedIn(String),
    /// The sign-in advanced a step: a code was sent, or 2FA is wanted.
    LoginStage(crate::login::Stage),
    /// The step failed, and the dialog stays open saying why.
    LoginFailed(String),
    /// The loaded chat list.
    Chats(Vec<tgx_tg::client::ChatInfo>),

    // -- counting ---------------------------------------------------------
    /// One chat's total. **`None` is not zero**: it means Telegram would not
    /// say, and the row must stay blank rather than claim an empty chat.
    Counted {
        chat_id: i64,
        total: Option<i64>,
    },
    /// How far the count has got, in chats — not messages.
    CountProgress {
        done: usize,
        total: usize,
    },
    /// The count ended, however it ended. Fired for a cancelled count too, or
    /// the button is left reading "Stop counting" over a run that has stopped.
    CountFinished {
        counted: usize,
        failed: usize,
    },

    // -- exporting --------------------------------------------------------
    ChatStarted {
        chat_id: i64,
        title: String,
    },
    /// The total this chat's export looked up. Free to publish, and without it
    /// the row reads blank beside a progress bar that clearly knows the answer.
    ChatTotal {
        chat_id: i64,
        total: i64,
    },
    /// How many topic folders this chat will produce.
    ChatTopics {
        chat_id: i64,
        topics: usize,
    },
    Progress {
        chat_id: i64,
        done: usize,
        total: i64,
    },
    /// A finished chat. `messages` is what was **written**, which is a measured
    /// number and so replaces whatever the list was carrying.
    ChatDone {
        chat_id: i64,
        messages: usize,
        expected: i64,
        /// Topic folders written, or `None` for a chat that has no topics at
        /// all. **Not zero.** The engine counts its single General sink as one
        /// "topic" for a chat that was never split, so passing the raw number
        /// through made every private chat report "1 topic folders" and turned
        /// the TOPICS column's em dash — which means *this chat has no topics*
        /// — into a `1` the moment it finished.
        topics: Option<usize>,
        media_downloaded: usize,
        media_failed: usize,
        root: std::path::PathBuf,
    },
    /// **A cancelled or failed export writes no count at all.** A truncated run
    /// must not leave its own length behind as the size of the chat.
    ChatFailed {
        chat_id: i64,
        message: String,
    },

    /// A rate-limit wait. Reported because two minutes of silence is
    /// indistinguishable from a hung export.
    FloodWait(u64),

    /// The queue ended.
    ///
    /// It carries **whether** it was stopped, not the sentence to display.
    /// The queue itself knows how many chats were done, failed and never run,
    /// and a worker that composes its own summary is a second writer of the
    /// same fact — which is how "Stopped" ended up overwritten by "Exported 3
    /// of 3 chats" a moment later.
    Finished {
        stopped: bool,
    },
    /// An action failed, and which action it was.
    ///
    /// **The activity is not decoration.** Without it this was a global switch:
    /// `Shell::apply` cleared `exporting` *and* `counting` on any `Failed`,
    /// whoever sent it — so a sign-in probe that could not reach Telegram while
    /// an export was running made the window believe the export had stopped.
    /// The Stop button vanished, the progress bar froze, and the export carried
    /// on writing files until its own `Finished` put the state back. Five
    /// senders share this variant and only one of them is the export.
    Failed {
        activity: Activity,
        message: String,
    },
}

/// Which action a [`Event::Failed`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    SignIn,
    Chats,
    Count,
    Export,
}

pub struct Bridge {
    runtime: Option<Runtime>,
    tx: Events,
    /// Taken **once**, by whoever is going to await it. An `Option` rather than
    /// a second channel, so "who owns the receiving end?" has one answer.
    rx: Option<UnboundedReceiver<Event>>,
    /// Set once shutdown has run, so a second call is a no-op rather than a
    /// second teardown of a dead runtime — which completed quietly in the
    /// original and was indistinguishable from one that had drained.
    stopped: Arc<Mutex<bool>>,
    /// Jobs submitted and not yet finished. What [`shutdown`](Self::shutdown)
    /// waits on.
    in_flight: Arc<AtomicUsize>,
}

impl Bridge {
    pub fn new() -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Ok(Self {
            runtime: Some(runtime),
            tx,
            rx: Some(rx),
            stopped: Arc::new(Mutex::new(false)),
            in_flight: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// A sender the worker side can keep.
    pub fn sender(&self) -> Events {
        self.tx.clone()
    }

    /// Submit work. Returns immediately; the UI never awaits.
    ///
    /// The future is wrapped so the bridge knows how many jobs are outstanding.
    /// That count is the only thing [`shutdown`](Self::shutdown) can actually
    /// wait on — see the note there for why the runtime's own timeout is not
    /// enough.
    pub fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        if let Some(rt) = &self.runtime {
            let in_flight = self.in_flight.clone();
            in_flight.fetch_add(1, Ordering::SeqCst);
            rt.spawn(async move {
                // A guard, not a decrement after the `await`: a task dropped at
                // a suspension point never reaches the line after it, and an
                // undercount would make shutdown wait the full grace period
                // every time.
                struct Done(Arc<AtomicUsize>);
                impl Drop for Done {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                let _done = Done(in_flight);
                future.await;
            });
        }
    }

    /// How many submitted jobs have not finished.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// The receiving end, for the task that will await it.
    ///
    /// Returns `None` on a second call: two awaiters would each see half the
    /// events, and a chat list that arrives on the half nobody is painting from
    /// is indistinguishable from one that never arrived at all.
    pub fn take_events(&mut self) -> Option<UnboundedReceiver<Event>> {
        self.rx.take()
    }

    /// Everything that has arrived since the last call. Non-blocking.
    ///
    /// Only for a bridge whose receiver has **not** been taken — the headless
    /// shell the interaction tests drive, and nothing else. Gated on `test` so
    /// that stays true: the window awaits the channel, and a polling drain
    /// reachable from the window is how the repaint defect would come back.
    #[cfg(test)]
    pub fn drain(&mut self) -> Vec<Event> {
        use tokio::sync::mpsc::error::TryRecvError;
        let mut out = Vec::new();
        let Some(rx) = self.rx.as_mut() else {
            return out;
        };
        loop {
            match rx.try_recv() {
                Ok(event) => out.push(event),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return out,
            }
        }
    }

    /// Wait for in-flight work, then stop. Runs **once**.
    ///
    /// **`Runtime::shutdown_timeout` alone does not do this.** It waits for
    /// *blocking* work; an async task is dropped at its next suspension point,
    /// and an export is parked on `iter.next().await` for most of its life —
    /// so it is never polled again, never sees the cancel flag, and never
    /// reaches `close_all`. The grace period expires against nothing. That is
    /// the difference between draining and stopping, and this module's whole
    /// contract is the first one.
    ///
    /// So the wait is on the job count, in slices, before the runtime is told
    /// to go: the caller cancels, the export's next poll sees it, unwinds
    /// through `close_all`, and the count reaches zero. Only then does the
    /// runtime shut down, and by then there is nothing left to abandon.
    ///
    /// If the grace expires with work still running, the runtime is stopped
    /// anyway — `Output`'s own `Drop` is the backstop, and hanging the quit
    /// indefinitely on a wedged request is worse than taking the backstop.
    pub fn shutdown(&mut self, grace: std::time::Duration) {
        let mut stopped = match self.stopped.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if *stopped {
            return;
        }
        *stopped = true;

        let deadline = std::time::Instant::now() + grace;
        let slice = std::time::Duration::from_millis(25);
        while self.in_flight() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(slice);
        }
        if let Some(rt) = self.runtime.take() {
            // Whatever is left gets the remainder, then goes.
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            rt.shutdown_timeout(left);
        }
    }

    /// Whether [`shutdown`](Self::shutdown) has run.
    ///
    /// Nothing in the window asks — `spawn` is already a no-op afterwards, and
    /// the pump ends when the channel closes — so this exists for the tests
    /// that prove a second shutdown is harmless rather than a second teardown.
    #[cfg(test)]
    pub fn is_stopped(&self) -> bool {
        self.stopped.lock().map(|g| *g).unwrap_or(true)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.shutdown(std::time::Duration::from_secs(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn events_arrive_without_blocking_the_caller() {
        let mut bridge = Bridge::new().unwrap();
        let tx = bridge.sender();
        bridge.spawn(async move {
            let _ = tx.send(Event::Status("working".into()));
        });
        // Poll rather than block; the UI thread does the same.
        for _ in 0..200 {
            let events = bridge.drain();
            if !events.is_empty() {
                assert!(matches!(events[0], Event::Status(_)));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("no event arrived");
    }

    #[test]
    fn draining_an_idle_bridge_returns_nothing_and_does_not_block() {
        let mut bridge = Bridge::new().unwrap();
        assert!(bridge.drain().is_empty());
    }

    #[test]
    fn the_receiving_end_can_only_be_taken_once() {
        // Two awaiters would each see half the events, and a chat list that
        // arrives on the half nobody is painting from is indistinguishable
        // from one that never arrived.
        let mut bridge = Bridge::new().unwrap();
        assert!(bridge.take_events().is_some());
        assert!(bridge.take_events().is_none());
        // And a bridge whose receiver has gone has nothing left to drain.
        assert!(bridge.drain().is_empty());
    }

    #[test]
    fn shutdown_waits_for_the_work_rather_than_dropping_it() {
        // **This is the test the previous one only looked like.** A destructor
        // running proves nothing: an abandoned task's locals are dropped too,
        // so a `Drop` guard fires whether the task *finished* or was thrown
        // away at its next await. What distinguishes the two is whether the
        // line after the await ever ran.
        static FINISHED: AtomicBool = AtomicBool::new(false);
        FINISHED.store(false, Ordering::SeqCst);

        let mut bridge = Bridge::new().unwrap();
        bridge.spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            FINISHED.store(true, Ordering::SeqCst);
        });
        // Let it reach the await, so the interesting case is the one under test.
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(bridge.in_flight(), 1);
        bridge.shutdown(std::time::Duration::from_secs(5));
        assert!(
            FINISHED.load(Ordering::SeqCst),
            "the task was abandoned at its await, not drained"
        );
        assert_eq!(bridge.in_flight(), 0);
    }

    #[test]
    fn work_that_will_not_end_does_not_hang_the_quit() {
        // The other half of the trade: `Output`'s own `Drop` is the backstop,
        // and waiting for ever on a wedged request is worse than taking it.
        let mut bridge = Bridge::new().unwrap();
        bridge.spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        let started = std::time::Instant::now();
        bridge.shutdown(std::time::Duration::from_millis(150));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_finished_job_stops_being_counted() {
        let mut bridge = Bridge::new().unwrap();
        assert_eq!(bridge.in_flight(), 0);
        bridge.spawn(async {});
        for _ in 0..200 {
            if bridge.in_flight() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(bridge.in_flight(), 0, "the count never came back down");
        // And a bridge holding no work shuts down without waiting out its grace.
        let started = std::time::Instant::now();
        bridge.shutdown(std::time::Duration::from_secs(5));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn shutdown_runs_once_and_a_second_call_is_harmless() {
        let mut bridge = Bridge::new().unwrap();
        bridge.shutdown(std::time::Duration::from_millis(50));
        assert!(bridge.is_stopped());
        // A second call against a dead bridge must not panic, and must not
        // read as a fresh drain.
        bridge.shutdown(std::time::Duration::from_millis(50));
        assert!(bridge.is_stopped());
    }

    #[test]
    fn submitting_after_shutdown_is_a_no_op_not_a_panic() {
        let mut bridge = Bridge::new().unwrap();
        bridge.shutdown(std::time::Duration::from_millis(50));
        bridge.spawn(async {});
        assert!(bridge.drain().is_empty());
    }
}
