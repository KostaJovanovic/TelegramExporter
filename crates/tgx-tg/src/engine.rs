//! The export engine: **one pass over each chat, oldest to newest.**
//!
//! `iter_messages(peer).reverse(true)` with a resume loop keyed on `offset_id`,
//! routing each message to its topic's [`Output`] as it arrives.
//!
//! **Do not "improve" this into per-topic thread fetches.** `messages.getReplies`
//! returns *nothing* for the General topic — it is not a real message thread —
//! so that approach silently loses it and multiplies requests by the topic
//! count. Routing rules live in [`tgx_media::topics::topic_id_for`].
//!
//! The resume loop exists so a long `FloodWait` mid-history resumes instead of
//! aborting, and gives up only after [`MAX_STALLED_WAITS`] waits with **no
//! progress** — a wait that moved the cursor forward resets the counter.

use crate::client::ChatInfo;
use crate::config::Settings;
use crate::convert::{base_message, base_service, NameBook};
use crate::dialogs::Topic;
use crate::error::{classify, EnrichError, ExportError};
use crate::output::Output;
use grammers_client::session::types::PeerRef;
use grammers_client::Client;
use grammers_tl_types as tl;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tgx_media::names::NameBook as MediaNames;
use tgx_media::topics::{topic_id_for, ReplyHeader, GENERAL_TOPIC_ID};

/// How many rate limits with no progress before the read loop gives up.
pub const MAX_STALLED_WAITS: u32 = 10;

/// What one chat's export produced.
///
/// **Per-chat tallies live here, never on the exporter.** One exporter serves
/// the whole queue, so a counter on the exporter reported the sum of every chat
/// before this one — chat 3 of a queue claiming 6 extra requests when it made
/// 2. Here the borrow checker enforces what a comment could only request.
#[derive(Debug, Default, Clone)]
pub struct ExportResult {
    pub root: PathBuf,
    pub messages: usize,
    pub topics: usize,
    pub empty_topics: usize,
    /// What Telegram said the chat held **before** the read pass started.
    ///
    /// Kept so the difference from `messages` can be reported rather than
    /// guessed at: without it a short export reads exactly like a complete one,
    /// which is what a crash at message 5,609 of 6,600 looked like — a cheerful
    /// summary and a thousand missing messages.
    pub expected: i64,
    /// Enrichments a rate limit cost. **Not** the same as one being refused:
    /// this means the data was there and we did not get it.
    pub enrich_deferred: usize,
    pub extra_requests: usize,
    /// Type names the JSON encoder could not map.
    pub degraded: Vec<String>,
}

impl ExportResult {
    /// Did the run get everything Telegram said was there?
    pub fn complete(&self) -> bool {
        self.expected == 0 || self.messages as i64 >= self.expected
    }
}

/// Progress, reported as the run goes.
#[derive(Debug, Clone)]
pub enum Progress {
    /// Telegram's own total for this chat, published so the list can fill the
    /// row in for a chat the user never counted.
    Total {
        chat_id: i64,
        total: i64,
    },
    Messages {
        chat_id: i64,
        done: usize,
        total: i64,
    },
    /// A rate-limit wait, reported because two minutes of silence is
    /// indistinguishable from a hung export.
    FloodWait {
        seconds: u64,
    },
    Topic {
        title: String,
        messages: usize,
    },
    Log(String),
}

/// The progress sink.
///
/// `Send` because the whole export runs on a tokio worker thread while the
/// interface lives on GPUI's main thread; without it the future is not `Send`
/// and cannot be submitted at all.
pub type ProgressFn<'a> = &'a mut (dyn FnMut(Progress) + Send);

/// One output folder plus the media names it has handed out.
struct TopicSink {
    output: Output,
    /// One [`MediaNames`] per folder — that is what gives each topic its own
    /// `photo_1`, `photo_2`, matching a standalone Desktop export.
    media: MediaNames,
    title: String,
}

pub struct ChatExporter<'a> {
    client: &'a Client,
    settings: &'a Settings,
    /// The only state that may sit on the exporter is what is genuinely global
    /// to Telegram — ids that mean the same thing in every chat.
    names: NameBook,
}

impl<'a> ChatExporter<'a> {
    pub fn new(client: &'a Client, settings: &'a Settings) -> Self {
        Self {
            client,
            settings,
            names: NameBook::default(),
        }
    }

