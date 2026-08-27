//! Listing chats, and discovering a forum's topics.
//!
//! **Counting is separate from listing.** The dialog list is free; a chat's
//! exact message count costs one request each, so it is a button the user
//! presses rather than something that happens on load. That is why
//! [`ChatInfo::message_count`] is an `Option` — see the note on it.

use crate::cancel::Cancel;
use crate::client::{ChatInfo, ChatKind};
use crate::engine::sleep_in_slices_until;
use crate::error::{classify, EnrichError};
use grammers_client::peer::Peer;
use grammers_client::session::types::PeerRef;
use grammers_client::Client;
use grammers_tl_types as tl;
use std::collections::HashMap;
use tgx_media::topics::{GENERAL_TITLE, GENERAL_TOPIC_ID};

/// Every chat the account can see.
pub async fn list_chats(client: &Client) -> Result<Vec<ChatInfo>, EnrichError> {
    let mut out = Vec::new();
    let mut iter = client.iter_dialogs();
    loop {
        match iter.next().await {
            Ok(Some(dialog)) => {
                if let Some(info) = chat_info(&dialog) {
                    out.push(info);
                }
            }
            Ok(None) => break,
            Err(e) => return Err(classify(&e)),
        }
    }
    Ok(out)
}

fn chat_info(dialog: &grammers_client::peer::Dialog) -> Option<ChatInfo> {
    let last_activity = dialog
        .last_message
        .as_ref()
        .map(|m| m.date().timestamp())
        .unwrap_or(0);

    // `bare_id` is the number Desktop writes into the chat `id` header —
    // 3586682625, not the bot-API -100… form. `id()` gives the tagged form.
    //
    // `public` comes off the raw TL object rather than being defaulted. It was
    // hardcoded — it did not exist — and every private supergroup was therefore
    // written to `result.json` as `"type": "public_supergroup"`.
    //
    // **There is no `access_hash` here any more.** It was collected in all
    // three arms, stored on `ChatInfo`, and read by nothing — and the comment
    // that stood here credited it with saving a rediscovery walk "once per
    // queued chat", which is real and is delivered by `peer_refs_for`'s single
    // dialog sweep, not by this field. A `pub` member of a `pub` struct that is
    // written and never read fires no dead-code lint, which is exactly the
    // class that has already hidden two bugs in this workspace.
    let (id, title, kind, is_forum, public) = match &dialog.peer {
        Peer::User(u) => {
            let public = match &u.raw {
                tl::enums::User::User(r) => r.username.is_some(),
                tl::enums::User::Empty(_) => false,
            };
            (
                u.id().bare_id().unwrap_or(0),
                u.full_name(),
                if u.is_bot() {
                    ChatKind::Bot
                } else {
                    ChatKind::Private
                },
                false,
                public,
            )
        }
        Peer::Group(g) => {
            // Only a *supergroup* can be a forum, and only supergroups carry
            // history a new member can read — which is why the two are listed
            // apart rather than lumped together.
            //
            // A basic `Chat` has no username: it cannot be public.
            let (forum, kind, public) = match &g.raw {
                tl::enums::Chat::Channel(c) => (c.forum, ChatKind::Supergroup, has_username(c)),
                _ => (false, ChatKind::Group, false),
            };
            (
                g.id().bare_id().unwrap_or(0),
                g.title().unwrap_or_default().to_string(),
                kind,
                forum,
                public,
            )
        }
        Peer::Channel(c) => (
            c.id().bare_id().unwrap_or(0),
            c.title().to_string(),
            ChatKind::Channel,
            false,
            has_username(&c.raw),
        ),
    };

    Some(ChatInfo {
        id,
        title,
        kind,
        last_activity,
        is_forum,
        public,
        message_count: None,
    })
}

/// Is this channel or supergroup reachable by a public `t.me/name` link?
///
/// Desktop's `public_*` / `private_*` split is exactly "does it have a
/// username". Both spellings have to be read: Telegram moved to a `usernames`
/// vector for collectible and multiple usernames and leaves the legacy
/// `username` field empty when it does, so testing only the singular reports a
/// public channel as private the moment its owner buys a second name.
fn has_username(c: &tl::types::Channel) -> bool {
    c.username.is_some() || c.usernames.as_ref().is_some_and(|u| !u.is_empty())
}

