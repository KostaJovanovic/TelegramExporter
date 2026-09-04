# Database output: a third export format that accumulates across runs

> **This document is the work order as it was written, kept as the record of
> what was planned. It has been carried out, and two decisions in it were
> reversed afterwards — read `CLAUDE.md` for what the code actually does.**
>
> 1. **Not a third format alongside HTML and JSON.** Classic and Database are
>    exclusive: Classic re-reads the whole chat anyway, so running both cost
>    exactly what Classic cost and was incremental in name only.
> 2. **Not one file for the export root.** One `<chat>.sqlite` per chat. A
>    database is the thing most likely to be handed to somebody, and a file
>    named after one conversation must not carry every other chat exported into
>    the same folder.

This is a self-contained work order. Read `CLAUDE.md` first; everything it says
still holds. Do the steps in order, run `save.bat test` after each step, and do
not start the next step until the suite and the three corpus legs are green at
their baseline (json 4/4, html 4/4, media 830/836). Never edit anything under
`reference/`. Never run a live export or read `TelegramExporterData/`: the
final live check is Kosta's to run, not yours.

## 1. Context

Today every export is a fresh, complete pass into a new folder: `result.json`,
`messages*.html` and the media tree, byte for byte in Telegram Desktop's
format, proven by the parity legs. **That stays exactly as it is.** A message
deleted on Telegram between two exports is simply absent from the second one,
and nothing ties two exports of the same chat together.

The feature: a third output format, **Database**, next to HTML and JSON in the
Format settings. When it is on, every message the run reads is merged into one
SQLite file for the whole export root, `Exports/telegram.sqlite`, together with
its media bytes. The file accumulates across runs: new messages are added,
changed messages keep their previous versions, and a message Telegram no longer
returns stays in the database, marked with the date it was noticed missing.

Decisions already taken (do not reopen them):

- **The normal HTML/JSON export is not touched.** No key-order change, no
  writer change, no wire-leg change, no crate moves. The deleted marker exists
  only in the database.
- **One file, all chats:** `<output_dir>/telegram.sqlite`, rows keyed by chat id
  and message id.
- **Media bytes go inside the database**, deduplicated across the whole file by
  Telegram's file id, under the same size limit and media kinds settings as the
  folder export. Telegram's own thumbnail is stored beside each file. The
  rendered preview (`<stem>_thumb<ext>`) is HTML-only and is not stored.
- **SQLite through the `libsql` crate**, version `0.9.30`, features `["core"]`
  only. It is already compiled into the binary because `grammers-session`
  enables its `sqlite-storage` feature with exactly that crate, version and
  feature (see `Cargo.lock`). Do **not** add `rusqlite`: its bundled
  `libsqlite3-sys` exports the same `sqlite3_*` symbols as `libsql-ffi` and the
  link fails. Pin it **exactly** (`"=0.9.30"`), winresource-style: bump on its
  own commit, in step with grammers' pin, legs green either side.
- **How much a run reads is decided by its formats, not by a mode switch.**
  With HTML or JSON on, the run reads the whole history anyway and the database
  is filled from that pass; a completed pass also marks every stored message it
  did not see as deleted. With **only** Database on, the run is a sync: it
  reads what is new plus a trailing window of the newest `reread_window` stored
  messages (default 500) for edits, reactions and deletions, and `reread_all`
  forces a whole-history pass to mark older deletions.
- **The database never holds a credential.** `Settings` serialises `api_id`,
  `api_hash` and `phone` first, and `Exports/` is not ACL-restricted the way
  `TelegramExporterData/` is. The run row records a redacted subset, defined
  once in `config.rs` (see Step 4), never `serde_json::to_string(settings)`.

## 2. The design

### A new crate, `crates/tgx-archive`

The store must be replayable by the oracle from recorded data, which is the
same reason `tgx-html` is its own layer. So it lives in a crate that knows
nothing of Telegram:

```
tgx-archive  the SQLite store (chats, topics, messages, versions, blobs, participants, runs)
             and `merge_payload`, the rule for re-reading a stored message.
             Depends on tgx-format, libsql, serde_json, thiserror, log. tokio is a dev-dependency.
             MUST NOT depend on grammers-*.   (new rule in layering.rs)
tgx-tg       gains `tgx-archive = { path = "../tgx-archive" }`.
tgx-parity   gains the same, for the archive leg.
```

### Schema, version 1 (timestamps are unix seconds)

