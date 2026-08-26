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
            // `from_name` is the fallback, not only the hidden-forward case:
            // Telegram sends both when the source is a peer we may not know,
            // and preferring the empty lookup over a name we were handed wrote
            // `"forwarded_from": ""` on 96 messages in a live export.
            let known = names.get(&key.to_string());
            let name = if known.is_empty() {
                fwd.from_name.as_deref().unwrap_or("")
            } else {
                known
            };
            out.insert("forwarded_from".into(), json!(name));
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
    // **Falls back to the chat.** A migration notice has no sender — the group
    // itself did it — and Desktop still writes an actor: the reference's one
    // `migrate_from_group` carries `"actor": "UA KOLAB"`,
    // `"actor_id": "channel3586682625"`. Keying only on `from_id` dropped both
    // keys from that message.
    if let Some(from) = m.from_id.as_ref().or(Some(&m.peer_id)) {
        let key = peer_key(from);
        out.insert("actor".into(), json!(names.get(&key.to_string())));
        out.insert("actor_id".into(), json!(key.to_string()));
    }
    // Desktop writes these even on a service message, always empty.
    out.insert("text".into(), json!(""));
    out.insert("text_entities".into(), json!([]));
    if let Some((action, payload)) = service_action(&m.action, m, names) {
        out.insert("action".into(), json!(action));
        for (k, v) in payload {
            out.insert(k, v);
        }
    }
    ordered(&out)
}

/// The presentation-only map the HTML writer reads and `result.json` never
/// sees.
///
/// **The engine never built this.** `grep '"_p"' crates/tgx-tg/src/` returned
/// nothing, so every key below was absent from every live export, and the
/// writer took its fallback path each time. What that cost, measured on a real
/// run against the same chat Desktop exported:
///
/// * **Zero `<img>` elements in the entire archive** — 649 in Desktop's, 0 in
///   ours. `render_media` gates every inline preview on `_p.preview`, so every
///   photo, sticker, animation and video came out as a coloured placeholder
///   row: 129 `photo_wrap` and 77 `sticker_wrap` in one topic became 0 and 0,
///   and `class="fill"` went from 47 to 306.
/// * Userpic letters derived from the display *string* rather than the name
///   fields, no self-chosen name colours, no forward-date tooltips, no
///   `reaction active`, and the `stripped_thumbnail` files we do write on disk
///   referenced by nothing.
///
/// **The html leg reads 4 of 4 anyway**, because `html_leg.rs` lifts `_p` out
/// of Desktop's own HTML before replaying — it proves the writer, not the
/// pipeline. This function is the pipeline's half, and `tests/wire.rs` is where
/// it is held to the same shape.
///
/// `out` is the message map as it will be written, so the media paths and
/// dimensions the plan already decided are read back from it rather than
/// recomputed from the TL object and allowed to disagree.
pub fn presentation(
    m: &tl::types::Message,
    out: &Map<String, Value>,
    names: &NameBook,
    preview_src: Option<&str>,
) -> Option<Map<String, Value>> {
    let mut p = Map::new();
    let s = |k: &str| out.get(k).and_then(Value::as_str).unwrap_or("");

    // Desktop's HTML name is `first + " " + last` **untrimmed**, which is not
    // the name `result.json` carries — hence a second table rather than a reuse.
    if let Some(from) = &m.from_id {
        let key = peer_key(from).to_string();
        let html = names.html_name(&key);
        if !html.is_empty() {
            p.insert("from_name".into(), json!(html));
        }
    }
    if let Some(fwd) = &m.fwd_from {
        let tl::enums::MessageFwdHeader::Header(fwd) = fwd;
        if let Some(peer) = &fwd.from_id {
            let html = names.html_name(&peer_key(peer).to_string());
            if !html.is_empty() {
                p.insert("forwarded_from_name".into(), json!(html));
            }
        }
        if let Some((date, _)) = tgx_format::date_pair(fwd.date as i64) {
            p.insert("forwarded_date".into(), json!(date));
        }
    }
    // Albums are one block in the HTML and separate messages in the JSON.
    if let Some(g) = m.grouped_id {
        p.insert("group".into(), json!(g.to_string()));
    }

    // The avatars Desktop paints, keyed by the peer each belongs to. A hidden
    // forward has no peer of its own, so Desktop keys it off the message id.
    let mut initials = Map::new();
    let mut colours = Map::new();
    let mut learn = |key: &str| {
        if key.is_empty() {
            return;
        }
        if let Some(v) = names.initials.get(key) {
            initials.insert(key.to_string(), json!(v));
        }
        if let Some(c) = names.colour.get(key) {
            // Zero-based here, one-based in the palette: `userpic_class` adds
            // the one back. The harness records it the same way.
            colours.insert(key.to_string(), json!(*c - 1));
        }
    };
    learn(s("from_id"));
    if out.contains_key("forwarded_from") {
        let fwd_peer = s("forwarded_from_id");
        let own = m.id.to_string();
        learn(if fwd_peer.is_empty() { &own } else { fwd_peer });
    }
    // Everyone Desktop names under a reaction gets an avatar too.
    if let Some(Value::Array(rs)) = out.get("reactions") {
        for r in rs {
            for who in r
                .get("recent")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                learn(who.get("from_id").and_then(Value::as_str).unwrap_or(""));
            }
        }
    }
    if !initials.is_empty() {
        p.insert("initials".into(), Value::Object(initials));
    }
    if !colours.is_empty() {
        p.insert("colours".into(), Value::Object(colours));
    }

    // Which reactions *we* pressed, in the order they appear.
    if let Some(r) = &m.reactions {
        let tl::enums::MessageReactions::Reactions(r) = r;
        let chosen: Vec<Value> = r
            .results
            .iter()
            .map(|e| {
                let tl::enums::ReactionCount::Count(e) = e;
                json!(e.chosen_order.is_some())
            })
            .collect();
        if chosen.iter().any(|v| v == &json!(true)) {
            p.insert("reactions_chosen".into(), Value::Array(chosen));
        }
    }

    if let Some(pv) = preview_of(out, preview_src) {
        p.insert("preview".into(), pv);
    }
    (!p.is_empty()).then_some(p)
}