/// The peer reference for a chat we listed.
///
/// **One canonical copy.** This lived twice — once in the CLI and once in the
/// window — and the two had already drifted apart in their error handling. A
/// peer reference is what every later request is addressed to, so two versions
/// of "which conversation is this" is not a duplication worth keeping.
///
/// `Ok(None)` means the chat is no longer in the dialog list, which is a real
/// answer: chats are left, deleted and archived between the list and the click.
/// `Err` means we never reached the end of the list and therefore do not know.
///
/// **Those two were once the same answer.** This paged with
/// `while let Ok(Some(d)) = iter.next().await`, so a `FLOOD_WAIT` left the loop
/// and returned `None` — and the caller told the user their chat was gone and
/// sent them looking for it. A rate limit is temporary; a chat leaving is not.
pub async fn peer_ref_for(client: &Client, chat_id: i64) -> Result<Option<PeerRef>, EnrichError> {
    // Answered by the same sweep the queue uses, so the one-chat and many-chat
    // paths cannot drift again in what either calls an absence.
    let found = peer_refs_for(client, &[chat_id], &Cancel::new()).await?;
    Ok(found.get(&chat_id).copied())
}

/// Ask Telegram how many messages a chat holds.
///
/// **`0` is a count.** An empty chat legitimately holds zero messages; a chat
/// that could not be counted holds *no* count at all. That is why this returns
/// `Result<i64, _>` and never an `i64` with a sentinel: a caller turns the
/// failure into `None`, never into `Some(0)`, or the list paints "0 messages"
/// over a channel of ten thousand that merely rate-limited.
///
/// **One request per chat.** This is the entire reason counting is a button the
/// user presses rather than something that happens when the list loads — a
/// hundred visible chats would be a hundred requests and a flood wait before
/// the window had finished drawing.
pub async fn count_messages(client: &Client, peer: PeerRef) -> Result<i64, EnrichError> {
    let mut probe = client.iter_messages(peer);
    match probe.total().await {
        Ok(n) => Ok(n as i64),
        Err(e) => Err(classify(&e)),
    }
}

/// Told a chat's id and what counting it produced — `None` for *not counted*.
///
/// Called for **every** chat attempted, failures included, so the caller can
/// distinguish a chat it never reached from one that refused.
pub type CountedFn<'a> = &'a mut (dyn FnMut(i64, Option<i64>) + Send);

/// Told how long Telegram wants us to wait, because silence during a two-minute
/// rate limit is indistinguishable from a hang.
pub type WaitingFn<'a> = &'a mut (dyn FnMut(u64) + Send);

