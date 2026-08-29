//! Ending an export: draining every topic's output, and writing the index
//! page they all link back to.
//!
//! **Closing drains.** The JSON is streamed, so a run abandoned without
//! `close()` leaves a file that is not truncated but *zero bytes*.

use super::*;

impl<'a> ChatExporter<'a> {
    /// **Closing drains.** Every path that can end an export comes through
    /// here, including the two error returns above: the JSON is streamed, so an
    /// abandoned output is a *zero-byte* file, not a truncated one.
    ///
    /// Eight arguments, and the grouping that would fix it is the run-state
    /// struct `run()` needs for the same reason — five of these are immutable
    /// borrows of the same export, but they cannot be bundled once and reused,
    /// because `&self.names` would then be held across a loop body that calls
    /// `&mut self`. It is nine call sites in the one file no parity leg covers,
    /// so it is its own change, not a rider on a defect fix.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn close_all(
        sinks: &mut HashMap<i64, TopicSink>,
        root: &Path,
        chat: &ChatInfo,
        topics: &[Topic],
        names: &NameBook,
        split: bool,
        result: &mut ExportResult,
        progress: ProgressFn<'_>,
    ) {
        let mut written: HashMap<i64, usize> = HashMap::new();
        let mut ids: Vec<i64> = sinks.keys().copied().collect();
        ids.sort();
        for id in ids {
            let Some(sink) = sinks.get_mut(&id) else {
                continue;
            };
            let n = sink.output.count();
            if let Err(e) = sink.output.close() {
                progress(Progress::Log(format!("closing {}: {e}", sink.title)));
            }
            if n == 0 {
                // Topics with no messages are skipped rather than producing
                // empty folders; the count is reported when the job finishes.
                result.empty_topics += 1;
                // **Only a topic subfolder.** When the chat is not split by
                // topic, `Output::new` was handed `root` itself, so
                // `sink.output.root == root` — and pruning it deleted the whole
                // chat directory, taking `participants.json` (written before
                // the read) with it and releasing the `unique_dir` reservation.
                // An empty chat should leave an empty export, not no export.
                if sink.output.root != root {
                    let _ = std::fs::remove_dir_all(&sink.output.root);
                }
            } else {
                result.topics += 1;
                written.insert(id, n);
                progress(Progress::Topic {
                    title: sink.title.clone(),
                    messages: n,
                });
            }
            for name in sink.output.degraded.names() {
                if !result.degraded.contains(&name) {
                    result.degraded.push(name);
                }
            }
        }

        // **The page every topic page links back to.** Each one opens with
        // `<a href="../export_results.html">`, and nothing ever wrote that
        // file — verified on a real export, where all nine pages carried the
        // link and the target was absent. The link comes from the same `split`
        // branch that sets `back_href`, so the two cannot drift apart.
        if split {
            if let Err(e) = Self::write_index(root, chat, topics, names, &written, result) {
                progress(Progress::Log(format!("writing the index: {e}")));
            }
        }
    }

    pub(super) fn write_index(
        root: &Path,
        chat: &ChatInfo,
        topics: &[Topic],
        names: &NameBook,
        written: &HashMap<i64, usize>,
        result: &ExportResult,
    ) -> std::io::Result<()> {
        // Only topics that produced a folder: an empty one had its directory
        // removed above, so listing it would be the same dead link again.
        let entries: Vec<tgx_html::index::IndexEntry> = topics
            .iter()
            .filter_map(|t| {
                let messages = *written.get(&t.id)?;
                Some(tgx_html::index::IndexEntry {
                    href: format!("{}/messages.html", t.dirname()),
                    title: t.title.clone(),
                    initials: tgx_format::peer::initials_from_title(&t.title).unwrap_or_default(),
                    colour: tgx_html::userpic::userpic_class(&t.id.to_string(), None),
                    detail: topic_state(t),
                    subname: topic_origin(t, names),
                    messages,
                })
            })
            .collect();

        let mut parts = vec![
            tgx_html::index::plural(entries.len(), "topic"),
            tgx_html::index::plural(result.messages, "message"),
        ];
        if result.members > 0 {
            parts.push(tgx_html::index::plural(result.members, "member"));
        }
        // See `ExportResult::has_invite_link` for why the link itself does not
        // come with it.
        let about = result
            .has_invite_link
            .then_some("Invite link: [INVITE LINK] — redacted here, kept in result.json");
        tgx_html::index::write_index(root, &chat.title, &parts.join(", "), about, &entries)
    }
}

