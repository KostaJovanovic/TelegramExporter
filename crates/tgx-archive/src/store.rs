//! The SQLite store: chats, topics, messages, versions, blobs, participants,
//! runs.
//!
//! Everything here takes values — a serialised message map, a file id, a
//! timestamp — and nothing here knows what Telegram is. That is what lets
//! `tgx-parity`'s archive leg merge a real Desktop export into a store twice
//! and read Desktop's own bytes back out with no connection.
//!
//! **Every method is `async` because libsql's API is**, not because anything
//! here is concurrent. There is one connection and one caller.

use libsql::params;
use libsql::params::IntoParams;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The one file, for the whole export root.
pub const FILE_NAME: &str = "telegram.sqlite";

/// Bumped only when the shape below changes in a way another build would
/// misread. A build refuses a file it does not know rather than guessing.
pub const SCHEMA_VERSION: i64 = 1;

/// The largest file that goes *into* the database.
///
/// **Not SQLite's ceiling**, which is 1 GB. The bytes are read whole into a
/// `Vec<u8>` and handed to libsql as another owned value, so the peak is twice
/// the file — and the machine on the other end of that is a laptop exporting
/// somebody's holiday videos. The folder export's own `size_limit_mb` is the
/// real limit and defaults to 20 MB; this exists for the run that sets it to
/// unlimited, so one 3 GB file cannot take the export down with it. Over this
/// the `media` row is still written, so the attachment is recorded rather than
/// silently forgotten, and the bytes stay in the folder.
pub const MAX_BLOB: i64 = 256 * 1024 * 1024;

/// How long to wait for another process holding the write lock.
///
/// The window and the `tgx` CLI can both be pointed at one export root, and
/// this project has made the two-writers mistake before — a second
/// `SqliteSession` opened on a file the first was still writing. With no busy
/// timeout the loser of that race does not wait; it fails instantly with
/// `SQLITE_BUSY`, in the middle of somebody's export.
const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// Keys a later run may overwrite on a message it has already stored.
///
/// **Short on purpose.** Everything else — who sent it, what it replied to,
/// which files hang off it — is a fact about a message that happened, and a
/// re-read that rewrote those would let a converter change history.
///
/// Four of these (`views`, `forwards`, `replies_count`, `pinned`) are keys the
/// converter does not write yet. They are listed anyway because `order.rs`
/// already ranks them, and the day one starts being emitted is not the day
/// anybody will remember this list exists.
pub const VOLATILE: &[&str] = &[
    "edited",
    "edited_unixtime",
    "text",
    "text_entities",
    "reactions",
    "views",
    "forwards",
    "replies_count",
    "pinned",
    "poll",
];

/// Media keys, and the prefix of the strings Desktop writes when it saved no
/// file: *"(File not included…"*, *"(File exceeds maximum size…"*.
///
/// A stored placeholder is not history to be preserved — it is a record that
/// the run declined to fetch something, and the moment a later run does fetch
/// it, the placeholder is simply wrong. So these keys are replaceable **only
/// when what is stored is a placeholder**; a real path is never overwritten,
/// which is what keeps a media replacement on an edited message from rewriting
/// what the message originally carried.
///
/// Without this, raising the size limit and re-syncing puts the bytes in
/// `blobs` and leaves the payload still saying the file was too large.
/// `tgx-tg`'s `the_skip_placeholders_keep_the_shape_the_archive_matches_on`
/// ties the prefix here to `plan::NOT_INCLUDED` and `plan::TOO_LARGE`, which
/// this layer may not import.
const MEDIA_KEYS: &[&str] = &["file", "photo", "thumbnail", "file_size", "photo_file_size"];
const SKIPPED_PREFIX: &str = "(File ";

/// A chat with no topics, and a forum's General topic, are both id 1 — mirrors
/// `tgx_media::topics::GENERAL_TOPIC_ID`, which this layer may not depend on.
/// Only ever the value *inserted* when the caller could not resolve a topic;
/// see [`Store::merge`] for why it never overwrites one.
const NO_TOPIC: i64 = 1;

const SCHEMA: &str = "\
CREATE TABLE chats (id INTEGER PRIMARY KEY, title TEXT NOT NULL, type TEXT NOT NULL,
                    first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL);

CREATE TABLE topics (chat_id INTEGER NOT NULL, id INTEGER NOT NULL, title TEXT NOT NULL,
                     head TEXT NOT NULL,
                     first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
                     PRIMARY KEY (chat_id, id));

CREATE TABLE messages (chat_id INTEGER NOT NULL, id INTEGER NOT NULL, topic_id INTEGER NOT NULL,
                       payload TEXT NOT NULL,
                       type TEXT NOT NULL, date_unixtime INTEGER NOT NULL, from_id TEXT,
                       version INTEGER NOT NULL DEFAULT 1,
                       first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
                       deleted_seen INTEGER,
                       PRIMARY KEY (chat_id, id));
