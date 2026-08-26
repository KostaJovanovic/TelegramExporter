//! Desktop's sentence for a service message.
//!
//! **Every part is escaped here.** A display name is attacker-controlled and an
//! archive opens as a local file, so an un-escaped actor is stored XSS that
//! fires when the archive is opened. The one value that arrives already-escaped
//! is `message_link`, which this module builds itself.

use crate::escape::esc;
use serde_json::{Map, Value};

fn s(m: &Map<String, Value>, k: &str) -> String {
    match m.get(k) {
        Some(Value::String(v)) => v.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Desktop lists whichever parts of a topic changed, comma-joined.
///
/// Its own output has **no space after that comma** — "changed topic title to
/// «foto video»,icon to «0»" — which is reproduced here.
fn topic_edit_text(actor: &str, m: &Map<String, Value>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if m.get("new_title").is_some() {
        parts.push(format!(
            "title to &laquo;{}&raquo;",
            esc(&s(m, "new_title"))
        ));
    }
    if m.get("new_icon_emoji_id").is_some() {
        parts.push(format!(
            "icon to &laquo;{}&raquo;",
            esc(&s(m, "new_icon_emoji_id"))
        ));
    }
    if let Some(v) = m.get("closed").and_then(Value::as_bool) {
        parts.push(if v { "closed" } else { "reopened" }.into());
    }
    if let Some(v) = m.get("hidden").and_then(Value::as_bool) {
        parts.push(if v { "hidden" } else { "shown" }.into());
    }
    if parts.is_empty() {
        format!("{actor} changed topic")
    } else {
        format!("{actor} changed topic {}", parts.join(","))
    }
}

/// The sentence for one service message.
pub fn service_text(m: &Map<String, Value>, message_link: Option<&str>) -> String {
    let actor = {
        let raw = s(m, "actor");
        esc(if raw.is_empty() { "Someone" } else { &raw })
    };
    let title = esc(&s(m, "title"));
    let members = esc(&m
        .get("members")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default());
    let action = s(m, "action");

    match action.as_str() {
        "create_group" => format!("{actor} created group &laquo;{title}&raquo;"),
        "create_channel" => format!("{actor} created channel &laquo;{title}&raquo;"),
        "edit_group_title" => {
            format!("{actor} changed group title to &laquo;{title}&raquo;")
        }
        "edit_group_photo" => format!("{actor} changed group photo"),
        "delete_group_photo" => format!("{actor} removed group photo"),
        "invite_members" => format!("{actor} invited {members}"),
        "remove_members" => format!("{actor} removed {members}"),
        "join_group_by_link" => format!(
            "{actor} joined group by link from {}",
            esc(&s(m, "inviter"))
        ),
        "join_group_by_request" => format!("{actor} joined group by request"),
        "pin_message" => match message_link {
            Some(link) => format!("{actor} pinned {link}"),
            None => format!("{actor} pinned message"),
        },
        "clear_history" => "History cleared".into(),
        "phone_call" => format!("{actor} made a call"),
        "group_call" => format!("{actor} started a voice chat"),
        "invite_to_group_call" => format!("{actor} invited {members} to the voice chat"),
        "topic_created" => format!("{actor} created topic &laquo;{title}&raquo;"),
        "topic_edit" => topic_edit_text(&actor, m),
        "joined_telegram" => format!("{actor} joined Telegram"),
        "migrate_to_supergroup" => "Group converted to supergroup".into(),
        "migrate_from_group" => {
            format!("{actor} converted a basic group to this supergroup &laquo;{title}&raquo;")
        }
        "score_in_game" => format!("{actor} scored {} in a game", esc(&s(m, "score"))),
        "send_payment" => format!("{actor} sent a payment"),
        "edit_chat_theme" => format!("{actor} changed chat theme"),
        "set_messages_ttl" => format!("{actor} changed messages auto-delete period"),
        "suggest_profile_photo" => format!("{actor} suggested a profile photo"),
        "poll_append_answer" => format!(
            "{actor} added &quot;{}&quot; to the poll.",
            esc(&s(m, "answer"))
        ),
        "poll_delete_answer" => format!(
            "{actor} removed &quot;{}&quot; from the poll.",
            esc(&s(m, "answer"))
        ),
        // Telegram adds action types faster than any exporter follows them.
        // Naming the unknown one is better than dropping the message.
        other => format!("{actor} &mdash; {}", esc(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn t(v: Value) -> String {
        service_text(&obj(v), None)
    }

    #[test]
    fn the_actions_the_reference_actually_uses() {
        // These nine are every action present in the reference export, by
        // frequency: join_group_by_link 26, invite_members 18, remove_members
        // 6, topic_created 3, poll_append_answer 3, pin_message 3,
        // topic_edit 2, migrate_from_group 1, edit_group_title 1.
        assert_eq!(
            t(json!({ "action": "join_group_by_link", "actor": "A", "inviter": "B" })),
            "A joined group by link from B"
        );
        assert_eq!(
            t(json!({ "action": "invite_members", "actor": "A", "members": ["B", "C"] })),
            "A invited B, C"
        );
        assert_eq!(
            t(json!({ "action": "remove_members", "actor": "A", "members": ["B"] })),
            "A removed B"
        );
        assert_eq!(
            t(json!({ "action": "topic_created", "actor": "A", "title": "foto video" })),
            "A created topic &laquo;foto video&raquo;"
        );
        assert_eq!(
            t(json!({ "action": "poll_append_answer", "actor": "A", "answer": "yes" })),
            "A added &quot;yes&quot; to the poll."
        );
        assert_eq!(
            t(json!({ "action": "edit_group_title", "actor": "A", "title": "X" })),
            "A changed group title to &laquo;X&raquo;"
        );
    }

    #[test]
    fn topic_edit_joins_with_no_space_after_the_comma() {
        // Desktop's own output: "changed topic title to «foto video»,icon to «0»".
        let out = t(json!({
            "action": "topic_edit", "actor": "A",
            "new_title": "foto video", "new_icon_emoji_id": "0"
        }));
        assert_eq!(
            out,
            "A changed topic title to &laquo;foto video&raquo;,icon to &laquo;0&raquo;"
        );
        assert!(!out.contains(", icon"), "a space crept in: {out}");
    }

    #[test]
    fn topic_edit_with_nothing_changed_still_reads() {
        assert_eq!(
            t(json!({ "action": "topic_edit", "actor": "A" })),
            "A changed topic"
        );
    }

    #[test]
    fn closed_and_hidden_read_both_ways() {
        assert!(
            t(json!({ "action": "topic_edit", "actor": "A", "closed": true })).ends_with("closed")
        );
        assert!(
            t(json!({ "action": "topic_edit", "actor": "A", "closed": false }))
                .ends_with("reopened")
        );
        assert!(
            t(json!({ "action": "topic_edit", "actor": "A", "hidden": true })).ends_with("hidden")
        );
        assert!(
            t(json!({ "action": "topic_edit", "actor": "A", "hidden": false })).ends_with("shown")
        );
    }

    #[test]
    fn a_pin_links_when_it_can_and_says_message_when_it_cannot() {
        let m = obj(json!({ "action": "pin_message", "actor": "A" }));
        assert_eq!(service_text(&m, None), "A pinned message");
        assert_eq!(
            service_text(&m, Some("<a href=\"#x\">this message</a>")),
            "A pinned <a href=\"#x\">this message</a>"
        );
    }

    #[test]
    fn an_attacker_controlled_actor_is_escaped() {
        // An archive opens as a local file; an un-escaped actor is stored XSS.
        let out = t(json!({
            "action": "invite_members",
            "actor": "<img src=x onerror=alert(1)>",
            "members": ["<script>alert(2)</script>"]
        }));
        assert!(!out.contains("<img"), "got {out}");
        assert!(!out.contains("<script"), "got {out}");
        assert!(out.contains("&lt;img"), "got {out}");
    }

    #[test]
    fn a_missing_actor_reads_as_someone() {
        assert_eq!(
            t(json!({ "action": "joined_telegram" })),
            "Someone joined Telegram"
        );
    }

    #[test]
    fn an_unknown_action_names_itself_rather_than_vanishing() {
        let out = t(json!({ "action": "something_new_in_2027", "actor": "A" }));
        assert_eq!(out, "A &mdash; something_new_in_2027");
    }

    #[test]
    fn an_unknown_action_name_is_also_escaped() {
        let out = t(json!({ "action": "<script>", "actor": "A" }));
        assert!(!out.contains("<script>"), "got {out}");
    }

    #[test]
    fn actions_with_no_actor_at_all_are_fixed_sentences() {
        assert_eq!(t(json!({ "action": "clear_history" })), "History cleared");
        assert_eq!(
            t(json!({ "action": "migrate_to_supergroup" })),
            "Group converted to supergroup"
        );
    }
}
