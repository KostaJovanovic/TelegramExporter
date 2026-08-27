//! What costs a request.
//!
//! Everything in Desktop's format arrives inside the message stream. These do
//! not, so each is a separate switch in [`Settings`], each **degrades to
//! nothing** on failure, and each fires **only when the data is actually
//! missing** — that conditional is the whole cost argument.
//!
//! | what | when it fires | measured on the reference |
//! |---|---|---|
//! | full reaction list | totals exceed the named reactors | 77 of 963 reacted messages, **+1.16% requests** |
//! | poll refresh | results came back `min`/all-zero | 2 of 7 polls |
//! | chat info | once per chat | nothing about the group was recorded before |
//! | participants | once per chat, paged at 200 | the export otherwise names someone *only if they spoke* |
//! | invites | once per chat, **admin-only** | 26 join messages credited a placeholder |
//! | scheduled | once per chat | not in the history at all |
//!
//! **The guard is the point of this module.** A rate limit is temporary and a
//! refusal is not; see [`crate::error`] for why conflating them was the most
//! damaging bug in the original.

use crate::config::Settings;
use crate::error::{classify, EnrichError};
use grammers_client::session::types::PeerRef;
use grammers_client::Client;
use grammers_tl_types as tl;
use serde_json::{json, Map, Value};
use std::time::Duration;

/// How long a single enrichment may wait out a rate limit.
///
/// **120 s is a deliberate trade, not a default.** It started at 20 s and was
/// raised on the instruction that waiting is preferable to losing data. Two
/// consequences follow from the size of it and are not optional:
///
/// * the wait emits a progress event, because two minutes of silence is
///   indistinguishable from a hung export, and
/// * it sleeps in one-second slices, because a flat sleep swallows a click on
///   Cancel for the whole two minutes.
///
/// Both were survivable at 20 s and are not at 120 s. Raising the cap without
/// them trades a silent data loss for an apparent freeze.
pub const ENRICH_MAX_WAIT: Duration = Duration::from_secs(120);

/// Telegram pages participants at 200.
pub const PARTICIPANT_PAGE: i32 = 200;

/// What one chat's optional requests recovered.
#[derive(Debug, Default, Clone)]
pub struct Enrichment {
    pub requests: usize,
    /// Enrichments a **rate limit** cost. Not the same as one being refused:
    /// this means the data was there and we did not get it.
    pub deferred: usize,
}

/// Run one optional request, waiting out a rate limit but not a refusal.
///
/// Retries once. Anything up to [`ENRICH_MAX_WAIT`] is waited out; past that
/// the loss is *counted* rather than hidden.
///
/// The original gave up instantly and uncounted, because its `except Exception`
/// was written for an admin-only method being refused — a permanent condition
/// where giving up quietly is correct — and a rate limit landed in the same
/// net. That is unrepresentable here: `Transient` and `Refused` are different
/// variants.
pub async fn guarded<T, F, Fut>(
    tally: &mut Enrichment,
    mut on_wait: impl FnMut(u64),
    mut call: F,
) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, EnrichError>>,
{
    for attempt in 0..2 {
        tally.requests += 1;
        match call().await {
            Ok(value) => return Some(value),
            Err(e) => match e.retry_after() {
                Some(wait) if wait <= ENRICH_MAX_WAIT && attempt == 0 => {
                    on_wait(wait.as_secs());
                    crate::engine::sleep_in_slices(wait).await;
                }
                // Past the cap, or a second rate limit: the data was there and
                // we did not get it. Count it.
                Some(_) => {
                    tally.deferred += 1;
                    return None;
                }
                // A refusal is permanent. Giving up quietly is correct, and it
                // is *not* a deferral.
                None => return None,
            },
        }
    }
    None
}

