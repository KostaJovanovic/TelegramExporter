//! The chat header, its invite links, and its scheduled messages.
//!
//! **Beyond Desktop's format on purpose.** Desktop's header is `name`, `type`
//! and `id`; every key here is an addition, which is why each is a switch and
//! why the switch has to actually work.
//!
//! Only the `InputPeer::Channel` arm has ever run against Telegram. The basic
//! group and private chat branches are argued from api.tl and have never been
//! exercised; see ROADMAP.md.

use super::*;

/// What the chat itself is, none of which is in the message stream.
///
/// One request. Before this an export recorded nothing whatsoever about the
/// group it came from — `chat_metadata` was offered as "Chat details",
/// defaulted to on, and was read by nothing.
///
/// **Beyond Desktop's format on purpose.** Desktop's header is `name`, `type`
/// and `id`; every key here is an addition, which is why it is a switch and
/// why the switch has to actually work.
pub async fn fetch_chat_info(
    client: &Client,
    peer: PeerRef,
    settings: &Settings,
    tally: &mut Enrichment,
    mut on_wait: impl FnMut(u64),
) -> Map<String, Value> {
    let mut info = Map::new();
    if !settings.chat_metadata {
        return info;
    }
    // **Three peer shapes, three requests.** Only the channel one existed, so
    // a basic group and a private chat got an empty map — the switch was on,
    // the request was never made, and nothing said so. Exactly the shape of
    // the bug this whole family had; it survived one layer further down
    // because "wired" and "wired for every peer" are not the same claim.
    let full = match peer.into() {
        tl::enums::InputPeer::Channel(c) => {
            let request = tl::functions::channels::GetFullChannel {
                channel: tl::enums::InputChannel::Channel(tl::types::InputChannel {
                    channel_id: c.channel_id,
                    access_hash: c.access_hash,
                }),
            };
            guarded(tally, &mut on_wait, || {
                let client = client.clone();
                let request = request.clone();
                async move { client.invoke(&request).await.map_err(|e| classify(&e)) }
            })
            .await
        }
        tl::enums::InputPeer::Chat(c) => {
            let request = tl::functions::messages::GetFullChat { chat_id: c.chat_id };
            guarded(tally, &mut on_wait, || {
                let client = client.clone();
                let request = request.clone();
                async move { client.invoke(&request).await.map_err(|e| classify(&e)) }
            })
            .await
        }
        tl::enums::InputPeer::User(u) => {
            let request = tl::functions::users::GetFullUser {
                id: tl::enums::InputUser::User(tl::types::InputUser {
                    user_id: u.user_id,
                    access_hash: u.access_hash,
                }),
            };
            let got = guarded(tally, &mut on_wait, || {
                let client = client.clone();
                let request = request.clone();
                async move { client.invoke(&request).await.map_err(|e| classify(&e)) }
            })
            .await;
            // A user has no chat to be full of. What it does have is the bio,
            // which is the one field of this set that means anything for a
            // private chat.
            if let Some(tl::enums::users::UserFull::Full(u)) = got {
                let tl::enums::UserFull::Full(f) = u.full_user;
                if let Some(about) = f.about.filter(|a| !a.is_empty()) {
                    info.insert("description".into(), json!(about));
                }
                if let Some(t) = f.ttl_period.filter(|t| *t != 0) {
                    info.insert("ttl_period".into(), json!(t));
                }
            }
            return info;
        }
        _ => return info,
    };

    let Some(tl::enums::messages::ChatFull::Full(full)) = full else {
        return info;
    };
    let c = match full.full_chat {
        tl::enums::ChatFull::ChannelFull(c) => c,
        // A basic group carries a much smaller set, and none of the counts:
        // its members arrive as a list rather than a number.
        tl::enums::ChatFull::Full(g) => {
            if !g.about.is_empty() {
                info.insert("description".into(), json!(g.about));
            }
            if let tl::enums::ChatParticipants::Participants(p) = &g.participants {
                info.insert("members_count".into(), json!(p.participants.len()));
            }
            if let Some(t) = g.ttl_period.filter(|t| *t != 0) {
                info.insert("ttl_period".into(), json!(t));
            }
            if let Some(r) = allowed_reactions(g.available_reactions.as_ref()) {
                info.insert("allowed_reactions".into(), r);
            }
            if let Some(tl::enums::ExportedChatInvite::ChatInviteExported(i)) = &g.exported_invite {
                info.insert("invite_link".into(), json!(i.link));
            }
            return info;
        }
    };

    if !c.about.is_empty() {
        info.insert("description".into(), json!(c.about));
    }
    for (value, key) in [
        (c.participants_count, "members_count"),
        (c.admins_count, "admins_count"),
        (c.kicked_count, "kicked_count"),
        (c.banned_count, "banned_count"),
        (c.online_count, "online_count"),
        (c.slowmode_seconds, "slow_mode_seconds"),
        (c.pinned_msg_id, "pinned_message_id"),
        (c.ttl_period, "ttl_period"),
    ] {
        // Absent and zero are the same thing for every one of these, so a
        // falsy value is left out rather than written as `0`.
        if let Some(v) = value.filter(|v| *v != 0) {
            info.insert(key.into(), json!(v));
        }
    }
    if let Some(linked) = c.linked_chat_id {
        info.insert("linked_chat_id".into(), json!(format!("channel{linked}")));
    }
    info.insert("can_view_members".into(), json!(c.can_view_participants));
    if c.participants_hidden {
        info.insert("members_hidden".into(), json!(true));
    }
    if let Some(r) = allowed_reactions(c.available_reactions.as_ref()) {
        info.insert("allowed_reactions".into(), r);
    }
    if let Some(tl::enums::ChannelLocation::Location(l)) = &c.location {
        info.insert("location".into(), json!(l.address));
    }
    if let Some(tl::enums::ExportedChatInvite::ChatInviteExported(i)) = &c.exported_invite {
        info.insert("invite_link".into(), json!(i.link));
    }
    info
}

