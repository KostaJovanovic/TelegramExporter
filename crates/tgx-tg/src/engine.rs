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
// The trait, for `peer_ref` on the session store. `SqliteSession` implements
// it; the method is not inherent.
use grammers_client::session::storages::SqliteSession;
use grammers_client::session::types::PeerId;
use grammers_client::session::Session as _;
use grammers_client::Client;
use grammers_tl_types as tl;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    /// The folder this chat was written into — `None` for a database-only run,
    /// which writes no folder at all. Everything that offers to open it has to
    /// cope with there being nothing to open.
    pub root: Option<PathBuf>,
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
    /// Download **jobs** that did not produce their file.
    pub media_failed: usize,
    /// Lines in `missing_media.txt` — always ≥ `media_failed`, because one
    /// failed job takes every path it had already promised down with it (the
    /// file, Telegram's thumbnail, the inline preview).
    ///
    /// It exists because the warning that points the user at that file was
    /// reporting `media_failed`: the run said "21 files could not be fetched —
    /// see missing_media.txt" over a file that listed 42.
    pub media_missing: usize,
    pub bytes_downloaded: i64,
    pub members: usize,
    /// **A short member list says so.** A truncated roster is
    /// indistinguishable from a complete one, which makes it worse than none.
    pub members_complete: bool,
    /// Whether this chat has an invite link in its header.
    ///
    /// A flag, not the link. `export_results.html` says the group has one and
    /// prints `[INVITE LINK]` in its place: the link is a **working credential
    /// to a private group** — anyone holding the page can join — and the index
    /// is the file most likely to be opened, mailed or archived somewhere less
    /// careful than `result.json`. Desktop writes the link there in full and
    /// that is where it stays.
    ///
    /// Carried on the result rather than passed down because the header is
    /// built before the read pass and the index is written after it, on nine
    /// different paths including every error return.
    pub has_invite_link: bool,

    // --- the database, all zero when it is off ------------------------------
    /// Messages this run put into the database for the first time.
    pub db_new: usize,
    /// Stored messages Telegram returned differently — an edit, a reaction, a
    /// counter. The payload each one replaced is kept in `versions`.
    pub db_changed: usize,
    /// Stored messages Telegram no longer returns, marked and kept.
    pub db_deleted: usize,
    /// Files whose bytes went into the database this run.
    pub db_blobs: usize,
    /// What the database holds for this chat now.
    pub db_archived: usize,
    /// **This run read only the newest N messages on purpose.**
    ///
    /// A database-only sync, which has no business being measured against the
    /// chat's total. See [`Self::complete`] — this is the flag that keeps the
    /// two kinds of short run apart.
    pub windowed: bool,
    /// The read loop ended because Telegram ran out of messages, rather than
    /// because it was cancelled, stalled or failed.
    pub reached_end: bool,
}