/// The member list, and whether it is the whole of it.
///
/// **A truncated roster is indistinguishable from a complete one**, which makes
/// it worse than no roster: wrong data that looks right. The original returned
/// whatever it had collected and said nothing.
#[derive(Debug, Default, Clone)]
pub struct Roster {
    pub members: Vec<Value>,
    /// Did Telegram serve the whole list?
    pub complete: bool,
    /// Did *we* stop first, at `Settings::member_limit`?
    ///
    /// Separate from `complete`, because hitting our own limit is a different
    /// thing from Telegram cutting us off.
    pub capped: bool,
    /// `(replaced, kept)` under `own_names` — see `convert::own_name_parts`.
    pub aliased: (usize, usize),
}

impl Roster {
    pub fn to_json(&self) -> Value {
        json!({
            "members": self.members,
            "complete": self.complete,
            "capped": self.capped,
        })
    }
}

/// Fetch the member roster, paged.
pub async fn fetch_participants(
    client: &Client,
    peer: PeerRef,
    settings: &Settings,
    tally: &mut Enrichment,
    mut on_wait: impl FnMut(u64),
) -> Roster {
    let mut roster = Roster {
        complete: true,
        ..Default::default()
    };
    if !settings.member_roster {
        return roster;
    }
    let Some(channel) = channel_ref(peer) else {
        // **Not a channel is not "no members".** `channels.getParticipants` is
        // channel-only, so a basic group returned an empty roster that claimed
        // to be complete — a member list of nobody, indistinguishable from a
        // group with no members. Its members ride along with
        // `messages.getFullChat` instead, which the chat-details pass already
        // makes; a group small enough to still be a basic group is never paged.
        return basic_group_roster(client, peer, settings.own_names, tally, on_wait).await;
    };

    let mut offset = 0i32;
    loop {
        let request = tl::functions::channels::GetParticipants {
            channel: channel.clone(),
            filter: tl::enums::ChannelParticipantsFilter::ChannelParticipantsRecent,
            offset,
            limit: PARTICIPANT_PAGE,
            hash: 0,
        };
        let page = guarded(tally, &mut on_wait, || {
            let client = client.clone();
            let request = request.clone();
            async move { client.invoke(&request).await.map_err(|e| classify(&e)) }
        })
        .await;

        let Some(page) = page else {
            // We did not finish. Say so rather than returning a short list
            // that looks whole.
            roster.complete = false;
            return roster;
        };

        let tl::enums::channels::ChannelParticipants::Participants(page) = page else {
            roster.complete = false;
            return roster;
        };

        let before = roster.members.len();
        for user in &page.users {
            if let tl::enums::User::User(u) = user {
                roster
                    .members
                    .push(member_json(u, settings.own_names, &mut roster.aliased));
            }
        }
        let gained = roster.members.len() - before;
        offset += PARTICIPANT_PAGE;

        if settings.member_limit > 0 && roster.members.len() as i64 >= settings.member_limit {
            roster.members.truncate(settings.member_limit as usize);
            roster.capped = true;
            return roster;
        }
        // No progress means the server has stopped serving the listing.
        if gained == 0 || roster.members.len() as i32 >= page.count {
            return roster;
        }
    }
}

/// Does this message's reaction list hide anyone?
///
/// **The three-name cap is per _message_, not per reaction.** A message with
/// two reactions of five shows three names and hides seven, so the test is
/// `sum(counts) > len(recent_reactions)` across the whole message.
pub fn reactions_are_truncated(reactions: &tl::types::MessageReactions) -> bool {
    let total: i64 = reactions
        .results
        .iter()
        .map(|r| {
            let tl::enums::ReactionCount::Count(c) = r;
            c.count as i64
        })
        .sum();
    let named = reactions
        .recent_reactions
        .as_ref()
        .map(Vec::len)
        .unwrap_or(0) as i64;
    total > named
}

/// Did a poll come back without usable results?
///
/// Telegram sends `min` results to a client that has not voted, and an
/// all-zero tally is the same thing by another name.
pub fn poll_needs_refresh(results: &tl::types::PollResults) -> bool {
    if results.min {
        return true;
    }
    match &results.results {
        None => true,
        // `voters` is optional: absent is the same "we were told nothing" as
        // zero, so both count as needing a refresh.
        Some(rs) => rs.iter().all(|r| {
            let tl::enums::PollAnswerVoters::Voters(v) = r;
            v.voters.unwrap_or(0) == 0
        }),
    }
}