/// Which reactions the chat permits, in Desktop's own vocabulary.
fn allowed_reactions(available: Option<&tl::enums::ChatReactions>) -> Option<Value> {
    match available? {
        tl::enums::ChatReactions::All(_) => Some(json!("all")),
        tl::enums::ChatReactions::None => Some(json!("none")),
        tl::enums::ChatReactions::Some(s) => Some(Value::Array(
            s.reactions
                .iter()
                .map(|r| match r {
                    tl::enums::Reaction::Emoji(e) => json!(e.emoticon),
                    tl::enums::Reaction::CustomEmoji(e) => json!(e.document_id.to_string()),
                    _ => Value::Null,
                })
                .filter(|v| !v.is_null())
                .collect(),
        )),
    }
}

/// Every invite link, which is what turns a placeholder inviter into a name.
///
/// **Admin-only.** Telegram refuses this for an ordinary member, which is the
/// common case, so a refusal is silent and costs nothing. `invite_links` was
/// offered, defaulted to on, and read by nothing.
pub async fn fetch_invites(
    client: &Client,
    peer: PeerRef,
    settings: &Settings,
    tally: &mut Enrichment,
    mut on_wait: impl FnMut(u64),
) -> Vec<Value> {
    if !settings.invite_links {
        return Vec::new();
    }
    let Ok(me) = client.get_me().await else {
        return Vec::new();
    };
    let admin: tl::enums::InputUser = tl::enums::InputUser::User(tl::types::InputUser {
        user_id: me.id().bare_id().unwrap_or(0),
        access_hash: match &me.raw {
            tl::enums::User::User(u) => u.access_hash.unwrap_or(0),
            _ => 0,
        },
    });
    let request = tl::functions::messages::GetExportedChatInvites {
        revoked: false,
        peer: peer.into(),
        admin_id: admin,
        offset_date: None,
        offset_link: None,
        limit: 100,
    };
    let result = guarded(tally, &mut on_wait, || {
        let client = client.clone();
        let request = request.clone();
        async move { client.invoke(&request).await.map_err(|e| classify(&e)) }
    })
    .await;
    let Some(tl::enums::messages::ExportedChatInvites::Invites(result)) = result else {
        return Vec::new();
    };

    result
        .invites
        .iter()
        .filter_map(|i| {
            let tl::enums::ExportedChatInvite::ChatInviteExported(i) = i else {
                return None;
            };
            let mut one = Map::new();
            one.insert("link".into(), json!(i.link));
            if let Some(t) = &i.title {
                one.insert("title".into(), json!(t));
            }
            if let Some((date, _)) = tgx_format::date_pair(i.date as i64) {
                one.insert("date".into(), json!(date));
            }
            one.insert("creator_id".into(), json!(format!("user{}", i.admin_id)));
            if let Some(u) = i.usage {
                one.insert("usage".into(), json!(u));
            }
            if let Some(u) = i.usage_limit {
                one.insert("usage_limit".into(), json!(u));
            }
            if i.permanent {
                one.insert("permanent".into(), json!(true));
            }
            if i.request_needed {
                one.insert("request_needed".into(), json!(true));
            }
            Some(Value::Object(one))
        })
        .collect()
}

/// Messages queued to send later, which are in no history at all.
///
/// `scheduled_messages` was offered, defaulted to on, and read by nothing —
/// so the file it is supposed to produce was never written.
pub async fn fetch_scheduled(
    client: &Client,
    peer: PeerRef,
    settings: &Settings,
    tally: &mut Enrichment,
    mut on_wait: impl FnMut(u64),
) -> Vec<tl::enums::Message> {
    if !settings.scheduled_messages {
        return Vec::new();
    }
    let request = tl::functions::messages::GetScheduledHistory {
        peer: peer.into(),
        hash: 0,
    };
    let result = guarded(tally, &mut on_wait, || {
        let client = client.clone();
        let request = request.clone();
        async move { client.invoke(&request).await.map_err(|e| classify(&e)) }
    })
    .await;
    match result {
        Some(tl::enums::messages::Messages::Messages(m)) => m.messages,
        Some(tl::enums::messages::Messages::Slice(m)) => m.messages,
        Some(tl::enums::messages::Messages::ChannelMessages(m)) => m.messages,
        _ => Vec::new(),
    }
}

pub(super) fn channel_ref(peer: PeerRef) -> Option<tl::enums::InputChannel> {
    match peer.into() {
        tl::enums::InputPeer::Channel(c) => {
            Some(tl::enums::InputChannel::Channel(tl::types::InputChannel {
                channel_id: c.channel_id,
                access_hash: c.access_hash,
            }))
        }
        _ => None,
    }
}