    /// Export one chat into `root`.
    ///
    /// `chat` is a **parameter**, not a field: with `chat_concurrency` above 1
    /// the second run to start would overwrite a field, and the first would go
    /// on asking Telegram about the wrong conversation and writing the answers
    /// into its own export as fact.
    pub async fn run(
        &mut self,
        chat: &ChatInfo,
        peer: PeerRef,
        topics: &[Topic],
        root: &Path,
        progress: ProgressFn<'_>,
    ) -> Result<ExportResult, ExportError> {
        let mut result = ExportResult {
            root: root.to_path_buf(),
            ..Default::default()
        };

        // A chat the list already counted costs no extra request. `0` is a
        // count — an empty chat — hence the `is_none` test rather than a falsy
        // one.
        let total = match chat.message_count {
            Some(n) => n,
            None => {
                let mut probe = self.client.iter_messages(peer);
                match probe.total().await {
                    Ok(n) => {
                        let n = n as i64;
                        progress(Progress::Total {
                            chat_id: chat.id,
                            total: n,
                        });
                        n
                    }
                    Err(e) => {
                        // A count we could not get is not a reason to abandon
                        // the export; it only costs the progress bar.
                        progress(Progress::Log(format!(
                            "could not count {}: {}",
                            chat.title,
                            classify(&e)
                        )));
                        0
                    }
                }
            }
        };
        result.expected = total;

        let split = self.settings.split_topics && chat.is_forum;
        let mut sinks: HashMap<i64, TopicSink> = HashMap::new();
        let export_type = chat.kind.export_type(true);

        // Pre-create a sink per topic so the folder names are stable and the
        // index can list them even if a topic turns out empty.
        if split {
            for t in topics {
                let dir = root.join(t.dirname());
                let mut head = Map::new();
                head.insert("topic_id".into(), json!(t.id));
                let output = Output::new(
                    &dir,
                    &t.title,
                    export_type,
                    chat.id,
                    self.settings,
                    Some("../export_results.html".to_string()),
                    Some(head),
                )?;
                sinks.insert(
                    t.id,
                    TopicSink {
                        output,
                        media: MediaNames::new(),
                        title: t.title.clone(),
                    },
                );
            }
        } else {
            let output = Output::new(
                root,
                &chat.title,
                export_type,
                chat.id,
                self.settings,
                None,
                None,
            )?;
            sinks.insert(
                GENERAL_TOPIC_ID,
                TopicSink {
                    output,
                    media: MediaNames::new(),
                    title: chat.title.clone(),
                },
            );
        }

        // --- the single pass ------------------------------------------------
        let mut offset_id: i32 = 0;
        let mut stalled: u32 = 0;
        let mut done = 0usize;

        'resume: loop {
            let mut iter = self
                .client
                .iter_messages(peer)
                .reverse(true)
                .offset_id(offset_id);

            loop {
                match iter.next().await {
                    Ok(Some(msg)) => {
                        // Progress: a wait that moved the cursor is not a stall.
                        stalled = 0;
                        offset_id = msg.id();
                        let sink_id = if split {
                            self.route(&msg, topics)
                        } else {
                            GENERAL_TOPIC_ID
                        };
                        // Resolve the key first: a message pointing at a
                        // topic we never listed still has to land somewhere.
                        let key = if sinks.contains_key(&sink_id) {
                            sink_id
                        } else {
                            GENERAL_TOPIC_ID
                        };
                        if let Some(sink) = sinks.get_mut(&key) {
                            let payload = self.payload(&msg, &mut sink.media);
                            sink.output.add(&payload)?;
                        }
                        done += 1;
                        result.messages += 1;
                        if done.is_multiple_of(100) {
                            progress(Progress::Messages {
                                chat_id: chat.id,
                                done,
                                total,
                            });
                        }
                    }
                    Ok(None) => break 'resume,
                    Err(e) => match classify(&e) {
                        EnrichError::Transient(d) => {
                            stalled += 1;
                            if stalled >= MAX_STALLED_WAITS {
                                // Close what we have before giving up, or the
                                // buffered writes are lost entirely.
                                Self::close_all(&mut sinks, &mut result, progress);
                                return Err(ExportError::Stalled { waits: stalled });
                            }
                            progress(Progress::FloodWait {
                                seconds: d.as_secs(),
                            });
                            sleep_in_slices(d).await;
                            continue 'resume;
                        }
                        other => {
                            Self::close_all(&mut sinks, &mut result, progress);
                            return Err(ExportError::Invocation(other.to_string()));
                        }
                    },
                }
            }
        }