```sql
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);          -- schema_version = "1"
CREATE TABLE chats (id INTEGER PRIMARY KEY, title TEXT NOT NULL, type TEXT NOT NULL,
                    first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL);
CREATE TABLE topics (chat_id INTEGER NOT NULL, id INTEGER NOT NULL, title TEXT NOT NULL,
                     head TEXT NOT NULL,                       -- the topic's header map as JSON
                     first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
                     PRIMARY KEY (chat_id, id));
CREATE TABLE messages (chat_id INTEGER NOT NULL, id INTEGER NOT NULL, topic_id INTEGER NOT NULL,
                       payload TEXT NOT NULL,                  -- the result.json message map, no `_p`
                       type TEXT NOT NULL, date_unixtime INTEGER NOT NULL, from_id TEXT,
                       version INTEGER NOT NULL DEFAULT 1,
                       first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
                       deleted_seen INTEGER,                   -- NULL while Telegram still has it
                       PRIMARY KEY (chat_id, id));
CREATE INDEX messages_topic ON messages (chat_id, topic_id, id);
CREATE INDEX messages_date  ON messages (chat_id, date_unixtime);
CREATE INDEX messages_from  ON messages (chat_id, from_id);
CREATE TABLE versions (chat_id INTEGER NOT NULL, id INTEGER NOT NULL, version INTEGER NOT NULL,
                       payload TEXT NOT NULL, seen INTEGER NOT NULL,
                       PRIMARY KEY (chat_id, id, version));    -- the payload before each change
CREATE TABLE blobs (family TEXT NOT NULL, file_id INTEGER NOT NULL,   -- family: "photo" | "document"
                    kind TEXT NOT NULL, mime TEXT NOT NULL, file_name TEXT, size INTEGER NOT NULL,
                    bytes BLOB NOT NULL, first_seen INTEGER NOT NULL,
                    PRIMARY KEY (family, file_id));
CREATE TABLE thumbs (family TEXT NOT NULL, file_id INTEGER NOT NULL, bytes BLOB NOT NULL,
                     PRIMARY KEY (family, file_id));
CREATE TABLE media (chat_id INTEGER NOT NULL, message_id INTEGER NOT NULL, role TEXT NOT NULL,
                    family TEXT NOT NULL, file_id INTEGER NOT NULL, path TEXT NOT NULL,
                    PRIMARY KEY (chat_id, message_id, role));   -- role: "file" | "photo"
                    -- path: the name the folder export used or would have used, for reference only
CREATE TABLE participants (chat_id INTEGER NOT NULL, peer TEXT NOT NULL, info TEXT NOT NULL,
                           first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
                           PRIMARY KEY (chat_id, peer));       -- peer: the member's "id", e.g. "user123"
CREATE TABLE scheduled (chat_id INTEGER NOT NULL, id INTEGER NOT NULL, payload TEXT NOT NULL,
                        first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
                        PRIMARY KEY (chat_id, id));            -- the rows scheduled.json carries; last payload wins
CREATE TABLE runs (id INTEGER PRIMARY KEY AUTOINCREMENT, chat_id INTEGER NOT NULL,
                   started INTEGER NOT NULL, finished INTEGER, mode TEXT NOT NULL,
                   root TEXT,                                  -- the export folder, if files were written
                   new INTEGER NOT NULL DEFAULT 0, changed INTEGER NOT NULL DEFAULT 0,
                   deleted INTEGER NOT NULL DEFAULT 0, blobs_added INTEGER NOT NULL DEFAULT 0,
                   reached_end INTEGER NOT NULL DEFAULT 0, settings TEXT NOT NULL);
                   -- settings: Settings::run_record(), the redacted subset
```

`payload` is the message map exactly as `result.json` carries it: Desktop's
keys, our extension keys, **no `_p`** (it is never rendered from here). `media`
links the message to the blob by `(family, file_id)`; photo ids and document
ids are separate namespaces on Telegram and must not share a key. The `file` /
`photo` path strings inside the payload are informational.

**No batching, no transactions.** Every statement autocommits. The file is
opened asking for WAL with `synchronous = NORMAL`, so a commit is a WAL append
without an fsync and costs tens of microseconds; 6,643 messages are well under
a second of database time. WAL is a request, not a fact: `output_dir` is free
text and can be a network share, where SQLite refuses or should not use WAL,
so the store reads back what `PRAGMA journal_mode` actually returned and logs
it when it is not `wal` (the run then pays an fsync per statement, which is
slower, not wrong). `PRAGMA busy_timeout = 5000` is set because the window
and the `tgx` CLI are two processes on one file, the same shape `CLAUDE.md`
records for the session store. Autocommit is deliberate: the engine's
`close_all` (`engine/finish.rs`) is synchronous, has eight arguments and nine
call sites, and `Drop` cannot await. A batch that had to be committed on every
exit would either make all of that async or lose its tail on a cancel. With
autocommit there is nothing to lose and nothing for an exit path to remember.

### Re-reading a stored message: `merge_payload`

When a message already in the database is read again, the fresh conversion
wins for **every key except the four path-bearing ones**: `file`, `photo`,
`thumbnail`, `stripped_thumbnail`. Those stay as first stored, because the
folder export assigns names per folder and per run (`photo_7@…` in one export
is `photo_6@…` in the next after a deletion) and a run-dependent path is not a
change to the message. Everything else, text, reactions, edits, poll results
and any key a converter fix adds or corrects, replaces the stored value and
the previous payload goes to `versions`.

Consequence to write in `ROADMAP.md`: changing an export setting that alters
the converter's output (the size limit, `link_previews`, `own_names`) produces
a version row for every re-read message that it affects. That is a true
record of what changed, and it is only paid on a full pass.

`merge_payload(stored: &Map, fresh: &Map) -> Map` is pure, lives in
`tgx-archive` (it needs `tgx_format::order::ordered` and nothing else), and is
what the archive leg drives.

### What one chat's run does when Database is on

```
files   = settings.export_html || settings.export_json
db_only = settings.export_db && !files
mode    = if !db_only || settings.reread_all || no messages stored for this chat { "full" } else { "window" }
start   = if mode == "window" { store.window_start(chat_id, settings.reread_window) } else { 0 }
```

1. Upsert the chat row and one topic row per listed topic (General included).
2. Read from `start` with the existing resume loop. For every message:
   - build the payload exactly as today (in `db_only` mode the folder's
     `NameBook` still runs so the metadata keys are the same, only nothing is
     written to disk);
   - if the message is **not** in the database: insert, and remember its
     download jobs, **one per `(family, file_id)` per run** — a second new
     message carrying the same file id gets a link row and no job;
   - if it **is**: `merge_payload`, and a version row if anything changed;
   - record the id as seen.
3. When the walk ended normally (`Ok(None)`): mark every stored id above
   `start` that was not seen and is not yet marked. A cancelled or stalled walk
   marks nothing, because it cannot tell unseen from unreached. A message
   Telegram returns again is unmarked.
4. Media: in a `files` run the pool writes the folder as today, and afterwards
   each file that landed is copied into `blobs` unless its `(family, file_id)`
   is already there. In a `db_only` run the pool runs into a scratch folder
   **per topic**, `<output_dir>/.tgx-scratch/<chat_id>/<topic_id>/`, only for
   ids not already in `blobs`, and the folder is removed the moment its
   ingest finishes. Per topic because the media pass is per sink and
   `NameBook` is per folder: two topics each name their first photo
   `photos/photo_1@…`, and one directory for the chat would have the second
   overwrite the first before ingest read it. Stripped thumbnails are not
   stored (an HTML-only inline placeholder).
