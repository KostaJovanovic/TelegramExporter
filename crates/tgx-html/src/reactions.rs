//! Reactions, their reactor avatars, and the count Desktop shows only when it
//! has to.
//!
//! **Which reaction is yours never reaches `result.json`.** Desktop draws it as
//! `reaction active` in the HTML and writes nothing about it to the JSON, so it
//! travels in the presentation dict — see [`crate::userpic::Presentation`].
//!
//! **The count is drawn only when it exceeds the named reactors.** Telegram
//! names at most three per message and never names an anonymous one, so a
//! reaction with three names and a count of three shows the faces alone.

use crate::escape::{esc, safe_href};
use crate::tree::{a, Tree};
use crate::userpic::{userpic_box, Presentation};
use serde_json::{Map, Value};

/// What Desktop paints for a custom-emoji reaction.
///
/// It does not try to render the sticker inline, and all 11 in the reference
/// show this one glyph — so this is reproduced rather than improved on.
pub const CUSTOM_REACTION_GLYPH: &str = "👋";

/// The contents of one reaction's `<span class="emoji">`.
fn reaction_glyph(r: &Map<String, Value>) -> String {
    let kind = r.get("type").and_then(Value::as_str).unwrap_or("");
    if kind != "custom_emoji" {
        return esc(r.get("emoji").and_then(Value::as_str).unwrap_or(""));
    }
    let doc = match r.get("document_id") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    let glyph = esc(CUSTOM_REACTION_GLYPH);
    // A saved sticker is linked; a bare numeric id has no file to point at, so
    // it gets the toast Desktop's own script shows for a missing custom emoji.
    // The path came off the wire like everything else, so it is vetted first.
    let href = if !doc.is_empty() && (doc.contains('/') || doc.contains('.')) {
        safe_href(&doc)
    } else {
        None
    };
    match href {
        Some(h) => format!("<a href = \"{}\">{glyph}</a>", esc(&h)),
        None => format!("<a href=\"\" onclick=\"return ShowNotLoadedEmoji()\">{glyph}</a>"),
    }
}

