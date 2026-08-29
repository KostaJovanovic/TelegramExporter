//! Forum topic naming and message-to-topic routing.
//!
//! Topics exist on *forum supergroups*, not on broadcast channels. **The
//! General topic is special: it is not a real message thread**, so
//! `messages.getReplies` returns nothing for it. That is why routing is done by
//! inspecting each message's reply header during a single pass over the history
//! rather than by fetching each thread separately — it handles General
//! correctly and costs one pass regardless of how many topics exist.
//!
//! This module holds only the pure part, so the routing rules are testable with
//! no connection. The Telegram-facing discovery lives in `tgx-tg`.

pub const GENERAL_TOPIC_ID: i64 = 1;
pub const GENERAL_TITLE: &str = "General";

fn is_illegal(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20
}

fn is_reserved(name: &str) -> bool {
    let upper = name.to_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            if rest.len() == 1 && matches!(rest.chars().next(), Some('1'..='9')) {
                return true;
            }
        }
    }
    false
}

/// Windows-safe single path component, **emoji preserved**.
pub fn sanitize_component(name: &str, fallback: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if is_illegal(c) { '_' } else { c })
        .collect();
    // Collapse runs of whitespace, then trim.
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim().trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    let base = if is_reserved(trimmed) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    };
    // Trimmed *after* the cut, not only before it — mirrors
    // names::sanitize_filename. A 120+ character topic title can land the
    // truncation boundary on a trailing dot or space, and Windows silently
    // drops a trailing one, so the folder on disk would be "abc" while
    // result.json's topic name still read "abc." or "abc ": a folder that no
    // longer matches the name recorded for it.
    let cut: String = base.chars().take(120).collect();
    let out = cut.trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

/// `0042 - Backend` — id-prefixed so retitles and duplicates never collide.
///
/// Two topics with the same name, or a topic renamed mid-history, would
/// otherwise land in one folder.
pub fn topic_dirname(topic_id: i64, title: &str) -> String {
    format!("{topic_id:04} - {}", sanitize_component(title, "topic"))
}

/// The reply header fields routing needs, lifted out of the TL object.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReplyHeader {
    pub forum_topic: bool,
    pub reply_to_top_id: Option<i64>,
    pub reply_to_msg_id: Option<i64>,
}