impl ExportResult {
    /// Did the run get everything Telegram said was there?
    ///
    /// **A windowed run is judged by `reached_end`, and nothing else is.** It
    /// is tempting to write this as `reached_end || <the old rule>` — the walk
    /// finished, so surely it is complete — and that would quietly disable the
    /// INCOMPLETE warning for *every* export. `Ok(None)` is the ordinary way
    /// the read loop ends, including on the run that read 5,609 of the 6,600
    /// messages Telegram counted, which is the exact failure that warning was
    /// added for. So the two rules are alternatives, not alternatives-with-an-or.
    pub fn complete(&self) -> bool {
        if self.windowed {
            return self.reached_end;
        }
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

/// Does this run read a trailing window instead of the whole history?
///
/// **Database mode is the incremental one; that is the whole point of it.** A
/// Classic export re-reads the chat from the beginning every time, because it
/// is producing a complete standalone folder. If Database mode did that too it
/// would cost exactly as much and save nothing.
///
/// A first sync reads everything (`stored == 0`): there is nothing to window
/// against, and windowing anyway would store the newest few hundred messages of
/// a chat and call the archive done.
pub fn reads_a_window(database_mode: bool, reread_all: bool, stored: i64) -> bool {
    database_mode && !reread_all && stored > 0
}

/// Is this file worth fetching in Database mode?
///
/// **The question is whether the archive has the bytes, not whether the message
/// is new.** Gating on `Merge::Inserted` looks right and is not: it means a
/// file is only ever fetched on the run that first saw its message, so anything
/// missed the first time — media switched off, over the size limit, a download
/// that failed — is missed for good, and no later sync goes back for it. The
/// database is supposed to end up holding the files.
///
/// `already_stored` cannot answer it alone: nothing reaches `blobs` until the
/// pool has run, so two messages in one run carrying the same file would both
/// pass and the file would be downloaded twice. `already_queued` is what this
/// run has already promised.
fn worth_fetching(file_id: i64, already_queued: bool, already_stored: bool) -> bool {
    file_id != 0 && !already_queued && !already_stored
}

/// A setting, spelled the way a log reader wants to read it.
fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

/// A topic's own metadata, for the head of its `result.json`.
///
/// Everything here arrived with the topic listing, so writing it costs
/// nothing; only `topic_id` was being kept.
fn topic_head(t: &Topic) -> Map<String, Value> {
    let mut head = Map::new();
    head.insert("topic_id".into(), json!(t.id));
    if !t.created_by.is_empty() {
        head.insert("topic_created_by".into(), json!(t.created_by));
    }
    if let Some((date, _)) = tgx_format::date_pair(t.created_date) {
        head.insert("topic_created".into(), json!(date));
    }
    // A string, as Desktop's own extension fields carry ids: a 64-bit
    // document id does not survive a JSON reader that parses numbers as
    // doubles, and this one is only ever compared, never arithmetic.
    if let Some(e) = t.icon_emoji_id {
        head.insert("topic_icon_emoji_id".into(), json!(e.to_string()));
    }
    if t.icon_color != 0 {
        head.insert("topic_icon_color".into(), json!(t.icon_color));
    }
    for (flag, key) in [
        (t.closed, "topic_closed"),
        (t.hidden, "topic_hidden"),
        (t.pinned, "topic_pinned"),
    ] {
        if flag {
            head.insert(key.into(), json!(true));
        }
    }
    head
}

/// One message, as a single line of log.
///
/// Reads the finished payload rather than the TL object, so what it reports is
/// what was actually written — a line saying "photo" for a message whose photo
/// was dropped somewhere between the two would be worse than no line at all.
fn describe(
    m: &Map<String, Value>,
    topic: &str,
    queued: usize,
    archived: Option<tgx_archive::Merge>,
) -> String {
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
    // What the database made of it, first, because on a re-read that is the
    // only thing that differs between one line and the next — and reading a
    // sync's log means looking for the handful that were not `same`.
    let verdict = match archived {
        Some(tgx_archive::Merge::Inserted) => "new ",
        Some(tgx_archive::Merge::Changed { .. }) => "changed ",
        Some(tgx_archive::Merge::Unchanged) => "same ",
        None => "",
    };
    format!(
        "  #{id} {verdict}[{topic}] {} {who} text:{len}{}{}",
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
/// `Message` is not ours to mutate, and an in-place edit of
/// `reactions.recent_reactions` is exactly the kind that makes it hard to tell
/// afterwards what came off the wire and what we asked for separately.
#[derive(Debug, Default)]
struct MessageExtras {
    /// Everyone who reacted, when the message's own sample was short.
    reactors: Option<Vec<tl::enums::MessagePeerReaction>>,
    /// Real tallies, when the poll came back `min` or all-zero.
    poll_results: Option<tl::enums::PollResults>,
}

/// The forward origin of a message that nothing has named, if there is one.
///
/// Split out of [`ChatExporter::learn_forward_origin`] so the decision is
/// testable: everything around it needs a `Client` and a session store, and this
/// is the part that decides whether a request is made at all.
fn unnamed_forward_origin<'m>(
    m: &'m tl::types::Message,
    names: &NameBook,
) -> Option<&'m tl::enums::Peer> {
    let Some(tl::enums::MessageFwdHeader::Header(fwd)) = &m.fwd_from else {
        return None;
    };
    let peer = fwd.from_id.as_ref()?;
    // Telegram sends `from_name` when the source is a peer we may not know, and
    // `convert.rs` already prefers it over an empty lookup. Nothing to recover
    // when it is there.
    if fwd.from_name.as_deref().is_some_and(|n| !n.is_empty()) {
        return None;
    }
    if !names
        .get(&crate::convert::peer_key(peer).to_string())
        .is_empty()
    {
        return None;
    }
    Some(peer)
}

/// A `tl::enums::Peer` as the session store keys it.
///
/// Deliberately the checked constructors: `PeerId::user`/`chat`/`channel` return
/// `None` for an id outside Telegram's valid range, and the `_unchecked` twins
/// `debug_assert!` — which is a panic in a debug build, on a value that arrives
/// off the wire. An out-of-range id simply goes unresolved, which is the same
/// outcome as a peer the store never cached.
fn session_peer_id(peer: &tl::enums::Peer) -> Option<PeerId> {
    match peer {
        tl::enums::Peer::User(p) => PeerId::user(p.user_id),
        tl::enums::Peer::Chat(p) => PeerId::chat(p.chat_id),
        tl::enums::Peer::Channel(p) => PeerId::channel(p.channel_id),
    }
}

/// What the archive needs to know about a download once it has landed.
///
/// Taken off the job before the pool consumes it. A `DownloadJob` carries the
/// stripped thumbnail's bytes, so cloning the jobs to keep them around would
/// copy those for nothing.
struct Ingest {
    dest: String,
    thumb_dest: Option<String>,
    file_id: i64,
    kind: &'static str,
    mime_type: String,
    role: &'static str,
    message_id: i64,
    /// Bytes that came inside the message rather than off the wire. Never
    /// archived — see `plan.rs`.
    inline: bool,
    /// The name to record beside the bytes.
    ///
    /// The basename of `dest`, which for a document *is* the name Telegram
    /// sent, and for a photo is the `photo_N@stamp.jpg` Desktop synthesises.
    /// Taken from the path rather than carried separately so it cannot come to
    /// disagree with the file it names.
    file_name: Option<String>,
}

impl Ingest {
    fn of(pending: &PendingDownload) -> Self {
        let job = &pending.job;
        Self {
            dest: job.dest.clone(),
            thumb_dest: job.thumb_dest.clone(),
            file_id: job.file_id,
            kind: job.kind,
            mime_type: job.mime_type.clone(),
            role: job.role,
            message_id: job.message_id,
            inline: job.inline_bytes.is_some(),
            file_name: job
                .dest
                .rsplit('/')
                .next()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
        }
    }
}

/// One output folder plus the media names it has handed out.
struct TopicSink {
    /// `None` in a database-only run, which writes no `result.json` and no
    /// pages. Everything else about the sink still happens: the payload is
    /// built through this topic's own [`MediaNames`], so the metadata keys and
    /// the file names are identical to what a folder export would have
    /// written.
    output: Option<Output>,
    /// Where this topic's media lands.
    ///
    /// **A field rather than `output.root`**, because in a database-only run
    /// there is no `Output` to ask, and because the two are genuinely different
    /// directories then: the pool downloads into a scratch folder that is
    /// deleted once the bytes are in the database. It is **per topic** either
    /// way — `MediaNames` is per folder, so two topics both hand out
    /// `photos/photo_1.jpg`, and one scratch directory for the whole chat would
    /// have the second topic's download overwrite the first's and put the wrong
    /// bytes in the archive.
    dir: PathBuf,
    /// One [`MediaNames`] per folder — that is what gives each topic its own
    /// `photo_1`, `photo_2`, matching a standalone Desktop export.
    media: MediaNames,
    title: String,
    /// Jobs this folder is waiting on. Filenames are already written into the
    /// JSON and HTML; only the bytes are outstanding.
    jobs: Vec<PendingDownload>,
    /// Messages routed here, counted by the run rather than by a file.
    ///
    /// `Output::count` is the number a folder run reports, and it does not
    /// exist when nothing is being written — so a database-only run would
    /// otherwise report every topic as empty and prune them all.
    stored: usize,
}

pub struct ChatExporter<'a> {
    client: &'a Client,
    settings: &'a Settings,
    /// The only state that may sit on the exporter is what is genuinely global
    /// to Telegram — ids that mean the same thing in every chat.
    names: NameBook,
    /// The session store, for peers grammers cached but will not hand over.
    ///
    /// `Message::peers` is `pub(crate)`, so the only peers reachable from a
    /// message are its sender and its chat — but the session has been caching
    /// *every* peer every response carried since the account signed in, forward
    /// origins included. It holds no names, only ids and access hashes, which is
    /// exactly the half a `PeerRef` is missing. See
    /// [`Self::learn_forward_origin`].
    session: Arc<SqliteSession>,
    /// Peers already looked up through the session store, successfully or not.
    ///
    /// One request per *person*, not per message: the 94 empty forward names in
    /// the last live run belonged to 13 people. Shared by the forward-origin and
    /// service-message paths, which ask the same question about the same store.
    peers_tried: std::collections::HashSet<String>,
    /// **One store for the whole queue**, like the one Telegram connection.
    ///
    /// Opened here rather than by the callers, so neither `tgx-app` nor the CLI
    /// has to name `tgx-archive` — the window may depend only on `tgx-ui` and
    /// `tgx-tg`, and `layering.rs` fails the build if that changes.
    store: Option<tgx_archive::Store>,
}

impl<'a> ChatExporter<'a> {
    /// Async only because opening the database is.
    ///
    /// An unopenable database ends the queue the way an unwritable output
    /// folder does: every later chat would fail the same way, so failing once
    /// and loudly beats failing per chat.
    pub async fn new(
        client: &'a Client,
        settings: &'a Settings,
        session: Arc<SqliteSession>,
    ) -> Result<Self, ExportError> {
        let store = if settings.export_db {
            let path = Path::new(&settings.output_dir).join(tgx_archive::FILE_NAME);
            Some(tgx_archive::Store::open(&path).await?)
        } else {
            None
        };
        Ok(Self {
            client,
            settings,
            names: NameBook {
                own_names: settings.own_names,
                ..NameBook::default()
            },
            session,
            peers_tried: std::collections::HashSet::new(),
            store,
        })
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
        root: Option<&Path>,
        progress: ProgressFn<'_>,
        cancel: &Cancel,
    ) -> Result<ExportResult, ExportError> {
        let started = std::time::Instant::now();
        let detail = detail_wanted();
        let mut result = ExportResult {
            root: root.map(Path::to_path_buf),
            ..Default::default()
        };
        // **Classic or Database — the two are a choice, not a set of ticks.**
        //
        // Classic writes Desktop's folders and re-reads the chat from the
        // beginning every time, because it is producing a complete standalone
        // export. Database writes one accumulating file, holds the media inside
        // it, and syncs. Letting them run together meant paying for a full
        // history read on every run and calling the result incremental, which
        // is the worst of both and was the first thing to go.
        //
        // `root` is `None` in Database mode, so `files` and `db` are always
        // opposites; both names are kept because the code below reads better
        // saying which of the two facts it depends on.
        let db = self.settings.export_db;
        let files = root.is_some();
        debug_assert_eq!(db, !files, "Classic and Database are exclusive");
        // **One timestamp for the whole run.** Every `first_seen`, `last_seen`
        // and `deleted_seen` this chat writes is the same second, so a query
        // asking "what did that export do" gets one answer rather than a smear
        // across however long the chat took.
        let seen_at = chrono::Utc::now().timestamp();

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
            match root {
                Some(r) => r.display().to_string(),
                None => self
                    .store
                    .as_ref()
                    .map(|s| s.path().display().to_string())
                    .unwrap_or_else(|| "nowhere".into()),
            }
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
        //
        // **A windowed run publishes no total**, because the chat's size is
        // not that run's denominator: a sync reading the newest 500 of 6,643
        // would fill 8% of a bar and stop, which reads as a failed export. The
        // row shows a bare counter instead, exactly as an uncounted chat does.
        // `windowed` is not known until the archive is consulted a few lines
        // below, so the decision is deferred to there.
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
        result.has_invite_link = !invites.is_empty() || chat_head.contains_key("invite_link");
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

        // --- enrichment, before the read ------------------------------------
        // **Before the sinks**, not merely before the read. The roster's names
        // have to reach the very first message, and what it found — how many,
        // and whether that was all of them — belongs in every header this
        // export is about to write.
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
            self.names.aliased.0 += roster.aliased.0;
            self.names.aliased.1 += roster.aliased.1;
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
            // **Everything the roster learned, not the display name twice.**
            // This used to reach into `names` and `html` by hand and write the
            // same string into both, which left the userpic letters and the
            // name colour unset for anyone the message stream never carried —
            // and the HTML then derived initials by splitting the display name,
            // the one rule Desktop provably does not use. See `Roster::book`.
            self.names.absorb(&roster.book);
            if !roster.members.is_empty() {
                if let Some(root) = root {
                    let body = serde_json::to_string_pretty(&roster.to_json())
                        .unwrap_or_else(|_| "{}".into());
                    std::fs::create_dir_all(root)?;
                    std::fs::write(root.join("participants.json"), body)?;
                }
                // **The database keeps ex-members.** `participants.json` is a
                // snapshot of who is in the chat now and is rewritten whole
                // every export; the table is who has ever been seen in it,
                // which is the question an archive gets asked. Written whether
                // or not a folder was, because the roster was fetched either
                // way — gating it on `root` would have made "tick only
                // Database" quietly drop the member list.
                if let Some(store) = &self.store {
                    for member in &roster.members {
                        // `"user1234"`, the same shape a message's `from_id`
                        // carries, so the two tables join.
                        let Some(key) = member.get("id").and_then(Value::as_str) else {
                            continue;
                        };
                        store
                            .upsert_participant(chat.id, key, member, seen_at)
                            .await?;
                    }
                }
            }
            if !roster.complete {
                progress(Progress::Log(
                    "the member list is incomplete — Telegram stopped serving it".into(),
                ));
            }
            // In the header as well as the file, because a reader with only
            // `result.json` in front of them otherwise cannot tell a roster
            // that was capped from one that was whole.
            if self.settings.chat_metadata {
                chat_head.insert("members_listed".into(), json!(roster.members.len()));
                chat_head.insert("members_complete".into(), json!(roster.complete));
            }
        }