5. Topic routing for the database **ignores `split_topics`.** The folder
   export routes everything to General when the switch is off
   (`engine.rs`, the `if split { self.route(..) } else { GENERAL_TOPIC_ID }`
   line); the database records the topic the message really belongs to, so
   the same chat exported split and unsplit files each message under one
   topic id. Non-forum chats are all General.
6. Scheduled messages go into their own table (`scheduled`) in every run
   that fetched them; a database-only run has no `scheduled.json` to write
   them to.
7. Roster rows, the run row, and the transcript lines.

**On "reached the end".** `Ok(None)` from the iterator is the only normal
exit of the read loop and it is what "the walk ended normally" means here. It
is Telegram saying there is nothing further, which is the only signal of an
end that exists; Telegram's `total()` routinely disagrees with the number of
messages a walk returns (deleted and service messages count differently), so
the count is **not** used to decide whether deletions may be marked. What
must never mark is a walk that stopped on a cancel, a stall or an error.

## 3. Steps

### Step 1 — the crate, the pin, the layering rule

1. Create `crates/tgx-archive/Cargo.toml`:
   ```toml
   [package]
   name = "tgx-archive"
   version.workspace = true
   edition.workspace = true
   rust-version.workspace = true
   license.workspace = true
   description = "The database output: one SQLite file of every message, version, participant and media blob an export has seen. No Telegram types."

   [dependencies]
   tgx-format = { path = "../tgx-format" }
   # Pinned exactly, not by caret. grammers-session already links this exact
   # version for its session file; a second version would carry its own
   # libsql-ffi and the sqlite3_* symbols would clash at link time. Bump it
   # deliberately, on its own commit, in step with grammers' pin, with the
   # parity legs green either side.
   libsql = { version = "=0.9.30", default-features = false, features = ["core"] }
   serde_json = { workspace = true }
   thiserror = { workspace = true }
   log = { workspace = true }

   [dev-dependencies]
   tokio = { version = "1", features = ["rt", "macros"] }
   ```
   Add `"crates/tgx-archive"` to `members` in the root `Cargo.toml` after
   `tgx-media`. Add the dependency to `crates/tgx-tg/Cargo.toml` and
   `crates/tgx-parity/Cargo.toml`. `tgx-parity` also needs
   `tokio = { version = "1", features = ["rt"] }` for the archive leg's
   current-thread runtime; it has no tokio today.
2. `crates/tgx-parity/tests/layering.rs`: add three tests.
   - `tgx_archive_does_not_depend_on_grammers`, modelled on
     `tgx_html_does_not_depend_on_grammers`, with a message saying the store
     must be replayable by the harness from recorded data.
   - `tgx_archive_does_not_depend_on_tgx_tg`, modelled on
     `tgx_parity_does_not_depend_on_tgx_tg`: the same replayability argument,
     and it is the other half of the rule.
   - `tgx_app_depends_only_on_tgx_ui_and_tgx_tg`: every dependency name
     starting with `tgx-` in `tgx-app/Cargo.toml` is one of those two.
     `CLAUDE.md` has stated this rule since the GPUI days and nothing
     enforced it; this feature is the first that could tempt a `tgx-archive`
     line into the window's manifest, and it must not (see Step 5.1).
3. `CLAUDE.md` Architecture table: a row for `tgx-archive` between
   `tgx-media` and `tgx-tg`.

Checkpoint: `save.bat test` green, nothing else changes.

### Step 2 — the store and `merge_payload`

`crates/tgx-archive/src/lib.rs` (`pub mod store; pub mod merge; pub use store::*; pub use merge::merge_payload;`),
`crates/tgx-archive/src/store.rs`, `crates/tgx-archive/src/merge.rs`.

All store methods are `async`; libsql's API is async and the engine already
runs on tokio. Open with `libsql::Builder::new_local(path).build().await?`
then `db.connect()?`, then `PRAGMA busy_timeout = 5000; PRAGMA synchronous = NORMAL;`
and `PRAGMA journal_mode = WAL` **as a query**, keeping its result on the
store (`pub fn journal_mode(&self) -> &str`) so the engine can log a mode that
is not `wal`. Create the schema if `meta` is missing; refuse a `schema_version` other than 1
with `Error::Schema { found, wanted }`. Add a compile-time check that the
store can cross threads:

```rust
#[cfg(test)]
fn _assert_send_sync() { fn f<T: Send + Sync>() {} f::<Store>(); }
// libsql::Connection is `Arc<dyn Conn + Send + Sync>` in 0.9.30 (src/connection.rs),
// so this holds today; the check is there so a bump that changes it says so at compile time.
```

