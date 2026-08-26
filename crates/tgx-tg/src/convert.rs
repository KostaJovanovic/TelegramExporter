//! TL message → Desktop's ordered JSON map.
//!
//! This is the one module in the pipeline that a parity harness **cannot**
//! verify, and it is worth being explicit about why: the reference export
//! records Desktop's *output*, not Telegram's *input*, so there is no recorded
//! TL message to replay through here. Everything downstream of this module is
//! pinned byte for byte; this module is pinned only by a live run.
//!
//! Two consequences shape the code:
//!
//! * Every value that reaches the map goes through [`tgx_format`] rather than
//!   being formatted here, so the parts that *are* verified stay verified.
//! * Nothing is emitted unless its source field is actually present. An
//!   ordinary message must gain **no** keys beyond Desktop's own — that is what
//!   lets the HTML leg keep reading 4 of 4.

use grammers_tl_types as tl;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use tgx_format::order::ordered;
use tgx_format::peer::PeerKey;
use tgx_format::text::{build_text_entities, build_text_field, Entity};

/// Display names by typed peer key, plus the variants only the HTML needs.
#[derive(Debug, Default, Clone)]
pub struct NameBook {
    /// The trimmed display name, which is what `result.json` carries.
    pub names: HashMap<String, String>,
    /// Desktop's HTML name: `first + " " + last`, **untrimmed**.
    pub html: HashMap<String, String>,
    /// The userpic letters, from the name *fields*.
    pub initials: HashMap<String, String>,
    /// A self-chosen name colour, numbered past the seven-entry palette.
    pub colour: HashMap<String, i64>,
}

impl NameBook {
    /// Learn a user from the fields that matter, rather than from the whole TL
    /// object.
    ///
    /// grammers regenerates `tl::types::User` from Telegram's schema and it has
    /// gained fields three times in recent releases; taking the parts keeps
    /// this testable without a 60-field fixture that rots on every bump.
    pub fn learn_user_parts(
        &mut self,
        id: i64,
        first: &str,
        last: &str,
        username: &str,
        deleted: bool,
        colour: Option<i64>,
    ) {
        let key = PeerKey::user(id).to_string();
        if let Some(n) = tgx_format::peer::display_name(first, last, username, deleted) {
            self.names.insert(key.clone(), n);
        }
        if let Some(n) = tgx_format::peer::html_name(first, last, username, deleted) {
            self.html.insert(key.clone(), n);
        }
        let letters = if deleted {
            "D".to_string()
        } else if first.is_empty() && last.is_empty() {
            username
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default()
        } else {
            tgx_format::peer::initials_from_fields(first, last)
        };
        self.initials.insert(key.clone(), letters);
        if let Some(v) = colour {
            self.colour.insert(key, v);
        }
    }

    pub fn learn_user(&mut self, u: &tl::types::User) {
        // `color` is an enum: a plain palette index, or one of the collectible
        // forms which carry no index at all. Only the plain one is a colour
        // Desktop would write.
        let colour = match &u.color {
            Some(tl::enums::PeerColor::Color(c)) => c.color.map(|v| v as i64),
            _ => None,
        };
        self.learn_user_parts(
            u.id,
            u.first_name.as_deref().unwrap_or(""),
            u.last_name.as_deref().unwrap_or(""),
            u.username.as_deref().unwrap_or(""),
            u.deleted,
            colour,
        );
    }

    pub fn learn_chat_title(&mut self, key: PeerKey, title: &str) {
        let k = key.to_string();
        self.names.insert(k.clone(), title.to_string());
        self.html.insert(k.clone(), title.to_string());
        if let Some(i) = tgx_format::peer::initials_from_title(title) {
            self.initials.insert(k, i);
        }
    }

    pub fn get(&self, key: &str) -> &str {
        self.names.get(key).map(String::as_str).unwrap_or("")
    }

    pub fn html_name(&self, key: &str) -> &str {
        self.html
            .get(key)
            .or_else(|| self.names.get(key))
            .map(String::as_str)
            .unwrap_or("")
    }
}

/// Desktop writes peers as `user123` / `chat123` / `channel123`.
pub fn peer_key(peer: &tl::enums::Peer) -> PeerKey {
    match peer {
        tl::enums::Peer::User(p) => PeerKey::user(p.user_id),
        tl::enums::Peer::Chat(p) => PeerKey::chat(p.chat_id),
        tl::enums::Peer::Channel(p) => PeerKey::channel(p.channel_id),
    }
}

