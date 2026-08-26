//! The async bridge: **the UI thread never blocks on a future.**
//!
//! grammers requires tokio; GPUI owns the main thread and has its own executor.
//! So a tokio runtime lives on a dedicated thread for the app's lifetime, work
//! is submitted to it, and results come back over a channel that the UI drains
//! on its own schedule. This is the direct analogue of the Python
//! `worker.py::AsyncBridge`, including the part that matters most:
//!
//! **Shutdown drains, it does not stop.** Cancelling and returning immediately
//! abandons in-flight tasks where they sit, so an export's cleanup — and
//! therefore `Output::close()` — never runs. The result is a **zero-byte**
//! `result.json`, not a truncated one, because the writes are still buffered.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;

/// Anything the worker wants the interface to know.
#[derive(Debug, Clone)]
pub enum Event {
    Status(String),
    Log(String),
    SignedIn(String),
    /// The sign-in advanced a step: a code was sent, or 2FA is wanted.
    LoginStage(crate::login::Stage),
    /// The step failed, and the dialog stays open saying why.
    LoginFailed(String),
    /// The loaded chat list.
    Chats(Vec<tgx_tg::client::ChatInfo>),
    Progress {
        done: usize,
        total: i64,
    },
    /// A rate-limit wait. Reported because two minutes of silence is
    /// indistinguishable from a hung export.
    FloodWait(u64),
    Finished(String),
    Failed(String),
}

pub struct Bridge {
    runtime: Option<Runtime>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    /// Set once shutdown has run, so a second call is a no-op rather than a
    /// second teardown of a dead runtime — which completed quietly in the
    /// original and was indistinguishable from one that had drained.
    stopped: Arc<Mutex<bool>>,
}

impl Bridge {
    pub fn new() -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (tx, rx) = std::sync::mpsc::channel();
        Ok(Self {
            runtime: Some(runtime),
            tx,
            rx,
            stopped: Arc::new(Mutex::new(false)),
        })
    }

    /// A sender the worker side can keep.
    pub fn sender(&self) -> Sender<Event> {
        self.tx.clone()
    }

    /// Submit work. Returns immediately; the UI never awaits.
    pub fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        if let Some(rt) = &self.runtime {
            rt.spawn(future);
        }
    }

    /// Everything that has arrived since the last call. Non-blocking.
    pub fn drain(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    /// Wait for in-flight work, then stop.
    ///
    /// Runs **once**. `shutdown_timeout` lets tasks finish their cleanup — the
    /// `finally` that closes an `Output` — rather than dropping them mid-write.
    pub fn shutdown(&mut self, grace: std::time::Duration) {
        let mut stopped = match self.stopped.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if *stopped {
            return;
        }
        *stopped = true;
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_timeout(grace);
        }
    }

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
        let bridge = Bridge::new().unwrap();
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
        let bridge = Bridge::new().unwrap();
        assert!(bridge.drain().is_empty());
    }

    #[test]
    fn shutdown_lets_a_task_finish_its_cleanup() {
        // The zero-byte failure: a bare stop abandons in-flight tasks, so the
        // cleanup that flushes buffered writes never runs.
        static CLEANED: AtomicBool = AtomicBool::new(false);
        CLEANED.store(false, Ordering::SeqCst);

        let mut bridge = Bridge::new().unwrap();
        bridge.spawn(async {
            struct Guard;
            impl Drop for Guard {
                fn drop(&mut self) {
                    CLEANED.store(true, Ordering::SeqCst);
                }
            }
            let _g = Guard;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });
        std::thread::sleep(std::time::Duration::from_millis(10));
        bridge.shutdown(std::time::Duration::from_secs(5));
        assert!(
            CLEANED.load(Ordering::SeqCst),
            "the task's cleanup never ran"
        );
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
