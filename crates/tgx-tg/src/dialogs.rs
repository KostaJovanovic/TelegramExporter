//! Listing chats, and discovering a forum's topics.
//!
//! **Counting is separate from listing.** The dialog list is free; a chat's
//! exact message count costs one request each, so it is a button the user
//! presses rather than something that happens on load. That is why
//! [`ChatInfo::message_count`] is an `Option` — see the note on it.

use crate::client::{ChatInfo, ChatKind};
use crate::error::{classify, EnrichError};
use grammers_client::peer::Peer;
use grammers_client::session::types::PeerRef;
use grammers_client::Client;
use grammers_tl_types as tl;
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
    let (id, title, kind, is_forum, access_hash) = match &dialog.peer {
        Peer::User(u) => (
            u.id().bare_id().unwrap_or(0),
            u.full_name(),
            if u.is_bot() {
                ChatKind::Bot
            } else {
                ChatKind::Private
            },
            false,
            0,
        ),
        Peer::Group(g) => {
            // Only a *supergroup* can be a forum, and only supergroups carry
            // history a new member can read — which is why the two are listed
            // apart rather than lumped together.
            let (forum, kind) = match &g.raw {
                tl::enums::Chat::Channel(c) => (c.forum, ChatKind::Supergroup),
                _ => (false, ChatKind::Group),
            };
            (
                g.id().bare_id().unwrap_or(0),
                g.title().unwrap_or_default().to_string(),
                kind,
                forum,
                0,
            )
        }
        Peer::Channel(c) => (
            c.id().bare_id().unwrap_or(0),
            c.title().to_string(),
            ChatKind::Channel,
            false,
            0,
        ),
    };

    Some(ChatInfo {
        id,
        title,
        kind,
        last_activity,
        is_forum,
        message_count: None,
        access_hash,
    })
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
pub async fn list_topics(client: &Client, peer: PeerRef) -> Result<Vec<Topic>, EnrichError> {
    let peer: tl::enums::InputPeer = peer.into();
    let mut out: Vec<Topic> = Vec::new();
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
                out.push(Topic {
                    id: t.id as i64,
                    title: t.title.clone(),
                    closed: t.closed,
                    hidden: t.hidden,
                    pinned: t.pinned,
                    icon_emoji_id: t.icon_emoji_id,
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
