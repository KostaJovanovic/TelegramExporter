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

use crate::cancel::Cancel;
use crate::client::ChatInfo;
use crate::config::Settings;
use crate::convert::{self, base_message, base_service, NameBook};
use crate::dialogs::Topic;
use crate::download::{self, PendingDownload};
use crate::enrich::{self, Enrichment};
use crate::error::{classify, EnrichError, ExportError};
use crate::output::Output;
use crate::plan;
use grammers_client::session::types::PeerRef;
use grammers_client::Client;
use grammers_tl_types as tl;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tgx_format::peer::PeerKey;
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
    pub media_downloaded: usize,
    pub media_failed: usize,
    pub bytes_downloaded: i64,
    pub members: usize,
    /// **A short member list says so.** A truncated roster is
    /// indistinguishable from a complete one, which makes it worse than none.
    pub members_complete: bool,
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
    /// How many messages this chat holds, published for **every** chat whose
    /// total the run knows — the one it looked up and the one the list had
    /// counted already alike. A row that never hears this has no denominator,
    /// so its progress bar stays empty for the whole export.
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
    /// One line of per-item detail: a message routed, a file fetched.
    ///
    /// **Never reaches the window's transcript.** That is a 2,000-line ring
    /// whose whole purpose is that the INCOMPLETE-export warning can still be
    /// scrolled to at the end of a long queue — and one chat of six thousand
    /// messages would push every such line out of it. This goes to `tgx.log`
    /// and to the CLI's stdout, both of which are files someone chose to read.
    ///
    /// Only emitted when [`detail_wanted`] is true, so the formatting cost is
    /// not paid on an ordinary run.
    Detail(String),
}

/// Is anyone listening for per-item detail?
///
/// Tied to the log level rather than a setting of our own: `RUST_LOG=debug`
/// already means "tell me everything", the file logger already honours it, and
/// a second switch would let the two disagree about what verbose means.
pub fn detail_wanted() -> bool {
    log::log_enabled!(log::Level::Debug)
}

/// A setting, spelled the way a log reader wants to read it.
fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

/// One message, as a single line of log.
///
/// Reads the finished payload rather than the TL object, so what it reports is
/// what was actually written — a line saying "photo" for a message whose photo
/// was dropped somewhere between the two would be worse than no line at all.
fn describe(m: &Map<String, Value>, topic: &str, queued: usize) -> String {
    let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("");
    let id = m.get("id").and_then(Value::as_i64).unwrap_or(0);

    let mut what: Vec<String> = Vec::new();
    if let Some(a) = m.get("action").and_then(Value::as_str) {
        what.push(format!("action={a}"));
    }
    if let Some(t) = m.get("media_type").and_then(Value::as_str) {
        what.push(t.to_string());
    }
    for key in ["photo", "file", "thumbnail", "stripped_thumbnail"] {
        let v = s(key);
        if v.is_empty() {
            continue;
        }
        // The skip placeholder is the interesting case, so name it as one
        // rather than printing the whole parenthesised sentence.
        what.push(if v.starts_with("(File") {
            format!("{key}=skipped")
        } else {
            format!("{key}={v}")
        });
    }
    if m.contains_key("poll") {
        what.push("poll".into());
    }
    if m.contains_key("location_information") {
        what.push("location".into());
    }
    if let Some(r) = m.get("reactions").and_then(Value::as_array) {
        what.push(format!("reactions={}", r.len()));
    }
    if let Some(r) = m.get("reply_to_message_id").and_then(Value::as_i64) {
        what.push(format!("reply_to={r}"));
    }
    if !s("forwarded_from").is_empty() {
        what.push(format!("fwd={}", s("forwarded_from")));
    }
    if queued > 0 {
        what.push(format!("+{queued} download(s)"));
    }
    // The text itself is never logged: `tgx.log` sits beside the executable
    // and an export is other people's conversation. Its length is enough to
    // tell an empty message from a lost one.
    let len = match m.get("text") {
        Some(Value::String(t)) => t.chars().count(),
        Some(Value::Array(a)) => a.len(),
        _ => 0,
    };
    let who = if s("from").is_empty() {
        s("actor")
    } else {
        s("from")
    };
    format!(
        "  #{id} [{topic}] {} {who} text:{len}{}{}",
        s("type"),
        if what.is_empty() { "" } else { " " },
        what.join(" ")
    )
}