```rust
pub const FILE_NAME: &str = "telegram.sqlite";
pub const SCHEMA_VERSION: i64 = 1;
/// A blob is read into memory and handed to libsql as one `Value::Blob`, so a
/// file costs about twice its size in RAM while it is stored. 256 MB keeps
/// that under a gigabyte; a larger file is linked in `media` with no blob and
/// named in the transcript. The 20 MB default size limit never reaches this.
pub const MAX_BLOB: i64 = 256 * 1024 * 1024;

pub struct Store { db: libsql::Database, conn: libsql::Connection, path: PathBuf }
// `db` is kept alive alongside the connection rather than dropped after `connect()`;
// do not rely on the connection outliving the database it came from.

#[derive(Debug, thiserror::Error)]
pub enum Error { Sql(#[from] libsql::Error), Json(#[from] serde_json::Error), Io(#[from] std::io::Error),
                 Schema { found: i64, wanted: i64 }, TooLarge { family: String, file_id: i64, size: i64 } }

pub enum Merge { Inserted, Unchanged, Changed { previous_version: i64 } }

pub struct StoredMessage { pub id: i64, pub topic_id: i64, pub payload: Map<String, Value>,
                           pub version: i64, pub deleted_seen: Option<i64> }

impl Store {
    pub async fn open(path: &Path) -> Result<Self, Error>;               // creates the file and schema if absent
    pub fn path(&self) -> &Path;

    pub async fn upsert_chat(&self, id: i64, title: &str, kind: &str, now: i64) -> Result<(), Error>;
    pub async fn upsert_topic(&self, chat_id: i64, id: i64, title: &str, head: &Map<String, Value>, now: i64) -> Result<(), Error>;

    pub async fn message_count(&self, chat_id: i64) -> Result<(i64 /*all*/, i64 /*not marked deleted*/), Error>;
    pub async fn window_start(&self, chat_id: i64, keep: usize) -> Result<i64, Error>;
        // SELECT id FROM messages WHERE chat_id=? ORDER BY id DESC LIMIT 1 OFFSET ?keep  -> 0 if no row
    pub async fn get(&self, chat_id: i64, id: i64) -> Result<Option<StoredMessage>, Error>;
    pub async fn merge(&self, chat_id: i64, id: i64, topic_id: i64, payload: &Map<String, Value>, now: i64) -> Result<Merge, Error>;
        // absent -> INSERT (version 1) -> Inserted. The columns come from the payload:
        //   `type` as is; `date_unixtime` is a *string* in Desktop's JSON ("1734...") and is
        //   parsed to i64 (0 if absent or unparsable); `from_id`, else `actor_id`, else NULL.
        // present and equal (compare the parsed Maps, not the text) -> UPDATE last_seen, deleted_seen = NULL -> Unchanged
        // present and different -> INSERT old into versions; UPDATE payload, columns, version+1, last_seen, deleted_seen = NULL -> Changed
        // (a message Telegram returns is not deleted, whatever an earlier run concluded)
        // in both present cases also UPDATE topic_id: a forum exported with split_topics
        // off routes everything to General, and the next split run must put it back
    pub async fn mark_unseen_deleted(&self, chat_id: i64, above_id: i64, seen: &HashSet<i64>, now: i64) -> Result<usize, Error>;
        // SELECT id WHERE chat_id=? AND id>? AND deleted_seen IS NULL; UPDATE those not in `seen`, one by one. No giant IN list.
    pub async fn messages_of(&self, chat_id: i64, topic_id: i64) -> Result<Vec<StoredMessage>, Error>;   // ORDER BY id

    pub async fn has_blob(&self, family: &str, file_id: i64) -> Result<bool, Error>;
    pub async fn put_blob(&self, family: &str, file_id: i64, kind: &str, mime: &str, file_name: Option<&str>, bytes: &[u8], now: i64) -> Result<(), Error>;
        // INSERT OR IGNORE; Err(TooLarge) above MAX_BLOB, checked before the bytes are read by the caller
    pub async fn put_thumb(&self, family: &str, file_id: i64, bytes: &[u8]) -> Result<(), Error>;
    pub async fn link_media(&self, chat_id: i64, message_id: i64, role: &str, family: &str, file_id: i64, path: &str) -> Result<(), Error>;
        // INSERT OR REPLACE: a full pass links every job again, and the path may differ per run
    pub async fn blob(&self, family: &str, file_id: i64) -> Result<Option<Vec<u8>>, Error>;   // for tests and a later `tgx` command

    pub async fn upsert_participant(&self, chat_id: i64, peer: &str, info: &Value, now: i64) -> Result<(), Error>;
    pub async fn upsert_scheduled(&self, chat_id: i64, id: i64, payload: &Map<String, Value>, now: i64) -> Result<(), Error>;

    pub async fn start_run(&self, chat_id: i64, mode: &str, root: Option<&Path>, settings_record: &str, now: i64) -> Result<i64, Error>;
    pub async fn finish_run(&self, run_id: i64, new: usize, changed: usize, deleted: usize, blobs_added: usize, reached_end: bool, now: i64) -> Result<(), Error>;
}
```

Payload text is `serde_json::to_string(&Value::Object(map))`; key order is
preserved by the workspace's `preserve_order` feature. Blob parameters are
`libsql::Value::Blob(Vec<u8>)`.

`merge.rs`:

```rust
/// The keys the folder export assigns per folder and per run. They are kept
/// as first stored; every other key comes from the fresh conversion.
pub const PATH_KEYS: &[&str] = &["file", "photo", "thumbnail", "stripped_thumbnail"];

pub fn merge_payload(stored: &Map<String, Value>, fresh: &Map<String, Value>) -> Map<String, Value> {
    let mut out = fresh.clone();
    for k in PATH_KEYS {
        match stored.get(*k) { Some(v) => { out.insert((*k).into(), v.clone()); } None => { out.remove(*k); } }
    }
    tgx_format::order::ordered(&out)
}
```

Tests (`#[tokio::test]`, one temp file each, modelled on the `tmp()` helper in
`crates/tgx-tg/src/output.rs`):

- `a_new_message_is_inserted_and_an_identical_one_is_unchanged`
- `a_changed_message_keeps_its_previous_payload_in_versions`
- `marking_touches_only_unseen_ids_above_the_start` (ids 1..10, start 5, seen
  {6,7,9,10} → only 8 marked; 1..5 untouched)
- `a_marked_message_is_unmarked_when_telegram_returns_it_again` (mark 8, merge
  an equal payload for 8 → `deleted_seen` NULL, no version row; a marked id
  that is not merged stays marked)
- `window_start_is_zero_for_a_small_archive_and_the_501st_newest_otherwise`
- `two_chats_do_not_see_each_others_messages`
- `a_photo_and_a_document_with_the_same_id_are_two_blobs`
- `a_blob_is_stored_once_and_linked_from_every_message`
- `a_blob_over_the_ceiling_is_refused_not_truncated`
- `a_schema_version_this_build_does_not_know_is_refused`
- `merge_payload_keeps_the_stored_paths_and_takes_everything_else_fresh`
  (a fresh map with a different `photo` path and a new `edited` key → the
  result has the stored path and the new key; a fresh map that lost
  `stripped_thumbnail` while the stored one has it → kept)

### Step 3 — the archive leg