/// Desktop's name for each entity type.
fn entity_kind(e: &tl::enums::MessageEntity) -> Option<&'static str> {
    use tl::enums::MessageEntity as E;
    Some(match e {
        E::Bold(_) => "bold",
        E::Italic(_) => "italic",
        E::Underline(_) => "underline",
        E::Strike(_) => "strikethrough",
        E::Spoiler(_) => "spoiler",
        E::Code(_) => "code",
        E::Pre(_) => "pre",
        E::TextUrl(_) => "text_link",
        E::Url(_) => "link",
        E::Email(_) => "email",
        E::Phone(_) => "phone",
        E::Mention(_) => "mention",
        E::MentionName(_) => "mention_name",
        E::Hashtag(_) => "hashtag",
        E::Cashtag(_) => "cashtag",
        E::BotCommand(_) => "bot_command",
        E::BankCard(_) => "bank_card",
        E::Blockquote(_) => "blockquote",
        E::CustomEmoji(_) => "custom_emoji",
        // Telegram adds entity types faster than any exporter follows them.
        // An unmapped one is dropped from the segmentation rather than
        // guessed at — the text still comes through as plain.
        _ => return None,
    })
}

/// The offset and length of an entity, or `None` for a variant that carries
/// neither.
///
/// A catch-all rather than an exhaustive match on purpose: grammers regenerates
/// this enum from Telegram's schema, so a new variant must not break the build.
/// Returning `None` drops the entity from the segmentation, which leaves the
/// text intact as plain — the same degradation an unmapped *kind* gets.
fn entity_offset_len(e: &tl::enums::MessageEntity) -> Option<(i64, i64)> {
    use tl::enums::MessageEntity as E;
    macro_rules! ol {
        ($v:expr) => {
            Some(($v.offset as i64, $v.length as i64))
        };
    }
    match e {
        E::Unknown(v) => ol!(v),
        E::Mention(v) => ol!(v),
        E::Hashtag(v) => ol!(v),
        E::BotCommand(v) => ol!(v),
        E::Url(v) => ol!(v),
        E::Email(v) => ol!(v),
        E::Bold(v) => ol!(v),
        E::Italic(v) => ol!(v),
        E::Code(v) => ol!(v),
        E::Pre(v) => ol!(v),
        E::TextUrl(v) => ol!(v),
        E::MentionName(v) => ol!(v),
        E::InputMessageEntityMentionName(v) => ol!(v),
        E::Phone(v) => ol!(v),
        E::Cashtag(v) => ol!(v),
        E::Underline(v) => ol!(v),
        E::Strike(v) => ol!(v),
        E::BankCard(v) => ol!(v),
        E::Spoiler(v) => ol!(v),
        E::CustomEmoji(v) => ol!(v),
        E::Blockquote(v) => ol!(v),
        _ => None,
    }
}

fn entity_extras(e: &tl::enums::MessageEntity) -> Vec<(&'static str, Value)> {
    use tl::enums::MessageEntity as E;
    match e {
        E::TextUrl(v) => vec![("href", json!(v.url))],
        E::MentionName(v) => vec![("user_id", json!(v.user_id))],
        E::Pre(v) if !v.language.is_empty() => vec![("language", json!(v.language))],
        // Desktop writes the *file path* into document_id once the emoji has
        // been saved; until then it is the bare id, as a string.
        E::CustomEmoji(v) => vec![("document_id", json!(v.document_id.to_string()))],
        _ => vec![],
    }
}

/// Convert the wire entities into the shape `tgx_format::text` expects.
pub fn entities_of(raw: Option<&Vec<tl::enums::MessageEntity>>) -> Vec<Entity> {
    let Some(raw) = raw else { return Vec::new() };
    raw.iter()
        .filter_map(|e| {
            let kind = entity_kind(e)?;
            let (offset, length) = entity_offset_len(e)?;
            Some(Entity {
                offset,
                length,
                kind,
                extras: entity_extras(e),
            })
        })
        .collect()
}

/// Desktop's `date` / `date_unixtime` pair.
fn put_date(map: &mut Map<String, Value>, key: &str, unix_key: &str, ts: i32) {
    if let Some((date, unix)) = tgx_format::date_pair(ts as i64) {
        map.insert(key.into(), json!(date));
        map.insert(unix_key.into(), json!(unix));
    }
}