/// The `<img>` Desktop would put inline for this message, and its CSS size.
///
/// Reads the paths the plan already wrote, so the preview cannot point
/// somewhere the media does not. A placeholder — a file skipped for size — has
/// no preview, which is what makes those messages render as a row.
fn preview_of(out: &Map<String, Value>, preview_src: Option<&str>) -> Option<Value> {
    let s = |k: &str| out.get(k).and_then(Value::as_str).unwrap_or("");
    let n = |k: &str| out.get(k).and_then(Value::as_i64).unwrap_or(0);
    let is_ph = |v: &str| v.starts_with("(File");

    let sized = |src: &str, box_size: i64| {
        let (_, css) = tgx_html::preview::preview_size(n("width"), n("height"), box_size);
        Some(json!({ "src": src, "width": css.0, "height": css.1 }))
    };

    // **The planner's name, never one derived here.** `claim` adds a `(1)` on
    // collision, so a path recomputed from `photo` would silently differ from
    // the file the pool is going to write — the dangling reference this whole
    // change exists to remove, reintroduced one layer up.
    let photo = s("photo");
    if !photo.is_empty() && !is_ph(photo) {
        return sized(preview_src?, tgx_html::preview::PREVIEW_BOX);
    }
    let file = s("file");
    if file.is_empty() || is_ph(file) {
        return None;
    }
    let kind = s("media_type");
    let lower = file.to_lowercase();
    if kind == "sticker" && (lower.ends_with(".webp") || lower.ends_with(".png")) {
        return sized(preview_src?, tgx_html::preview::STICKER_BOX);
    }
    if matches!(kind, "video_file" | "video_message" | "animation") {
        // A video's inline image is Telegram's own thumbnail — the `thumbnail`
        // key, already on the map — not a rendered downscale, and only when we
        // actually saved one.
        let thumb = s("thumbnail");
        if !thumb.is_empty() && !is_ph(thumb) {
            return sized(thumb, tgx_html::preview::PREVIEW_BOX);
        }
    }
    None
}