`crates/tgx-parity/src/archive_leg.rs`, `pub mod archive_leg;` in `lib.rs`.
The legs are named in three places in `src/main.rs`: the `use` line, the
`match leg`, and `USAGE` (plus the module doc's example block); add `archive`
to all of them. `save.bat` invokes the three legs in **two** labels,
`:parity` (around lines 357–363) and `:runparity` (around 629–633, the one
`test` and `build` call); add the fourth to both, the same shape.

**All four corpus topics go into one store** — they are four topics of one
chat and share the header's `id` — so the leg exercises topic routing, not
only round-tripping. Inside a
`tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(...)`:

1. For each topic: parse `result.json`; `header` = every key but `messages`;
   `chat_id` = `header["id"]`; the messages as maps. **The reference headers
   carry no `topic_id`** (they are `name`, `type`, `id`, and `id` is the same
   chat on all four), so take `topic_id` from the **first message's `id`**:
   the message that creates a topic has the topic's id (66, 15, 12 in the
   corpus) and General starts at 1, which is exactly the rule
   `tgx_media::topics::topic_id_for` applies. Assert the four are distinct.
   Assert the ids are strictly increasing in file order —
   `messages_of … ORDER BY id` depends on it, and a reference where it is
   false must fail loudly rather than reorder.
2. Open one store in a temp file. `upsert_chat` once (title `"corpus"`, type
   from the header — the reference headers name topics, not the chat), then
   `upsert_topic` per topic with the header's `name` and an empty head.
3. **Run A:** merge the first 60% of every topic.
4. **Run B:** for every message of every topic, `store.get` it first.
   - **Stored** (the first 60%): build a "fresh" map that is the reference
     message with each key in `PATH_KEYS` that it carries replaced by
     `"<other run>/" + value`, call `merge_payload(&stored.payload, &fresh)`,
     then `merge` the result. It must come back `Unchanged`.
   - **Not stored** (the rest): `merge` the reference message **as it is**.
     Do not perturb it and do not pass it through `merge_payload` with itself
     — that would keep the fake path, and the read-back could never match.
     It must come back `Inserted`.
   `versions` must stay empty, and no read-back payload may contain the
   substring `<other run>/`; anything else names the id and fails the topic.
5. Per topic: `messages_of(chat_id, topic_id)`, emit with
   `tgx_format::json::header_prelude(&header)`, `message_block` per payload
   joined by `",\n"`, `footer()`, and require **byte equality** with the
   reference `result.json` (reuse `crate::first_difference` /
   `differing_lines`, report like `json_leg::run`).

Add to `crates/tgx-parity/tests/corpus.rs`:

```rust
#[test]
fn a_two_run_archive_reads_back_desktops_bytes() {
    let Some(topics) = topics_or_skip() else { return };
    let failures = archive_leg::run(&topics).expect("running the archive leg");
    assert_eq!(failures, 0, "{failures} topics did not read back exactly");
}
```

Checkpoint: json/html/media at baseline, archive 4/4.

### Step 4 — settings, the format gate, the redacted run record

`crates/tgx-tg/src/config.rs`, in `Settings` next to `export_html` /
`export_json`:

```rust
/// Merge every message the run reads into `<output_dir>/telegram.sqlite`,
/// media bytes included. Accumulates across runs and keeps what Telegram
/// deletes. Off by default: it is a second copy of everything.
pub export_db: bool,                 // default false
/// A database-only run re-reads this many of the newest stored messages for
/// edits, reactions and deletions. 0 == only what is new.
pub reread_window: usize,            // default 500
/// A database-only run walks the whole history, marking older deletions.
pub reread_all: bool,                // default false
```

Every one must be read in engine code as `settings.export_db` etc. (a receiver
literally ending in `settings`), or `crates/tgx-tg/tests/settings_are_wired.rs`
fails.

Also in `config.rs`:

```rust
/// What a run records about its settings. **Never the whole struct**: it
/// starts with the api hash and the phone number, and the database sits in
/// `Exports/`, which is not ACL-restricted. Everything that shapes an export's
/// output is here; nothing that identifies the account is.
pub fn run_record(&self) -> String   // serde_json of a private struct RunRecord { export_html, export_json, export_db,
                                     //   media_kinds, size_limit_mb, download_media, link_previews, own_names,
                                     //   full_reactions, chat_metadata, invite_links, refresh_polls, scheduled_messages,
                                     //   member_roster, member_limit, split_topics, reread_window, reread_all }
```

with a test `a_run_record_carries_no_credential` that sets `api_hash` and
`phone` to sentinel strings and asserts neither appears in the output.

The window: `crates/tgx-app/src/shell/commands.rs::start_export` — the
"Nothing to write" guard becomes `!(export_html || export_json || export_db)`
with the message naming all three. `crates/tgx-app/src/settings_form.rs` gets
`reread_window: String`, synced and collected like `member_limit` with a new
`REREAD_RANGE: (i64, i64) = (0, 10_000)` through the existing `number()`
clamp. `crates/tgx-app/src/shell/settings.rs`: a **Database** checkbox under
Format with the hint "one file, telegram.sqlite, kept up to date across runs —
media inside"; under Export, `self.number_row(...)` (defined around line 282)
for "Re-read the last N messages (database-only runs)", and a checkbox bound
to `settings.reread_all`, "Re-read the whole history (database-only runs)".
Follow the existing rows; do not invent components.

### Step 5 — the engine

All in `crates/tgx-tg/src/engine.rs`, `engine/finish.rs`, `engine/payload.rs`,
`plan.rs`, `download.rs`, `error.rs`, plus the two callers
(`crates/tgx-app/src/actions/export.rs`, `crates/tgx-tg/src/bin/tgx.rs`).