/// Count a list of chats, the given order preserved, stopping when cancelled.
///
/// Returns `(counted, failed)`. Every chat reached is reported through `on`
/// exactly once, with `Some(n)` — **including `Some(0)`, which is a count** —
/// or `None` for one that could not be counted. Chats after a cancel are not
/// reported at all, which is what separates "not counted" from "counted as
/// nothing".
///
/// **One chat refusing must not end the run.** The Python original's counting
/// pass collapsed a rate limit into a permanent refusal and abandoned the
/// remaining chats; here a [`EnrichError::Transient`] waits and retries that one
/// chat *once*, then gives up on it with `None` and moves on. A second wait for
/// the same chat means Telegram is throttling in earnest and spending the rest
/// of the queue's patience on it helps nobody.
///
/// The one dialog sweep that resolves the whole queue gets that same patience,
/// and failing it costs every chat its count — but as `None`, *not counted*,
/// never as a chat that has left the dialog list.
pub async fn count_all(
    client: &Client,
    chat_ids: &[i64],
    cancel: &Cancel,
    on: CountedFn<'_>,
    waiting: WaitingFn<'_>,
) -> (usize, usize) {
    let mut counted = 0usize;
    let mut failed = 0usize;

    // One dialog sweep for the whole queue, not one per chat: resolving each id
    // separately would page the entire dialog list N times to answer N
    // questions the same response already contains.
    //
    // The sweep gets the same patience as a single count below — wait once,
    // then give up — because a second rate limit on the dialog list means
    // Telegram is throttling in earnest, and spending the rest of the queue's
    // patience on the list nobody asked to see helps nobody.
    let mut swept = None;
    let mut retried_sweep = false;
    loop {
        match peer_refs_for(client, chat_ids, cancel).await {
            Ok(peers) => {
                swept = Some(peers);
                break;
            }
            Err(EnrichError::Transient(d)) if !retried_sweep => {
                retried_sweep = true;
                waiting(d.as_secs());
                sleep_in_slices_until(d, cancel).await;
                if cancel.is_cancelled() {
                    return (0, 0);
                }
            }
            Err(_) => break,
        }
    }
    let Some(peers) = swept else {
        if cancel.is_cancelled() {
            // Stop landed while the sweep was failing. A cancel leaves every
            // row showing what it already showed; only a failure the user did
            // not ask for is worth overwriting them with "not counted".
            return (0, 0);
        }
        // A sweep that failed is **not** a queue of chats that are gone. Every
        // chat is reported `None`, "not counted", which is the answer the type
        // exists to keep distinct from `Some(0)`; reporting nothing at all is
        // reserved for a cancel, where each row keeping what it already showed
        // is right. Here the user pressed Count, so a row left untouched would
        // read as the button having done nothing.
        for id in chat_ids {
            on(*id, None);
        }
        return (0, chat_ids.len());
    };

    for id in chat_ids {
        if cancel.is_cancelled() {
            break;
        }
        let Some(peer) = peers.get(id).copied() else {
            // Gone from the dialog list between listing and counting. That is
            // an absence of a count, not a count of zero.
            failed += 1;
            on(*id, None);
            continue;
        };

        let mut retried = false;
        loop {
            match count_messages(client, peer).await {
                Ok(n) => {
                    counted += 1;
                    on(*id, Some(n));
                    break;
                }
                Err(EnrichError::Transient(d)) if !retried => {
                    retried = true;
                    waiting(d.as_secs());
                    // Sliced and cancellable: a click on Stop during the wait
                    // must not be held for the whole of Telegram's two minutes.
                    sleep_in_slices_until(d, cancel).await;
                    if cancel.is_cancelled() {
                        return (counted, failed);
                    }
                }
                Err(_) => {
                    failed += 1;
                    on(*id, None);
                    break;
                }
            }
        }
    }

    (counted, failed)
}

/// Resolve many chat ids in a single pass over the dialog list.
///
/// **Public because an export queue needs this and not [`peer_ref_for`].**
/// Resolving each chat as its turn comes round pages the whole dialog list once
/// per chat — twenty queued chats on a six-hundred-dialog account is twenty
/// full sweeps before the first message is fetched, which is how an export
/// earns a flood wait before it has exported anything.
///
/// An id absent from the map is a chat genuinely absent from the dialog list.
/// An `Err` means the sweep did not finish, which is not the same thing and
/// must not be reported as one — see [`peer_ref_for`] for what that confusion
/// cost. A cancelled sweep returns `Ok` with what it had, because a cancel is
/// the caller's own doing and the caller's own check is what stops it there.
pub async fn peer_refs_for(
    client: &Client,
    chat_ids: &[i64],
    cancel: &Cancel,
) -> Result<HashMap<i64, PeerRef>, EnrichError> {
    let mut out = HashMap::new();
    // Nothing asked, nothing to page. Without this an empty queue still fetches
    // the first page before the "every id answered" test below can notice that
    // it already is.
    if chat_ids.is_empty() {
        return Ok(out);
    }
    let mut iter = client.iter_dialogs();
    loop {
        // Before the request, not after: a cancel should not be held for the
        // length of one more page fetch.
        if cancel.is_cancelled() {
            break;
        }
        let d = match iter.next().await {
            Ok(Some(d)) => d,
            Ok(None) => break,
            Err(e) => return Err(classify(&e)),
        };
        // `bare_id`, matching what `chat_info` recorded — the tagged `-100…`
        // form would never compare equal to an id taken from the same list.
        if let Some(id) = d.peer.id().bare_id() {
            if chat_ids.contains(&id) {
                out.insert(id, d.peer_ref());
            }
        }
        // Stop paging once every id asked about is answered; the dialog list
        // can run to hundreds and a count of three should not read all of it.
        if out.len() == chat_ids.len() {
            break;
        }
    }
    Ok(out)
}