/// Which topic a message belongs to.
///
/// * The service message that **creates** a topic has no reply header — its own
///   id is the topic id.
/// * `forum_topic` set → `reply_to_top_id`, else `reply_to_msg_id`.
/// * **A plain reply with no `forum_topic` flag lives in General.** This is the
///   case that looks wrong and is not: a reply inside General is still General.
/// * No reply header at all → General.
pub fn topic_id_for(msg_id: i64, creates_topic: bool, reply: Option<ReplyHeader>) -> i64 {
    if creates_topic {
        return msg_id;
    }
    let Some(reply) = reply else {
        return GENERAL_TOPIC_ID;
    };
    if !reply.forum_topic {
        return GENERAL_TOPIC_ID;
    }
    reply
        .reply_to_top_id
        .or(reply.reply_to_msg_id)
        .unwrap_or(GENERAL_TOPIC_ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_topic_creating_service_message_names_its_own_topic() {
        assert_eq!(topic_id_for(66, true, None), 66);
        // Even if it somehow carried a reply header.
        let r = ReplyHeader {
            forum_topic: true,
            reply_to_top_id: Some(9),
            reply_to_msg_id: None,
        };
        assert_eq!(topic_id_for(66, true, Some(r)), 66);
    }

    #[test]
    fn no_reply_header_is_general() {
        assert_eq!(topic_id_for(5, false, None), GENERAL_TOPIC_ID);
    }

    #[test]
    fn a_forum_reply_routes_to_its_top_id() {
        let r = ReplyHeader {
            forum_topic: true,
            reply_to_top_id: Some(42),
            reply_to_msg_id: Some(77),
        };
        assert_eq!(topic_id_for(5, false, Some(r)), 42);
    }

    #[test]
    fn a_forum_reply_without_a_top_id_falls_back_to_the_message_id() {
        // The first reply in a topic points straight at the opening message.
        let r = ReplyHeader {
            forum_topic: true,
            reply_to_top_id: None,
            reply_to_msg_id: Some(42),
        };
        assert_eq!(topic_id_for(5, false, Some(r)), 42);
    }

    #[test]
    fn a_plain_reply_with_no_forum_flag_is_general() {
        // This is the case that looks wrong and is not: a reply inside General
        // is still General, and General is not a real thread.
        let r = ReplyHeader {
            forum_topic: false,
            reply_to_top_id: Some(42),
            reply_to_msg_id: Some(77),
        };
        assert_eq!(topic_id_for(5, false, Some(r)), GENERAL_TOPIC_ID);
    }

    #[test]
    fn a_forum_reply_with_neither_id_is_general() {
        let r = ReplyHeader {
            forum_topic: true,
            reply_to_top_id: None,
            reply_to_msg_id: None,
        };
        assert_eq!(topic_id_for(5, false, Some(r)), GENERAL_TOPIC_ID);
    }

    #[test]
    fn topic_folders_are_id_prefixed_and_zero_padded() {
        assert_eq!(topic_dirname(1, "General"), "0001 - General");
        assert_eq!(topic_dirname(42, "Backend"), "0042 - Backend");
        assert_eq!(topic_dirname(12345, "Big"), "12345 - Big");
    }

    #[test]
    fn two_topics_with_the_same_title_never_collide() {
        assert_ne!(topic_dirname(42, "Design"), topic_dirname(43, "Design"));
    }

    #[test]
    fn emoji_survive_but_illegal_characters_do_not() {
        assert_eq!(topic_dirname(43, "Design 🎨"), "0043 - Design 🎨");
        assert_eq!(sanitize_component("a/b:c", "x"), "a_b_c");
    }

    #[test]
    fn whitespace_is_collapsed_and_trimmed() {
        assert_eq!(sanitize_component("  a   b  ", "x"), "a b");
    }

    #[test]
    fn control_characters_become_underscores_before_the_collapse() {
        // A tab and a newline are path-illegal on Windows, so they are
        // replaced *first* and the survivors are then collapsed as ordinary
        // text. "a \t\n b" is therefore "a __ b", not "a b" — the order of
        // those two steps is the whole of what decides which.
        assert_eq!(sanitize_component("  a \t\n b  ", "x"), "a __ b");
    }

    #[test]
    fn a_reserved_name_is_escaped() {
        assert_eq!(sanitize_component("CON", "x"), "_CON");
        assert_eq!(sanitize_component("com1", "x"), "_com1");
        assert_eq!(sanitize_component("COM10", "x"), "COM10");
    }

    #[test]
    fn an_empty_title_falls_back() {
        assert_eq!(sanitize_component("", "topic"), "topic");
        assert_eq!(sanitize_component("   ", "topic"), "topic");
        assert_eq!(sanitize_component("...", "topic"), "topic");
    }

    #[test]
    fn a_long_title_is_capped_by_characters_not_bytes() {
        // 120 emoji are 480 bytes; slicing by bytes would panic mid-character.
        let long = "🎨".repeat(200);
        let out = sanitize_component(&long, "x");
        assert_eq!(out.chars().count(), 120);
    }

    #[test]
    fn a_long_title_truncated_onto_a_dot_drops_it() {
        // Windows silently drops a trailing dot, so if truncation lands on
        // one the folder on disk stops matching the title result.json
        // records for the topic.
        let long = format!("{}.{}", "a".repeat(119), "b".repeat(50));
        let out = sanitize_component(&long, "x");
        assert!(!out.ends_with('.'), "got {out}");
        assert_eq!(out.chars().count(), 119);
    }

    #[test]
    fn a_long_title_truncated_onto_a_space_drops_it() {
        let long = format!("{} {}", "a".repeat(119), "b".repeat(50));
        let out = sanitize_component(&long, "x");
        assert!(!out.ends_with(' '), "got {out}");
        assert_eq!(out.chars().count(), 119);
    }

    #[test]
    fn the_reference_topic_titles_round_trip() {
        // The four real topics in the reference export.
        for (id, title) in [
            (1, "ćaskanje"),
            (12, "foto video"),
            (15, "editorijal"),
            (66, "bitno pročitaj"),
        ] {
            let d = topic_dirname(id, title);
            assert!(d.ends_with(title), "got {d}");
            assert!(d.starts_with(&format!("{id:04}")), "got {d}");
        }
    }
}