1. **One store for the whole queue**, like the one Telegram connection, and
   **the window never names `tgx-archive`**: `tgx-app` depends on `tgx-ui`
   and `tgx-tg` only (`CLAUDE.md`, and the new layering test). So the
   exporter opens the store itself. Keep `ChatExporter::new` as it is (tests
   use it, and it stays synchronous) and add
   ```rust
   /// `new`, plus the database when `settings.export_db`: creates
   /// `output_dir` if it is missing, opens `<output_dir>/telegram.sqlite`,
   /// and sweeps `<output_dir>/.tgx-scratch` (a crash mid-media-pass leaves
   /// it behind). No progress sink: the exporter is built once per queue,
   /// before any chat's progress closure exists. A journal mode other than
   /// WAL goes to `log::warn!` here and into the per-chat "database:" line.
   pub async fn open(client: &'a Client, settings: &'a Settings, session: Arc<SqliteSession>)
       -> Result<Self, ExportError>
   ```
   with a private `store: Option<Arc<tgx_archive::Store>>` field. **`Arc`,
   not a bare `Store`:** `run` needs the store while calling
   `self.payload(..)`, which takes `&mut self`, so it cannot hold
   `self.store.as_ref()` across that call. It clones the `Arc` into a local
   at the top (`let db = self.store.clone().filter(|_| self.settings.export_db);`)
   and the borrow of `self` ends there. Both callers
   (`actions/export.rs`, `bin/tgx.rs`) switch from `new` to `open` and treat
   an error exactly as they treat a `Session::connect` failure today
   (`Event::Failed`, then `Finished { stopped: true }`; the CLI returns it).
   Neither caller mentions a store, a path or the crate. `error.rs`:
   `ExportError::Archive(#[from] tgx_archive::Error)`, whose `Display` maps
   `tgx_archive::Error::Sql(libsql::Error::SqliteFailure(code, _))` with
   `code & 0xff` of 5 (`SQLITE_BUSY`) or 6 (`SQLITE_LOCKED`) to
   `the database is in use by another program — close the other TelegramExporter or tgx, or the viewer holding telegram.sqlite, and retry`
   and everything else to `database: {inner}`. Without the mapping a second
   process touching the file mid-export ends the chat with a raw
   `SQLite failure:` string in the transcript, past the five-second
   `busy_timeout`. Unit-test the mapping with a constructed error.
2. **The folder is optional.** Both callers call `unique_dir` only when
   `files` (HTML or JSON on); otherwise `run` gets `None`:
   `pub async fn run(&mut self, chat, peer, topics, root: Option<&Path>, progress, cancel)`.
   Inside `run`, gate on `root`: `participants.json`, `scheduled.json`, every
   `Output::new`, the folder media pass, the index. `TopicSink.output` becomes
   `Option<Output>` and `TopicSink` gains `root: PathBuf` of its own — the
   media pass reads the folder from `sink.output.root` today (`engine.rs`,
   the `media_order` loop) and `close_all`'s prune check compares
   `sink.output.root != root` (`finish.rs`); both move to `sink.root`. In a
   database-only run `sink.root` is the topic's scratch folder. `close_all`
   skips a `None` output (nothing to close or prune) and, for a database-only
   run, takes the per-topic counts from a `HashMap<i64, usize>` the read loop
   keeps. **`close_all` stays synchronous
   and its signature otherwise unchanged**; there is no batch to commit.
   `ExportResult.root` becomes `Option<PathBuf>`; `Event::ChatDone.root` too;
   `Queue::finished` takes `Option<PathBuf>` and the row's open-folder
   affordance is absent when it is `None`.
3. **Mode and start.** After the count is known:
   ```rust
   let files = self.settings.export_html || self.settings.export_json;
   let db: Option<Arc<Store>> = self.store.clone().filter(|_| self.settings.export_db);   // owned; see 5.1
   let (stored_all, stored_live) = match &db { Some(s) => s.message_count(chat.id).await?, None => (0, 0) };
   let db_only = db.is_some() && !files;
   let mode = read_mode(files, self.settings.export_db, self.settings.reread_all, stored_all);   // "full" | "window"
   let start = if mode == "window" { db.unwrap().window_start(chat.id, self.settings.reread_window).await? } else { 0 };
   ```
   `read_mode` is a pure `fn` so it can be tested. `offset_id = start as i32`
   for the first `iter_messages` — `offset_id` is `i32` in the loop and
   `window_start` returns `i64`; cast once, at that line, and nowhere else.
   The `resuming from message {offset_id} ({done} written so far)` line at
   the top of `'resume` fires on the first iteration whenever `offset_id != 0`;
   guard it with `done > 0` so a sync does not open with "resuming … 0 written
   so far" beside its own "syncing from" line. In `window` mode do **not** send `Progress::Total`
   (the count is not this run's denominator; the row shows a counter with no
   bar, as an uncounted chat does) and send `Progress::Messages { total: 0, .. }`.
   Log lines:
   - `database: <path> — 6,643 archived for this chat (12 deleted on Telegram, kept)`,
     with ` — journal mode <mode>, expect slower writes` appended when
     `store.journal_mode()` is not `wal`
   - `database: syncing from #6120 (the last 500 archived) for edits and deletions` / `database: re-reading the whole history` / nothing in a normal full run.
   `run_id = db.start_run(chat.id, mode, root, &self.settings.run_record(), now)`
   and `upsert_chat` go **right after the count**, before the roster block —
   the roster's `upsert_participant` calls (step 5) need the chat row to
   exist and the roster is fetched before the sinks are built.
   `upsert_topic` per listed topic goes where the sinks are built, with the
   same `head` map `Output::new` receives. In `window` mode set
   `result.expected = 0`: Telegram's count is not this run's denominator, and
   leaving it in place would make a cancelled sync of twelve messages read as
   "INCOMPLETE — Telegram counted 6,643, 12 came through".