/// The core of an ordinary message, before media and extras.
pub fn base_message(m: &tl::types::Message, names: &NameBook) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("id".into(), json!(m.id));
    out.insert("type".into(), json!("message"));
    put_date(&mut out, "date", "date_unixtime", m.date);
    if let Some(edit) = m.edit_date {
        put_date(&mut out, "edited", "edited_unixtime", edit);
    }

    if let Some(from) = &m.from_id {
        let key = peer_key(from);
        out.insert("from".into(), json!(names.get(&key.to_string())));
        out.insert("from_id".into(), json!(key.to_string()));
    }
    if let Some(author) = &m.post_author {
        out.insert("author".into(), json!(author));
    }

    if let Some(fwd) = &m.fwd_from {
        let tl::enums::MessageFwdHeader::Header(fwd) = fwd;
        if let Some(peer) = &fwd.from_id {
            let key = peer_key(peer);
            out.insert("forwarded_from".into(), json!(names.get(&key.to_string())));
            out.insert("forwarded_from_id".into(), json!(key.to_string()));
        } else if let Some(name) = &fwd.from_name {
            // A forward from someone who hides their account: a name and
            // nothing else.
            out.insert("forwarded_from".into(), json!(name));
        }
    }

    if let Some(via) = m.via_bot_id {
        let key = PeerKey::user(via).to_string();
        out.insert("via_bot".into(), json!(names.get(&key)));
    }

    if let Some(tl::enums::MessageReplyHeader::Header(r)) = &m.reply_to {
        if let Some(id) = r.reply_to_msg_id {
            out.insert("reply_to_message_id".into(), json!(id));
        }
        // Telegram marks a reply whose target lives outside this history, and
        // Desktop does not pretend it can link there.
        if let Some(peer) = &r.reply_to_peer_id {
            out.insert("reply_to_peer_id".into(), json!(peer_key(peer).to_string()));
        }
    }

    let entities = entities_of(m.entities.as_ref());
    let segments = build_text_entities(&m.message, &entities);
    out.insert("text".into(), build_text_field(&segments));
    out.insert("text_entities".into(), Value::Array(segments));

    ordered(&out)
}