        // Pre-create a sink per topic so the folder names are stable and the
        // index can list them even if a topic turns out empty.
        // Where this chat's media lands when there is no export folder: a
        // scratch tree the pool downloads into and the ingest deletes. **Per
        // topic**, one level down, because `MediaNames` is per folder and two
        // topics both hand out `photos/photo_1.jpg`.
        let scratch = Path::new(&self.settings.output_dir)
            .join(".tgx-scratch")
            .join(chat.id.to_string());

        if split {
            for t in topics {
                let dir = match root {
                    Some(root) => root.join(t.dirname()),
                    None => scratch.join(t.id.to_string()),
                };
                // **All of it, because none of it costs a request.** The
                // forum listing hands 22 fields over with the title and we
                // were keeping one. Who opened a topic and when is part of
                // what the topic is, and the folder name records neither.
                let mut head = topic_head(t);
                // Every topic folder is a standalone export, so each carries
                // the chat's details rather than one of them holding it.
                for (k, v) in &chat_head {
                    head.insert(k.clone(), v.clone());
                }
                // The same `head` either way: in a database-only run it is
                // what the topic row records, so a folder export made later
                // carries the header this run already knew.
                if let Some(store) = &self.store {
                    store
                        .upsert_topic(chat.id, t.id, &t.title, &head, seen_at)
                        .await?;
                }
                let output = match root {
                    Some(_) => Some(Output::new(
                        &dir,
                        &t.title,
                        export_type,
                        chat.id,
                        self.settings,
                        Some("../export_results.html".to_string()),
                        Some(head),
                    )?),
                    None => None,
                };
                sinks.insert(
                    t.id,
                    TopicSink {
                        output,
                        dir,
                        media: MediaNames::new(),
                        title: t.title.clone(),
                        jobs: Vec::new(),
                        stored: 0,
                    },
                );
            }
        } else {
            if let Some(store) = &self.store {
                store
                    .upsert_topic(chat.id, GENERAL_TOPIC_ID, &chat.title, &chat_head, seen_at)
                    .await?;
            }
            let dir = match root {
                Some(root) => root.to_path_buf(),
                None => scratch.join(GENERAL_TOPIC_ID.to_string()),
            };
            let output = match root {
                Some(root) => Some(Output::new(
                    root,
                    &chat.title,
                    export_type,
                    chat.id,
                    self.settings,
                    None,
                    (!chat_head.is_empty()).then(|| chat_head.clone()),
                )?),
                None => None,
            };
            sinks.insert(
                GENERAL_TOPIC_ID,
                TopicSink {
                    output,
                    dir,
                    media: MediaNames::new(),
                    title: chat.title.clone(),
                    jobs: Vec::new(),
                    stored: 0,
                },
            );
        }