/// A page of reactors, at a time.
const REACTION_PAGE: i32 = 100;

/// 2,000 reactors is far past any real message; a loop with no bound is not.
const REACTION_PAGES: usize = 20;

/// Everyone who reacted, in place of the three names Telegram volunteers.
///
/// **This is what `full_reactions` was supposed to do and did not.** The
/// setting has defaulted to `true` since it was written, was offered in the
/// panel as "Full reaction lists", and was read by nothing — so every export
/// carried the sample of at most three names that arrives with the message,
/// exactly as if the switch were off. The predicate that decides whether to
/// ask ([`reactions_are_truncated`]) was written at the same time and was
/// equally unreachable.
///
/// People who react anonymously are simply not in the response and no amount
/// of asking reveals them, so the returned list can still be shorter than the
/// counts imply. That is Telegram's answer, not a truncation of ours.
///
/// `Err` is only ever a rate limit, so [`guarded`] can wait it out; anything
/// else is a refusal and comes back as an empty list. Restarting from an empty
/// accumulator on retry is correct — the caller re-invokes this, not the loop.
pub async fn fetch_reactors(
    client: &Client,
    peer: PeerRef,
    message_id: i32,
) -> Result<Vec<tl::enums::MessagePeerReaction>, EnrichError> {
    let mut collected = Vec::new();
    let mut offset: Option<String> = None;
    for _ in 0..REACTION_PAGES {
        let request = tl::functions::messages::GetMessageReactionsList {
            peer: peer.into(),
            id: message_id,
            reaction: None,
            offset: offset.clone(),
            limit: REACTION_PAGE,
        };
        let page = match client.invoke(&request).await {
            Ok(p) => p,
            Err(e) => {
                let err = classify(&e);
                // A rate limit is the caller's to wait out. A refusal — this
                // is admin-visible on some chats — costs the message its full
                // list and nothing else.
                if err.is_transient() {
                    return Err(err);
                }
                return Ok(Vec::new());
            }
        };
        let tl::enums::messages::MessageReactionsList::List(page) = page;
        let batch = page.reactions.len();
        collected.extend(page.reactions);
        offset = page.next_offset;
        if offset.is_none() || batch == 0 {
            break;
        }
    }
    Ok(collected)
}

/// A poll's real tallies, for one Telegram answered with `min`.
///
/// Two of the reference export's seven polls arrive this way: every answer
/// zeroed, which exports as a poll nobody voted in. `refresh_polls` existed to
/// fix that and was read by nothing.
///
/// `poll_hash: 0` is required and means "I have nothing cached, send it in
/// full". The Python original records that omitting it raised before any
/// request went out, and that the surrounding `except` then swallowed the
/// error — so the feature silently never worked until a test caught it.
pub async fn fetch_poll_results(
    client: &Client,
    peer: PeerRef,
    message_id: i32,
) -> Result<Option<tl::enums::PollResults>, EnrichError> {
    let request = tl::functions::messages::GetPollResults {
        peer: peer.into(),
        msg_id: message_id,
        poll_hash: 0,
    };
    let updates = match client.invoke(&request).await {
        Ok(u) => u,
        Err(e) => {
            let err = classify(&e);
            if err.is_transient() {
                return Err(err);
            }
            return Ok(None);
        }
    };
    // Only an update that actually carries tallies is worth grafting on: the
    // response can echo the same empty results back.
    for update in updates_of(&updates) {
        if let tl::enums::Update::MessagePoll(u) = update {
            let tl::enums::PollResults::Results(r) = &u.results;
            if r.results.as_ref().is_some_and(|rs| !rs.is_empty()) {
                return Ok(Some(u.results.clone()));
            }
        }
    }
    Ok(None)
}