/// One forum topic.
///
/// `ForumTopic` carries 22 fields and this keeps 8. Who opened a topic and when
/// is part of what a topic *is*, and it arrives in the same response as the
/// title — no extra request buys it.
#[derive(Debug, Clone, PartialEq)]
pub struct Topic {
    pub id: i64,
    pub title: String,
    pub closed: bool,
    pub hidden: bool,
    pub pinned: bool,
    pub icon_emoji_id: Option<i64>,
    /// The palette index Telegram assigned the topic's icon.
    pub icon_color: i32,
    /// Who opened it, as a typed peer key.
    ///
    /// **The listing hands this over with the title, so it costs nothing.**
    /// It was being read off the wire and thrown away: who opened a topic and
    /// when is part of what the topic is, and the folder name records neither.
    pub created_by: String,
    pub top_message: i64,
    pub created_date: i64,
}

impl Topic {
    pub fn general() -> Self {
        Self {
            id: GENERAL_TOPIC_ID,
            title: GENERAL_TITLE.to_string(),
            closed: false,
            hidden: false,
            pinned: false,
            icon_emoji_id: None,
            icon_color: 0,
            created_by: String::new(),
            top_message: 0,
            created_date: 0,
        }
    }

    pub fn dirname(&self) -> String {
        tgx_media::topics::topic_dirname(self.id, &self.title)
    }
}

/// Discover a forum's topics.
///
/// There is no high-level API for this: `messages.getForumTopics` is invoked
/// raw. Note the module — in this TL layer the `channels` namespace has only
/// `ToggleForum` and `ToggleViewForumAsMessages`; every topic call lives under
/// `messages`.
///
/// **Telegram always has a General topic even though it is never listed**, so
/// it is added unconditionally. General is not a real message thread, which is
/// why routing reads each message's reply header rather than fetching threads.
/// Pages of 100 topics before we stop asking, whatever the server says.
///
/// 200 pages is 20,000 topics — orders of magnitude past any real forum, so
/// reaching it means the paging contract is broken rather than that the chat is
/// large. Named after `enrich`'s own cap, which this loop was missing.
const MAX_TOPIC_PAGES: u32 = 200;