/// The core of a service message.
pub fn base_service(m: &tl::types::MessageService, names: &NameBook) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("id".into(), json!(m.id));
    out.insert("type".into(), json!("service"));
    put_date(&mut out, "date", "date_unixtime", m.date);
    if let Some(from) = &m.from_id {
        let key = peer_key(from);
        out.insert("actor".into(), json!(names.get(&key.to_string())));
        out.insert("actor_id".into(), json!(key.to_string()));
    }
    // Desktop writes these even on a service message, always empty.
    out.insert("text".into(), json!(""));
    out.insert("text_entities".into(), json!([]));
    ordered(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer_user(id: i64) -> tl::enums::Peer {
        tl::enums::Peer::User(tl::types::PeerUser { user_id: id })
    }

    fn blank_message() -> tl::types::Message {
        tl::types::Message {
            out: false,
            mentioned: false,
            media_unread: false,
            silent: false,
            post: false,
            from_scheduled: false,
            legacy: false,
            edit_hide: false,
            pinned: false,
            noforwards: false,
            invert_media: false,
            offline: false,
            video_processing_pending: false,
            paid_suggested_post_stars: false,
            paid_suggested_post_ton: false,
            id: 1,
            from_id: None,
            from_boosts_applied: None,
            from_rank: None,
            peer_id: peer_user(1),
            saved_peer_id: None,
            fwd_from: None,
            via_bot_id: None,
            via_business_bot_id: None,
            guestchat_via_from: None,
            reply_to: None,
            date: 1_766_071_072,
            message: String::new(),
            media: None,
            reply_markup: None,
            entities: None,
            views: None,
            forwards: None,
            replies: None,
            edit_date: None,
            post_author: None,
            grouped_id: None,
            reactions: None,
            restriction_reason: None,
            ttl_period: None,
            quick_reply_shortcut_id: None,
            effect: None,
            factcheck: None,
            report_delivery_until_date: None,
            paid_message_stars: None,
            suggested_post: None,
            schedule_repeat_period: None,
            summary_from_language: None,
            rich_message: None,
        }
    }

    #[test]
    fn peer_keys_are_typed() {
        assert_eq!(peer_key(&peer_user(7)).to_string(), "user7");
        assert_eq!(
            peer_key(&tl::enums::Peer::Channel(tl::types::PeerChannel {
                channel_id: 7
            }))
            .to_string(),
            "channel7"
        );
    }

    #[test]
    fn an_ordinary_message_gains_no_keys_beyond_desktops() {
        // This is the invariant that keeps the HTML leg at 4 of 4: a branch
        // that fired without its source field would show up as a diff on
        // every page.
        let m = blank_message();
        let out = base_message(&m, &NameBook::default());
        let keys: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "id",
                "type",
                "date",
                "date_unixtime",
                "text",
                "text_entities"
            ]
        );
    }

    #[test]
    fn keys_come_out_in_desktops_order() {
        let mut m = blank_message();
        m.from_id = Some(peer_user(5));
        m.edit_date = Some(1_766_071_100);
        let out = base_message(&m, &NameBook::default());
        let keys: Vec<&str> = out.keys().map(String::as_str).collect();
        // id, type, date, date_unixtime, edited, edited_unixtime, from, from_id, …
        assert_eq!(keys[0], "id");
        assert_eq!(keys[4], "edited");
        assert_eq!(keys[6], "from");
        assert_eq!(keys[7], "from_id");
        assert_eq!(keys[keys.len() - 1], "text_entities");
    }

    #[test]
    fn utf16_offsets_survive_the_round_trip() {
        // The invariant that silently corrupts every message with an emoji.
        let mut m = blank_message();
        m.message = "👍 bold".into();
        m.entities = Some(vec![tl::enums::MessageEntity::Bold(
            tl::types::MessageEntityBold {
                offset: 3,
                length: 4,
            },
        )]);
        let out = base_message(&m, &NameBook::default());
        let segs = out["text_entities"].as_array().unwrap();
        let bold: Vec<&Value> = segs.iter().filter(|s| s["type"] == "bold").collect();
        assert_eq!(bold.len(), 1);
        assert_eq!(bold[0]["text"], "bold");
    }

    #[test]
    fn an_unmapped_entity_type_does_not_lose_the_text() {
        let mut m = blank_message();
        m.message = "hello".into();
        m.entities = Some(vec![tl::enums::MessageEntity::Unknown(
            tl::types::MessageEntityUnknown {
                offset: 0,
                length: 5,
            },
        )]);
        let out = base_message(&m, &NameBook::default());
        assert_eq!(out["text"], "hello");
    }

    #[test]
    fn a_service_message_still_carries_empty_text_fields() {
        let s = tl::types::MessageService {
            out: false,
            mentioned: false,
            media_unread: false,
            reactions_are_possible: false,
            silent: false,
            post: false,
            legacy: false,
            id: 66,
            from_id: Some(peer_user(9)),
            peer_id: peer_user(1),
            saved_peer_id: None,
            reply_to: None,
            date: 1_766_071_072,
            action: tl::enums::MessageAction::Empty,
            reactions: None,
            ttl_period: None,
        };
        let out = base_service(&s, &NameBook::default());
        assert_eq!(out["type"], "service");
        assert_eq!(out["text"], "");
        assert_eq!(out["text_entities"], json!([]));
        assert_eq!(out["actor_id"], "user9");
    }

    #[test]
    fn a_hidden_forward_carries_a_name_and_no_peer() {
        let mut m = blank_message();
        m.fwd_from = Some(tl::enums::MessageFwdHeader::Header(
            tl::types::MessageFwdHeader {
                imported: false,
                saved_out: false,
                from_id: None,
                from_name: Some("Немања Фарм ГМ".into()),
                date: 1_766_000_000,
                channel_post: None,
                post_author: None,
                saved_from_peer: None,
                saved_from_msg_id: None,
                saved_from_id: None,
                saved_from_name: None,
                saved_date: None,
                psa_type: None,
            },
        ));
        let out = base_message(&m, &NameBook::default());
        assert_eq!(out["forwarded_from"], "Немања Фарм ГМ");
        assert!(!out.contains_key("forwarded_from_id"));
    }

    #[test]
    fn the_name_book_keeps_trimmed_and_untrimmed_apart() {
        let mut b = NameBook::default();
        b.learn_user_parts(1, "Ivana", "", "", false, None);
        // 94 of 601 names on one reference page end in a space. The two really
        // are different strings and both have to be kept.
        assert_eq!(b.get("user1"), "Ivana");
        assert_eq!(b.html_name("user1"), "Ivana ");
    }

    #[test]
    fn initials_come_from_the_fields_not_the_joined_string() {
        let mut b = NameBook::default();
        b.learn_user_parts(
            1,
            "Nada",
            "Gavrilovic arh blokade fotograf",
            "",
            false,
            None,
        );
        assert_eq!(b.initials.get("user1").unwrap(), "NG");
    }

    #[test]
    fn a_deleted_account_is_named_and_lettered() {
        let mut b = NameBook::default();
        b.learn_user_parts(1, "", "", "", true, None);
        assert_eq!(b.get("user1"), "Deleted Account");
        assert_eq!(b.initials.get("user1").unwrap(), "D");
    }

    #[test]
    fn a_self_chosen_colour_is_kept() {
        let mut b = NameBook::default();
        b.learn_user_parts(1, "A", "B", "", false, Some(11));
        assert_eq!(b.colour.get("user1"), Some(&11));
    }
}