fn updates_of(u: &tl::enums::Updates) -> Vec<&tl::enums::Update> {
    match u {
        tl::enums::Updates::Combined(c) => c.updates.iter().collect(),
        tl::enums::Updates::Updates(c) => c.updates.iter().collect(),
        tl::enums::Updates::UpdateShort(s) => std::slice::from_ref(&s.update).iter().collect(),
        _ => Vec::new(),
    }
}

/// The members of a basic group, which do not come from `getParticipants`.
///
/// A private chat has no roster at all and returns an empty *complete* one:
/// there is genuinely nobody to list, which is different from failing to list
/// them.
async fn basic_group_roster(
    client: &Client,
    peer: PeerRef,
    own_names: bool,
    tally: &mut Enrichment,
    mut on_wait: impl FnMut(u64),
) -> Roster {
    let mut roster = Roster {
        complete: true,
        ..Default::default()
    };
    let tl::enums::InputPeer::Chat(chat) = peer.into() else {
        return roster;
    };
    let request = tl::functions::messages::GetFullChat {
        chat_id: chat.chat_id,
    };
    let full = guarded(tally, &mut on_wait, || {
        let client = client.clone();
        let request = request.clone();
        async move { client.invoke(&request).await.map_err(|e| classify(&e)) }
    })
    .await;
    let Some(tl::enums::messages::ChatFull::Full(full)) = full else {
        roster.complete = false;
        return roster;
    };
    for user in &full.users {
        if let tl::enums::User::User(u) = user {
            roster
                .members
                .push(member_json(u, own_names, &mut roster.aliased));
        }
    }
    roster
}

/// One roster row. Shared so the two paths cannot describe a member
/// differently depending on which kind of chat they are in.
///
/// `own_names` is honoured here as well as in `NameBook`, or
/// `participants.json` would name a contact one way and every message they
/// sent another — the export disagreeing with itself about who someone is.
fn member_json(u: &tl::types::User, own_names: bool, tally: &mut (usize, usize)) -> Value {
    let username = u.username.as_deref().unwrap_or("");
    let (first, last) = crate::convert::own_name_parts(
        own_names,
        u.contact,
        username,
        u.first_name.as_deref().unwrap_or(""),
        u.last_name.as_deref().unwrap_or(""),
        tally,
    );
    json!({
        "id": format!("user{}", u.id),
        "name": tgx_format::peer::display_name(first, last, username, u.deleted),
        "username": u.username,
        "bot": u.bot,
    })
}

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