CREATE INDEX messages_topic ON messages (chat_id, topic_id, id);
CREATE INDEX messages_date  ON messages (chat_id, date_unixtime);
CREATE INDEX messages_from  ON messages (chat_id, from_id);

CREATE TABLE versions (chat_id INTEGER NOT NULL, id INTEGER NOT NULL, version INTEGER NOT NULL,
                       payload TEXT NOT NULL, seen INTEGER NOT NULL,
                       PRIMARY KEY (chat_id, id, version));

CREATE TABLE blobs (file_id INTEGER NOT NULL, kind TEXT NOT NULL, mime TEXT NOT NULL,
                    file_name TEXT, size INTEGER NOT NULL, bytes BLOB NOT NULL,
                    first_seen INTEGER NOT NULL,
                    PRIMARY KEY (file_id, kind));

CREATE TABLE thumbs (file_id INTEGER NOT NULL, kind TEXT NOT NULL, bytes BLOB NOT NULL,
                     PRIMARY KEY (file_id, kind));

CREATE TABLE media (chat_id INTEGER NOT NULL, message_id INTEGER NOT NULL, role TEXT NOT NULL,
                    file_id INTEGER NOT NULL, kind TEXT NOT NULL, path TEXT NOT NULL,
                    PRIMARY KEY (chat_id, message_id, role));

CREATE TABLE participants (chat_id INTEGER NOT NULL, peer TEXT NOT NULL, info TEXT NOT NULL,
                           first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
                           PRIMARY KEY (chat_id, peer));

CREATE TABLE runs (id INTEGER PRIMARY KEY AUTOINCREMENT, chat_id INTEGER NOT NULL,
                   started INTEGER NOT NULL, finished INTEGER, mode TEXT NOT NULL,
                   root TEXT,
                   new INTEGER NOT NULL DEFAULT 0, changed INTEGER NOT NULL DEFAULT 0,
                   deleted INTEGER NOT NULL DEFAULT 0, blobs_added INTEGER NOT NULL DEFAULT 0,
                   reached_end INTEGER NOT NULL DEFAULT 0, settings TEXT NOT NULL);
";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Sql(#[from] libsql::Error),
    #[error("database payload: {0}")]
    Json(#[from] serde_json::Error),
    #[error("database file: {0}")]
    Io(#[from] std::io::Error),
    /// A file written by a different build of this program.
    ///
    /// Refused in **both** directions. A newer file read by an older build is
    /// the obvious hazard; an older file silently upgraded by a newer one is
    /// the worse one, because the other machine can then no longer open the
    /// export that was just made on this one.
    #[error(
        "telegram.sqlite is schema version {found}, this build understands {wanted} — use the \
         build that wrote it, or export to a different folder"
    )]
    Schema { found: i64, wanted: i64 },
    #[error("file {file_id} is {size} bytes, over the database's blob ceiling")]
    TooLarge { file_id: i64, size: i64 },
}

/// What [`Store::merge`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merge {
    /// Not in the database before this call.
    Inserted,
    /// Byte-identical to what was already there.
    Unchanged,
    /// Different; what was there has been kept in `versions`.
    Changed { previous_version: i64 },
}

/// One message as the database holds it.
#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub id: i64,
    pub topic_id: i64,
    pub payload: Map<String, Value>,
    pub version: i64,
    /// `None` while Telegram still returns it.
    pub deleted_seen: Option<i64>,
}

/// Merge a freshly read message into one already stored.
///
/// Starts from `stored` and copies [`VOLATILE`] across, removing a key the
/// fresh payload no longer has. Everything else is the stored message,
/// untouched.
///
/// **This is where the database's one real limitation lives.** A key outside
/// the list keeps whatever the build that first saw the message wrote, for
/// good: a later converter fix never reaches an already-stored message, and a
/// new extension key never appears on an old row. That is the price of not
/// letting a re-read rewrite history, and it is recorded in ROADMAP.
///
/// The result goes back through `order::ordered`, which the json leg proves is
/// a no-op on a real export — so a merged message still emits Desktop's bytes.
pub fn merge_volatile(
    stored: &Map<String, Value>,
    fresh: &Map<String, Value>,
) -> Map<String, Value> {
    let mut out = stored.clone();
    for key in VOLATILE {
        match fresh.get(*key) {
            Some(v) => {
                out.insert((*key).to_string(), v.clone());
            }
            None => {
                out.remove(*key);
            }
        }
    }
    // A media key the earlier run declined to fetch. See [`MEDIA_KEYS`]: the
    // placeholder is a record of a refusal, not of what the message carried,
    // and a run that has now fetched the file must be allowed to say so.
    for key in MEDIA_KEYS {
        let was_skipped = out
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with(SKIPPED_PREFIX));
        if !was_skipped {
            continue;
        }
        match fresh.get(*key) {
            Some(v) => {
                out.insert((*key).to_string(), v.clone());
            }
            None => {
                out.remove(*key);
            }
        }
    }
    tgx_format::order::ordered(&out)
}