/// Desktop's `reactions` array for a message that has any.
///
/// **This did not exist either.** 963 of the reference's 6,643 messages carry
/// reactions and a live export carried none, on any message, ever — and the
/// wire leg scored it as *honest drift*, because `reactions` is on its
/// may-differ list. That licence is about a reaction added between two runs
/// changing a *count*; it was reading a field we never wrote at all as two runs
/// at two moments. `enrich.rs` has had `reactions_are_truncated` since the
/// beginning, so the decision about whether to re-fetch was built before the
/// thing that emits them.
///
/// Shape measured from the reference: `{type, count, emoji | document_id}` plus
/// `recent` when Telegram named anyone — 1,056 of the 1,085 entries have it.
///
/// `document_id` stays the numeric id rather than becoming a sticker path. That
/// is the documented custom-emoji ceiling: Desktop downloads the emoji's
/// document and rewrites the field to point at it, and the media leg's 830 of
/// 836 is the same six files.
pub fn reactions_of(r: &tl::types::MessageReactions, names: &NameBook) -> Option<Value> {
    let recent = r.recent_reactions.as_deref().unwrap_or(&[]);
    let mut out = Vec::new();
    for entry in &r.results {
        let tl::enums::ReactionCount::Count(entry) = entry;
        let mut one = Map::new();
        let (kind, key, value) = match &entry.reaction {
            tl::enums::Reaction::Emoji(e) => ("emoji", "emoji", json!(e.emoticon)),
            tl::enums::Reaction::CustomEmoji(e) => (
                "custom_emoji",
                "document_id",
                json!(e.document_id.to_string()),
            ),
            // `reactionEmpty` and anything Telegram adds later. Skipped rather
            // than written as a reaction with no identity.
            _ => continue,
        };
        one.insert("type".into(), json!(kind));
        one.insert("count".into(), json!(entry.count));
        one.insert(key.into(), value);

        // Telegram names at most a few reactors per *message*, not per
        // reaction, so each entry takes the ones whose reaction matches it.
        let named: Vec<Value> = recent
            .iter()
            .filter_map(|p| {
                let tl::enums::MessagePeerReaction::Reaction(p) = p;
                if !same_reaction(&p.reaction, &entry.reaction) {
                    return None;
                }
                let key = peer_key(&p.peer_id);
                let mut m = Map::new();
                m.insert("from".into(), json!(names.get(&key.to_string())));
                m.insert("from_id".into(), json!(key.to_string()));
                if let Some((date, _)) = tgx_format::date_pair(p.date as i64) {
                    m.insert("date".into(), json!(date));
                }
                Some(Value::Object(m))
            })
            .collect();
        if !named.is_empty() {
            one.insert("recent".into(), Value::Array(named));
        }
        out.push(Value::Object(one));
    }
    (!out.is_empty()).then(|| Value::Array(out))
}

fn same_reaction(a: &tl::enums::Reaction, b: &tl::enums::Reaction) -> bool {
    use tl::enums::Reaction as R;
    match (a, b) {
        (R::Emoji(x), R::Emoji(y)) => x.emoticon == y.emoticon,
        (R::CustomEmoji(x), R::CustomEmoji(y)) => x.document_id == y.document_id,
        _ => false,
    }
}