        // --- how much of the history this run reads --------------------------
        //
        // Decided by the formats, not by a mode switch. With a folder being
        // written the run walks the whole chat anyway, so the database is
        // filled from that pass and every stored message it did not see can be
        // marked deleted. With **only** the database on there is nothing else
        // to write, so the run is a sync: what is new, plus a trailing window
        // for edits and deletions.
        let mut run_id: Option<i64> = None;
        let mut start: i64 = 0;
        if let Some(store) = &self.store {
            let (stored_all, stored_live) = store.message_count(chat.id).await?;
            result.db_archived = stored_all as usize;
            // A first sync has nothing to window against, so it reads
            // everything — otherwise a fresh archive would store the newest 500
            // messages of a chat and call itself done.
            result.windowed = reads_a_window(db, self.settings.reread_all, stored_all);
            if result.windowed {
                start = store
                    .window_start(chat.id, self.settings.reread_window)
                    .await?;
            }
            progress(Progress::Log(format!(
                "database: {} — {} archived for this chat{}",
                store.path().display(),
                stored_all,
                if stored_all == stored_live {
                    String::new()
                } else {
                    format!(" ({} deleted on Telegram, kept)", stored_all - stored_live)
                }
            )));
            if result.windowed {
                progress(Progress::Log(format!(
                    "database: syncing from #{start} (the last {} archived) for edits and \
                     deletions — older deletions need \"Re-read the whole history\"",
                    self.settings.reread_window
                )));
            } else if db {
                progress(Progress::Log(
                    "database: reading the whole history, so every deletion is noticed".into(),
                ));
            }
            let record = serde_json::to_string(&self.settings.without_credentials())
                .unwrap_or_else(|_| "{}".into());
            store
                .upsert_chat(chat.id, &chat.title, export_type, seen_at)
                .await?;
            run_id = Some(
                store
                    .start_run(
                        chat.id,
                        if result.windowed { "window" } else { "full" },
                        root,
                        &record,
                        seen_at,
                    )
                    .await?,
            );
            store.begin_batch().await?;
        }
        // See the comment where `known` is computed: a windowed run has no
        // denominator to publish.
        if let (Some(n), false) = (known, result.windowed) {
            progress(Progress::Total {
                chat_id: chat.id,
                total: n,
            });
        }
        // What the read loop reports against. A windowed run has no total, so
        // its rows carry a counter and no bar.
        let reported_total = if result.windowed { 0 } else { total };