/// Bytes, at the scale a human reads them.
pub(crate) fn human_bytes(n: i64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / MB)
    } else if n >= 1024 {
        format!("{:.1} kB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// The progress sink.
///
/// `Send` because the whole export runs on a tokio worker thread while the
/// interface lives on GPUI's main thread; without it the future is not `Send`
/// and cannot be submitted at all.
pub type ProgressFn<'a> = &'a mut (dyn FnMut(Progress) + Send);

/// What the two per-message requests recovered, if either fired.
///
/// Carried alongside the message rather than grafted onto it: `grammers`'
/// `Message` is not ours to mutate, and the Python original's in-place
/// `msg.reactions.recent_reactions = ...` is exactly the kind of edit that
/// makes it hard to tell afterwards what came off the wire and what we asked
/// for separately.
#[derive(Debug, Default)]
struct MessageExtras {
    /// Everyone who reacted, when the message's own sample was short.
    reactors: Option<Vec<tl::enums::MessagePeerReaction>>,
    /// Real tallies, when the poll came back `min` or all-zero.
    poll_results: Option<tl::enums::PollResults>,
}

/// One output folder plus the media names it has handed out.
struct TopicSink {
    output: Output,
    /// One [`MediaNames`] per folder — that is what gives each topic its own
    /// `photo_1`, `photo_2`, matching a standalone Desktop export.
    media: MediaNames,
    title: String,
    /// Jobs this folder is waiting on. Filenames are already written into the
    /// JSON and HTML; only the bytes are outstanding.
    jobs: Vec<PendingDownload>,
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
    ///
    /// **`cancel` is honoured, not obeyed instantly.** Every exit it takes goes
    /// through [`Self::close_all`] before returning [`ExportError::Cancelled`],
    /// because the JSON is streamed: an output dropped at the point of the
    /// click leaves a *zero-byte* file, which is worse than the partial export
    /// the user asked to keep.
    pub async fn run(
        &mut self,
        chat: &ChatInfo,
        peer: PeerRef,
        topics: &[Topic],
        root: &Path,
        progress: ProgressFn<'_>,
        cancel: &Cancel,
    ) -> Result<ExportResult, ExportError> {
        let started = std::time::Instant::now();
        let detail = detail_wanted();
        let mut result = ExportResult {
            root: root.to_path_buf(),
            ..Default::default()
        };

        // **The settings, written down before anything uses them.** Nearly
        // every "why is this export different from the last one" question is
        // answered by one of these, and a log that records the outcome without
        // the configuration that produced it cannot answer any of them.
        let s = self.settings;
        progress(Progress::Log(format!(
            "{} ({}, id {}) -> {}",
            chat.title,
            chat.kind.export_type(chat.public),
            chat.id,
            root.display()
        )));
        progress(Progress::Log(format!(
            "settings: media {}, size limit {}, kinds [{}], {} at a time, \
             pages of {}, link previews {}, roster {}",
            on_off(s.download_media),
            match s.size_limit_bytes() {
                Some(b) => format!("{} MB", b / (1024 * 1024)),
                None => "none".into(),
            },
            s.media_kinds.join(", "),
            s.download_concurrency,
            s.page_size,
            on_off(s.link_previews),
            on_off(s.member_roster),
        )));

        // A chat the list already counted costs no extra request. `0` is a
        // count — an empty chat — hence the `is_none` test rather than a falsy
        // one.
        let known = match chat.message_count {
            Some(n) => {
                progress(Progress::Log(format!(
                    "count: {n} messages, already known from the list — no request"
                )));
                Some(n)
            }
            None => {
                let mut probe = self.client.iter_messages(peer);
                match probe.total().await {
                    Ok(n) => {
                        progress(Progress::Log(format!(
                            "count: {n} messages, in {:.1}s",
                            started.elapsed().as_secs_f64()
                        )));
                        Some(n as i64)
                    }
                    Err(e) => {
                        // A count we could not get is not a reason to abandon
                        // the export; it only costs the progress bar.
                        progress(Progress::Log(format!(
                            "could not count {}: {}",
                            chat.title,
                            classify(&e)
                        )));
                        None
                    }
                }
            }
        };
        // **The request is what is conditional, not the report.** A chat whose
        // count the list already knew used to skip this too, so the queue row
        // never learnt its size and its progress bar contributed nothing for
        // the whole of the chat in flight — with the number sitting right there
        // in the parameter. A count we *failed* to get is still not published:
        // it is `None` here, not `0`, and sending `Total { total: 0 }` would
        // paint "0 messages" over a channel of ten thousand that rate-limited.
        if let Some(n) = known {
            progress(Progress::Total {
                chat_id: chat.id,
                total: n,
            });
        }
        let total = known.unwrap_or(0);
        result.expected = total;

        let split = self.settings.split_topics && chat.is_forum;
        if split {
            progress(Progress::Log(format!(
                "forum: {} topics, one folder each — {}",
                topics.len(),
                topics
                    .iter()
                    .map(|t| t.title.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        } else if chat.is_forum {
            progress(Progress::Log(
                "forum, but split_topics is off: everything into one folder".into(),
            ));
        } else {
            progress(Progress::Log("not a forum: one folder".into()));
        }
        let mut sinks: HashMap<i64, TopicSink> = HashMap::new();
        // `chat.public`, not `true`. Hardcoding it made the `false` arm of
        // `export_type` unreachable in production, so every export claimed
        // `public_supergroup` — including the invite-link-only groups this tool
        // is mostly used on.
        let export_type = chat.kind.export_type(chat.public);

        // **Before the sinks, because it goes in their headers.** One request
        // for the description, counts, pinned message and permanent invite,
        // and one more — admin-only, silent when refused — for the full invite
        // list. Both switches defaulted to on and were read by nothing, so an
        // export recorded nothing at all about the group it came from.
        let mut tally = Enrichment::default();
        let mut chat_head =
            enrich::fetch_chat_info(self.client, peer, self.settings, &mut tally, |seconds| {
                progress(Progress::FloodWait { seconds })
            })
            .await;
        let invites =
            enrich::fetch_invites(self.client, peer, self.settings, &mut tally, |seconds| {
                progress(Progress::FloodWait { seconds })
            })
            .await;
        if !invites.is_empty() {
            chat_head.insert("invite_links".into(), Value::Array(invites.clone()));
        }
        if !chat_head.is_empty() {
            progress(Progress::Log(format!(
                "chat details: {} ({} invite link(s))",
                chat_head
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
                invites.len()
            )));
        }

        // Pre-create a sink per topic so the folder names are stable and the
        // index can list them even if a topic turns out empty.
        if split {
            for t in topics {
                let dir = root.join(t.dirname());
                let mut head = Map::new();
                head.insert("topic_id".into(), json!(t.id));
                // Every topic folder is a standalone export, so each carries
                // the chat's details rather than one of them holding it.
                for (k, v) in &chat_head {
                    head.insert(k.clone(), v.clone());
                }
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
                        jobs: Vec::new(),
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
                (!chat_head.is_empty()).then(|| chat_head.clone()),
            )?;
            sinks.insert(
                GENERAL_TOPIC_ID,
                TopicSink {
                    output,
                    media: MediaNames::new(),
                    title: chat.title.clone(),
                    jobs: Vec::new(),
                },
            );
        }

        // --- enrichment, before the read ------------------------------------
        // **Before**, so the roster's names are available to the very first
        // message rather than the last.
        if self.settings.member_roster {
            let roster = enrich::fetch_participants(
                self.client,
                peer,
                self.settings,
                &mut tally,
                |seconds| progress(Progress::FloodWait { seconds }),
            )
            .await;
            result.members = roster.members.len();
            result.members_complete = roster.complete;
            progress(Progress::Log(format!(
                "roster: {} members, {} — {} extra request(s), {:.1}s in",
                roster.members.len(),
                if roster.complete {
                    "complete"
                } else {
                    "CAPPED"
                },
                tally.requests,
                started.elapsed().as_secs_f64()
            )));
            for m in &roster.members {
                if let Some(id) = m.get("id").and_then(Value::as_str) {
                    if let Some(name) = m.get("name").and_then(Value::as_str) {
                        self.names.names.insert(id.to_string(), name.to_string());
                        self.names.html.insert(id.to_string(), name.to_string());
                    }
                }
            }
            if !roster.members.is_empty() {
                let body =
                    serde_json::to_string_pretty(&roster.to_json()).unwrap_or_else(|_| "{}".into());
                std::fs::create_dir_all(root)?;
                std::fs::write(root.join("participants.json"), body)?;
            }
            if !roster.complete {
                progress(Progress::Log(
                    "the member list is incomplete — Telegram stopped serving it".into(),
                ));
            }
        }

        // --- the single pass ------------------------------------------------
        progress(Progress::Log(format!(
            "reading history oldest first{}",
            if total > 0 {
                format!(", {total} expected")
            } else {
                String::new()
            }
        )));
        let mut offset_id: i32 = 0;
        let mut stalled: u32 = 0;
        let mut done = 0usize;
        let stride = progress_stride(total);

        'resume: loop {
            // Checked here as well as per message so a cancel during a rate
            // limit is not held until the next message arrives — after a
            // two-minute wait the loop comes back to the top of `'resume`, not
            // to the top of a message.
            if cancel.is_cancelled() {
                Self::close_all(&mut sinks, root, chat, topics, split, &mut result, progress);
                return Err(ExportError::Cancelled);
            }

            if offset_id != 0 {
                progress(Progress::Log(format!(
                    "resuming from message {offset_id} ({done} written so far)"
                )));
            }
            let mut iter = self
                .client
                .iter_messages(peer)
                .reverse(true)
                .offset_id(offset_id);

            loop {
                // Per message, because a chat can hold tens of thousands and
                // anything coarser makes Stop take as long as a page fetch.
                // The partial export is kept, closed and complete as far as it
                // goes; `result.complete()` is what tells it apart from a whole
                // one.
                if cancel.is_cancelled() {
                    Self::close_all(&mut sinks, root, chat, topics, split, &mut result, progress);
                    return Err(ExportError::Cancelled);
                }
                match iter.next().await {
                    Ok(Some(msg)) => {
                        // Progress: a wait that moved the cursor is not a stall.
                        stalled = 0;
                        offset_id = msg.id();
                        // **Learn the sender before converting the message.**
                        // The roster was the only source of names, so anyone
                        // who posted and then left the group had no name at
                        // all: 206 fields across a live export came out as the
                        // empty string, with a perfectly correct `from_id`
                        // beside them. Telegram sends the sender's user object
                        // with the page that carries their message — grammers
                        // keeps it on `Message` — and we were discarding it.
                        // With the roster switched off this was every name in
                        // the export, not merely the ex-members'.
                        self.learn_peers(&msg);
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
                        // **Before the conversion, not after.** The extra
                        // requests replace what the message itself carries —
                        // the three-name reaction sample and a `min` poll —
                        // so a converter that had already run would have
                        // written the short version.
                        let extra = self.enrich_message(&msg, peer, &mut tally, progress).await;
                        if let Some(sink) = sinks.get_mut(&key) {
                            let before = sink.jobs.len();
                            let payload =
                                self.payload(&msg, &extra, &mut sink.media, &mut sink.jobs);
                            if detail {
                                progress(Progress::Detail(describe(
                                    &payload,
                                    &sink.title,
                                    sink.jobs.len() - before,
                                )));
                            }
                            sink.output.add(&payload)?;
                        }
                        done += 1;
                        result.messages += 1;
                        if done.is_multiple_of(stride) {
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
                                Self::close_all(
                                    &mut sinks,
                                    root,
                                    chat,
                                    topics,
                                    split,
                                    &mut result,
                                    progress,
                                );
                                return Err(ExportError::Stalled { waits: stalled });
                            }
                            progress(Progress::FloodWait {
                                seconds: d.as_secs(),
                            });
                            sleep_in_slices_until(d, cancel).await;
                            continue 'resume;
                        }
                        other => {
                            Self::close_all(
                                &mut sinks,
                                root,
                                chat,
                                topics,
                                split,
                                &mut result,
                                progress,
                            );
                            return Err(ExportError::Invocation(other.to_string()));
                        }
                    },
                }
            }
        }

        // Messages queued to send later are in no history, so they get their
        // own file beside the export rather than being mixed into it.
        let queued =
            enrich::fetch_scheduled(self.client, peer, self.settings, &mut tally, |seconds| {
                progress(Progress::FloodWait { seconds })
            })
            .await;
        if !queued.is_empty() {
            let rows: Vec<Value> = queued
                .iter()
                .map(|m| match m {
                    tl::enums::Message::Message(m) => Value::Object(base_message(m, &self.names)),
                    tl::enums::Message::Service(sm) => Value::Object(base_service(sm, &self.names)),
                    tl::enums::Message::Empty(e) => json!({ "id": e.id }),
                })
                .collect();
            let body = json!({ "count": rows.len(), "messages": rows });
            match serde_json::to_string_pretty(&body)
                .map_err(std::io::Error::other)
                .and_then(|b| std::fs::write(root.join("scheduled.json"), b))
            {
                Ok(()) => progress(Progress::Log(format!(
                    "scheduled: {} message(s) -> scheduled.json",
                    rows.len()
                ))),
                Err(e) => progress(Progress::Log(format!("scheduled.json: {e}"))),
            }
        }

        result.extra_requests += tally.requests;
        result.enrich_deferred += tally.deferred;
        progress(Progress::Log(format!(
            "read {done} messages in {:.1}s{}",
            started.elapsed().as_secs_f64(),
            if total > 0 && (done as i64) < total {
                format!(" — {} SHORT of the {total} expected", total - done as i64)
            } else {
                String::new()
            }
        )));

        // --- the media pass -------------------------------------------------
        // After the read loop, not during it: the read is bounded by Telegram's
        // paging and the downloads are bounded by the pool, so overlapping them
        // buys little and costs a much harder cancel path.
        //
        // Cancelled before the first byte is fetched, the text export is
        // already whole — so this returns the JSON and HTML rather than
        // throwing them away for the sake of the media nobody waited for.
        if cancel.is_cancelled() {
            Self::close_all(&mut sinks, root, chat, topics, split, &mut result, progress);
            return Err(ExportError::Cancelled);
        }
        // Keyed rather than `values_mut`: the cancel path below has to hand the
        // whole map to `close_all`, which it cannot do while an iterator holds
        // it borrowed.
        let media_order: Vec<i64> = {
            let mut ids: Vec<i64> = sinks.keys().copied().collect();
            ids.sort();
            ids
        };
        for id in media_order {
            // The folder's name and root are lifted out of the map so the
            // borrow ends here: the cancel check below needs the map back.
            let Some((dir, title, jobs)) = sinks.get_mut(&id).map(|sink| {
                (
                    sink.output.root.clone(),
                    sink.title.clone(),
                    std::mem::take(&mut sink.jobs),
                )
            }) else {
                continue;
            };
            if jobs.is_empty() {
                continue;
            }
            // Between batches, which is as fine-grained as a cancel gets here:
            // `run_all` puts the whole folder onto the pool at once, and
            // interrupting it mid-flight would leave part-written files that
            // the HTML already links to.
            if cancel.is_cancelled() {
                Self::close_all(&mut sinks, root, chat, topics, split, &mut result, progress);
                return Err(ExportError::Cancelled);
            }
            let queued = jobs.len();
            let expected: i64 = jobs.iter().map(|j| j.job.size).sum();
            progress(Progress::Log(format!(
                "{title}: fetching {queued} files ({}) — {} at a time",
                human_bytes(expected),
                self.settings.download_concurrency
            )));
            if detail {
                for j in &jobs {
                    progress(Progress::Detail(format!(
                        "  queue #{} {} ({})",
                        j.job.message_id,
                        j.job.dest,
                        human_bytes(j.job.size)
                    )));
                }
            }
            let batch_started = std::time::Instant::now();
            // Drained while the pool runs, so the lines arrive during the
            // batch rather than in a burst after it.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let pool = download::run_all(
                self.client,
                &dir,
                jobs,
                self.settings.download_concurrency,
                Some(tx),
            );
            tokio::pin!(pool);
            let tally = loop {
                tokio::select! {
                    t = &mut pool => break t,
                    Some(p) = rx.recv() => progress(p),
                }
            };
            while let Ok(p) = rx.try_recv() {
                progress(p);
            }
            let secs = batch_started.elapsed().as_secs_f64();
            progress(Progress::Log(format!(
                "{title}: {} saved, {} failed, {} in {:.1}s ({}/s)",
                tally.downloaded,
                tally.failed,
                human_bytes(tally.bytes),
                secs,
                human_bytes((tally.bytes as f64 / secs.max(0.001)) as i64)
            )));
            for m in &tally.missing {
                progress(Progress::Log(format!("  not saved: {m}")));
            }
            result.media_downloaded += tally.downloaded;
            result.media_failed += tally.failed;
            result.bytes_downloaded += tally.bytes;
            // A dangling reference is worse than a stated gap.
            if let Err(e) = download::write_missing(&dir, &tally.missing) {
                progress(Progress::Log(format!("missing_media.txt: {e}")));
            }
        }

        Self::close_all(&mut sinks, root, chat, topics, split, &mut result, progress);
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

    /// Add whatever peers this message brought with it to the name book.
    ///
    /// grammers keeps the whole peer set a response carried, but only the
    /// sender and the chat are reachable from outside the crate — which is
    /// enough: the names that were missing belonged to people who had posted,
    /// so they arrive as the sender of their own messages.
    ///
    /// The chat is learned too, because a migration notice's actor *is* the
    /// chat and had no other source.
    fn learn_peers(&mut self, msg: &grammers_client::message::Message) {
        for peer in [msg.sender(), msg.peer()].into_iter().flatten() {
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
    async fn enrich_message(
        &mut self,
        msg: &grammers_client::message::Message,
        peer: PeerRef,
        tally: &mut Enrichment,
        progress: ProgressFn<'_>,
    ) -> MessageExtras {
        let mut extras = MessageExtras::default();
        let tl::enums::Message::Message(m) = &msg.raw else {
            return extras;
        };
        let id = m.id;

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

    fn payload(
        &mut self,
        msg: &grammers_client::message::Message,
        extras: &MessageExtras,
        names: &mut MediaNames,
        jobs: &mut Vec<PendingDownload>,
    ) -> Map<String, Value> {
        match &msg.raw {
            tl::enums::Message::Message(m) => {
                let mut out = base_message(m, &self.names);
                let mut preview_src: Option<String> = None;
                // Filenames are decided **before** bytes are fetched, so the
                // JSON and HTML stream out now and the pool catches up later.
                if let Some(media) = &m.media {
                    if let Some(facts) = plan::classify(media, self.settings.link_previews) {
                        let stamp = media_stamp(m.date);
                        let (fields, job) =
                            plan::plan(&facts, m.id as i64, &stamp, names, self.settings);
                        for (k, v) in fields {
                            out.insert(k, v);
                        }
                        // Read before the job is moved: the preview's name was
                        // claimed by the planner, and deriving it again here
                        // would miss any `(1)` collision suffix and point the
                        // `<img>` at a file the pool never writes.
                        preview_src = job.as_ref().and_then(|j| j.preview_dest.clone());
                        if let Some(job) = job {
                            let handle = if job.inline_bytes.is_some() {
                                None
                            } else {
                                msg.media()
                            };
                            jobs.push(PendingDownload { job, media: handle });
                        }
                        out = tgx_format::order::ordered(&out);
                    }
                    // Media that is not a *file*. `classify` only answers "what
                    // would we download", so a poll and a location fell through
                    // it and the message reached the JSON as bare text — all
                    // seven polls and all three locations in the reference.
                    if let tl::enums::MessageMedia::Poll(p) = media {
                        out.insert(
                            "poll".into(),
                            convert::poll_of(p, extras.poll_results.as_ref()),
                        );
                        out = tgx_format::order::ordered(&out);
                    }
                    if let Some((place, period)) = convert::location_of(media) {
                        out.insert("location_information".into(), place);
                        if let Some(seconds) = period {
                            out.insert("live_location_period_seconds".into(), json!(seconds));
                        }
                        out = tgx_format::order::ordered(&out);
                    }
                }
                if let Some(r) = &m.reactions {
                    let tl::enums::MessageReactions::Reactions(r) = r;
                    if let Some(v) =
                        convert::reactions_of(r, extras.reactors.as_deref(), &self.names)
                    {
                        out.insert("reactions".into(), v);
                        out = tgx_format::order::ordered(&out);
                    }
                }
                // Last, because it reads the finished map: the media paths and
                // sizes the plan decided are what the preview points at.
                // `Output::close` strips `_p` before the JSON is written, so
                // this reaches the HTML writer and nothing else.
                if let Some(p) = convert::presentation(m, &out, &self.names, preview_src.as_deref())
                {
                    out.insert("_p".into(), Value::Object(p));
                }
                out
            }
            tl::enums::Message::Service(s) => {
                let mut out = base_service(s, &self.names);
                // A service message can be reacted to like any other.
                if let Some(r) = &s.reactions {
                    let tl::enums::MessageReactions::Reactions(r) = r;
                    if let Some(v) = convert::reactions_of(r, None, &self.names) {
                        out.insert("reactions".into(), v);
                        out = tgx_format::order::ordered(&out);
                    }
                }
                out
            }
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

    fn write_index(
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

/// How often a read pass reports the messages it has written.
///
/// **One report per percent, and never less often than every ten messages.** A
/// flat every-hundredth told a 60-message chat's row nothing at all: it showed
/// `0` for the entire export and jumped straight to its final count. Reporting
/// every message instead puts one channel send per message on the bridge — 6,643
/// of them for one chat, each one repainting the window — to move a bar by a
/// fraction of a pixel.
///
/// A percent is the finest step a progress bar can actually show, so that is
/// the step. The floor of ten is what keeps a short chat moving, and is the
/// whole rule when `total` is `0` — a count we never got, where there is no bar
/// to fill and only a counter to advance.
fn progress_stride(total: i64) -> usize {
    (total / 100).max(10) as usize
}

/// `DD-MM-YYYY_HH-MM-SS`, the stamp Desktop puts in a synthesised filename.
fn media_stamp(ts: i32) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(ts as i64, 0).single() {
        Some(dt) => dt.format("%d-%m-%Y_%H-%M-%S").to_string(),
        None => String::new(),
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

/// Sleep in one-second slices, giving up early if `cancel` fires.
///
/// **Not a flat sleep.** The cap is two minutes, and a flat
/// `sleep(Duration::from_secs(120))` swallows a click on Cancel for the whole
/// two minutes. Both this and the progress event above were survivable at the
/// original 20s cap and are not at 120s; raising the cap without them trades a
/// silent data loss for an apparent freeze.
///
/// The slicing is what makes the check possible at all: the flag is read
/// between slices, so Stop costs at most one second here instead of the whole
/// wait. That is the entire reason the loop is not a single `sleep`.
pub async fn sleep_in_slices_until(total: std::time::Duration, cancel: &Cancel) {
    let mut left = total;
    let slice = std::time::Duration::from_secs(1);
    while left > std::time::Duration::ZERO {
        if cancel.is_cancelled() {
            return;
        }
        let step = left.min(slice);
        tokio::time::sleep(step).await;
        left -= step;
    }
}

/// Sleep in one-second slices with nothing able to interrupt it.
///
/// A thin wrapper for the callers that hold no signal — see
/// [`sleep_in_slices_until`] for why the slicing exists.
pub async fn sleep_in_slices(total: std::time::Duration) {
    sleep_in_slices_until(total, &Cancel::new()).await;
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

    /// How many `Progress::Messages` a chat of `total` messages would send.
    fn reports_for(total: i64) -> usize {
        let stride = progress_stride(total);
        (1..=total as usize)
            .filter(|d| d.is_multiple_of(stride))
            .count()
    }

    #[test]
    fn a_short_chat_reports_more_than_its_final_count() {
        // The defect: at a flat every-hundredth, a 60-message chat sent nothing
        // during the read and its row sat at 0 until the export finished.
        assert_eq!(progress_stride(60), 10);
        assert_eq!(reports_for(60), 6);
    }

    #[test]
    fn a_long_chat_reports_once_per_percent_and_not_once_per_message() {
        assert_eq!(progress_stride(6643), 66);
        assert_eq!(reports_for(6643), 100);
    }

    #[test]
    fn an_uncounted_chat_still_advances_its_counter() {
        // `0` here is "we never got a count", so there is no bar to fill; the
        // floor is the whole rule and the counter still moves.
        assert_eq!(progress_stride(0), 10);
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

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_token_stops_a_long_wait() {
        // The defect this prevents: Stop clicked during a two-minute rate limit
        // used to do nothing until the wait ran out. One slice is the ceiling.
        let signal = Cancel::new();
        signal.cancel();
        let start = tokio::time::Instant::now();
        sleep_in_slices_until(std::time::Duration::from_secs(120), &signal).await;
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "waited {:?} of a cancelled 120s sleep",
            start.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_uncancelled_wait_still_runs_its_full_length() {
        let signal = Cancel::new();
        let start = tokio::time::Instant::now();
        sleep_in_slices_until(std::time::Duration::from_secs(120), &signal).await;
        assert_eq!(start.elapsed(), std::time::Duration::from_secs(120));
        assert!(!signal.is_cancelled());
    }

    #[test]
    fn a_fresh_signal_is_not_cancelled_and_reset_clears_one_that_is() {
        let signal = Cancel::new();
        assert!(!signal.is_cancelled());
        signal.cancel();
        assert!(signal.is_cancelled());
        signal.reset();
        assert!(!signal.is_cancelled());
    }

    #[test]
    fn the_stall_ceiling_is_documented_and_used() {
        assert_eq!(MAX_STALLED_WAITS, 10);
    }

    #[test]
    fn a_detail_line_never_carries_the_message_text() {
        // `tgx.log` sits beside the executable and an export is other people's
        // conversation. The line reports the text's *length* so an empty
        // message can be told from a lost one, and never the text.
        let secret = "meet me at the usual place";
        let m = serde_json::json!({
            "id": 104,
            "type": "message",
            "from": "Nada",
            "text": secret,
            "media_type": "sticker",
            "file": "stickers/sticker.webp",
        });
        let line = describe(m.as_object().unwrap(), "ćaskanje", 1);
        assert!(
            !line.contains(secret),
            "the text leaked into the log: {line}"
        );
        assert!(line.contains("#104"));
        assert!(line.contains("[ćaskanje]"));
        assert!(line.contains("Nada"));
        assert!(line.contains("sticker"));
        assert!(line.contains("stickers/sticker.webp"));
        assert!(line.contains("text:26"), "the length is the point: {line}");
        assert!(line.contains("+1 download"));
        assert_eq!(line.lines().count(), 1, "one message, one line");
    }

    #[test]
    fn a_segmented_text_is_counted_not_quoted() {
        // A formatted message arrives as an array of segments, and one of them
        // is the text. Counting the array is what keeps it out of the log.
        let m = serde_json::json!({
            "id": 1,
            "type": "message",
            "actor": "UA KOLAB",
            "text": ["private words ", {"type": "mention", "text": "@someone"}],
        });
        let line = describe(m.as_object().unwrap(), "t", 0);
        assert!(!line.contains("private words"));
        assert!(!line.contains("@someone"));
        assert!(line.contains("text:2"));
        // Falls back to the actor when there is no `from`.
        assert!(line.contains("UA KOLAB"));
    }

    #[test]
    fn a_skipped_file_is_named_as_skipped_rather_than_quoted() {
        let m = serde_json::json!({
            "id": 7,
            "type": "message",
            "text": "",
            "file": crate::plan::TOO_LARGE,
        });
        let line = describe(m.as_object().unwrap(), "t", 0);
        assert!(line.contains("file=skipped"), "{line}");
        assert!(!line.contains("exceeds maximum size"), "{line}");
    }

    #[test]
    fn bytes_are_reported_at_a_scale_a_reader_can_use() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 kB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