pub async fn list_topics(client: &Client, peer: PeerRef) -> Result<Vec<Topic>, EnrichError> {
    let peer: tl::enums::InputPeer = peer.into();
    let mut out: Vec<Topic> = Vec::new();
    let mut seen: std::collections::HashSet<i32> = std::collections::HashSet::new();
    let mut pages = 0u32;
    let mut offset_date = 0i32;
    let mut offset_id = 0i32;
    let mut offset_topic = 0i32;

    loop {
        let request = tl::functions::messages::GetForumTopics {
            peer: peer.clone(),
            q: None,
            offset_date,
            offset_id,
            offset_topic,
            limit: 100,
        };
        let response = client.invoke(&request).await.map_err(|e| classify(&e))?;
        let tl::enums::messages::ForumTopics::Topics(page) = response;

        let before = out.len();
        for t in &page.topics {
            if let tl::enums::ForumTopic::Topic(t) = t {
                // **Keyed on the id, not merely appended.** The old loop's only
                // exit was "this page added nothing", which a server replaying
                // the same page never satisfies: the same topics were pushed
                // again, `out.len()` grew every iteration, and the loop ran
                // forever while the vector ate memory. Refusing a duplicate id
                // turns that case back into no progress, which does exit.
                if !seen.insert(t.id) {
                    continue;
                }
                out.push(Topic {
                    id: t.id as i64,
                    title: t.title.clone(),
                    closed: t.closed,
                    hidden: t.hidden,
                    pinned: t.pinned,
                    icon_emoji_id: t.icon_emoji_id,
                    icon_color: t.icon_color,
                    created_by: crate::convert::peer_key(&t.from_id).to_string(),
                    top_message: t.top_message as i64,
                    created_date: t.date as i64,
                });
                offset_topic = t.id;
                offset_id = t.top_message;
                offset_date = t.date;
            }
        }
        // No progress means the server has stopped paging; stop rather than
        // spinning.
        if out.len() == before || page.topics.is_empty() {
            break;
        }
        // A backstop for the case dedupe cannot see: a server paging forever
        // with genuinely new ids. `fetch_participants` has had a cap from the
        // start and this did not, which is the whole of the asymmetry.
        pages += 1;
        if pages >= MAX_TOPIC_PAGES {
            break;
        }
    }

    if !out.iter().any(|t| t.id == GENERAL_TOPIC_ID) {
        out.push(Topic::general());
    }
    out.sort_by_key(|t| t.id);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_is_always_present_and_first() {
        let mut topics = vec![
            Topic {
                id: 66,
                title: "bitno".into(),
                ..Topic::general()
            },
            Topic {
                id: 12,
                title: "foto".into(),
                ..Topic::general()
            },
        ];
        if !topics.iter().any(|t| t.id == GENERAL_TOPIC_ID) {
            topics.push(Topic::general());
        }
        topics.sort_by_key(|t| t.id);
        assert_eq!(topics[0].id, GENERAL_TOPIC_ID);
        assert_eq!(topics[0].title, "General");
        assert_eq!(topics.len(), 3);
    }

    #[test]
    fn a_listed_general_is_not_duplicated() {
        let mut topics = vec![Topic {
            title: "ćaskanje".into(),
            ..Topic::general()
        }];
        if !topics.iter().any(|t| t.id == GENERAL_TOPIC_ID) {
            topics.push(Topic::general());
        }
        assert_eq!(topics.len(), 1);
        // And the real title wins over the placeholder.
        assert_eq!(topics[0].title, "ćaskanje");
    }

    /// What a caller is expected to do with [`count_messages`]'s answer.
    fn to_column(answer: Result<i64, EnrichError>) -> Option<i64> {
        answer.ok()
    }

    #[test]
    fn zero_is_a_count_and_a_failure_is_not() {
        // The two look identical in the list — a blank cell and "0" are one
        // keystroke apart — and mean opposite things: one chat is empty, the
        // other was never asked. A failure that became `Some(0)` would paint
        // "0 messages" over a channel that merely rate-limited.
        assert_eq!(to_column(Ok(0)), Some(0));
        assert_eq!(to_column(Ok(6643)), Some(6643));
        assert_eq!(
            to_column(Err(EnrichError::Transient(std::time::Duration::from_secs(
                30
            )))),
            None
        );
        assert_eq!(
            to_column(Err(EnrichError::Refused("CHANNEL_PRIVATE".into()))),
            None
        );
    }

    /// What a caller is expected to say about [`peer_ref_for`]'s answer.
    fn to_diagnosis(answer: Result<Option<()>, EnrichError>) -> &'static str {
        match answer {
            Ok(Some(())) => "found",
            Ok(None) => "no longer in the dialog list",
            Err(_) => "could not be looked up",
        }
    }

    #[test]
    fn a_rate_limit_while_paging_dialogs_is_not_a_chat_that_left() {
        // The three answers were two: any error from paging exited the loop and
        // came back as `None`, so a flood wait told the user the chat was gone.
        // "Gone" sends them looking for a chat they still have; "could not be
        // looked up" tells them to try again, which is the truth.
        assert_eq!(to_diagnosis(Ok(Some(()))), "found");
        assert_eq!(to_diagnosis(Ok(None)), "no longer in the dialog list");
        assert_eq!(
            to_diagnosis(Err(EnrichError::Transient(std::time::Duration::from_secs(
                30
            )))),
            "could not be looked up"
        );
        assert_eq!(
            to_diagnosis(Err(EnrichError::Failed("session closed".into()))),
            "could not be looked up"
        );
    }

    #[test]
    fn topic_folders_are_id_prefixed() {
        let t = Topic {
            id: 42,
            title: "Backend".into(),
            ..Topic::general()
        };
        assert_eq!(t.dirname(), "0042 - Backend");
    }
}