fn channel_ref(peer: PeerRef) -> Option<tl::enums::InputChannel> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn count(n: i32) -> tl::enums::ReactionCount {
        tl::enums::ReactionCount::Count(tl::types::ReactionCount {
            chosen_order: None,
            reaction: tl::enums::Reaction::Emoji(tl::types::ReactionEmoji {
                emoticon: "x".into(),
            }),
            count: n,
        })
    }

    fn reactions(counts: &[i32], named: usize) -> tl::types::MessageReactions {
        tl::types::MessageReactions {
            min: false,
            can_see_list: true,
            reactions_as_tags: false,
            results: counts.iter().map(|n| count(*n)).collect(),
            recent_reactions: Some(
                (0..named)
                    .map(|i| {
                        tl::enums::MessagePeerReaction::Reaction(tl::types::MessagePeerReaction {
                            big: false,
                            unread: false,
                            my: false,
                            peer_id: tl::enums::Peer::User(tl::types::PeerUser {
                                user_id: i as i64,
                            }),
                            date: 0,
                            reaction: tl::enums::Reaction::Emoji(tl::types::ReactionEmoji {
                                emoticon: "x".into(),
                            }),
                        })
                    })
                    .collect(),
            ),
            top_reactors: None,
        }
    }

    #[test]
    fn the_three_name_cap_is_per_message_not_per_reaction() {
        // Two reactions of five: three names shown, seven hidden. Testing per
        // reaction (5 > 3 twice) happens to agree here, but the message-level
        // sum is the rule and this pins it.
        assert!(reactions_are_truncated(&reactions(&[5, 5], 3)));
        // Three names and a total of three: nothing is hidden.
        assert!(!reactions_are_truncated(&reactions(&[2, 1], 3)));
        // One reaction of three with three names: complete.
        assert!(!reactions_are_truncated(&reactions(&[3], 3)));
    }

    #[test]
    fn a_reaction_with_no_named_reactors_is_truncated() {
        assert!(reactions_are_truncated(&reactions(&[4], 0)));
    }

    fn results(min: bool, voters: Option<Vec<i32>>) -> tl::types::PollResults {
        tl::types::PollResults {
            min,
            has_unread_votes: false,
            can_view_stats: false,
            solution_media: None,
            results: voters.map(|vs| {
                vs.into_iter()
                    .map(|v| {
                        tl::enums::PollAnswerVoters::Voters(tl::types::PollAnswerVoters {
                            chosen: false,
                            correct: false,
                            option: vec![0],
                            voters: Some(v),
                            recent_voters: None,
                        })
                    })
                    .collect()
            }),
            total_voters: None,
            recent_voters: None,
            solution: None,
            solution_entities: None,
        }
    }

    #[test]
    fn a_min_poll_needs_refreshing() {
        assert!(poll_needs_refresh(&results(true, Some(vec![3, 4]))));
    }

    #[test]
    fn an_all_zero_poll_needs_refreshing() {
        assert!(poll_needs_refresh(&results(false, Some(vec![0, 0]))));
        assert!(poll_needs_refresh(&results(false, None)));
    }

    #[test]
    fn a_poll_with_real_results_is_left_alone() {
        assert!(!poll_needs_refresh(&results(false, Some(vec![0, 3]))));
    }

    #[tokio::test]
    async fn a_refusal_gives_up_quietly_and_is_not_a_deferral() {
        let mut tally = Enrichment::default();
        let out: Option<()> = guarded(
            &mut tally,
            |_| {},
            || async { Err(EnrichError::Refused("CHAT_ADMIN_REQUIRED".into())) },
        )
        .await;
        assert!(out.is_none());
        assert_eq!(tally.requests, 1, "a refusal must not be retried");
        assert_eq!(tally.deferred, 0, "a refusal is not lost data");
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_is_waited_out_and_retried_once() {
        let mut tally = Enrichment::default();
        let mut waited = Vec::new();
        let calls = std::cell::Cell::new(0);
        let out = guarded(
            &mut tally,
            |s| waited.push(s),
            || {
                let n = calls.get();
                calls.set(n + 1);
                async move {
                    if n == 0 {
                        Err(EnrichError::Transient(Duration::from_secs(30)))
                    } else {
                        Ok(42)
                    }
                }
            },
        )
        .await;
        assert_eq!(out, Some(42));
        assert_eq!(waited, vec![30], "the wait must be reported, not silent");
        assert_eq!(tally.deferred, 0, "it was recovered, so nothing was lost");
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_past_the_cap_is_counted_rather_than_hidden() {
        let mut tally = Enrichment::default();
        let out: Option<()> = guarded(
            &mut tally,
            |_| {},
            || async { Err(EnrichError::Transient(Duration::from_secs(600))) },
        )
        .await;
        assert!(out.is_none());
        assert_eq!(
            tally.deferred, 1,
            "the data was there and we did not get it"
        );
    }

    #[test]
    fn the_cap_is_the_documented_two_minutes() {
        assert_eq!(ENRICH_MAX_WAIT, Duration::from_secs(120));
        assert_eq!(PARTICIPANT_PAGE, 200);
    }

    #[test]
    fn a_short_roster_says_it_is_short() {
        // A truncated roster is indistinguishable from a complete one, which
        // makes it worse than no roster.
        let short = Roster {
            members: vec![json!({"id": "user1"})],
            complete: false,
            capped: false,
            ..Default::default()
        };
        assert_eq!(short.to_json()["complete"], false);
        // And our own cap is a different statement from Telegram cutting us off.
        let capped = Roster {
            members: vec![],
            complete: true,
            capped: true,
            ..Default::default()
        };
        assert_eq!(capped.to_json()["complete"], true);
        assert_eq!(capped.to_json()["capped"], true);
    }
}
