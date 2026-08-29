//! Typed errors, and the one distinction that is easiest to lose.
//!
//! **A rate limit is temporary; a refusal is not. Never treat them alike.**
//!
//! The way the two get collapsed is always the same: a catch-all handler is
//! written for an admin-only method being *refused* — a permanent condition
//! where giving up quietly is correct — and a rate limit lands in the same net.
//! Where a retry guard sits behind such a handler, every line of it becomes
//! **unreachable**: the guard sees an ordinary "no" and cannot retry, cannot
//! wait, cannot count.
//!
//! That is the most damaging bug class this code can have, and it is
//! unrepresentable here: a [`Transient`] cannot be absorbed by a handler that
//! matches on [`Refused`], because they are different variants and the compiler
//! will not let a match on one silently swallow the other.
//!
//! **There is a third case, and collapsing it into [`Refused`] cost 7,140
//! files.** A stale file reference is not Telegram declining and not Telegram
//! asking for patience: it is Telegram saying *ask again with a fresh
//! reference*. See [`Stale`].
//!
//! [`Transient`]: EnrichError::Transient
//! [`Refused`]: EnrichError::Refused
//! [`Stale`]: EnrichError::Stale

use std::time::Duration;
use thiserror::Error;

/// Why an optional, degradable request did not produce data.
///
/// Every enrichment returns this. The distinction between the two variants is
/// the whole point of the type.
#[derive(Debug, Error)]
pub enum EnrichError {
    /// Telegram is rate-limiting us. **The data is there and we did not get
    /// it.** Wait and try again; count what is still lost.
    #[error("rate limited for {0:?}")]
    Transient(Duration),

    /// The short-lived *reference* Telegram issues alongside a file has aged
    /// out — `FILE_REFERENCE_EXPIRED` and the rest of its family.
    ///
    /// **Neither of the other two, which is the whole reason it exists.**
    /// Waiting does not help: nothing is rate-limiting us and the reference
    /// will not become valid again. Giving up is wrong too: the file is still
    /// there, and re-reading the message yields a fresh reference that works.
    ///
    /// Classified as [`Refused`] it was asked for five times with the same dead
    /// blob — five requests that could not succeed — and then written down as a
    /// permanent gap. The live export of 2026-08-27 lost **7,140 files** that
    /// way in one chat, 20.6% of its media, every one of them still on
    /// Telegram: 5,577 photos, 810 videos, 465 stickers and voice messages, and
    /// 274 PDFs and Word documents out of topics named `zapisnici` and
    /// `mejlovi`. The cure is
    /// [`download::refreshed_media`], and it is the only cure — a reference
    /// cannot be renewed in place, only re-obtained.
    ///
    /// [`Refused`]: EnrichError::Refused
    /// [`download::refreshed_media`]: crate::download
    #[error("stale file reference: {0}")]
    Stale(String),

    /// Telegram said no, and will keep saying no — an admin-only method for a
    /// non-admin, a channel we cannot read. Giving up quietly is correct.
    #[error("refused: {0}")]
    Refused(String),

    /// Something else went wrong. Treated as a refusal for control-flow
    /// purposes but reported separately so it cannot hide.
    #[error("failed: {0}")]
    Failed(String),
}

impl EnrichError {
    /// Is waiting worth it?
    pub fn is_transient(&self) -> bool {
        matches!(self, EnrichError::Transient(_))
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            EnrichError::Transient(d) => Some(*d),
            _ => None,
        }
    }

    /// Would asking again with a **fresh file reference** help?
    ///
    /// Deliberately not folded into [`is_transient`]: a caller that reacts to
    /// this by sleeping and retrying the same request would burn attempts on a
    /// reference that cannot come back, which is exactly the behaviour this
    /// variant was added to end.
    ///
    /// [`is_transient`]: EnrichError::is_transient
    pub fn is_stale(&self) -> bool {
        matches!(self, EnrichError::Stale(_))
    }
}