/// A topic's state markers, joined — `pinned`, `closed`, `pinned, closed`.
///
/// **Every marker that applies, not the first one that does.** A `match` on
/// `(pinned, closed)` whose first arm was `(true, _)` reported a topic that was
/// both as merely pinned, which lost `closed` on the two topics in the reference
/// where both are true — and "closed" is the marker a reader needs, because it
/// is the one that explains why a topic stops. `IndexEntry::detail` has always
/// documented the joined form; nothing produced it.
fn topic_state(t: &Topic) -> Option<String> {
    let mut marks = Vec::new();
    if t.pinned {
        marks.push("pinned");
    }
    if t.closed {
        marks.push("closed");
    }
    if t.hidden {
        marks.push("hidden");
    }
    (!marks.is_empty()).then(|| marks.join(", "))
}

/// `opened by Ivana ETF, 14.12.2025` — or just the date, if we cannot name them.
///
/// `Topic::created_by` arrives free with the title and was read off the wire and
/// then rendered by nothing. It is a typed peer key, so it needs the name book;
/// an unknown peer degrades to the date alone rather than to `opened by ,`.
///
/// **A zero date is no date, not the epoch.** General is synthesised rather than
/// listed — Telegram never sends it — so it carries `created_date: 0`, and
/// handing that to `date_pair` gave every forum export an index whose first row
/// read `opened 1970-01-01`.
fn topic_origin(t: &Topic, names: &NameBook) -> Option<String> {
    if t.created_date == 0 {
        return None;
    }
    let date = tgx_format::date_pair(t.created_date).map(|(d, _)| d[..10].to_string())?;
    match names.get(&t.created_by) {
        "" => Some(format!("opened {date}")),
        who => Some(format!("opened by {who}, {date}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::UserFacts;

    fn topic(pinned: bool, closed: bool) -> Topic {
        Topic {
            pinned,
            closed,
            created_by: PeerKey::user(7).to_string(),
            created_date: 1_765_000_000,
            ..Topic::general()
        }
    }

    #[test]
    fn a_topic_that_is_both_pinned_and_closed_says_both() {
        // The old `match` led with `(true, _)`, so `closed` was unreachable for
        // any pinned topic — and `closed` is the marker that explains why a
        // topic stops. Two topics in the reference are both.
        assert_eq!(
            topic_state(&topic(true, true)).as_deref(),
            Some("pinned, closed")
        );
        assert_eq!(topic_state(&topic(true, false)).as_deref(), Some("pinned"));
        assert_eq!(topic_state(&topic(false, true)).as_deref(), Some("closed"));
        assert_eq!(topic_state(&topic(false, false)), None);
    }

    #[test]
    fn a_topic_says_who_opened_it() {
        let mut names = NameBook::default();
        names.learn(UserFacts {
            id: 7,
            first: "Ivana",
            last: "ETF",
            ..UserFacts::default()
        });
        let line = topic_origin(&topic(false, false), &names).expect("a date");
        assert!(line.starts_with("opened by Ivana ETF, "), "{line}");
    }

    #[test]
    fn an_unnamed_creator_leaves_the_date_alone() {
        // `created_by` is a peer key, so an unknown one must degrade to the
        // date rather than to `opened by , 06.12.2025`.
        let line = topic_origin(&topic(false, false), &NameBook::default()).expect("a date");
        assert!(line.starts_with("opened "), "{line}");
        assert!(!line.contains("by"), "{line}");
    }

    #[test]
    fn a_topic_with_no_creation_date_renders_nothing() {
        // `Topic::general()` is synthesised, not listed, so it has no date.
        assert_eq!(topic_origin(&Topic::general(), &NameBook::default()), None);
    }
}
