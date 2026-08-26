//! Telegram Desktop's export JSON schema.
//!
//! No I/O, no network, no UI — everything here is a pure function of values
//! that have already been read off the wire. That is what lets the parity
//! harness replay a real `result.json` through it with no Telegram connection,
//! and it is the property that keeps the JSON and the HTML from drifting: both
//! are rendered from the same map.
//!
//! The four invariants that are easy to break, each with a test that fails when
//! it is broken:
//!
//! * [`text`] — entity offsets are **UTF-16 code units**.
//! * [`order`] — **key order is part of the format**.
//! * [`json`] — one-space indent, raw UTF-8, and the `reactions` over-indent.
//! * [`peer`] — **name cache keys are typed**; the three id spaces collide.

pub mod json;
pub mod order;
pub mod peer;
pub mod size;
pub mod text;

pub use json::Degraded;
pub use peer::{PeerKey, PeerKind};
pub use text::Entity;

/// Desktop's `date` (naive local wall clock) and `date_unixtime` (UTC epoch
/// seconds) pair.
///
/// Both are written for every message. Anything with a calendar or a clock face
/// on it reads `date`; anything measuring a *duration* reads `date_unixtime`,
/// which stays monotonic across a DST change where `date` does not.
pub fn date_pair(ts: i64) -> Option<(String, String)> {
    use chrono::{Local, TimeZone};
    let local = Local.timestamp_opt(ts, 0).single()?;
    Some((
        local.format("%Y-%m-%dT%H:%M:%S").to_string(),
        ts.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_pair_is_naive_local_plus_utc_seconds() {
        let (date, unix) = date_pair(1_766_071_072).expect("a real timestamp");
        // No zone suffix: Desktop writes a naive local wall clock.
        assert!(!date.ends_with('Z'), "got {date}");
        assert!(!date.contains('+'), "got {date}");
        assert_eq!(date.len(), 19, "got {date}");
        assert_eq!(unix, "1766071072");
    }
}