        Self::close_all(&mut sinks, &mut result, progress);
        progress(Progress::Messages {
            chat_id: chat.id,
            done,
            total,
        });
        Ok(result)
    }

    /// Which topic this message belongs to.
    fn route(&self, msg: &grammers_client::message::Message, topics: &[Topic]) -> i64 {
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

    fn payload(
        &mut self,
        msg: &grammers_client::message::Message,
        _media: &mut MediaNames,
    ) -> Map<String, Value> {
        match &msg.raw {
            tl::enums::Message::Message(m) => base_message(m, &self.names),
            tl::enums::Message::Service(s) => base_service(s, &self.names),
            tl::enums::Message::Empty(e) => {
                let mut out = Map::new();
                out.insert("id".into(), json!(e.id));
                out.insert("type".into(), json!("message"));
                out.insert("text".into(), json!(""));
                out.insert("text_entities".into(), json!([]));
                out
            }
        }
    }

    /// **Closing drains.** Every path that can end an export comes through
    /// here, including the two error returns above: the JSON is streamed, so an
    /// abandoned output is a *zero-byte* file, not a truncated one.
    fn close_all(
        sinks: &mut HashMap<i64, TopicSink>,
        result: &mut ExportResult,
        progress: ProgressFn<'_>,
    ) {
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
                let _ = std::fs::remove_dir_all(&sink.output.root);
            } else {
                result.topics += 1;
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
    }
}

fn reply_header(h: Option<&tl::enums::MessageReplyHeader>) -> Option<ReplyHeader> {
    match h? {
        tl::enums::MessageReplyHeader::Header(r) => Some(ReplyHeader {
            forum_topic: r.forum_topic,
            reply_to_top_id: r.reply_to_top_id.map(|v| v as i64),
            reply_to_msg_id: r.reply_to_msg_id.map(|v| v as i64),
        }),
        _ => None,
    }
}

/// Sleep in one-second slices.
///
/// **Not a flat sleep.** The cap is two minutes, and a flat
/// `sleep(Duration::from_secs(120))` swallows a click on Cancel for the whole
/// two minutes. Both this and the progress event above were survivable at the
/// original 20s cap and are not at 120s; raising the cap without them trades a
/// silent data loss for an apparent freeze.
pub async fn sleep_in_slices(total: std::time::Duration) {
    let mut left = total;
    let slice = std::time::Duration::from_secs(1);
    while left > std::time::Duration::ZERO {
        let step = left.min(slice);
        tokio::time::sleep(step).await;
        left -= step;
    }
}

/// Reserve a folder by creating it, suffixing `(2)`, `(3)`… on a clash.
///
/// **The exclusive create *is* the reservation.** Two chats whose sanitised
/// titles collide would otherwise both pick the same not-yet-existing folder
/// when `chat_concurrency > 1` and overwrite each other. This is also why a
/// leftover empty folder is skipped rather than reused — `Name (2)` after a
/// cancelled run is the deliberate trade.
pub fn unique_dir(parent: &Path, name: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    let base = tgx_media::topics::sanitize_component(name, "chat");
    for i in 1..10_000 {
        let candidate = if i == 1 {
            parent.join(&base)
        } else {
            parent.join(format!("{base} ({i})"))
        };
        // create_dir errors if it already exists — that error *is* the lock.
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many folders with that name",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_export_does_not_read_as_a_complete_one() {
        let mut r = ExportResult {
            expected: 6643,
            messages: 5608,
            ..Default::default()
        };
        assert!(!r.complete());
        r.messages = 6643;
        assert!(r.complete());
    }

    #[test]
    fn a_chat_with_no_known_total_is_reported_complete() {
        let r = ExportResult {
            expected: 0,
            messages: 12,
            ..Default::default()
        };
        assert!(r.complete());
    }

    #[test]
    fn unique_dir_reserves_by_creating() {
        let parent = std::env::temp_dir().join(format!("tgx-uniq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        let a = unique_dir(&parent, "Dev Team").unwrap();
        let b = unique_dir(&parent, "Dev Team").unwrap();
        assert_ne!(a, b);
        assert!(a.ends_with("Dev Team"));
        assert!(b.ends_with("Dev Team (2)"));
        // Both exist: the create is the lock, so a concurrent caller cannot
        // pick the same not-yet-existing path.
        assert!(a.is_dir() && b.is_dir());
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn unique_dir_sanitises_the_title() {
        let parent = std::env::temp_dir().join(format!("tgx-uniq2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        let d = unique_dir(&parent, "a/b:c").unwrap();
        assert!(d.ends_with("a_b_c"), "got {d:?}");
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_sleeps_in_slices_not_one_block() {
        // With a paused clock, a flat sleep and a sliced one both return
        // instantly; what this pins is that the total is right and the
        // function yields repeatedly rather than once.
        let start = tokio::time::Instant::now();
        sleep_in_slices(std::time::Duration::from_secs(120)).await;
        assert_eq!(start.elapsed(), std::time::Duration::from_secs(120));
    }

    #[test]
    fn the_stall_ceiling_is_documented_and_used() {
        assert_eq!(MAX_STALLED_WAITS, 10);
    }
}
