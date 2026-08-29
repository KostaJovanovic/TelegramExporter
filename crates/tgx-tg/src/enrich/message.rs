//! The two per-message recoveries: the full reactor list, and a poll's real
//! results.
//!
//! Both exist because Telegram sends a *sample* unless asked otherwise.
//! Reactions come with at most three names **per message**, not per reaction,
//! so a message with two reactions of five shows three people and hides seven.
//! A poll that has closed arrives with its results zeroed, which exports as a
//! poll nobody voted in.
//!
//! Each is separately switchable, costs traffic, and degrades to nothing on
//! failure -- never to an error that ends the export.

use super::*;

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
/// full". Omitting it raises before any request goes out, and an error
/// swallowed at that point leaves the feature silently never working — which
/// is how this last failed, and why the test below exists.
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
