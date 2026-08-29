//! Learning who people are, and the two per-message requests that recover
//! what the wire did not carry.
//!
//! **Names are harvested from the messages themselves**, not only from the
//! participant roster. Anybody who posted and then left the group is absent
//! from the roster but is, by definition, the sender of their own messages;
//! filling the book from the roster alone left 206 fields empty in a live
//! export -- and that was with the roster on. With it off it would have been
//! every name.

use super::*;

impl<'a> ChatExporter<'a> {
    /// Which topic this message belongs to.
    pub(super) fn route(&self, msg: &grammers_client::message::Message, topics: &[Topic]) -> i64 {
        let raw = &msg.raw;
        let (creates_topic, reply) = match raw {
            tl::enums::Message::Service(s) => (
                matches!(s.action, tl::enums::MessageAction::TopicCreate(_)),
                reply_header(s.reply_to.as_ref()),
            ),
            tl::enums::Message::Message(m) => (false, reply_header(m.reply_to.as_ref())),
            tl::enums::Message::Empty(_) => (false, None),
        };
        let id = msg.id() as i64;
        let routed = topic_id_for(id, creates_topic, reply);
        // A message pointing at a topic we never saw listed still has to land
        // somewhere; General is where Telegram itself would put it.
        if topics.iter().any(|t| t.id == routed) {
            routed
        } else {
            GENERAL_TOPIC_ID
        }
    }

    /// Add whatever peers this message brought with it to the name book.
    ///
    /// grammers keeps the whole peer set a response carried, but only the
    /// sender and the chat are reachable from outside the crate. This used to
    /// say "which is enough: the names that were missing belonged to people who
    /// had posted, so they arrive as the sender of their own messages", and the
    /// first live run falsified it — 94 `forwarded_from` fields came out empty
    /// across 13 people, every one with a correct `forwarded_from_id` and every
    /// one named in Desktop's export of the same chat. Someone you forward
    /// *from* need never have posted in the chat you are exporting.
    /// [`Self::learn_forward_origin`] is the other half.
    ///
    /// The chat is learned too, because a migration notice's actor *is* the
    /// chat and had no other source.
    pub(super) fn learn_peers(&mut self, msg: &grammers_client::message::Message) {
        for peer in [msg.sender(), msg.peer()].into_iter().flatten() {
            self.learn_peer(peer);
        }
    }

    pub(super) fn learn_peer(&mut self, peer: &grammers_client::peer::Peer) {
        match peer {
            grammers_client::peer::Peer::User(u) => {
                if let tl::enums::User::User(raw) = &u.raw {
                    self.names.learn_user(raw);
                }
            }
            grammers_client::peer::Peer::Group(g) => {
                let key = match &g.raw {
                    tl::enums::Chat::Channel(c) => PeerKey::channel(c.id),
                    _ => PeerKey::chat(g.id().bare_id().unwrap_or(0)),
                };
                self.names
                    .learn_chat_title(key, g.title().unwrap_or_default());
            }
            grammers_client::peer::Peer::Channel(c) => {
                self.names
                    .learn_chat_title(PeerKey::channel(c.raw.id), c.title());
            }
        }
    }

    /// Name a forward origin that neither the message nor the name book knows.
    ///
    /// **This has to happen before the message is converted**, not in a batch at
    /// the end: `result.json` is streamed, so a name resolved afterwards has
    /// nowhere left to go.
    ///
    /// The route needs no change in grammers. `grammers_session::Session` is a
    /// public trait; the session has been caching every peer every response
    /// carried since sign-in; `peer_ref` turns a cached id into the id *plus the
    /// access hash*; and `Client::resolve_peer` turns that into a named peer with
    /// one request. `PeerInfo` holds no name, so the request is real — but it is
    /// one per *person*, and only for people nothing else named: 13 requests
    /// against 6,643 messages on the last live run.
    ///
    /// Every failure degrades to exactly what happens today, an empty name.
    pub(super) async fn learn_forward_origin(
        &mut self,
        m: &tl::types::Message,
        tally: &mut Enrichment,
    ) {
        let Some(peer) = unnamed_forward_origin(m, &self.names) else {
            return;
        };
        let Some(id) = session_peer_id(peer) else {
            return;
        };
        self.resolve_and_learn(crate::convert::peer_key(peer).to_string(), id, tally)
            .await;
    }

