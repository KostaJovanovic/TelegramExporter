//! Desktop's key order, recovered from a reference export.
//!
//! **Key order is part of the format.** Desktop's JSON is pretty-printed, so a
//! differently ordered map produces a file that diffs against a real export on
//! nearly every line. This is not a style preference; it is the wire format.

use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Desktop's own sequence.
pub const DESKTOP_ORDER: &[&str] = &[
    "id",
    "type",
    "date",
    "date_unixtime",
    "edited",
    "edited_unixtime",
    "from",
    "from_id",
    "author",
    "forwarded_from",
    "forwarded_from_id",
    "saved_from",
    "via_bot",
    "reply_to_message_id",
    "reply_to_peer_id",
    "actor",
    "actor_id",
    "action",
    "title",
    "new_title",
    "message_id",
    "members",
    "inviter",
    "icon_emoji_id",
    "new_icon_emoji_id",
    "closed",
    "hidden",
    "emoticon",
    "period",
    "score",
    "game_message_id",
    "amount",
    "currency",
    "discard_reason",
    "file",
    "file_name",
    "file_size",
    "thumbnail",
    "thumbnail_file_size",
    "media_type",
    "sticker_emoji",
    "mime_type",
    "performer",
    "photo",
    "photo_file_size",
    "duration_seconds",
    "width",
    "height",
    "contact_information",
    "contact_vcard",
    "location_information",
    "live_location_period_seconds",
    "place_name",
    "address",
    "poll",
    "game_title",
    "game_description",
    "game_link",
    "invoice_information",
    "dice_emoji",
    "dice_value",
];

/// Keys Desktop's exporter never writes, carrying data Telegram *did* send and
/// Desktop simply drops.
///
/// They sit after every Desktop key and before the text, so a reader diffing
/// against a real export sees them as one contiguous block rather than
/// interleaved through the familiar ones. **Every one of them is gated on being
/// present**, which is what lets the parity harness pass: a branch that fired
/// without its key would show up as a diff on every page.
pub const EXTRA_ORDER: &[&str] = &[
    // message flags and counters
    "outgoing",
    "channel_post",
    "pinned",
    "silent",
    "noforwards",
    "invert_media",
    "edit_hide",
    "offline",
    "mentioned",
    "media_unread",
    "from_scheduled",
    "from_rank",
    "from_boosts_applied",
    "paid_message_stars",
    "ttl_period",
    "effect_id",
    "views",
    "forwards",
    "replies_count",
    "restriction_reason",
    "fact_check",
    "grouped_id",
    "inline_buttons",
    // forward header
    "forwarded_date",
    "forwarded_date_unixtime",
    "forwarded_channel_post",
    "forwarded_post_author",
    "forwarded_imported",
    "forwarded_psa_type",
    // reply header
    "reply_to_quote",
    "reply_to_quote_entities",
    "reply_to_quote_offset",
    // service-action payloads Desktop names but empties out
    "message",
    "to",
    "boosts",
    "distance",
    "days",
    "slug",
    "stars",
    "via_giveaway",
    "winners_count",
    "unclaimed_count",
    "answer",
    "tasks",
    "completed",
    "incompleted",
    "gift_flags",
    // media
    "spoiler",
    "ttl_seconds",
    "stripped_thumbnail",
    "sticker_set",
    "voice_waveform",
    "has_video_cover",
    "alt_documents_count",
    "link_preview",
    "giveaway_information",
    "story_information",
    "paid_media_information",
    "todo_list",
    "video_stream_information",
];

/// `text` / `text_entities` / `reactions` always trail, in that order.
///
/// `reactions` being last is not incidental — the deliberate one-space
/// over-indent Desktop applies to it (see `json::desktop_reaction_indent`)
/// relies on everything after its opening bracket belonging to the array.
pub const TAIL_ORDER: &[&str] = &["text", "text_entities", "reactions"];

fn rank_table() -> &'static HashMap<&'static str, usize> {
    static TABLE: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    TABLE.get_or_init(|| {
        DESKTOP_ORDER
            .iter()
            .chain(EXTRA_ORDER.iter())
            .chain(TAIL_ORDER.iter())
            .enumerate()
            .map(|(i, k)| (*k, i))
            .collect()
    })
}

/// Total number of ranked keys; anything unlisted sorts here, then alphabetically.
pub fn tail_rank() -> usize {
    rank_table().len()
}

/// Rank of a key, for sorting.
pub fn rank(key: &str) -> usize {
    rank_table().get(key).copied().unwrap_or_else(tail_rank)
}

/// Re-key a message map into Desktop's order.
///
/// Unlisted keys keep a stable position at the end (ranked equal, broken
/// alphabetically) ahead of nothing — they land after `reactions`, which only
/// happens for a key nobody has classified yet, and is visible in a diff rather
/// than silently reordering the known ones.
pub fn ordered(fields: &Map<String, Value>) -> Map<String, Value> {
    let mut keys: Vec<&String> = fields.keys().collect();
    keys.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.as_str().cmp(b.as_str()))
    });
    let mut out = Map::new();
    for k in keys {
        out.insert(k.clone(), fields[k].clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn desktop_keys_lead_extras_follow_text_trails() {
        assert!(rank("id") < rank("date"));
        assert!(rank("date") < rank("outgoing"));
        assert!(rank("outgoing") < rank("text"));
        assert!(rank("text") < rank("text_entities"));
        assert!(rank("text_entities") < rank("reactions"));
    }

    #[test]
    fn reactions_is_the_last_ranked_key() {
        // The one-space over-indent depends on this and nothing else.
        let last = TAIL_ORDER.last().unwrap();
        assert_eq!(*last, "reactions");
        assert_eq!(rank("reactions"), tail_rank() - 1);
    }

    #[test]
    fn ordering_is_desktops_not_insertion_or_alphabetical() {
        let mut m = Map::new();
        m.insert("text".into(), json!("hi"));
        m.insert("date".into(), json!("2025-01-01T00:00:00"));
        m.insert("id".into(), json!(1));
        m.insert("type".into(), json!("message"));
        let out = ordered(&m);
        let keys: Vec<&str> = out.keys().map(|s| s.as_str()).collect();
        assert_eq!(keys, vec!["id", "type", "date", "text"]);
    }

    #[test]
    fn no_key_appears_in_two_blocks() {
        let mut seen = std::collections::HashSet::new();
        for k in DESKTOP_ORDER.iter().chain(EXTRA_ORDER).chain(TAIL_ORDER) {
            assert!(seen.insert(*k), "{k} is listed twice");
        }
    }

    #[test]
    fn unknown_keys_sort_last_and_stably() {
        let mut m = Map::new();
        m.insert("zzz_unknown".into(), json!(1));
        m.insert("aaa_unknown".into(), json!(1));
        m.insert("id".into(), json!(1));
        let keys: Vec<String> = ordered(&m).keys().cloned().collect();
        assert_eq!(keys, vec!["id", "aaa_unknown", "zzz_unknown"]);
    }
}