4. **Per message**, after the existing `payload` call and independent of
   `sink.output.add` (which stays as it is for a `files` run):
   ```rust
   if let Some(store) = &db {
       seen.insert(id);
       // The message's real topic, whatever `split_topics` says. `route`
       // computes `topic_id_for` and then substitutes General for an unlisted
       // topic; split that first half out as `fn topic_of(&msg) -> i64` in
       // engine/peers.rs and use it here (General for a non-forum chat).
       let db_topic = if chat.is_forum { self.topic_of(&msg) } else { GENERAL_TOPIC_ID };
       let body = strip_p(&payload);                       // engine/payload.rs; `_p` removed, as output.rs does
       let merged = match store.get(chat.id, id).await? {
           None => body,
           Some(old) => tgx_archive::merge_payload(&old.payload, &body),
       };
       match store.merge(chat.id, id, db_topic, &merged, now).await? {
           Merge::Inserted => {
               result.db_new += 1;
               // `before` is sink.jobs.len() taken before `payload()` ran, as the
               // detail line already does; these are this message's pendings.
               for pending in &sink.jobs[before..] {
                   let job = &pending.job;
                   let wanted = job.file_id != 0
                       && queued_ids.insert((job.family, job.file_id))
                       && !store.has_blob(job.family, job.file_id).await?;
                   if wanted { db_wanted.insert(job.dest.clone()); } else { link_only.push(job.clone()); }
               }
           }
           Merge::Changed { .. } => result.db_changed += 1,
           Merge::Unchanged => {}
       }
   }
   ```
   `db_wanted: HashSet<String>` (job destinations) and `link_only:
   Vec<DownloadJob>` are per chat. The pool needs `PendingDownload` (the
   `Media` handle travels with the job), so nothing is cloned into a second
   pending list: in a `db_only` run, when the media pass comes,
   `sink.jobs` is **partitioned** with `drain(..)` into the pendings whose
   `dest` is in `db_wanted` (they go to the scratch pool) and the rest (they
   are dropped; their links were already queued). In a `files` run
   `sink.jobs` goes to the folder pool whole, untouched.
   `queued_ids: HashSet<(&'static str, i64)>` is per chat run: the second
   new message in the same run that carries a file id already queued gets a
   link row and no download. This filter decides only what the **database**
   does; the folder export's jobs are not filtered. In a `db_only` run the
   message still goes through `payload()` with the topic's `NameBook` so the
   metadata keys are identical to a folder export.
5. **After the walk**, only when the loop left via `Ok(None)`:
   `result.reached_end = true; result.db_deleted = store.mark_unseen_deleted(chat.id, start, &seen, now).await?`.
   Roster (this one runs **before** the read, inside the existing
   `member_roster` block, since that is where the roster is in scope): when
   it produced members, `upsert_participant(chat.id, member["id"], member,
   now)` for each — `member["id"]` is the `"user123"`-style key `enrich.rs`
   builds. Scheduled: where `fetch_scheduled` runs (after the walk), the rows
   it already builds for `scheduled.json` go through `upsert_scheduled` one by
   one, whether or not there is a folder. Log at the end:
   `database: +12 new, 3 changed, 1 deleted on Telegram (kept) — 6,655 archived`.
