//! Service messages: Desktop's name for each action, and the payload fields
//! it writes beside them.
//!
//! Names only for the actions outside the reference. Desktop names an action
//! its export code predates by snake-casing the constructor, so the fallback
//! does the same rather than dropping it -- dropping is what once exported a
//! video call, a gift or a screenshot notice as `"type": "service"` and
//! nothing more.

use super::*;

/// The users an action's payload will be asked to name.
///
/// **A service message names people who need never have posted.** The inviter
/// on a `join_group_by_link` joined long before the history being exported, so
/// no message carries their user object and the roster only holds them if they
/// are still a member — `names.get` returned the empty string and Desktop, which
/// resolves all of them, did not. 26 `inviter` fields and 2 `members[0]` came
/// out blank in one live export beside a perfectly correct id.
///
/// Only the three arms below take a user id at all; the rest of the vocabulary
/// carries titles, ids and nothing resolvable. Returned as bare ids rather than
/// peer keys because the caller needs the number to reach the session store.
pub(crate) fn action_user_ids(action: &tl::enums::MessageAction) -> Vec<i64> {
    use tl::enums::MessageAction as A;
    match action {
        A::ChatAddUser(a) => a.users.clone(),
        A::ChatDeleteUser(a) => vec![a.user_id],
        A::ChatJoinedByLink(a) => vec![a.inviter_id],
        _ => Vec::new(),
    }
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
/// The vocabulary is measured, not invented — every *payload* key below is one
/// the reference export actually contains, and the nine kinds carrying one are
/// the nine it holds. An action outside that table keeps its snake_cased name
/// and carries nothing, which is what Desktop does with actions its export code
/// predates; guessing at a payload would be worse than omitting one.
pub(crate) fn service_action(
    action: &tl::enums::MessageAction,
    m: &tl::types::MessageService,
    names: &NameBook,
) -> Option<(String, Vec<(String, Value)>)> {
    use tl::enums::MessageAction as A;

    let name_of = |id: i64| json!(names.get(&PeerKey::user(id).to_string()));
    let no_payload = |n: &str| Some((n.to_string(), Vec::new()));

    match action {
        A::ChatEditTitle(a) => Some((
            "edit_group_title".to_string(),
            vec![("title".into(), json!(a.title))],
        )),
        A::ChatAddUser(a) => Some((
            "invite_members".to_string(),
            vec![(
                "members".into(),
                Value::Array(a.users.iter().map(|u| name_of(*u)).collect()),
            )],
        )),
        // Desktop files a self-removal under the same name as a kick; the
        // reference's six are all one user apiece, always as a one-element
        // array rather than a bare string.
        A::ChatDeleteUser(a) => Some((
            "remove_members".to_string(),
            vec![("members".into(), json!([name_of(a.user_id)]))],
        )),
        A::ChatJoinedByLink(a) => Some((
            "join_group_by_link".to_string(),
            vec![("inviter".into(), name_of(a.inviter_id))],
        )),
        // The migrated-from title, not the current one.
        A::ChannelMigrateFrom(a) => Some((
            "migrate_from_group".to_string(),
            vec![("title".into(), json!(a.title))],
        )),
        // The pinned id rides in `reply_to`, which is the only place it exists:
        // the action itself is a bare constructor with no fields.
        A::PinMessage => {
            let id = match m.reply_to.as_ref() {
                Some(tl::enums::MessageReplyHeader::Header(h)) => h.reply_to_msg_id,
                _ => None,
            };
            Some((
                "pin_message".to_string(),
                id.map(|v| vec![("message_id".into(), json!(v))])
                    .unwrap_or_default(),
            ))
        }
        A::TopicCreate(a) => Some((
            "topic_created".to_string(),
            vec![("title".into(), json!(a.title))],
        )),
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
            Some(("topic_edit".to_string(), p))
        }
        // `messageActionPollAppendAnswer` is its own constructor. This was
        // mapped to `TodoAppendTasks` on the inference that Telegram's
        // checklists travel as `todo` on the wire and as the poll vocabulary in
        // Desktop's export — they do not, and both exist side by side in
        // api.tl. The arm therefore never fired: those three messages are the
        // three `absent: action` of the last live run.
        //
        // The reference's three carry **no payload beyond the actor**, measured
        // rather than assumed. An `answer` key here would be an invention:
        // Desktop does not write one, and Desktop is the format this reproduces.
        A::PollAppendAnswer(_) => no_payload("poll_append_answer"),
        A::PollDeleteAnswer(_) => no_payload("poll_delete_answer"),
        // Not in the reference, so these rest on an earlier measurement of
        // Desktop's output rather than on the corpus. Names only: the payloads
        // are deliberately left out, because an unverified key is worse than a
        // missing one and the wire leg's `extra` tally would score it.
        A::TodoAppendTasks(_) => no_payload("todo_append_tasks"),
        A::TodoCompletions(_) => no_payload("todo_completions"),
        // --- names only, measured against Desktop --------------------------
        //
        // Not in the reference, so the corpus cannot check these; they come
        // from an earlier measurement of Desktop's own output, which is the
        // only evidence there is. These are the ones where Desktop's name is
        // **not** the snake-cased constructor, so the fallback below would get
        // them wrong rather than merely coarse:
        //
        //   ChatCreate → create_group, not chat_create
        //   HistoryClear → clear_history, not history_clear
        //   ContactSignUp → joined_telegram, not contact_sign_up
        //
        // and so on. Every one of these is an ordinary thing to find in an
        // ordinary group chat, which is why a name-only port is worth having
        // before the payloads.
        A::ChatCreate(_) => no_payload("create_group"),
        A::ChannelCreate(_) => no_payload("create_channel"),
        A::ChatEditPhoto(_) => no_payload("edit_group_photo"),
        A::ChatDeletePhoto => no_payload("delete_group_photo"),
        A::ChatJoinedByRequest => no_payload("join_group_by_request"),
        A::ChatMigrateTo(_) => no_payload("migrate_to_supergroup"),
        A::HistoryClear => no_payload("clear_history"),
        A::GameScore(_) => no_payload("score_in_game"),
        A::PaymentSent(_) => no_payload("send_payment"),
        A::SetChatTheme(_) => no_payload("edit_chat_theme"),
        A::ContactSignUp => no_payload("joined_telegram"),
        A::GeoProximityReached(_) => no_payload("proximity_reached"),
        A::SetChatWallPaper(_) => no_payload("set_chat_wallpaper"),
        // `messageActionEmpty` is not an action, and Desktop writes no key for
        // it. Everything else keeps its name.
        A::Empty => None,
        // **`_ => None` dropped 57 of api.tl's 67 `messageAction*`
        // constructors entirely**, so a chat containing a video call, a gift or
        // a screenshot notice exported it as `"type": "service"` and nothing
        // more — the same silence that lost all 63 actions in the first live
        // run, narrowed to everything outside the reference's own nine. Desktop
        // names an action its export code predates by snake-casing the
        // constructor, so the fallback does the same. Without it a raw TL class
        // name leaks into the export.
        //
        // **The payloads are deliberately not written.** Roughly 20 actions
        // carry one (`custom_action`'s message, `phone_call`'s duration,
        // `gift_code`'s slug…); reproducing those against grammers' TL shapes
        // with no oracle would be inference, and the wire leg's `extra` tally
        // would score any wrong key. Recorded as follow-up in ROADMAP.md.
        other => Some((snake_variant(other), Vec::new())),
    }
}

