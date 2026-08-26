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
        return roster;
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
                roster.members.push(json!({
                    "id": format!("user{}", u.id),
                    "name": tgx_format::peer::display_name(
                        u.first_name.as_deref().unwrap_or(""),
                        u.last_name.as_deref().unwrap_or(""),
                        u.username.as_deref().unwrap_or(""),
                        u.deleted,
                    ),
                    "username": u.username,
                    "bot": u.bot,
                }));
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
        };
        assert_eq!(short.to_json()["complete"], false);
        // And our own cap is a different statement from Telegram cutting us off.
        let capped = Roster {
            members: vec![],
            complete: true,
            capped: true,
        };
        assert_eq!(capped.to_json()["complete"], true);
        assert_eq!(capped.to_json()["capped"], true);
    }
}
