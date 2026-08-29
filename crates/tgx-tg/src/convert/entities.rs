//! Text entities: Desktop's name for each type, its offset and length, and
//! the extra keys a few of them carry.
//!
//! Offsets are UTF-16 code units, which is what Telegram counts in. See
//! `tgx_format::text` for what that costs.

use super::*;

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