pub struct Store {
    conn: libsql::Connection,
    /// Held so the connection cannot outlive the handle that made it.
    _db: libsql::Database,
    path: PathBuf,
}

impl Store {
    /// Open `path`, creating the file and the schema if they are not there.
    pub async fn open(path: &Path) -> Result<Self, Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = libsql::Builder::new_local(path).build().await?;
        let conn = db.connect()?;
        conn.busy_timeout(BUSY_TIMEOUT)?;

        // **Read back what the pragma actually did.** WAL is what we want — it
        // lets a reader open the file while an export is writing it — but it
        // is not available everywhere, and the case that matters here is real:
        // the output folder is free text and people point it at a network
        // share. SQLite refuses WAL on most of those and stays in the old
        // journal mode, which is correct, and which we would otherwise be
        // recording in the log as WAL.
        let mut rows = conn.query("PRAGMA journal_mode = WAL", ()).await?;
        let mode = match rows.next().await? {
            Some(row) => row.get::<String>(0).unwrap_or_default(),
            None => String::new(),
        };
        if !mode.eq_ignore_ascii_case("wal") {
            log::warn!(
                "{}: journal mode is {mode:?}, not WAL — a reader cannot open this file while \
                 an export is writing to it (a network share does this)",
                path.display()
            );
        }
        conn.execute_batch("PRAGMA synchronous = NORMAL;").await?;

