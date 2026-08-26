//! When Desktop keeps two messages in one sender block.
//!
//! Both numbers here were swept across all 6,643 messages of the reference
//! rather than chosen, and both matter:
//!
//! * **Same sender within 900 s** for ordinary messages.
//! * **Consecutive forwards only within 3 s.** Telegram sends a forwarded album
//!   as a burst inside one second and Desktop keeps only that burst together —
//!   two forwards from the same person 16 s apart are drawn as two blocks. Any
//!   window in [1, 5] reproduces the reference exactly; 0 misses 17 joins and 8
//!   invents one, so 3 sits in the middle.
//!
//! Both are *durations*, so both are measured on `date_unixtime` and not on the
//! naive local `date` — the rule stated on `tgx_format::date_pair`. The gap used
//! to come off `date`, which reads an hour backwards through a DST fall-back:
//! two messages 90 s apart across Belgrade's 26 October change compute a gap of
//! −3510 s, fall outside `0..=900`, and Desktop's single sender block comes out
//! as two. The corpus cannot arbitrate this — it is one December range, so the
//! two clocks agree on every one of its 6,643 messages and the html leg reads
//! 256,780 lines either way. It is settled by the invariant, not by the oracle.

use chrono::NaiveDateTime;
use serde_json::Value;

/// Desktop starts a new sender block once this many seconds have passed.
pub const JOIN_WITHIN_SECONDS: i64 = 900;

/// The much tighter window for consecutive forwards.
pub const FORWARD_JOIN_WITHIN_SECONDS: i64 = 3;

/// What has to stay the same for Desktop to keep one sender block.
///
/// Sender and forward source, and deliberately **not** the forward's original
/// date: Desktop joins two forwards whose originals were sent eleven hours
/// apart, so the original date plays no part. What separates them is the
/// send-time window.
///
/// It also carries the message's `date_unixtime`, which is **not** part of the
/// identity — see the hand-written `PartialEq` below. It rides here because it
/// is the one number [`JoinState::joined`] needs and has no other way to reach:
/// the window is a duration, and a duration read off the naive local `date`
/// runs backwards through a DST fall-back.
#[derive(Debug, Clone, Eq, Default)]
pub struct JoinKey {
    from_id: String,
    forwarded_from: String,
    forwarded_from_id: String,
    /// Desktop writes this as a decimal string, not a number.
    unixtime: Option<i64>,
}

/// Identity only, and never the timestamp: two messages a minute apart are the
/// same sender, and comparing the stamp would end every block at its first
/// message. This is why the impl is written out instead of derived.
impl PartialEq for JoinKey {
    fn eq(&self, other: &Self) -> bool {
        self.from_id == other.from_id
            && self.forwarded_from == other.forwarded_from
            && self.forwarded_from_id == other.forwarded_from_id
    }
}

impl JoinKey {
    pub fn of(m: &serde_json::Map<String, Value>) -> Self {
        let s = |k: &str| -> String {
            m.get(k)
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        };
        Self {
            from_id: s("from_id"),
            forwarded_from: s("forwarded_from"),
            forwarded_from_id: s("forwarded_from_id"),
            unixtime: m.get("date_unixtime").and_then(|v| match v {
                Value::String(s) => s.parse().ok(),
                other => other.as_i64(),
            }),
        }
    }
}

/// Parse Desktop's naive local `date` field.
pub fn parse_date(value: Option<&Value>) -> Option<NaiveDateTime> {
    let s = value?.as_str()?;
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()
}

/// Tracks the running block state across a page.
#[derive(Debug, Default)]
pub struct JoinState {
    last_key: Option<JoinKey>,
    last_dt: Option<NaiveDateTime>,
}

impl JoinState {
    /// A page break always starts a fresh block, and re-states the date it
    /// opens on.
    pub fn reset(&mut self) {
        self.last_key = None;
        self.last_dt = None;
    }

    /// A service message breaks the block on both sides.
    pub fn broke(&mut self) {
        self.last_key = None;
    }