/// Desktop's `poll` object.
///
/// All seven polls in the reference came out of a live export as an ordinary
/// text message with no `poll` key: `plan::classify` has no `Poll` arm, so the
/// media was simply not recognised. Shape is fixed — `question`, `closed`,
/// `total_voters`, `answers[{text, voters, chosen}]` — with no variation across
/// the seven.
///
/// `voters` and `chosen` come from `results`, which Telegram omits entirely
/// before anyone has voted; a poll in that state reports every answer at zero
/// rather than losing its answers.
pub fn poll_of(m: &tl::types::MessageMediaPoll) -> Value {
    let tl::enums::Poll::Poll(poll) = &m.poll;
    let tl::enums::PollResults::Results(results) = &m.results;
    let tallies = results.results.as_deref().unwrap_or(&[]);

    let answers: Vec<Value> = poll
        .answers
        .iter()
        .filter_map(|a| {
            let tl::enums::PollAnswer::Answer(a) = a else {
                return None;
            };
            let tally = tallies.iter().find_map(|t| {
                let tl::enums::PollAnswerVoters::Voters(t) = t;
                (t.option == a.option).then_some(t)
            });
            let tl::enums::TextWithEntities::Entities(text) = &a.text;
            let mut one = Map::new();
            one.insert("text".into(), json!(text.text));
            one.insert(
                "voters".into(),
                json!(tally.and_then(|t| t.voters).unwrap_or(0)),
            );
            one.insert("chosen".into(), json!(tally.is_some_and(|t| t.chosen)));
            Some(Value::Object(one))
        })
        .collect();

    let tl::enums::TextWithEntities::Entities(question) = &poll.question;
    let mut out = Map::new();
    out.insert("question".into(), json!(question.text));
    out.insert("closed".into(), json!(poll.closed));
    out.insert(
        "total_voters".into(),
        json!(results.total_voters.unwrap_or(0)),
    );
    out.insert("answers".into(), Value::Array(answers));
    Value::Object(out)
}

/// Desktop's `location_information`, and the live-location period beside it.
///
/// Two messages in the reference carry a plain point and one carries a live
/// one; a live export wrote neither, for the same reason as the poll — no arm
/// in `classify`, so the media went unrecognised and the message came out as
/// bare text.
pub fn location_of(media: &tl::enums::MessageMedia) -> Option<(Value, Option<i32>)> {
    let (geo, period) = match media {
        tl::enums::MessageMedia::Geo(g) => (&g.geo, None),
        tl::enums::MessageMedia::GeoLive(g) => (&g.geo, Some(g.period)),
        _ => return None,
    };
    let tl::enums::GeoPoint::Point(p) = geo else {
        return None;
    };
    let mut out = Map::new();
    // Desktop writes latitude first, and `order.rs` does not reach inside a
    // nested object, so the insertion order here is the file's order.
    out.insert("latitude".into(), json!(p.lat));
    out.insert("longitude".into(), json!(p.long));
    Some((Value::Object(out), period))
}