/// Errors that can end a chat's export.
#[derive(Debug, Error)]
pub enum ExportError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("telegram: {0}")]
    Invocation(String),

    /// The read loop gave up after this many rate limits with no progress.
    #[error("gave up after {waits} rate limits with no progress")]
    Stalled { waits: u32 },

    #[error("cancelled")]
    Cancelled,
}

/// Classify a grammers error into our own vocabulary.
///
/// This is the **only** place a Telegram error is interpreted, so the
/// distinction cannot be lost by a caller that was not thinking about it.
pub fn classify(err: &grammers_client::InvocationError) -> EnrichError {
    use grammers_client::InvocationError;
    match err {
        InvocationError::Rpc(rpc) => {
            // FLOOD_WAIT_N, FLOOD_PREMIUM_WAIT_N, and the slow-mode variants
            // all carry the seconds in `value`.
            //
            // Matched as two independent substrings, because the family is not
            // spelled consistently: `FLOOD_PREMIUM_WAIT` does **not** contain
            // `FLOOD_WAIT`. Testing for the joined form read as though it did,
            // so a premium rate limit fell through to `Refused` — a wait that
            // Telegram told us the length of, reported to the user as a
            // permanent refusal. That is precisely the mistake this module
            // exists to make unrepresentable.
            let name = &rpc.name;
            let flood_wait = name.contains("FLOOD") && name.contains("WAIT");
            if flood_wait || name.contains("SLOWMODE_WAIT") {
                // Floored at one second, not zero. A FLOOD_WAIT with no
                // `value` — Telegram does send them — became a zero-length
                // wait, so the retry fired immediately and asked for the same
                // rate limit again as fast as the loop could go. That is the
                // one response guaranteed to make a rate limit worse, from the
                // arm whose whole purpose is to wait it out.
                let secs = rpc.value.unwrap_or(1).max(1) as u64;
                return EnrichError::Transient(Duration::from_secs(secs));
            }
            // Matched on the family rather than the one spelling, for the same
            // reason as the waits above: `FILE_REFERENCE_EXPIRED`,
            // `_INVALID`, `_EMPTY` and the `..._ALL_...` variants all mean the
            // same thing to us — the blob we asked with is no good and a fresh
            // one would be. Only `EXPIRED` was ever observed here; the others
            // failing over into "Telegram declined" would be the same bug
            // wearing a different name.
            if name.contains("FILE_REFERENCE") {
                return EnrichError::Stale(name.clone());
            }
            // Everything else from the RPC layer is Telegram declining.
            EnrichError::Refused(rpc.name.clone())
        }
        // Session, Io and anything grammers adds later. Deliberately a
        // catch-all: a new variant must not break this build, and must not be
        // silently classified as a rate limit either.
        other => EnrichError::Failed(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_limit_and_a_refusal_are_different_types() {
        // The property that makes that bug class unrepresentable: a
        // handler written for one cannot silently absorb the other.
        let flood = EnrichError::Transient(Duration::from_secs(30));
        let refused = EnrichError::Refused("CHAT_ADMIN_REQUIRED".into());

        assert!(flood.is_transient());
        assert!(!refused.is_transient());
        assert_eq!(flood.retry_after(), Some(Duration::from_secs(30)));
        assert_eq!(refused.retry_after(), None);
    }

    /// Build the error grammers would hand us for a real wire string.
    ///
    /// Deliberately routed through `RpcError::from` rather than hand-filling
    /// the struct: grammers strips the digits out of the name into `value`, so
    /// `"FLOOD_WAIT_31"` arrives at `classify` as `name: "FLOOD_WAIT"`. A test
    /// that set `name` directly would be testing our own assumption about that
    /// split instead of the thing that actually reaches us.
    fn rpc(code: i32, message: &str) -> grammers_client::InvocationError {
        grammers_client::InvocationError::Rpc(grammers_client::sender::RpcError::from(
            grammers_tl_types::types::RpcError {
                error_code: code,
                error_message: message.to_string(),
            },
        ))
    }

    #[test]
    fn every_spelling_of_a_wait_is_a_wait() {
        // `FLOOD_PREMIUM_WAIT` does not contain `FLOOD_WAIT`, so the old
        // substring test classified a premium rate limit as a permanent
        // refusal — a wait whose length Telegram had just told us, reported as
        // "Telegram declined". No test drove `classify` with a real RPC name,
        // which is why the comment could claim the case was handled.
        for (message, secs) in [
            ("FLOOD_WAIT_31", 31),
            ("FLOOD_PREMIUM_WAIT_60", 60),
            ("SLOWMODE_WAIT_12", 12),
        ] {
            let got = classify(&rpc(420, message));
            assert!(got.is_transient(), "{message} was not treated as a wait");
            assert_eq!(
                got.retry_after(),
                Some(Duration::from_secs(secs)),
                "{message} lost its duration"
            );
        }
    }

    #[test]
    fn a_wait_with_no_duration_still_waits() {
        // Telegram does send a FLOOD_WAIT with no number on it. `unwrap_or(0)`
        // turned that into a zero-length wait, so the retry fired immediately
        // and asked for the same rate limit again as fast as the loop could go
        // — the one response guaranteed to make a rate limit worse, from the
        // arm whose entire purpose is to wait it out.
        let got = classify(&rpc(420, "FLOOD_WAIT"));
        assert!(got.is_transient());
        let waited = got.retry_after().expect("a wait has a duration");
        assert!(
            waited >= Duration::from_secs(1),
            "an immediate retry is not a wait: {waited:?}"
        );
    }

    #[test]
    fn a_stale_file_reference_is_neither_a_wait_nor_a_refusal() {
        // Landing in `Refused` meant this was asked five times with the same
        // expired blob and then written down as a permanent gap. One live
        // export lost 7,140 files to it, every one still on Telegram.
        //
        // The whole family, not just the spelling that was observed: the
        // others falling through to "Telegram declined" would be this bug
        // again under a different name.
        for message in [
            "FILE_REFERENCE_EXPIRED",
            "FILE_REFERENCE_INVALID",
            "FILE_REFERENCE_EMPTY",
        ] {
            let got = classify(&rpc(400, message));
            assert!(got.is_stale(), "{message} was not recognised as stale");
            assert!(!got.is_transient(), "{message} was mistaken for a wait");
            assert_eq!(
                got.retry_after(),
                None,
                "{message} must not be slept on — waiting cannot refresh a reference"
            );
        }
    }

    #[test]
    fn a_refusal_is_still_a_refusal() {
        // The other half of the same property: widening the wait test must not
        // start swallowing errors that are genuinely permanent, and neither
        // must widening the stale test — a refusal that reads as stale would be
        // re-read and re-asked forever.
        for message in [
            "CHAT_ADMIN_REQUIRED",
            "CHANNEL_PRIVATE",
            "AUTH_KEY_UNREGISTERED",
        ] {
            let got = classify(&rpc(400, message));
            assert!(!got.is_transient(), "{message} was mistaken for a wait");
            assert!(
                !got.is_stale(),
                "{message} was mistaken for a stale reference"
            );
            assert_eq!(got.retry_after(), None);
        }
    }

    #[test]
    fn a_failure_is_not_mistaken_for_a_rate_limit() {
        let f = EnrichError::Failed("bad response".into());
        assert!(!f.is_transient());
        assert_eq!(f.retry_after(), None);
    }

    #[test]
    fn the_messages_name_what_happened() {
        assert!(EnrichError::Refused("CHAT_ADMIN_REQUIRED".into())
            .to_string()
            .contains("CHAT_ADMIN_REQUIRED"));
        assert!(ExportError::Stalled { waits: 10 }
            .to_string()
            .contains("10"));
    }
}