pub fn render(t: &mut Tree, m: &Map<String, Value>, p: &Presentation) {
    let Some(Value::Array(reactions)) = m.get("reactions") else {
        return;
    };
    if reactions.is_empty() {
        return;
    }
    let chosen: Vec<bool> = p
        .get("reactions_chosen")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| v.as_bool().unwrap_or(false)).collect())
        .unwrap_or_default();

    t.open("span", &[a("class", "reactions")]);
    for (i, r) in reactions.iter().enumerate() {
        let Some(r) = r.as_object() else { continue };
        let active = if chosen.get(i).copied().unwrap_or(false) {
            " active"
        } else {
            ""
        };
        t.open("span", &[a("class", format!("reaction{active}"))]);
        t.leaf("span", &reaction_glyph(r), &[a("class", "emoji")]);

        let recent = r
            .get("recent")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if !recent.is_empty() {
            t.open("span", &[a("class", "userpics")]);
            for who in recent {
                let Some(who) = who.as_object() else { continue };
                let name = who.get("from").and_then(Value::as_str).unwrap_or("");
                let from_id = who.get("from_id").and_then(Value::as_str).unwrap_or("");
                userpic_box(t, p, name, from_id, 20, Some(name));
            }
            t.close("span");
        }

        let count = match r.get("count") {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
            _ => 0,
        };
        if count > recent.len() as i64 {
            t.leaf("span", &esc(&count.to_string()), &[a("class", "count")]);
        }
        t.close("span");
    }
    t.close("span");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn render_of(v: Value) -> String {
        let m = obj(v);
        let p = Presentation::of(&m);
        let mut t = Tree::new();
        render(&mut t, &m, &p);
        t.into_string()
    }

    #[test]
    fn no_reactions_emits_nothing() {
        assert_eq!(render_of(json!({ "id": 1 })), "");
        assert_eq!(render_of(json!({ "reactions": [] })), "");
    }

    #[test]
    fn the_count_is_hidden_when_every_reactor_is_named() {
        // Three names, count of three: the faces speak for themselves.
        let out = render_of(json!({ "reactions": [{
            "type": "emoji", "count": 3, "emoji": "❤",
            "recent": [
                { "from": "a", "from_id": "user1" },
                { "from": "b", "from_id": "user2" },
                { "from": "c", "from_id": "user3" }
            ]
        }]}));
        assert!(!out.contains("class=\"count\""), "got:\n{out}");
        assert_eq!(out.matches("class=\"userpic userpic").count(), 3);
    }

    #[test]
    fn the_count_appears_when_it_exceeds_the_named() {
        // Telegram names at most three and never names an anonymous reactor.
        let out = render_of(json!({ "reactions": [{
            "type": "emoji", "count": 10, "emoji": "❤",
            "recent": [{ "from": "a", "from_id": "user1" }]
        }]}));
        assert!(out.contains("class=\"count\""), "got:\n{out}");
        assert!(out.contains("\n10\n"), "got:\n{out}");
    }

    #[test]
    fn a_reaction_with_no_reactors_shows_its_count_alone() {
        let out = render_of(json!({ "reactions": [{
            "type": "emoji", "count": 4, "emoji": "❤"
        }]}));
        assert!(!out.contains("userpics"), "got:\n{out}");
        assert!(out.contains("class=\"count\""), "got:\n{out}");
    }

    #[test]
    fn the_reaction_you_picked_is_marked_active() {
        let m = obj(json!({
            "reactions": [
                { "type": "emoji", "count": 1, "emoji": "a" },
                { "type": "emoji", "count": 1, "emoji": "b" }
            ],
            "_p": { "reactions_chosen": [false, true] }
        }));
        let p = Presentation::of(&m);
        let mut t = Tree::new();
        render(&mut t, &m, &p);
        let out = t.into_string();
        assert!(out.contains("class=\"reaction\""), "got:\n{out}");
        assert!(out.contains("class=\"reaction active\""), "got:\n{out}");
    }

    #[test]
    fn a_custom_emoji_reaction_uses_the_waving_hand() {
        let out = render_of(json!({ "reactions": [{
            "type": "custom_emoji", "count": 1, "document_id": "12345"
        }]}));
        assert!(out.contains(CUSTOM_REACTION_GLYPH), "got:\n{out}");
        assert!(out.contains("ShowNotLoadedEmoji"), "got:\n{out}");
    }

    #[test]
    fn a_saved_custom_emoji_links_to_its_file() {
        let out = render_of(json!({ "reactions": [{
            "type": "custom_emoji", "count": 1, "document_id": "stickers/a.webp"
        }]}));
        assert!(out.contains("href = \"stickers/a.webp\""), "got:\n{out}");
    }

    #[test]
    fn a_custom_emoji_path_is_still_scheme_checked() {
        let out = render_of(json!({ "reactions": [{
            "type": "custom_emoji", "count": 1, "document_id": "javascript:alert(1)"
        }]}));
        assert!(!out.contains("javascript"), "got:\n{out}");
        assert!(out.contains("ShowNotLoadedEmoji"), "got:\n{out}");
    }

    #[test]
    fn a_hostile_emoji_string_is_escaped() {
        let out = render_of(json!({ "reactions": [{
            "type": "emoji", "count": 1, "emoji": "<img src=x onerror=alert(1)>"
        }]}));
        assert!(!out.contains("<img"), "got:\n{out}");
        assert!(out.contains("&lt;img"), "got:\n{out}");
    }

    #[test]
    fn more_chosen_flags_than_reactions_does_not_panic() {
        let m = obj(json!({
            "reactions": [{ "type": "emoji", "count": 1, "emoji": "a" }],
            "_p": { "reactions_chosen": [true, true, true] }
        }));
        let p = Presentation::of(&m);
        let mut t = Tree::new();
        render(&mut t, &m, &p);
        assert!(t.as_str().contains("reaction active"));
    }
}