    /// Name the people a **service message** is about.
    ///
    /// The same problem as a forward origin and the same cure: the inviter on a
    /// `join_group_by_link` joined before the exported history begins, so no
    /// message carries their user object, and the roster holds them only if they
    /// are still a member. 26 `inviter` fields and 2 `members[0]` came out empty
    /// in one live export, each beside a correct id, each named in Desktop's
    /// export of the same chat. No replay leg can see it: all three start from a
    /// Desktop `result.json` that already has the name.
    ///
    /// **Before the conversion**, like every other name: `result.json` is
    /// streamed, so one resolved afterwards has nowhere left to go.
    ///
    /// Cheap for the same reason the forward path is — one request per *person*,
    /// tried once whether or not it worked, and only for people nothing else
    /// named. The reference's 63 service messages name 28 fields between them.
    pub(super) async fn learn_service_actors(
        &mut self,
        s: &tl::types::MessageService,
        tally: &mut Enrichment,
    ) {
        for id in crate::convert::action_user_ids(&s.action) {
            let key = PeerKey::user(id).to_string();
            if !self.names.get(&key).is_empty() {
                continue;
            }
            let Some(peer) = PeerId::user(id) else {
                continue;
            };
            self.resolve_and_learn(key, peer, tally).await;
        }
    }

    /// One name, from the session's peer store and one request.
    ///
    /// `tried` is recorded whether or not the lookup worked: a peer the store
    /// does not hold must not be looked up again on every one of that person's
    /// messages.
    async fn resolve_and_learn(&mut self, key: String, id: PeerId, tally: &mut Enrichment) {
        if !self.peers_tried.insert(key) {
            return;
        }
        let peer_ref = match self.session.peer_ref(id).await {
            Ok(Some(r)) => r,
            // Not cached, or the store could not be read. Either way there is no
            // access hash, and without one the request cannot be made at all.
            _ => return,
        };
        tally.requests += 1;
        match self.client.resolve_peer(peer_ref).await {
            Ok(p) => self.learn_peer(&p),
            Err(_) => tally.deferred += 1,
        }
    }

    /// The two per-message requests, each fired **only when the message says
    /// the data is missing**.
    ///
    /// That conditional is the whole cost argument: on the reference export the
    /// reaction list is short on 77 of 963 reacted messages, which is a 1.16%
    /// increase in requests, not one per message.
    ///
    /// Both settings defaulted to on and were read by nothing, so neither
    /// request had ever been made. `enrich::reactions_are_truncated` and
    /// `enrich::poll_needs_refresh` were written to gate them and were
    /// unreachable too.
    pub(super) async fn enrich_message(
        &mut self,
        msg: &grammers_client::message::Message,
        peer: PeerRef,
        tally: &mut Enrichment,
        progress: ProgressFn<'_>,
    ) -> MessageExtras {
        let mut extras = MessageExtras::default();
        let m = match &msg.raw {
            tl::enums::Message::Message(m) => m,
            // A service message has no media, no poll and no reaction sample to
            // top up, but it does name people — and until this arm existed it
            // came through here untouched and reached `base_service` with an
            // empty name book for anyone who had not posted.
            tl::enums::Message::Service(s) => {
                self.learn_service_actors(s, tally).await;
                return extras;
            }
            tl::enums::Message::Empty(_) => return extras,
        };
        let id = m.id;

        // Before the conversion, like everything else here, and for the sharper
        // version of the same reason: the JSON is streamed, so a name learned
        // after `base_message` has run has nowhere left to go.
        self.learn_forward_origin(m, tally).await;

        if self.settings.full_reactions {
            if let Some(tl::enums::MessageReactions::Reactions(r)) = &m.reactions {
                if enrich::reactions_are_truncated(r) {
                    let client = self.client;
                    let got = enrich::guarded(
                        tally,
                        |secs| progress(Progress::FloodWait { seconds: secs }),
                        || enrich::fetch_reactors(client, peer, id),
                    )
                    .await;
                    // Longer, or it is not an improvement on the sample the
                    // message already carried. Anonymous reactors are in
                    // neither, so a shorter answer is Telegram's, not a loss.
                    let named = r.recent_reactions.as_ref().map(Vec::len).unwrap_or(0);
                    if let Some(list) = got.filter(|l| l.len() > named) {
                        extras.reactors = Some(list);
                    }
                }
            }
        }

        if self.settings.refresh_polls {
            if let Some(tl::enums::MessageMedia::Poll(p)) = &m.media {
                let tl::enums::PollResults::Results(r) = &p.results;
                if enrich::poll_needs_refresh(r) {
                    let client = self.client;
                    extras.poll_results = enrich::guarded(
                        tally,
                        |secs| progress(Progress::FloodWait { seconds: secs }),
                        || enrich::fetch_poll_results(client, peer, id),
                    )
                    .await
                    .flatten();
                }
            }
        }
        extras
    }
}