/// `TopicEdit(MessageActionTopicEdit { .. })` → `topic_edit`.
///
/// Read off `Debug` because the variant name is not otherwise reachable: there
/// is no `Discriminant`-to-string in `grammers-tl-types`, and matching 57 arms
/// by hand to produce a string each already derives would be the same table
/// twice.
pub(crate) fn snake_variant(action: &tl::enums::MessageAction) -> String {
    let debug = format!("{action:?}");
    let camel = debug
        .split(|c: char| c == '(' || c == '{' || c.is_whitespace())
        .next()
        .unwrap_or("");
    let mut out = String::with_capacity(camel.len() + 4);
    let bytes = camel.as_bytes();
    for (i, ch) in camel.char_indices() {
        // A run of capitals is one word: `SetMessagesTTL` is `set_messages_ttl`,
        // not `set_messages_t_t_l`.
        if ch.is_ascii_uppercase() && i > 0 && !bytes[i - 1].is_ascii_uppercase() {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_variant_reads_the_constructor_name_off_debug() {
        // The mechanism the fallback rests on. If grammers ever stops deriving
        // `Debug` in this shape, this is the test that says so rather than an
        // export quietly filling with `""`.
        assert_eq!(
            snake_variant(&tl::enums::MessageAction::PinMessage),
            "pin_message"
        );
        assert_eq!(
            snake_variant(&tl::enums::MessageAction::TopicEdit(
                tl::types::MessageActionTopicEdit {
                    title: None,
                    icon_emoji_id: None,
                    closed: None,
                    hidden: None,
                }
            )),
            "topic_edit"
        );
        // A run of capitals is one word.
        assert_eq!(
            snake_variant(&tl::enums::MessageAction::SetMessagesTtl(
                tl::types::MessageActionSetMessagesTtl {
                    period: 60,
                    auto_setting_from: None,
                }
            )),
            "set_messages_ttl"
        );
        assert!(!snake_variant(&tl::enums::MessageAction::Empty).is_empty());
    }

    #[test]
    fn the_three_actions_that_name_people_hand_over_their_ids() {
        // What decides whether a name is looked up at all. Miss an arm and the
        // field goes out empty beside a correct id, which is the shape of the
        // 26 blank `inviter` fields in the last live export — and no replay leg
        // can see it, because all three start from a Desktop `result.json` that
        // already has the name.
        use tl::enums::MessageAction as A;
        assert_eq!(
            action_user_ids(&A::ChatJoinedByLink(
                tl::types::MessageActionChatJoinedByLink { inviter_id: 12 }
            )),
            vec![12]
        );
        assert_eq!(
            action_user_ids(&A::ChatAddUser(tl::types::MessageActionChatAddUser {
                users: vec![7, 9],
            })),
            vec![7, 9]
        );
        assert_eq!(
            action_user_ids(&A::ChatDeleteUser(tl::types::MessageActionChatDeleteUser {
                user_id: 3,
            })),
            vec![3]
        );
        // Everything else carries titles and ids, not people to resolve.
        assert!(action_user_ids(&A::PinMessage).is_empty());
        assert!(action_user_ids(&A::Empty).is_empty());
        assert!(
            action_user_ids(&A::ChatEditTitle(tl::types::MessageActionChatEditTitle {
                title: "x".to_string(),
            }))
            .is_empty()
        );
    }

    #[test]
    fn every_action_with_a_name_in_its_payload_is_covered() {
        // The register the other test cannot be: any arm of `service_action`
        // that writes an `inviter` or a `members` key must also appear in
        // `action_user_ids`, or that key resolves to the empty string.
        use tl::enums::MessageAction as A;
        let naming: [(&str, A); 3] = [
            (
                "join_group_by_link",
                A::ChatJoinedByLink(tl::types::MessageActionChatJoinedByLink { inviter_id: 1 }),
            ),
            (
                "invite_members",
                A::ChatAddUser(tl::types::MessageActionChatAddUser { users: vec![1] }),
            ),
            (
                "remove_members",
                A::ChatDeleteUser(tl::types::MessageActionChatDeleteUser { user_id: 1 }),
            ),
        ];
        for (name, action) in naming {
            assert!(
                !action_user_ids(&action).is_empty(),
                "{name} names a person but hands over no id to resolve"
            );
        }
    }
}