        // --- the single pass ------------------------------------------------
        progress(Progress::Log(format!(
            "reading history oldest first{}",
            if reported_total > 0 {
                format!(", {reported_total} expected")
            } else {
                String::new()
            }
        )));
        // Telegram's message ids are 32-bit; the archive stores them as
        // `INTEGER`, so the window start comes back wider than the cursor.
        let mut offset_id: i32 = start as i32;
        // Every id this run saw, for the deletion sweep at the end.
        let mut seen: HashSet<i64> = HashSet::new();
        // File ids already queued for download this run. `has_blob` cannot
        // answer this: nothing is written to `blobs` until the pool has run, so
        // two messages carrying the same file would both pass the check and the
        // file would be fetched twice.
        let mut queued_files: HashSet<(i64, &'static str)> = HashSet::new();
        let mut stalled: u32 = 0;
        let mut done = 0usize;
        let stride = progress_stride(reported_total);

        'resume: loop {
            // Checked here as well as per message so a cancel during a rate
            // limit is not held until the next message arrives — after a
            // two-minute wait the loop comes back to the top of `'resume`, not
            // to the top of a message.
            if cancel.is_cancelled() {
                Self::close_all(
                    &mut sinks,
                    root,
                    chat,
                    topics,
                    &self.names,
                    split,
                    &mut result,
                    self.store.as_ref(),
                    progress,
                )
                .await;
                return Err(ExportError::Cancelled);
            }

            // **`start`, not zero.** A windowed run begins with a non-zero
            // cursor by design, and testing against zero made its very first
            // pass announce "resuming from message 6120 (0 written so far)" —
            // a resume that had not happened, immediately after the line that
            // already said where the sync was starting.
            if offset_id as i64 != start {
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
                    Self::close_all(
                        &mut sinks,
                        root,
                        chat,
                        topics,
                        &self.names,
                        split,
                        &mut result,
                        self.store.as_ref(),
                        progress,
                    )
                    .await;
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
                        let mut db_verdict = None;
                        if let Some(sink) = sinks.get_mut(&key) {
                            let before = sink.jobs.len();
                            // **The payload is built the same way in a
                            // database-only run**, through this topic's own
                            // `MediaNames`, so the metadata keys and the file
                            // paths are what a folder export would have
                            // written. Only the writing is skipped.
                            let payload =
                                self.payload(&msg, &extra, &mut sink.media, &mut sink.jobs);
                            sink.stored += 1;

                            // --- the database -------------------------------
                            if let Some(store) = &self.store {
                                let id = msg.id() as i64;
                                seen.insert(id);
                                // `_p` is presentation for the HTML writer and
                                // nothing renders from the database, so it is
                                // stripped here for the same reason
                                // `Output::add` strips it for `result.json`.
                                let body: Map<String, Value> = payload
                                    .iter()
                                    .filter(|(k, _)| k.as_str() != "_p")
                                    .map(|(k, v)| (k.clone(), v.clone()))
                                    .collect();
                                let merged = match store.get(chat.id, id).await? {
                                    Some(old) => tgx_archive::merge_volatile(&old.payload, &body),
                                    None => body,
                                };
                                // `None` when the run could not resolve
                                // topics: a forum exported unsplit calls
                                // everything General, and that must not
                                // overwrite what a split run learned.
                                let topic = split.then_some(key);
                                let what =
                                    store.merge(chat.id, id, topic, &merged, seen_at).await?;
                                match what {
                                    tgx_archive::Merge::Inserted => result.db_new += 1,
                                    tgx_archive::Merge::Changed { .. } => result.db_changed += 1,
                                    tgx_archive::Merge::Unchanged => {}
                                }
                                db_verdict = Some(what);
                                // A batch per couple of hundred messages: a
                                // crash costs a batch rather than a chat, and
                                // six thousand un-batched inserts is six
                                // thousand fsyncs.
                                if done.is_multiple_of(200) {
                                    store.commit_batch().await?;
                                    store.begin_batch().await?;
                                }
                            }

                            if detail {
                                progress(Progress::Detail(describe(
                                    &payload,
                                    &sink.title,
                                    sink.jobs.len() - before,
                                    db_verdict,
                                )));
                            }
                            // Through `close_all`, not `?`. This was the one
                            // error return in `run` that did not drain: `Drop`
                            // keeps the JSON valid, but the index
                            // (`export_results.html`) is never written and the
                            // empty-folder pruning never runs — which
                            // reintroduces exactly the dead back-link a
                            // previous audit closed. Every other exit here goes
                            // through `close_all`, and the doc on this function
                            // says every one does.
                            if let Some(out) = sink.output.as_mut() {
                                if let Err(e) = out.add(&payload) {
                                    Self::close_all(
                                        &mut sinks,
                                        root,
                                        chat,
                                        topics,
                                        &self.names,
                                        split,
                                        &mut result,
                                        self.store.as_ref(),
                                        progress,
                                    )
                                    .await;
                                    return Err(e.into());
                                }
                            }

                            // **In a database-only run the only reason to fetch
                            // a file is that its bytes are not archived yet.**
                            // A folder run keeps every job, because the folder
                            // has to be complete whatever the database already
                            // holds.
                            //
                            // `has_blob` alone is not enough: nothing reaches
                            // `blobs` until the pool has run, so two messages
                            // carrying the same file would both pass it and the
                            // file would be downloaded twice. `queued_files` is
                            // what this run has already promised to fetch.
                            if db {
                                if let Some(store) = &self.store {
                                    let fresh: Vec<PendingDownload> = sink.jobs.split_off(before);
                                    for pending in fresh {
                                        let id = pending.job.file_id;
                                        let kind = pending.job.kind;
                                        let wanted = worth_fetching(
                                            id,
                                            queued_files.contains(&(id, kind)),
                                            id != 0 && store.has_blob(id, kind).await?,
                                        );
                                        if wanted {
                                            queued_files.insert((id, kind));
                                            sink.jobs.push(pending);
                                        }
                                    }
                                }
                            }
                        }
                        done += 1;
                        result.messages += 1;
                        if done.is_multiple_of(stride) {
                            progress(Progress::Messages {
                                chat_id: chat.id,
                                done,
                                total: reported_total,
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
                                    &self.names,
                                    split,
                                    &mut result,
                                    self.store.as_ref(),
                                    progress,
                                )
                                .await;
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
                                &self.names,
                                split,
                                &mut result,
                                self.store.as_ref(),
                                progress,
                            )
                            .await;
                            return Err(ExportError::Invocation(other.to_string()));
                        }
                    },
                }
            }
        }

        // --- what Telegram no longer has --------------------------------------
        //
        // **Only here**, which is the one place reached by the loop leaving via
        // `Ok(None)`. Every other exit — cancelled, stalled, a wire error —
        // returns from inside the loop, and none of them can tell "Telegram no
        // longer has it" from "we never got that far". Marking on one of those
        // would write a deletion date across most of a chat.
        result.reached_end = true;
        if let Some(store) = &self.store {
            result.db_deleted = store
                .mark_unseen_deleted(chat.id, start, &seen, seen_at)
                .await?;
            let (archived, _) = store.message_count(chat.id).await?;
            result.db_archived = archived as usize;
            progress(Progress::Log(format!(
                "database: +{} new, {} changed, {} deleted on Telegram (kept) — {} archived",
                result.db_new, result.db_changed, result.db_deleted, result.db_archived
            )));
        }

        // Messages queued to send later are in no history, so they get their
        // own file beside the export rather than being mixed into it.
        //
        // **Not fetched at all without a folder to put them in.** The database
        // has no table for them: they are not messages that happened, they are
        // messages that might, and merging them into `messages` would have the
        // next sync mark every one of them deleted the moment it was sent for
        // real. Asking Telegram for a list nothing can record is a request
        // spent on nothing. Recorded in ROADMAP as still open.
        let queued = if files {
            enrich::fetch_scheduled(self.client, peer, self.settings, &mut tally, |seconds| {
                progress(Progress::FloodWait { seconds })
            })
            .await
        } else {
            Vec::new()
        };
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
                .and_then(|b| {
                    // Unreachable without a folder: `queued` is empty then.
                    let root = root.unwrap_or(Path::new("."));
                    std::fs::write(root.join("scheduled.json"), b)
                }) {
                Ok(()) => progress(Progress::Log(format!(
                    "scheduled: {} message(s) -> scheduled.json",
                    rows.len()
                ))),
                Err(e) => progress(Progress::Log(format!("scheduled.json: {e}"))),
            }
        }

        // **Reported even when it is zero.** On an account with no contacts in
        // the chat this option legitimately changes nothing, which looks
        // exactly like a switch that does nothing — the failure this codebase
        // has had five of.
        if self.settings.own_names {
            let (replaced, kept) = self.names.aliased;
            progress(Progress::Log(format!(
                "own names: {replaced} contact(s) written as their @handle,                  {kept} kept under your name for want of one"
            )));
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
            Self::close_all(
                &mut sinks,
                root,
                chat,
                topics,
                &self.names,
                split,
                &mut result,
                self.store.as_ref(),
                progress,
            )
            .await;
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
                    sink.dir.clone(),
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
                Self::close_all(
                    &mut sinks,
                    root,
                    chat,
                    topics,
                    &self.names,
                    split,
                    &mut result,
                    self.store.as_ref(),
                    progress,
                )
                .await;
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
            // What the archive needs about each job, taken before the pool
            // consumes them. Cloning the jobs themselves would copy every
            // stripped thumbnail's bytes for nothing.
            let ingest: Vec<Ingest> = if self.store.is_some() {
                jobs.iter().map(Ingest::of).collect()
            } else {
                Vec::new()
            };

            let batch_started = std::time::Instant::now();
            // Drained while the pool runs, so the lines arrive during the
            // batch rather than in a burst after it.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let pool = download::run_all(
                self.client,
                &dir,
                jobs,
                // The pool needs the peer because a file reference can only be
                // replaced by re-reading the message that carried it, and
                // every reference in this batch was captured before the read
                // above finished. See `download::refreshed_media`.
                download::Refresh {
                    peer,
                    link_previews: self.settings.link_previews,
                },
                self.settings.download_concurrency,
                Some(tx),
                cancel,
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
            // **Both numbers count paths.** This said `tally.failed`, which
            // counts *jobs*, directly above a list of the paths that were lost
            // and directly below `fetching {queued} files`, which counts jobs
            // again — so a batch reported "1 failed" and then named two files,
            // and "467 saved, 672 failed" out of 829. Same defect as the
            // chat-level warning that once said 21 over a file listing 42; it
            // was fixed there and not here.
            progress(Progress::Log(format!(
                "{title}: {} saved, {} missing, {} in {:.1}s ({}/s)",
                tally.downloaded,
                tally.missing.len(),
                human_bytes(tally.bytes),
                secs,
                human_bytes((tally.bytes as f64 / secs.max(0.001)) as i64)
            )));
            // Capped, because the transcript is a 2,000-line ring whose whole
            // purpose is that the INCOMPLETE warning can still be scrolled to.
            // A Stop during a 1,781-file folder now records every un-run job as
            // a stated gap, and listing all of them here would flush the very
            // thing the ring exists to keep. The file has the full list.
            const SHOWN: usize = 20;
            for m in tally.missing.iter().take(SHOWN) {
                // With the reason, not just the path. Without it a permanent,
                // hundred-percent-reproducible refusal reads exactly like a
                // flaky network — which is how 21 link-preview photos failed
                // every run of 2026-08-27 without any artifact saying why.
                progress(Progress::Log(format!(
                    "  not saved: {} — {}",
                    m.path, m.reason
                )));
            }
            if tally.missing.len() > SHOWN {
                progress(Progress::Log(format!(
                    "  … and {} more — all of them in missing_media.txt",
                    tally.missing.len() - SHOWN
                )));
            }
            result.media_downloaded += tally.downloaded;
            result.media_failed += tally.failed;
            result.media_missing += tally.missing.len();
            result.bytes_downloaded += tally.bytes;
            // A dangling reference is worse than a stated gap. Only where there
            // is a folder to leave the note in — a scratch tree is deleted a
            // few lines below, and the transcript already named every file that
            // did not arrive.
            if files {
                if let Err(e) = download::write_missing(&dir, &tally.missing) {
                    progress(Progress::Log(format!("missing_media.txt: {e}")));
                }
            }

            // --- the bytes into the database ---------------------------------
            if let Some(store) = &self.store {
                for item in &ingest {
                    // Nothing to store, or nothing that arrived. A job the pool
                    // could not fetch is in `missing`, and reading its `dest`
                    // would either fail or — worse — pick up a stale file from
                    // an earlier export into the same folder.
                    if item.file_id == 0
                        || item.inline
                        || tally.missing.iter().any(|m| m.path == item.dest)
                    {
                        continue;
                    }
                    // Already archived, by an earlier run or an earlier message
                    // in this one: `already_saved` jobs share their bytes with a
                    // file the folder wrote once, and re-reading them off disk
                    // to hand libsql something it would ignore is pure I/O.
                    if !store.has_blob(item.file_id, item.kind).await? {
                        let bytes = match std::fs::read(dir.join(&item.dest)) {
                            Ok(b) => b,
                            Err(e) => {
                                progress(Progress::Log(format!(
                                    "database: {} could not be read back to archive it: {e}",
                                    item.dest
                                )));
                                continue;
                            }
                        };
                        match store
                            .put_blob(
                                item.file_id,
                                item.kind,
                                &item.mime_type,
                                item.file_name.as_deref(),
                                &bytes,
                                seen_at,
                            )
                            .await
                        {
                            Ok(()) => result.db_blobs += 1,
                            // Not fatal: the folder still has it, and the
                            // `media` row below still records that the message
                            // had an attachment. An export must not fail on one
                            // enormous video.
                            Err(tgx_archive::Error::TooLarge { size, .. }) => {
                                progress(Progress::Log(format!(
                                    "database: {} is {} — too large to archive, {}",
                                    item.dest,
                                    human_bytes(size),
                                    if files {
                                        "left in the export folder"
                                    } else {
                                        "not saved anywhere"
                                    }
                                )));
                            }
                            Err(e) => return Err(e.into()),
                        }
                        if let Some(thumb) = &item.thumb_dest {
                            if let Ok(b) = std::fs::read(dir.join(thumb)) {
                                store.put_thumb(item.file_id, item.kind, &b).await?;
                            }
                        }
                    }
                    store
                        .link_media(
                            chat.id,
                            item.message_id,
                            item.role,
                            item.file_id,
                            item.kind,
                            &item.dest,
                        )
                        .await?;
                }
            }
        }

        // **The scratch tree goes, whatever happened.** It only exists in a
        // database-only run, the bytes it held are in the archive by now, and
        // leaving it behind puts a folder of somebody's photos next to their
        // exports under a name that looks like a mistake.
        if !files {
            let _ = std::fs::remove_dir_all(&scratch);
        }

        Self::close_all(
            &mut sinks,
            root,
            chat,
            topics,
            &self.names,
            split,
            &mut result,
            self.store.as_ref(),
            progress,
        )
        .await;
        // After `close_all`, which commits: the run row is the record that the
        // batch above it landed, so writing it inside the transaction it
        // describes would make it disappear with everything else on a crash.
        if let (Some(store), Some(run_id)) = (&self.store, run_id) {
            store
                .finish_run(
                    run_id,
                    result.db_new,
                    result.db_changed,
                    result.db_deleted,
                    result.db_blobs,
                    result.reached_end,
                    seen_at,
                )
                .await?;
        }
        progress(Progress::Messages {
            chat_id: chat.id,
            done,
            total: reported_total,
        });
        Ok(result)
    }
}