6. **Media into the database.** `plan::DownloadJob` gains
   `family: &'static str` (`"photo"` for a `MessageMediaPhoto`, `"document"`
   otherwise), `file_id: i64`, `kind: &'static str`, `mime_type: String`,
   `file_name: Option<String>`, `role: &'static str` (`"photo"` when the
   payload key written was `photo`, else `"file"`) — all available in
   `plan::plan` from `MediaFacts` (`id`, `kind`, `mime_type`, `file_name`).
   `download::run_all` takes the `Vec<PendingDownload>` **by value**, so
   clone the plain jobs before handing the pending ones over:
   `let jobs_run: Vec<DownloadJob> = jobs.iter().map(|p| p.job.clone()).collect();`
   (`DownloadJob` is `Clone`; the only payload it carries is a stripped
   thumbnail's ~180 bytes). After each folder's pool finishes in a `files`
   run, and for the scratch folder in a `db_only` run:
   ```rust
   for job in &jobs_run {
       if job.file_id == 0 || job.inline_bytes.is_some() { continue; }
       if !job.already_saved && !tally.missing.iter().any(|m| m.path == job.dest) && !store.has_blob(job.family, job.file_id).await? {
           if job.size > tgx_archive::MAX_BLOB {
               progress(Progress::Log(format!("database: {} is over the {} MB blob ceiling — linked, bytes left in the folder", job.dest, tgx_archive::MAX_BLOB / (1024 * 1024))));
           } else {
               let bytes = std::fs::read(dir.join(&job.dest))?;          // the pool validated it is non-empty
               store.put_blob(job.family, job.file_id, job.kind, &job.mime_type, job.file_name.as_deref(), &bytes, now).await?;
               result.db_blobs += 1;
               if let Some(t) = &job.thumb_dest { if let Ok(b) = std::fs::read(dir.join(t)) { store.put_thumb(job.family, job.file_id, &b).await?; } }
           }
       }
       store.link_media(chat.id, job.message_id, job.role, job.family, job.file_id, &job.dest).await?;
   }
   for job in &link_only { store.link_media(chat.id, job.message_id, job.role, job.family, job.file_id, &job.dest).await?; }
   ```
   In a `files` run `jobs_run` covers every job of the folder, new message
   or not, which is intended: a stored message whose bytes the database
   never had gets them the first time a folder export downloads them.
   `already_saved` jobs and `link_only` jobs link and read nothing. One blob
   is in memory at a time: read, `put_blob`, drop, next; there is no
   transaction holding them. In a `db_only` run the scratch dir is
   `<output_dir>/.tgx-scratch/<chat_id>/<topic_id>/` (`sink.root`), created
   before the pool and removed **immediately after ingest, before the cancel
   check that follows the batch** — `download::run_all` already
   lets in-flight jobs finish on cancel, so there is no exit between the pool
   returning and the removal. `ChatExporter::open` sweeps whatever a crash left. The
   pool call itself is unchanged (`download::run_all` with the same
   `Refresh`, concurrency, reporter and cancel).
7. **Result and report.** `ExportResult` gains `db_new`, `db_changed`,
   `db_deleted`, `db_blobs: usize`, `db_archived: usize`, `reached_end: bool`,
   `sync: bool` (true for a `window` run). `complete()` becomes
   `if self.sync { self.reached_end } else { self.expected == 0 || self.messages as i64 >= self.expected }`
   — a sync has no comparable count, so reaching the end is its only
   definition of whole, and a cancelled sync must not read as complete just
   because `expected` was zeroed. Keep the two existing `complete` tests and
   add `a_sync_is_complete_only_when_it_reached_the_end`. In
   `report_result`, a sync that is not complete warns
   `{title}: sync INCOMPLETE — stopped before reaching the newest message`
   instead of the counted form. **`Event::ChatDone` gains `sync: bool`**,
   and on it `Queue::finished` leaves `expected` alone and
   `shell/events.rs` skips `set_count` — otherwise a window run's twelve
   messages become the chat's size in the list and an "INCOMPLETE" reading
   in the queue (`queue.rs:185`, `events.rs:166`). Add a `shell/tests.rs`
   case: a `ChatDone { sync: true, messages: 12, .. }` after a `ChatTotal`
   of 6,643 leaves the list at 6,643 and the row's expected untouched.
   `actions/export.rs::report_result` adds one line when the database was on:
   `{title}: database +{new} new, {changed} changed, {deleted} deleted on Telegram (kept), {blobs} files stored — {archived} archived`.
8. **CLI** (`bin/tgx.rs`): `tgx export [--db] [--db-only] [--full] <title>`
   (`--db` sets `export_db`, `--db-only` also clears `export_html` and
   `export_json`, `--full` sets `reread_all`); update `USAGE`. No other
   command.
9. **Detail lines** (`describe`): when the database is on, prefix `new`,
   `same` or `changed` so `RUST_LOG=debug` shows what a re-read did.

Synthetic tests in `crates/tgx-tg/tests/` (fixtures in `tests/fixtures.rs`,
which is a plain file, not a crate: each new test file needs its own
`mod fixtures;` and `use fixtures::*;` lines the way `tests/wire.rs` has them;
`fixtures.rs` carries `#![allow(dead_code)]` for exactly this reason):

- `the_database_topic_ignores_split_topics` (a forum reply with a
  `forum_topic` header routes to its top id for the database even when the
  folder export would route it to General)
- `read_mode_is_full_unless_the_run_is_database_only_with_history`
- `a_file_id_already_queued_this_run_is_linked_not_downloaded` (drive the
  job filter in step 4 as a pure function over a `HashSet` and a `has_blob`
  closure; extract it so it can be tested)
- `an_unfinished_walk_marks_nothing` (the guard, not the network)
- `a_database_only_run_writes_no_folder` (with `root: None` and both file
  formats off, no directory is created under a temp output dir — drive the
  gate, not the network)
- `a_run_with_the_database_off_creates_no_database` (`ChatExporter::open`
  opens the store only under `export_db`; assert a temp output dir has no
  `telegram.sqlite` after it runs with the switch off — this needs no
  connection if the store-opening half is factored into
  `async fn database_for(settings: &Settings) -> Result<Option<Arc<Store>>, ExportError>`
  that `open` calls; test that)

### Step 6 — documentation

- `CLAUDE.md`: the architecture row; under Commands the new `tgx` flags; a
  paragraph "**The database is a fourth output, not a fourth writer.** It is
  fed by the same payload the JSON and HTML receive, minus `_p`, and never
  renders anything. What a run reads is decided by its formats: HTML or JSON
  on means the whole history and every unseen row marked deleted; Database
  alone means a sync from the trailing window. Every statement autocommits;
  there is no batch for an exit path to lose."; the oracle table gains
  `archive | reference result.json → two merges → read back → byte diff | 4/4`;
  the libsql pin under Dependencies; under paths and credentials, that
  `telegram.sqlite` is other people's conversation with media inside, lives
  in `Exports/` (gitignored), and the run row never carries the api hash or
  the phone.
- `README.md`: a short "Database" paragraph under Settings: one file beside
  the exports, media inside, keeps what Telegram deletes after you first saw
  it, tick only Database for a quick sync. The "won't bring back deleted
  messages" bullet becomes "won't bring back messages deleted before you first
  exported them".
- `ROADMAP.md`: status row; "seven crates" becomes eight and why; "Still
  open": a changed export setting produces a version row per affected
  re-read message on the next full pass; the four path keys are frozen as
  first stored; no command yet to write a blob back out to disk;
  `participants` keeps ex-members but `participants.json` does not.

## 4. Verification

After every step: `save.bat test` — fmt, clippy `-D warnings`, the suite, the
corpus legs against `reference/` with `TGX_REQUIRE_CORPUS=1`. The three
existing legs must read 4/4, 4/4, 830/836 throughout, **unchanged**, because
the folder export is untouched; from Step 3 the archive leg must read 4/4.

Targeted runs while working:

```powershell
cargo test -p tgx-archive
cargo test -p tgx-parity --test corpus a_two_run_archive_reads_back_desktops_bytes -- --nocapture
cargo test -p tgx-parity --test layering
cargo test -p tgx-tg --test settings_are_wired
cargo test -p tgx-tg a_run_record_carries_no_credential
cargo run -p tgx-parity -- archive reference
```

What each layer proves:

- The archive leg: two merges of a real export, the second through
  `merge_payload` with perturbed paths, read back as Desktop's exact bytes
  with no version rows.
- The store's own tests: versions, marking and unmarking, per-chat isolation,
  one blob per family and file id, the size ceiling, the schema guard.
- The synthetic engine tests: nothing is fetched twice within a run or across
  runs, the read mode is what the formats say, no folder and no database
  appear when their format is off.
- `save.bat build` stays under the 30 MB ceiling; expect no measurable change,
  since libsql was already linked.

The live check is **not yours to run** (the session key is a bearer credential
and the reference chat is other people's conversation). Leave these
instructions at the end of your report for Kosta:

1. With HTML, JSON and Database ticked, export the reference supergroup. The
   folder export must be byte-identical to one made with Database off
   (compare `result.json` of each topic with `fc /b`), and the transcript must
   show `database: +6,643 new` and the number of files stored.
2. Untick HTML and JSON, export again. The run must log `syncing from #…`,
   read only the window, download nothing, end with `+0 new` (or the true
   count) and no marked deletions unless something was really deleted, and
   the chat's count in the list must still read Telegram's number.
3. Delete one of your own test messages in the chat, sync again: the
   transcript reports `1 deleted on Telegram (kept)` and
   `SELECT id, deleted_seen FROM messages WHERE deleted_seen IS NOT NULL` shows
   it, with its payload intact.
4. `SELECT settings FROM runs LIMIT 1` contains no api hash and no phone.
5. `save.bat wire <export dir>` against the reference run must read exactly as
   before this feature: nothing in the folder export changed.
