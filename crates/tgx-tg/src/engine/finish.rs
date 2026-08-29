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
    pub(super) fn close_all(
        sinks: &mut HashMap<i64, TopicSink>,
        root: &Path,
        chat: &ChatInfo,
        topics: &[Topic],
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
            if let Err(e) = Self::write_index(root, chat, topics, &written, result) {
                progress(Progress::Log(format!("writing the index: {e}")));
            }
        }
    }

    pub(super) fn write_index(
        root: &Path,
        chat: &ChatInfo,
        topics: &[Topic],
        written: &HashMap<i64, usize>,
        result: &ExportResult,
    ) -> std::io::Result<()> {
        // Only topics that produced a folder: an empty one had its directory
        // removed above, so listing it would be the same dead link again.
        let entries: Vec<tgx_html::index::IndexEntry> = topics
            .iter()
            .filter_map(|t| {
                let messages = *written.get(&t.id)?;
                let detail = match (t.pinned, t.closed) {
                    (true, _) => Some("pinned".to_string()),
                    (false, true) => Some("closed".to_string()),
                    _ => None,
                };
                Some(tgx_html::index::IndexEntry {
                    href: format!("{}/messages.html", t.dirname()),
                    title: t.title.clone(),
                    initials: tgx_format::peer::initials_from_title(&t.title).unwrap_or_default(),
                    colour: tgx_html::userpic::userpic_class(&t.id.to_string(), None),
                    detail,
                    subname: tgx_format::date_pair(t.created_date)
                        .map(|(d, _)| format!("opened {}", &d[..10])),
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
        tgx_html::index::write_index(root, &chat.title, &parts.join(", "), None, &entries)
    }
}