mod finish;
mod payload;
mod peers;

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

    /// The same minimal fixture `convert`'s tests use; its module is private.
    fn blank_message() -> tl::types::Message {
        tl::types::Message {
            out: false,
            mentioned: false,
            media_unread: false,
            silent: false,
            post: false,
            from_scheduled: false,
            legacy: false,
            edit_hide: false,
            pinned: false,
            noforwards: false,
            invert_media: false,
            offline: false,
            video_processing_pending: false,
            paid_suggested_post_stars: false,
            paid_suggested_post_ton: false,
            id: 1,
            from_id: None,
            from_boosts_applied: None,
            from_rank: None,
            peer_id: tl::enums::Peer::User(tl::types::PeerUser { user_id: 1 }),
            saved_peer_id: None,
            fwd_from: None,
            via_bot_id: None,
            via_business_bot_id: None,
            guestchat_via_from: None,
            reply_to: None,
            date: 1_766_071_072,
            message: String::new(),
            media: None,
            reply_markup: None,
            entities: None,
            views: None,
            forwards: None,
            replies: None,
            edit_date: None,
            post_author: None,
            grouped_id: None,
            reactions: None,
            restriction_reason: None,
            ttl_period: None,
            quick_reply_shortcut_id: None,
            effect: None,
            factcheck: None,
            report_delivery_until_date: None,
            paid_message_stars: None,
            suggested_post: None,
            schedule_repeat_period: None,
            summary_from_language: None,
            rich_message: None,
        }
    }

    /// A message forwarded from `from_id`, optionally carrying a `from_name`.
    fn forwarded(from_id: Option<tl::enums::Peer>, from_name: Option<&str>) -> tl::types::Message {
        let fwd = tl::types::MessageFwdHeader {
            imported: false,
            saved_out: false,
            from_id,
            from_name: from_name.map(str::to_string),
            date: 0,
            channel_post: None,
            post_author: None,
            saved_from_peer: None,
            saved_from_msg_id: None,
            saved_from_id: None,
            saved_from_name: None,
            saved_date: None,
            psa_type: None,
        };
        let mut m = blank_message();
        m.fwd_from = Some(tl::enums::MessageFwdHeader::Header(fwd));
        m
    }

    fn user_peer(id: i64) -> tl::enums::Peer {
        tl::enums::Peer::User(tl::types::PeerUser { user_id: id })
    }

    #[test]
    fn a_forward_origin_nobody_named_is_worth_one_request() {
        // 94 `forwarded_from` fields came out empty in the last live run, across
        // 13 people, every one with a correct id and every one named in
        // Desktop's export. `learn_peers` reaches only a message's sender and
        // its chat — and someone you forward *from* need never have posted in
        // the chat you are exporting.
        let names = NameBook::default();
        let m = forwarded(Some(user_peer(42)), None);
        assert!(unnamed_forward_origin(&m, &names).is_some());
    }

    #[test]
    fn a_forward_telegram_already_named_costs_nothing() {
        // Telegram sends `from_name` when the source is a peer we may not know,
        // and `convert.rs` prefers it over an empty lookup — so there is nothing
        // to recover and no reason to spend a request.
        let names = NameBook::default();
        let m = forwarded(Some(user_peer(42)), Some("Ivana"));
        assert!(unnamed_forward_origin(&m, &names).is_none());
        // An empty `from_name` is not a name.
        let m = forwarded(Some(user_peer(42)), Some(""));
        assert!(unnamed_forward_origin(&m, &names).is_some());
    }

    #[test]
    fn a_forward_origin_already_in_the_book_costs_nothing() {
        let mut names = NameBook::default();
        // Written straight into the book rather than through `learn`: what is
        // under test is the lookup, not how the name got there.
        names.names.insert(
            crate::convert::peer_key(&user_peer(42)).to_string(),
            "Nađa".into(),
        );
        let m = forwarded(Some(user_peer(42)), None);
        assert!(unnamed_forward_origin(&m, &names).is_none());
    }

    #[test]
    fn a_message_that_is_not_a_forward_is_left_alone() {
        let names = NameBook::default();
        assert!(unnamed_forward_origin(&blank_message(), &names).is_none());
        // A hidden forward has a name and no peer; there is no id to resolve.
        let m = forwarded(None, Some("Someone"));
        assert!(unnamed_forward_origin(&m, &names).is_none());
    }

    #[test]
    fn a_peer_id_out_of_range_is_none_rather_than_a_panic() {
        // The checked constructors, deliberately: the `_unchecked` twins
        // `debug_assert!`, which is a panic in a debug build, on a value that
        // arrives off the wire. An out-of-range id simply goes unresolved.
        assert!(session_peer_id(&user_peer(42)).is_some());
        assert!(session_peer_id(&user_peer(-1)).is_none());
        assert!(session_peer_id(&user_peer(0)).is_none());
        assert!(
            session_peer_id(&tl::enums::Peer::Channel(tl::types::PeerChannel {
                channel_id: 3_586_682_625,
            }))
            .is_some()
        );
    }

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

    /// The trap the database output walks straight into.
    ///
    /// `reached_end` is true for essentially every successful export — `Ok(None)`
    /// is the ordinary way the read loop finishes — so writing `complete()` as
    /// `reached_end || <the count rule>` would report the 5,608-of-6,643 run
    /// above as complete and take the INCOMPLETE warning out of the product
    /// without failing a single test. The two rules are alternatives.
    #[test]
    fn reaching_the_end_does_not_excuse_a_short_folder_export() {
        let r = ExportResult {
            expected: 6643,
            messages: 5608,
            reached_end: true,
            windowed: false,
            ..Default::default()
        };
        assert!(
            !r.complete(),
            "a folder export short of Telegram's own count is INCOMPLETE, \
             whether or not the walk ended tidily"
        );
    }

    #[test]
    fn a_windowed_sync_is_judged_by_the_walk_and_not_by_the_count() {
        // It read the newest 500 of 6,643 on purpose. Measured against the
        // chat's size that is an 8% export; measured against what it set out to
        // do it is a complete one.
        let mut r = ExportResult {
            expected: 6643,
            messages: 500,
            windowed: true,
            reached_end: true,
            ..Default::default()
        };
        assert!(r.complete());
        // But a sync that was cancelled or stalled is still short.
        r.reached_end = false;
        assert!(!r.complete());
    }

    #[test]
    fn only_a_database_run_reads_a_window() {
        // Database mode with an archive to window against: the whole point.
        assert!(reads_a_window(true, false, 6643));
        // Classic re-reads the chat from the beginning every time — it is
        // producing a complete standalone folder.
        assert!(!reads_a_window(false, false, 6643));
        // The first sync has nothing to window against, so it reads everything.
        assert!(!reads_a_window(true, false, 0));
        // "Re-read the whole history" overrides the window.
        assert!(!reads_a_window(true, true, 6643));
    }

    #[test]
    fn a_file_missing_from_the_archive_is_fetched_however_old_its_message_is() {
        // The question is whether the *archive* has the bytes, never whether
        // the message is new. Gating on "the message was just inserted" means a
        // file skipped the first time — media off, over the size limit, a
        // download that failed — is skipped for good.
        assert!(worth_fetching(77, false, false));

        // Already stored by an earlier run: nothing to do.
        assert!(!worth_fetching(77, false, true));
        // Already promised by an earlier message in *this* run. `has_blob`
        // cannot see this — nothing reaches `blobs` until the pool has run — so
        // without the queue set the file would be fetched twice.
        assert!(!worth_fetching(77, true, false));
        // A stripped thumbnail has no Telegram file id and is never archived.
        assert!(!worth_fetching(0, false, false));
    }

    /// `tgx-archive` may not depend on this crate, so `merge_volatile` matches
    /// Desktop's "I did not save this" placeholders on their shared prefix
    /// rather than on these constants. That is the only thing letting a
    /// re-sync with a raised size limit replace the placeholder with the file
    /// it finally fetched — so if the strings ever stop starting this way, the
    /// backfill goes silently back to leaving the payload wrong.
    #[test]
    fn the_skip_placeholders_keep_the_shape_the_archive_matches_on() {
        for placeholder in [plan::NOT_INCLUDED, plan::TOO_LARGE] {
            assert!(
                placeholder.starts_with("(File "),
                "{placeholder:?} no longer matches tgx-archive's SKIPPED_PREFIX"
            );
        }
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
        let line = describe(m.as_object().unwrap(), "ćaskanje", 1, None);
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
        let line = describe(m.as_object().unwrap(), "t", 0, None);
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
        let line = describe(m.as_object().unwrap(), "t", 0, None);
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