        let store = Self {
            conn,
            _db: db,
            path: path.to_path_buf(),
        };
        store.ensure_schema().await?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn ensure_schema(&self) -> Result<(), Error> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .await?;
        let found = self
            .row("SELECT value FROM meta WHERE key = 'schema_version'", ())
            .await?
            .map(|r| r.get::<String>(0).unwrap_or_default());
        match found {
            Some(v) => {
                // A value that is not a number is a file this build cannot
                // account for, which is the same answer as a version it does
                // not know: refuse it. `-1` can never equal SCHEMA_VERSION.
                let found: i64 = v.trim().parse().unwrap_or(-1);
                if found != SCHEMA_VERSION {
                    return Err(Error::Schema {
                        found,
                        wanted: SCHEMA_VERSION,
                    });
                }
            }
            None => {
                self.conn.execute_batch(SCHEMA).await?;
                self.conn
                    .execute(
                        "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                        params![SCHEMA_VERSION.to_string()],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// The first row of a query, or `None` if it returned no rows.
    async fn row(
        &self,
        sql: &str,
        args: impl IntoParams,
    ) -> Result<Option<libsql::Row>, libsql::Error> {
        let mut rows = self.conn.query(sql, args).await?;
        rows.next().await
    }

    // --- chats and topics ---------------------------------------------------

    /// `first_seen` is set once and never moved; `last_seen` is this run.
    pub async fn upsert_chat(
        &self,
        id: i64,
        title: &str,
        kind: &str,
        now: i64,
    ) -> Result<(), Error> {
        self.conn
            .execute(
                "INSERT INTO chats (id, title, type, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(id) DO UPDATE SET title = ?2, type = ?3, last_seen = ?4",
                params![id, title, kind, now],
            )
            .await?;
        Ok(())
    }

    pub async fn upsert_topic(
        &self,
        chat_id: i64,
        id: i64,
        title: &str,
        head: &Map<String, Value>,
        now: i64,
    ) -> Result<(), Error> {
        let head = serde_json::to_string(head)?;
        self.conn
            .execute(
                "INSERT INTO topics (chat_id, id, title, head, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(chat_id, id) DO UPDATE SET title = ?3, head = ?4, last_seen = ?5",
                params![chat_id, id, title, head, now],
            )
            .await?;
        Ok(())
    }

    // --- messages -----------------------------------------------------------

    /// `(everything stored, everything Telegram still has)`.
    ///
    /// The difference is what earlier runs concluded was deleted, which is the
    /// number worth putting in the log: an archive of 6,643 messages of which
    /// 12 no longer exist on Telegram is a different thing from one of 6,631.
    pub async fn message_count(&self, chat_id: i64) -> Result<(i64, i64), Error> {
        let row = self
            .row(
                "SELECT COUNT(*), COUNT(*) FILTER (WHERE deleted_seen IS NULL)
                 FROM messages WHERE chat_id = ?1",
                params![chat_id],
            )
            .await?;
        Ok(match row {
            Some(r) => (r.get::<i64>(0)?, r.get::<i64>(1)?),
            None => (0, 0),
        })
    }

    /// The id to resume a database-only run from: the `keep`-th newest stored
    /// message, or 0 when fewer than that are stored.
    ///
    /// 0 is not a message id, and the read loop treats `offset_id = 0` as "from
    /// the beginning" — so an archive smaller than the window is walked whole,
    /// which is what it should be.
    pub async fn window_start(&self, chat_id: i64, keep: usize) -> Result<i64, Error> {
        let row = self
            .row(
                "SELECT id FROM messages WHERE chat_id = ?1 ORDER BY id DESC LIMIT 1 OFFSET ?2",
                params![chat_id, keep as i64],
            )
            .await?;
        Ok(match row {
            Some(r) => r.get::<i64>(0)?,
            None => 0,
        })
    }

    pub async fn get(&self, chat_id: i64, id: i64) -> Result<Option<StoredMessage>, Error> {
        let row = self
            .row(
                "SELECT topic_id, payload, version, deleted_seen FROM messages
                 WHERE chat_id = ?1 AND id = ?2",
                params![chat_id, id],
            )
            .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(StoredMessage {
            id,
            topic_id: row.get::<i64>(0)?,
            payload: serde_json::from_str(&row.get::<String>(1)?)?,
            version: row.get::<i64>(2)?,
            deleted_seen: row.get::<Option<i64>>(3)?,
        }))
    }

    /// Insert, or update in place keeping the old payload in `versions`.
    ///
    /// `topic_id` is `None` when the run could not resolve topics — a forum
    /// exported with the topic split turned off reports General for every
    /// message, and letting that overwrite what a split run learned would
    /// flatten the whole archive on one careless export. So an unknown topic
    /// inserts as [`NO_TOPIC`] and never overwrites a stored one.
    ///
    /// **A message Telegram returns is not deleted, whatever an earlier run
    /// concluded**: every present branch clears `deleted_seen`. Deletion is a
    /// thing we infer from an absence, and an inference has to yield to an
    /// observation — the message may have been in a gap a cancelled run never
    /// reached.
    pub async fn merge(
        &self,
        chat_id: i64,
        id: i64,
        topic_id: Option<i64>,
        payload: &Map<String, Value>,
        now: i64,
    ) -> Result<Merge, Error> {
        let text = serde_json::to_string(payload)?;
        let kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
        // Desktop writes `date_unixtime` as a *string* — see
        // `tgx_format::date_pair`. Stored as an integer here because the
        // column is what a date range query would use.
        let date: i64 = payload
            .get("date_unixtime")
            .and_then(|v| match v {
                Value::String(s) => s.parse().ok(),
                Value::Number(n) => n.as_i64(),
                _ => None,
            })
            .unwrap_or(0);
        let from = payload.get("from_id").and_then(Value::as_str);

        let existing = self
            .row(
                "SELECT payload, version, topic_id FROM messages WHERE chat_id = ?1 AND id = ?2",
                params![chat_id, id],
            )
            .await?;

        let Some(row) = existing else {
            self.conn
                .execute(
                    "INSERT INTO messages
                       (chat_id, id, topic_id, payload, type, date_unixtime, from_id,
                        version, first_seen, last_seen, deleted_seen)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8, NULL)",
                    params![
                        chat_id,
                        id,
                        topic_id.unwrap_or(NO_TOPIC),
                        text,
                        kind,
                        date,
                        from,
                        now
                    ],
                )
                .await?;
            return Ok(Merge::Inserted);
        };

        let stored_text = row.get::<String>(0)?;
        let version = row.get::<i64>(1)?;
        let stored_topic = row.get::<i64>(2)?;
        let topic = topic_id.unwrap_or(stored_topic);

        if stored_text == text {
            self.conn
                .execute(
                    "UPDATE messages SET last_seen = ?3, deleted_seen = NULL, topic_id = ?4
                     WHERE chat_id = ?1 AND id = ?2",
                    params![chat_id, id, now, topic],
                )
                .await?;
            return Ok(Merge::Unchanged);
        }

        self.conn
            .execute(
                "INSERT OR REPLACE INTO versions (chat_id, id, version, payload, seen)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![chat_id, id, version, stored_text, now],
            )
            .await?;
        self.conn
            .execute(
                "UPDATE messages
                    SET payload = ?3, type = ?4, date_unixtime = ?5, from_id = ?6,
                        topic_id = ?7, version = version + 1, last_seen = ?8, deleted_seen = NULL
                  WHERE chat_id = ?1 AND id = ?2",
                params![chat_id, id, text, kind, date, from, topic, now],
            )
            .await?;
        Ok(Merge::Changed {
            previous_version: version,
        })
    }

    /// Mark every stored message above `above_id` that this run did not see.
    ///
    /// **Only ever called after a walk that ended normally.** A cancelled or
    /// stalled run cannot tell "Telegram no longer has it" from "we never got
    /// that far", and marking on the second would write a deletion date onto
    /// most of a chat.
    pub async fn mark_unseen_deleted(
        &self,
        chat_id: i64,
        above_id: i64,
        seen: &HashSet<i64>,
        now: i64,
    ) -> Result<usize, Error> {
        // Collected first, then updated. The alternative is an `IN (…)` list of
        // every id the run saw, which for this chat is six thousand of them in
        // one statement.
        let mut rows = self
            .conn
            .query(
                "SELECT id FROM messages
                  WHERE chat_id = ?1 AND id > ?2 AND deleted_seen IS NULL",
                params![chat_id, above_id],
            )
            .await?;
        let mut gone = Vec::new();
        while let Some(row) = rows.next().await? {
            let id = row.get::<i64>(0)?;
            if !seen.contains(&id) {
                gone.push(id);
            }
        }
        for id in &gone {
            self.conn
                .execute(
                    "UPDATE messages SET deleted_seen = ?3 WHERE chat_id = ?1 AND id = ?2",
                    params![chat_id, *id, now],
                )
                .await?;
        }
        Ok(gone.len())
    }

    /// How many superseded payloads this chat has accumulated.
    ///
    /// One per time a re-read found a message different from the stored one.
    /// The archive leg reads it as an assertion: two merges of an export
    /// nobody edited in between must leave this at zero.
    pub async fn version_count(&self, chat_id: i64) -> Result<i64, Error> {
        let row = self
            .row(
                "SELECT COUNT(*) FROM versions WHERE chat_id = ?1",
                params![chat_id],
            )
            .await?;
        Ok(match row {
            Some(r) => r.get::<i64>(0)?,
            None => 0,
        })
    }

    /// Every message of one topic, oldest first.
    pub async fn messages_of(
        &self,
        chat_id: i64,
        topic_id: i64,
    ) -> Result<Vec<StoredMessage>, Error> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, payload, version, deleted_seen FROM messages
                  WHERE chat_id = ?1 AND topic_id = ?2 ORDER BY id",
                params![chat_id, topic_id],
            )
            .await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(StoredMessage {
                id: row.get::<i64>(0)?,
                topic_id,
                payload: serde_json::from_str(&row.get::<String>(1)?)?,
                version: row.get::<i64>(2)?,
                deleted_seen: row.get::<Option<i64>>(3)?,
            });
        }
        Ok(out)
    }

    // --- media --------------------------------------------------------------
    //
    // **Keyed on `(file_id, kind)`, not on `file_id` alone.** Telegram's photo
    // ids and document ids are two spaces, and this file dedupes across every
    // chat it has ever seen, for good — so a bare id is a global, permanent
    // assumption that the two never collide. `kind` is the `_LAYOUT` folder the
    // file's *shape* put it in, which is already on the job, so the second half
    // of the key costs nothing. `NameBook::by_id` takes the same risk but only
    // within one folder and only for the length of a run.

    pub async fn has_blob(&self, file_id: i64, kind: &str) -> Result<bool, Error> {
        Ok(self
            .row(
                "SELECT 1 FROM blobs WHERE file_id = ?1 AND kind = ?2",
                params![file_id, kind],
            )
            .await?
            .is_some())
    }

    /// Store the bytes once. A second call for the same file is a no-op.
    pub async fn put_blob(
        &self,
        file_id: i64,
        kind: &str,
        mime: &str,
        file_name: Option<&str>,
        bytes: &[u8],
        now: i64,
    ) -> Result<(), Error> {
        let size = bytes.len() as i64;
        if size > MAX_BLOB {
            return Err(Error::TooLarge { file_id, size });
        }
        self.conn
            .execute(
                "INSERT OR IGNORE INTO blobs (file_id, kind, mime, file_name, size, bytes, first_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    file_id,
                    kind,
                    mime,
                    file_name,
                    size,
                    libsql::Value::Blob(bytes.to_vec()),
                    now
                ],
            )
            .await?;
        Ok(())
    }

    /// Telegram's own thumbnail for a file. The *rendered* preview
    /// (`<stem>_thumb<ext>`) is an HTML-only artefact and is not stored.
    pub async fn put_thumb(&self, file_id: i64, kind: &str, bytes: &[u8]) -> Result<(), Error> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO thumbs (file_id, kind, bytes) VALUES (?1, ?2, ?3)",
                params![file_id, kind, libsql::Value::Blob(bytes.to_vec())],
            )
            .await?;
        Ok(())
    }

    /// Point a message at a blob. `path` is the name the folder export used, or
    /// would have used, and is informational.
    pub async fn link_media(
        &self,
        chat_id: i64,
        message_id: i64,
        role: &str,
        file_id: i64,
        kind: &str,
        path: &str,
    ) -> Result<(), Error> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO media (chat_id, message_id, role, file_id, kind, path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![chat_id, message_id, role, file_id, kind, path],
            )
            .await?;
        Ok(())
    }

    pub async fn blob(&self, file_id: i64, kind: &str) -> Result<Option<Vec<u8>>, Error> {
        let row = self
            .row(
                "SELECT bytes FROM blobs WHERE file_id = ?1 AND kind = ?2",
                params![file_id, kind],
            )
            .await?;
        Ok(match row {
            Some(r) => Some(r.get::<Vec<u8>>(0)?),
            None => None,
        })
    }

    // --- participants and runs ----------------------------------------------

    /// **Ex-members stay.** `participants.json` is a snapshot of who is in the
    /// chat now; this table is who has ever been seen in it, which is the
    /// question an archive gets asked.
    pub async fn upsert_participant(
        &self,
        chat_id: i64,
        peer: &str,
        info: &Value,
        now: i64,
    ) -> Result<(), Error> {
        let info = serde_json::to_string(info)?;
        self.conn
            .execute(
                "INSERT INTO participants (chat_id, peer, info, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(chat_id, peer) DO UPDATE SET info = ?3, last_seen = ?4",
                params![chat_id, peer, info, now],
            )
            .await?;
        Ok(())
    }

    /// Open a write transaction, if one is not open already.
    ///
    /// Six thousand messages is six thousand `INSERT`s, and each one outside a
    /// transaction is its own fsync. The engine commits every couple of hundred
    /// so a crash costs a batch rather than a chat.
    pub async fn begin_batch(&self) -> Result<(), Error> {
        if self.conn.is_autocommit() {
            self.conn.execute_batch("BEGIN;").await?;
        }
        Ok(())
    }

    /// Commit whatever is open. **A no-op when nothing is** — which is what
    /// lets every exit path call it without first asking whether it needs to.
    pub async fn commit_batch(&self) -> Result<(), Error> {
        if !self.conn.is_autocommit() {
            self.conn.execute_batch("COMMIT;").await?;
        }
        Ok(())
    }

    pub async fn start_run(
        &self,
        chat_id: i64,
        mode: &str,
        root: Option<&Path>,
        settings_json: &str,
        now: i64,
    ) -> Result<i64, Error> {
        let root = root.map(|p| p.to_string_lossy().into_owned());
        self.conn
            .execute(
                "INSERT INTO runs (chat_id, started, mode, root, settings)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![chat_id, now, mode, root, settings_json],
            )
            .await?;
        Ok(self.conn.last_insert_rowid())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_run(
        &self,
        run_id: i64,
        new: usize,
        changed: usize,
        deleted: usize,
        blobs_added: usize,
        reached_end: bool,
        now: i64,
    ) -> Result<(), Error> {
        self.conn
            .execute(
                "UPDATE runs SET finished = ?2, new = ?3, changed = ?4, deleted = ?5,
                                 blobs_added = ?6, reached_end = ?7
                  WHERE id = ?1",
                params![
                    run_id,
                    now,
                    new as i64,
                    changed as i64,
                    deleted as i64,
                    blobs_added as i64,
                    i64::from(reached_end)
                ],
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One temp file per test, modelled on `tgx_tg::output`'s `tmp`.
    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tgx-archive-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{name}.sqlite"));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        path
    }

    async fn store(name: &str) -> Store {
        Store::open(&tmp(name)).await.expect("opening the store")
    }

    fn msg(id: i64, text: &str) -> Map<String, Value> {
        json!({
            "id": id,
            "type": "message",
            "date": "2025-12-18T16:00:00",
            "date_unixtime": "1766071072",
            "from": "Ivana",
            "from_id": "user31774388",
            "text": text,
            "text_entities": [],
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[tokio::test]
    async fn a_new_message_is_inserted_and_an_identical_one_is_unchanged() {
        let s = store("insert").await;
        assert_eq!(
            s.merge(1, 10, Some(1), &msg(10, "hi"), 100).await.unwrap(),
            Merge::Inserted
        );
        assert_eq!(
            s.merge(1, 10, Some(1), &msg(10, "hi"), 200).await.unwrap(),
            Merge::Unchanged
        );
        assert_eq!(s.message_count(1).await.unwrap(), (1, 1));
    }

    #[tokio::test]
    async fn a_changed_message_keeps_its_previous_payload_in_versions() {
        let s = store("versions").await;
        s.merge(1, 10, Some(1), &msg(10, "before"), 100)
            .await
            .unwrap();
        let what = s
            .merge(1, 10, Some(1), &msg(10, "after"), 200)
            .await
            .unwrap();
        assert_eq!(
            what,
            Merge::Changed {
                previous_version: 1
            }
        );

        let now = s.get(1, 10).await.unwrap().expect("still there");
        assert_eq!(now.payload["text"], json!("after"));
        assert_eq!(now.version, 2);

        let row = s
            .row(
                "SELECT payload FROM versions WHERE chat_id = 1 AND id = 10 AND version = 1",
                (),
            )
            .await
            .unwrap()
            .expect("the old payload is kept");
        let old: Map<String, Value> = serde_json::from_str(&row.get::<String>(0).unwrap()).unwrap();
        assert_eq!(old["text"], json!("before"));
    }

    #[tokio::test]
    async fn marking_touches_only_unseen_ids_above_the_start() {
        let s = store("marking").await;
        for id in 1..=10 {
            s.merge(1, id, Some(1), &msg(id, "x"), 100).await.unwrap();
        }
        let seen: HashSet<i64> = [6, 7, 9, 10].into_iter().collect();
        assert_eq!(s.mark_unseen_deleted(1, 5, &seen, 300).await.unwrap(), 1);

        assert!(s.get(1, 8).await.unwrap().unwrap().deleted_seen.is_some());
        for id in 1..=5 {
            assert!(
                s.get(1, id).await.unwrap().unwrap().deleted_seen.is_none(),
                "id {id} is below the start and must not be touched"
            );
        }
        assert_eq!(s.message_count(1).await.unwrap(), (10, 9));
    }

    #[tokio::test]
    async fn a_marked_message_is_unmarked_when_telegram_returns_it_again() {
        let s = store("unmark").await;
        for id in [8, 9] {
            s.merge(1, id, Some(1), &msg(id, "x"), 100).await.unwrap();
        }
        s.mark_unseen_deleted(1, 0, &HashSet::new(), 200)
            .await
            .unwrap();
        assert!(s.get(1, 8).await.unwrap().unwrap().deleted_seen.is_some());

        // Returned again, unchanged: unmarked, and no version written — a
        // deletion that turned out not to be one is not a change to the
        // message.
        assert_eq!(
            s.merge(1, 8, Some(1), &msg(8, "x"), 300).await.unwrap(),
            Merge::Unchanged
        );
        assert!(s.get(1, 8).await.unwrap().unwrap().deleted_seen.is_none());
        assert!(s
            .row("SELECT 1 FROM versions WHERE chat_id = 1 AND id = 8", ())
            .await
            .unwrap()
            .is_none());

        // And the one that really is gone stays gone.
        assert!(s.get(1, 9).await.unwrap().unwrap().deleted_seen.is_some());
    }

    #[tokio::test]
    async fn window_start_is_zero_for_a_small_archive_and_the_501st_newest_otherwise() {
        let s = store("window").await;
        for id in 1..=100 {
            s.merge(1, id, Some(1), &msg(id, "x"), 100).await.unwrap();
        }
        assert_eq!(s.window_start(1, 500).await.unwrap(), 0);
        assert_eq!(s.window_start(1, 10).await.unwrap(), 90);
        // 0 means "only what is new": start at the newest stored id.
        assert_eq!(s.window_start(1, 0).await.unwrap(), 100);
    }

    #[tokio::test]
    async fn two_chats_do_not_see_each_others_messages() {
        let s = store("two-chats").await;
        s.merge(1, 10, Some(1), &msg(10, "one"), 100).await.unwrap();
        s.merge(2, 10, Some(1), &msg(10, "two"), 100).await.unwrap();
        assert_eq!(s.message_count(1).await.unwrap(), (1, 1));
        assert_eq!(s.get(2, 10).await.unwrap().unwrap().payload["text"], "two");
        // Marking one chat leaves the other alone.
        s.mark_unseen_deleted(1, 0, &HashSet::new(), 200)
            .await
            .unwrap();
        assert!(s.get(2, 10).await.unwrap().unwrap().deleted_seen.is_none());
    }

    #[tokio::test]
    async fn an_unresolved_topic_never_overwrites_one_a_split_run_learned() {
        let s = store("topics").await;
        s.merge(1, 10, Some(4242), &msg(10, "x"), 100)
            .await
            .unwrap();
        // A later run with the topic split turned off reports General for
        // everything. It must not flatten the archive.
        s.merge(1, 10, None, &msg(10, "x"), 200).await.unwrap();
        assert_eq!(s.get(1, 10).await.unwrap().unwrap().topic_id, 4242);
        assert_eq!(s.messages_of(1, 4242).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_blob_is_stored_once_per_file_id_and_linked_from_every_message() {
        let s = store("blobs").await;
        assert!(!s.has_blob(77, "photos").await.unwrap());
        s.put_blob(77, "photos", "image/jpeg", None, b"first", 100)
            .await
            .unwrap();
        s.put_blob(77, "photos", "image/jpeg", None, b"second", 200)
            .await
            .unwrap();
        assert!(s.has_blob(77, "photos").await.unwrap());
        assert_eq!(s.blob(77, "photos").await.unwrap().unwrap(), b"first");

        // The same id in another kind is another file, not the same one.
        assert!(!s.has_blob(77, "video_files").await.unwrap());

        s.link_media(1, 10, "photo", 77, "photos", "photos/photo_1.jpg")
            .await
            .unwrap();
        s.link_media(1, 11, "photo", 77, "photos", "photos/photo_1.jpg")
            .await
            .unwrap();
        let row = s
            .row("SELECT COUNT(*) FROM media WHERE file_id = 77", ())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 2);
    }

    #[tokio::test]
    async fn a_blob_over_the_ceiling_is_refused_not_truncated() {
        let s = store("too-big").await;
        let huge = vec![0u8; (MAX_BLOB + 1) as usize];
        match s
            .put_blob(5, "video_files", "video/mp4", None, &huge, 100)
            .await
        {
            Err(Error::TooLarge { file_id, size }) => {
                assert_eq!(file_id, 5);
                assert_eq!(size, MAX_BLOB + 1);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        assert!(!s.has_blob(5, "video_files").await.unwrap());
    }

    #[tokio::test]
    async fn a_schema_version_this_build_does_not_know_is_refused() {
        let path = tmp("schema");
        {
            let s = Store::open(&path).await.unwrap();
            s.conn
                .execute(
                    "UPDATE meta SET value = '99' WHERE key = 'schema_version'",
                    (),
                )
                .await
                .unwrap();
        }
        match Store::open(&path).await {
            Err(Error::Schema { found, wanted }) => {
                assert_eq!((found, wanted), (99, SCHEMA_VERSION));
            }
            other => panic!("expected a schema refusal, got {:?}", other.map(|_| ())),
        }
    }

    #[tokio::test]
    async fn a_run_records_what_it_did() {
        let s = store("runs").await;
        let id = s
            .start_run(1, "window", None, "{\"export_db\":true}", 100)
            .await
            .unwrap();
        s.finish_run(id, 12, 3, 1, 4, true, 200).await.unwrap();
        let row = s
            .row(
                "SELECT mode, new, changed, deleted, blobs_added, reached_end, finished
                   FROM runs WHERE id = ?1",
                params![id],
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "window");
        assert_eq!(row.get::<i64>(1).unwrap(), 12);
        assert_eq!(row.get::<i64>(5).unwrap(), 1);
        assert_eq!(row.get::<i64>(6).unwrap(), 200);
    }

    #[tokio::test]
    async fn committing_with_nothing_open_is_harmless() {
        let s = store("batch").await;
        s.commit_batch().await.unwrap();
        s.begin_batch().await.unwrap();
        s.begin_batch().await.unwrap();
        s.merge(1, 1, Some(1), &msg(1, "x"), 100).await.unwrap();
        s.commit_batch().await.unwrap();
        s.commit_batch().await.unwrap();
        assert_eq!(s.message_count(1).await.unwrap(), (1, 1));
    }

    #[test]
    fn merge_volatile_replaces_text_and_reactions_and_keeps_everything_else() {
        let mut stored = msg(10, "before");
        stored.insert("file".into(), json!("files/a.mp3"));
        stored.insert("reactions".into(), json!([{ "emoji": "👍", "count": 1 }]));

        let mut fresh = msg(10, "after");
        fresh.insert("edited".into(), json!("2025-12-19T10:00:00"));
        // The fresh read has no reactions any more, and a different file — one
        // of those must survive and the other must not.
        fresh.insert("file".into(), json!("files/REPLACED.mp3"));

        let out = merge_volatile(&stored, &fresh);
        assert_eq!(out["text"], json!("after"));
        assert_eq!(out["edited"], json!("2025-12-19T10:00:00"));
        assert!(!out.contains_key("reactions"), "a removed reaction is gone");
        assert_eq!(
            out["file"],
            json!("files/a.mp3"),
            "media keys are never taken from a re-read"
        );
        // And the result is in Desktop's key order, so it still emits exactly.
        let keys: Vec<&String> = out.keys().collect();
        let ordered = tgx_format::order::ordered(&out);
        assert_eq!(keys, ordered.keys().collect::<Vec<&String>>());
    }

    #[test]
    fn a_file_the_earlier_run_declined_to_fetch_is_filled_in_later() {
        // Exported once with a 20 MB limit, so Desktop's placeholder went in.
        let mut stored = msg(10, "look at this");
        stored.insert(
            "file".into(),
            json!("(File exceeds maximum size. Change data exporting settings to download.)"),
        );
        // Re-synced with the limit raised: the run has the real path now.
        let mut fresh = msg(10, "look at this");
        fresh.insert("file".into(), json!("video_files/clip.mp4"));
        fresh.insert("file_size".into(), json!(48_000_000));

        let out = merge_volatile(&stored, &fresh);
        assert_eq!(
            out["file"],
            json!("video_files/clip.mp4"),
            "a placeholder is a record of a refusal, not of what the message carried"
        );
    }

    #[test]
    fn a_real_media_path_is_never_overwritten_by_a_re_read() {
        let mut stored = msg(10, "x");
        stored.insert("file".into(), json!("files/original.mp3"));
        let mut fresh = msg(10, "x");
        fresh.insert("file".into(), json!("files/REPLACED.mp3"));

        assert_eq!(
            merge_volatile(&stored, &fresh)["file"],
            json!("files/original.mp3"),
            "only placeholders are replaceable; history is not"
        );
    }
}
