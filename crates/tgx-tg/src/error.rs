//! Typed errors, and the one distinction the Python original kept losing.
//!
//! **A rate limit is temporary; a refusal is not. Never treat them alike.**
//!
//! In the Python implementation four separate places collapsed a `FloodWait`
//! into a permanent refusal, because the `except Exception` around them was
//! written for an admin-only method being *refused* — a permanent condition
//! where giving up quietly is correct — and a rate limit landed in the same
//! net. In two of those places it made every line of the retry guard
//! **unreachable**: the guard saw an ordinary "no" and could not retry, could
//! not wait, could not count.
//!
//! That is the single most damaging bug class in the original, and it is
//! unrepresentable here: a [`Transient`] cannot be absorbed by a handler that
//! matches on [`Refused`], because they are different variants and the compiler
//! will not let a match on one silently swallow the other.
//!
//! [`Transient`]: EnrichError::Transient
//! [`Refused`]: EnrichError::Refused

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
                let secs = rpc.value.unwrap_or(0) as u64;
                return EnrichError::Transient(Duration::from_secs(secs));
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
        // The property that makes the original bug class unrepresentable: a
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
    fn a_refusal_is_still_a_refusal() {
        // The other half of the same property: widening the wait test must not
        // start swallowing errors that are genuinely permanent.
        for message in [
            "CHAT_ADMIN_REQUIRED",
            "CHANNEL_PRIVATE",
            "AUTH_KEY_UNREGISTERED",
        ] {
            let got = classify(&rpc(400, message));
            assert!(!got.is_transient(), "{message} was mistaken for a wait");
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