    /// Would this message be drawn `joined` onto the one before it?
    pub fn joined(&self, key: &JoinKey, dt: Option<NaiveDateTime>, is_forward: bool) -> bool {
        let window = if is_forward {
            FORWARD_JOIN_WITHIN_SECONDS
        } else {
            JOIN_WITHIN_SECONDS
        };
        let (Some(last_key), Some(last_dt), Some(dt)) = (self.last_key.as_ref(), self.last_dt, dt)
        else {
            return false;
        };
        if last_key != key {
            return false;
        }
        // The gap is a duration, so it comes off `date_unixtime` whenever both
        // messages carry one; the naive `date` is the fallback for a map that
        // does not (synthetic fixtures, and nothing an export writes). Reading
        // it off `date` is what makes a DST fall-back split a block: the second
        // message's wall clock is an hour *earlier*, the gap goes negative and
        // leaves `0..=window`.
        let gap = match (last_key.unixtime, key.unixtime) {
            (Some(before), Some(after)) => after.saturating_sub(before),
            _ => dt.signed_duration_since(last_dt).num_seconds(),
        };
        (0..=window).contains(&gap)
    }

    pub fn advance(&mut self, key: JoinKey, dt: Option<NaiveDateTime>) {
        self.last_key = Some(key);
        self.last_dt = dt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(from: &str, date: &str) -> serde_json::Map<String, Value> {
        json!({ "from_id": from, "date": date })
            .as_object()
            .unwrap()
            .clone()
    }

    /// A message with both clocks, as an export writes it.
    fn msg_at(from: &str, date: &str, unixtime: i64) -> serde_json::Map<String, Value> {
        json!({ "from_id": from, "date": date, "date_unixtime": unixtime.to_string() })
            .as_object()
            .unwrap()
            .clone()
    }

    fn at(s: &str) -> Option<NaiveDateTime> {
        parse_date(Some(&json!(s)))
    }

    #[test]
    fn same_sender_inside_fifteen_minutes_joins() {
        let m1 = msg("user1", "2025-12-18T16:00:00");
        let m2 = msg("user1", "2025-12-18T16:14:59");
        let mut st = JoinState::default();
        st.advance(JoinKey::of(&m1), at("2025-12-18T16:00:00"));
        assert!(st.joined(&JoinKey::of(&m2), at("2025-12-18T16:14:59"), false));
    }

    #[test]
    fn one_second_past_the_window_does_not() {
        let m1 = msg("user1", "2025-12-18T16:00:00");
        let m2 = msg("user1", "2025-12-18T16:15:01");
        let mut st = JoinState::default();
        st.advance(JoinKey::of(&m1), at("2025-12-18T16:00:00"));
        assert!(!st.joined(&JoinKey::of(&m2), at("2025-12-18T16:15:01"), false));
    }

    #[test]
    fn a_different_sender_never_joins() {
        let m1 = msg("user1", "2025-12-18T16:00:00");
        let m2 = msg("user2", "2025-12-18T16:00:01");
        let mut st = JoinState::default();
        st.advance(JoinKey::of(&m1), at("2025-12-18T16:00:00"));
        assert!(!st.joined(&JoinKey::of(&m2), at("2025-12-18T16:00:01"), false));
    }

    #[test]
    fn forwards_get_the_tight_window() {
        // 16 seconds apart: an ordinary pair joins, a forwarded pair does not.
        let m1 = msg("user1", "2025-12-18T16:00:00");
        let m2 = msg("user1", "2025-12-18T16:00:16");
        let mut st = JoinState::default();
        st.advance(JoinKey::of(&m1), at("2025-12-18T16:00:00"));
        assert!(st.joined(&JoinKey::of(&m2), at("2025-12-18T16:00:16"), false));
        assert!(!st.joined(&JoinKey::of(&m2), at("2025-12-18T16:00:16"), true));
    }

    #[test]
    fn a_forward_burst_inside_three_seconds_joins() {
        let m1 = msg("user1", "2025-12-18T16:00:00");
        let m2 = msg("user1", "2025-12-18T16:00:03");
        let mut st = JoinState::default();
        st.advance(JoinKey::of(&m1), at("2025-12-18T16:00:00"));
        assert!(st.joined(&JoinKey::of(&m2), at("2025-12-18T16:00:03"), true));
    }

    #[test]
    fn the_forwards_original_date_is_not_part_of_the_key() {
        // Desktop joins two forwards whose originals are eleven hours apart.
        let a = json!({ "from_id": "user1", "forwarded_from": "X", "forwarded_date": "2020-01-01T00:00:00" });
        let b = json!({ "from_id": "user1", "forwarded_from": "X", "forwarded_date": "2020-01-01T11:00:00" });
        assert_eq!(
            JoinKey::of(a.as_object().unwrap()),
            JoinKey::of(b.as_object().unwrap())
        );
    }

    #[test]
    fn a_page_break_starts_a_fresh_block() {
        let m = msg("user1", "2025-12-18T16:00:00");
        let mut st = JoinState::default();
        st.advance(JoinKey::of(&m), at("2025-12-18T16:00:00"));
        st.reset();
        assert!(!st.joined(&JoinKey::of(&m), at("2025-12-18T16:00:01"), false));
    }

    #[test]
    fn a_service_message_breaks_the_block() {
        let m = msg("user1", "2025-12-18T16:00:00");
        let mut st = JoinState::default();
        st.advance(JoinKey::of(&m), at("2025-12-18T16:00:00"));
        st.broke();
        assert!(!st.joined(&JoinKey::of(&m), at("2025-12-18T16:00:01"), false));
    }

    #[test]
    fn a_message_going_backwards_in_time_does_not_join() {
        // The window is [0, n], not |gap| <= n.
        let m = msg("user1", "2025-12-18T16:00:00");
        let mut st = JoinState::default();
        st.advance(JoinKey::of(&m), at("2025-12-18T16:00:10"));
        assert!(!st.joined(&JoinKey::of(&m), at("2025-12-18T16:00:00"), false));
    }

    #[test]
    fn a_dst_fall_back_does_not_split_the_block() {
        // Belgrade, 26 October 2025: 03:00 CEST becomes 02:00 CET. These two
        // messages are 90 seconds apart, and on the naive `date` field that
        // reads as 58 minutes *backwards* — outside 0..=900, so the block used
        // to break here and the second message grew its own userpic and header.
        let before = msg_at("user1", "2025-10-26T02:59:00", 1_761_440_340);
        let after = msg_at("user1", "2025-10-26T02:00:30", 1_761_440_430);
        // The wall clock really does run backwards across the pair.
        assert!(at("2025-10-26T02:00:30") < at("2025-10-26T02:59:00"));

        let mut st = JoinState::default();
        st.advance(JoinKey::of(&before), at("2025-10-26T02:59:00"));
        assert!(st.joined(&JoinKey::of(&after), at("2025-10-26T02:00:30"), false));
    }

    #[test]
    fn the_real_duration_wins_over_the_wall_clock_the_other_way_too() {
        // The mirror of the fall-back, so the fix is not just "never split":
        // one minute on the clock face, 901 seconds of real time, no join.
        let before = msg_at("user1", "2025-03-30T01:59:00", 1_743_000_000);
        let after = msg_at("user1", "2025-03-30T02:00:00", 1_743_000_901);
        let mut st = JoinState::default();
        st.advance(JoinKey::of(&before), at("2025-03-30T01:59:00"));
        assert!(!st.joined(&JoinKey::of(&after), at("2025-03-30T02:00:00"), false));
    }

    #[test]
    fn the_timestamp_the_key_carries_is_not_part_of_its_identity() {
        // JoinKey exists to answer "same sender?"; it carries date_unixtime
        // only so the gap can be measured. Comparing it would end every block
        // at its first message.
        assert_eq!(
            JoinKey::of(&msg_at("user1", "2025-12-18T16:00:00", 1_766_073_600)),
            JoinKey::of(&msg_at("user1", "2025-12-18T16:00:01", 1_766_073_601))
        );
    }

    #[test]
    fn dates_parse_in_desktops_naive_local_form() {
        assert!(parse_date(Some(&json!("2025-12-18T16:17:52"))).is_some());
        assert!(parse_date(Some(&json!("2025-12-18T16:17:52Z"))).is_none());
        assert!(parse_date(Some(&json!(""))).is_none());
        assert!(parse_date(None).is_none());
    }
}
