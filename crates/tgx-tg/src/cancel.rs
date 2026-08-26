//! The stop signal, shared between the window and the worker.
//!
//! **Cooperative, because the alternative loses the file.** The JSON is
//! streamed and an abandoned [`crate::output::Output`] leaves a *zero-byte*
//! file rather than a truncated one, so an export cannot be stopped by dropping
//! its future or aborting its task — the writes are still sitting in a buffer.
//! The worker has to be told, reach the next check, and close on its way out.
//!
//! Before this existed the window's Stop button set a local flag, wrote
//! "Stopped", and sent nothing: the export ran to completion, went on writing
//! files, and then overwrote the message with a success line.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A cooperative stop signal, shared between the UI and the worker.
///
/// **`Clone` shares the flag, it does not copy it.** That is the entire point:
/// the UI keeps one handle and hands another to the worker, and a clone that
/// carried its own `false` would leave Stop doing nothing at all — the defect
/// this type was added to fix.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// A fresh signal, not cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask every holder to stop at its next check.
    pub fn cancel(&self) {
        // `SeqCst` throughout: this is read at most a few times a second on the
        // worker and once per click on the UI thread, so the ordering costs
        // nothing measurable and removes the only interesting question a reader
        // could have about it.
        self.0.store(true, Ordering::SeqCst);
    }

    /// Has someone asked us to stop?
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Clear the flag so the same signal can drive the next run.
    ///
    /// Reset when a run **starts**, never when one ends: a cancel that lands
    /// between the worker's last check and its teardown would otherwise be
    /// wiped by the very run it was meant to stop, and the next export would
    /// begin already carrying a stale `true`.
    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_signal_is_not_cancelled() {
        assert!(!Cancel::new().is_cancelled());
        assert!(!Cancel::default().is_cancelled());
    }

    #[test]
    fn a_clone_sees_the_originals_cancel() {
        // The property the whole type exists for. A `Cancel` that copied its
        // flag on clone would pass every other test here and still leave the
        // Stop button inert, because the UI and the worker hold different
        // handles by construction.
        let ui = Cancel::new();
        let worker = ui.clone();
        assert!(!worker.is_cancelled());
        ui.cancel();
        assert!(worker.is_cancelled());
    }

    #[test]
    fn the_original_sees_a_clones_cancel_too() {
        let ui = Cancel::new();
        let worker = ui.clone();
        worker.cancel();
        assert!(ui.is_cancelled());
    }

    #[test]
    fn reset_clears_it_for_the_next_run() {
        let signal = Cancel::new();
        signal.cancel();
        assert!(signal.is_cancelled());
        signal.reset();
        assert!(!signal.is_cancelled());
        // And a clone taken before the reset agrees — one flag, not two.
        let other = signal.clone();
        assert!(!other.is_cancelled());
    }
}