/// Desktop's `action` name for a service message, and the fields it carries.
///
/// **This did not exist.** `base_service` wrote id/type/date/actor/text and
/// stopped, so all 63 service messages in a live export came out as
/// `"type": "service"` with no `action` at all — and with it went `inviter`,
/// `members`, `title`, `new_title`, `message_id` and `new_icon_emoji_id`. No
/// replay leg can see this: all three start from a Desktop `result.json` that
/// already *has* the action, so they exercise the writers and never the
/// converter.
///
/// The vocabulary is measured, not invented — every name and payload key below
/// is one the reference export actually contains, and the nine kinds here are
/// the nine it holds. Anything else returns `None` rather than guessing at a
/// spelling, which keeps an unmapped action a message with no `action` key
/// instead of a wrong one.
fn service_action(
    action: &tl::enums::MessageAction,
    m: &tl::types::MessageService,
    names: &NameBook,
) -> Option<(&'static str, Vec<(String, Value)>)> {
    use tl::enums::MessageAction as A;

    let name_of = |id: i64| json!(names.get(&PeerKey::user(id).to_string()));
    let no_payload = |n: &'static str| Some((n, Vec::new()));

    match action {
        A::ChatEditTitle(a) => Some(("edit_group_title", vec![("title".into(), json!(a.title))])),
        A::ChatAddUser(a) => Some((
            "invite_members",
            vec![(
                "members".into(),
                Value::Array(a.users.iter().map(|u| name_of(*u)).collect()),
            )],
        )),
        // Desktop files a self-removal under the same name as a kick; the
        // reference's six are all one user apiece, always as a one-element
        // array rather than a bare string.
        A::ChatDeleteUser(a) => Some((
            "remove_members",
            vec![("members".into(), json!([name_of(a.user_id)]))],
        )),
        A::ChatJoinedByLink(a) => Some((
            "join_group_by_link",
            vec![("inviter".into(), name_of(a.inviter_id))],
        )),
        // The migrated-from title, not the current one.
        A::ChannelMigrateFrom(a) => {
            Some(("migrate_from_group", vec![("title".into(), json!(a.title))]))
        }
        // The pinned id rides in `reply_to`, which is the only place it exists:
        // the action itself is a bare constructor with no fields.
        A::PinMessage => {
            let id = match m.reply_to.as_ref() {
                Some(tl::enums::MessageReplyHeader::Header(h)) => h.reply_to_msg_id,
                _ => None,
            };
            Some((
                "pin_message",
                id.map(|v| vec![("message_id".into(), json!(v))])
                    .unwrap_or_default(),
            ))
        }
        A::TopicCreate(a) => Some(("topic_created", vec![("title".into(), json!(a.title))])),
        // Both keys are written whenever either changed, and the reference's
        // `new_icon_emoji_id` is the integer `0` — not a string, and not
        // omitted — when the icon was cleared.
        A::TopicEdit(a) => {
            let mut p = Vec::new();
            if let Some(t) = &a.title {
                p.push(("new_title".into(), json!(t)));
            }
            p.push((
                "new_icon_emoji_id".into(),
                json!(a.icon_emoji_id.unwrap_or(0)),
            ));
            Some(("topic_edit", p))
        }
        // Telegram's checklists travel as `todo` on the wire and as the poll
        // vocabulary in Desktop's export. The reference's three carry no
        // payload beyond the actor, so nothing is inferred but the name.
        A::TodoAppendTasks(_) => no_payload("poll_append_answer"),
        _ => None,
    }
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

    fn service_with(action: tl::enums::MessageAction) -> tl::types::MessageService {
        tl::types::MessageService {
            out: false,
            mentioned: false,
            media_unread: false,
            reactions_are_possible: false,
            silent: false,
            post: false,
            legacy: false,
            id: 66,
            from_id: Some(peer_user(9)),
            peer_id: tl::enums::Peer::Channel(tl::types::PeerChannel {
                channel_id: 3_586_682_625,
            }),
            saved_peer_id: None,
            reply_to: None,
            date: 1_766_071_072,
            action,
            reactions: None,
            ttl_period: None,
        }
    }

    #[test]
    fn every_action_the_reference_holds_is_named_the_way_desktop_names_it() {
        // All 63 service messages in a live export came out with no `action`
        // key whatsoever, because `base_service` never looked at `m.action`.
        // The nine names below are exactly the nine the reference export
        // contains, with the payload keys it carries for each.
        let mut names = NameBook::default();
        names.learn_user_parts(11, "Nađa", "Gavrilović", "", false, None);
        names.learn_user_parts(12, "Group", "", "", false, None);

        /// One action, the name Desktop gives it, and the payload it carries.
        type Case = (
            tl::enums::MessageAction,
            &'static str,
            Vec<(&'static str, Value)>,
        );

        let cases: Vec<Case> = vec![
            (
                tl::enums::MessageAction::ChatEditTitle(tl::types::MessageActionChatEditTitle {
                    title: "UA KOLAB".into(),
                }),
                "edit_group_title",
                vec![("title", json!("UA KOLAB"))],
            ),
            (
                tl::enums::MessageAction::ChatAddUser(tl::types::MessageActionChatAddUser {
                    users: vec![11],
                }),
                "invite_members",
                vec![("members", json!(["Nađa Gavrilović"]))],
            ),
            (
                tl::enums::MessageAction::ChatDeleteUser(tl::types::MessageActionChatDeleteUser {
                    user_id: 11,
                }),
                "remove_members",
                // A one-element array, never a bare string.
                vec![("members", json!(["Nađa Gavrilović"]))],
            ),
            (
                tl::enums::MessageAction::ChatJoinedByLink(
                    tl::types::MessageActionChatJoinedByLink { inviter_id: 12 },
                ),
                "join_group_by_link",
                vec![("inviter", json!("Group"))],
            ),
            (
                tl::enums::MessageAction::ChannelMigrateFrom(
                    tl::types::MessageActionChannelMigrateFrom {
                        title: "samo jos jedna grupa i gotov je".into(),
                        chat_id: 7,
                    },
                ),
                "migrate_from_group",
                vec![("title", json!("samo jos jedna grupa i gotov je"))],
            ),
            (
                tl::enums::MessageAction::TopicCreate(tl::types::MessageActionTopicCreate {
                    title_missing: false,
                    title: "bitno pročitaj".into(),
                    icon_color: 7_322_096,
                    icon_emoji_id: None,
                }),
                "topic_created",
                vec![("title", json!("bitno pročitaj"))],
            ),
            (
                tl::enums::MessageAction::TopicEdit(tl::types::MessageActionTopicEdit {
                    title: Some("foto video".into()),
                    icon_emoji_id: None,
                    closed: None,
                    hidden: None,
                }),
                "topic_edit",
                // The reference writes the integer 0, not a string and not
                // nothing, when the icon was cleared.
                vec![
                    ("new_title", json!("foto video")),
                    ("new_icon_emoji_id", json!(0)),
                ],
            ),
            (
                tl::enums::MessageAction::TodoAppendTasks(
                    tl::types::MessageActionTodoAppendTasks { list: vec![] },
                ),
                "poll_append_answer",
                vec![],
            ),
        ];

        for (action, expected, payload) in cases {
            let out = base_service(&service_with(action), &names);
            assert_eq!(out["action"], expected, "wrong name for {expected}");
            for (k, v) in payload {
                assert_eq!(out.get(k), Some(&v), "{expected} lost {k}");
            }
        }
    }

    #[test]
    fn a_pinned_message_takes_its_id_from_the_reply_header() {
        // `messageActionPinMessage` is a bare constructor — the id it refers to
        // exists nowhere but `reply_to`, so reading only the action yields an
        // action with no `message_id`.
        let mut s = service_with(tl::enums::MessageAction::PinMessage);
        s.reply_to = Some(tl::enums::MessageReplyHeader::Header(
            tl::types::MessageReplyHeader {
                reply_to_scheduled: false,
                forum_topic: false,
                quote: false,
                reply_to_msg_id: Some(6),
                reply_to_peer_id: None,
                reply_from: None,
                reply_media: None,
                reply_to_top_id: None,
                quote_text: None,
                quote_entities: None,
                quote_offset: None,
                todo_item_id: None,
                reply_to_ephemeral: false,
                poll_option: None,
            },
        ));
        let out = base_service(&s, &NameBook::default());
        assert_eq!(out["action"], "pin_message");
        assert_eq!(out["message_id"], 6);
    }

    #[test]
    fn a_migration_notice_still_has_an_actor() {
        // It has no sender — the group did it — so keying the actor on
        // `from_id` alone dropped `actor` and `actor_id` from the one message
        // in the reference that has neither a `from` nor a `text`.
        let mut s = service_with(tl::enums::MessageAction::ChannelMigrateFrom(
            tl::types::MessageActionChannelMigrateFrom {
                title: "old".into(),
                chat_id: 7,
            },
        ));
        s.from_id = None;
        let mut names = NameBook::default();
        names
            .names
            .insert("channel3586682625".into(), "UA KOLAB".into());
        let out = base_service(&s, &names);
        assert_eq!(out["actor"], "UA KOLAB");
        assert_eq!(out["actor_id"], "channel3586682625");
    }

    #[test]
    fn an_action_we_have_not_mapped_leaves_the_key_off_rather_than_guessing() {
        let out = base_service(
            &service_with(tl::enums::MessageAction::HistoryClear),
            &NameBook::default(),
        );
        assert!(out.get("action").is_none());
        assert_eq!(out["type"], "service");
    }

    fn emoji_reaction(emoticon: &str, count: i32) -> tl::enums::ReactionCount {
        tl::enums::ReactionCount::Count(tl::types::ReactionCount {
            chosen_order: None,
            reaction: tl::enums::Reaction::Emoji(tl::types::ReactionEmoji {
                emoticon: emoticon.into(),
            }),
            count,
        })
    }

    #[test]
    fn a_reaction_carries_its_count_and_the_people_telegram_named() {
        // 963 of the reference's messages carry reactions and a live export
        // carried none on any message — and the wire leg called it drift,
        // because `reactions` is allowed to *differ* between two runs and the
        // check never asked whether it was there at all.
        let mut names = NameBook::default();
        names.learn_user_parts(1401106198, "Petar", "Markov", "", false, None);

        let r = tl::types::MessageReactions {
            min: false,
            can_see_list: true,
            reactions_as_tags: false,
            results: vec![emoji_reaction("❤", 6), emoji_reaction("👍", 2)],
            recent_reactions: Some(vec![tl::enums::MessagePeerReaction::Reaction(
                tl::types::MessagePeerReaction {
                    big: false,
                    unread: false,
                    my: false,
                    peer_id: peer_user(1401106198),
                    date: 1_766_071_072,
                    reaction: tl::enums::Reaction::Emoji(tl::types::ReactionEmoji {
                        emoticon: "❤".into(),
                    }),
                },
            )]),
            top_reactors: None,
        };
        let out = reactions_of(&r, &names).expect("two reactions");
        let a = &out[0];
        assert_eq!(a["type"], "emoji");
        assert_eq!(a["emoji"], "❤");
        assert_eq!(a["count"], 6);
        assert_eq!(a["recent"][0]["from"], "Petar Markov");
        assert_eq!(a["recent"][0]["from_id"], "user1401106198");
        // The named reactor belongs to the reaction they actually pressed, not
        // to every entry on the message.
        assert!(
            out[1].get("recent").is_none(),
            "the ❤ reactor leaked onto 👍"
        );
    }

    #[test]
    fn a_custom_emoji_reaction_reports_a_document_id_not_an_emoji() {
        let r = tl::types::MessageReactions {
            min: false,
            can_see_list: true,
            reactions_as_tags: false,
            results: vec![tl::enums::ReactionCount::Count(tl::types::ReactionCount {
                chosen_order: None,
                reaction: tl::enums::Reaction::CustomEmoji(tl::types::ReactionCustomEmoji {
                    document_id: 5_296_383_305_254_459_545,
                }),
                count: 1,
            })],
            recent_reactions: None,
            top_reactors: None,
        };
        let out = reactions_of(&r, &NameBook::default()).expect("one reaction");
        assert_eq!(out[0]["type"], "custom_emoji");
        assert_eq!(out[0]["document_id"], "5296383305254459545");
        assert!(out[0].get("emoji").is_none());
    }

    fn poll_media(
        closed: bool,
        tallies: Option<Vec<(u8, i32, bool)>>,
    ) -> tl::types::MessageMediaPoll {
        let answer = |b: u8, t: &str| {
            tl::enums::PollAnswer::Answer(tl::types::PollAnswer {
                text: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                    text: t.into(),
                    entities: vec![],
                }),
                option: vec![b],
                media: None,
                added_by: None,
                date: None,
            })
        };
        tl::types::MessageMediaPoll {
            poll: tl::enums::Poll::Poll(tl::types::Poll {
                id: 1,
                closed,
                public_voters: false,
                multiple_choice: false,
                quiz: false,
                open_answers: false,
                revoting_disabled: false,
                shuffle_answers: false,
                hide_results_until_close: false,
                creator: false,
                subscribers_only: false,
                question: tl::enums::TextWithEntities::Entities(tl::types::TextWithEntities {
                    text: "klip".into(),
                    entities: vec![],
                }),
                answers: vec![answer(0, "da"), answer(1, "ne")],
                close_period: None,
                close_date: None,
                countries_iso2: None,
                hash: 0,
            }),
            results: tl::enums::PollResults::Results(Box::new(tl::types::PollResults {
                min: false,
                has_unread_votes: false,
                can_view_stats: false,
                results: tallies.map(|ts| {
                    ts.into_iter()
                        .map(|(opt, voters, chosen)| {
                            tl::enums::PollAnswerVoters::Voters(tl::types::PollAnswerVoters {
                                chosen,
                                correct: false,
                                option: vec![opt],
                                voters: Some(voters),
                                recent_voters: None,
                            })
                        })
                        .collect()
                }),
                total_voters: Some(8),
                recent_voters: None,
                solution: None,
                solution_entities: None,
                solution_media: None,
            })),
            attached_media: None,
        }
    }

    #[test]
    fn a_poll_reports_its_question_answers_and_tallies() {
        // All seven polls in the reference came out of a live export as plain
        // text: `plan::classify` answers "what would we download", and a poll
        // is not a file, so nothing recognised it.
        let out = poll_of(&poll_media(false, Some(vec![(0, 5, true), (1, 3, false)])));
        assert_eq!(out["question"], "klip");
        assert_eq!(out["closed"], false);
        assert_eq!(out["total_voters"], 8);
        assert_eq!(out["answers"][0]["text"], "da");
        assert_eq!(out["answers"][0]["voters"], 5);
        assert_eq!(out["answers"][0]["chosen"], true);
        assert_eq!(out["answers"][1]["chosen"], false);
    }

    #[test]
    fn a_poll_nobody_has_voted_in_keeps_its_answers() {
        // Telegram omits `results` entirely before the first vote. Reading it
        // as "no answers" would drop the question's options.
        let out = poll_of(&poll_media(false, None));
        assert_eq!(out["answers"].as_array().map(Vec::len), Some(2));
        assert_eq!(out["answers"][0]["voters"], 0);
        assert_eq!(out["answers"][0]["chosen"], false);
    }

    #[test]
    fn a_location_is_latitude_then_longitude_and_a_live_one_carries_its_period() {
        let point = tl::enums::GeoPoint::Point(tl::types::GeoPoint {
            long: 20.370197,
            lat: 44.857507,
            access_hash: 0,
            accuracy_radius: None,
        });
        let (place, period) =
            location_of(&tl::enums::MessageMedia::Geo(tl::types::MessageMediaGeo {
                geo: point.clone(),
            }))
            .expect("a point");
        assert_eq!(place["latitude"], 44.857507);
        assert_eq!(place["longitude"], 20.370197);
        assert_eq!(period, None);
        // Desktop writes latitude first and `order.rs` does not reach inside a
        // nested object, so insertion order is the file's order.
        assert_eq!(
            place
                .as_object()
                .map(|m| m.keys().next().map(String::as_str)),
            Some(Some("latitude"))
        );

        let (_, period) = location_of(&tl::enums::MessageMedia::GeoLive(
            tl::types::MessageMediaGeoLive {
                geo: point,
                heading: None,
                period: 28_800,
                proximity_notification_radius: None,
            },
        ))
        .expect("a live point");
        assert_eq!(period, Some(28_800));
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
