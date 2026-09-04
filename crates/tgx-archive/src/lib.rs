//! The database output: **one SQLite file per chat**, accumulating across runs.
//!
//! The folder export is a fresh, complete pass into a new directory every
//! time, byte for byte in Telegram Desktop's format. A message deleted on
//! Telegram between two exports is simply absent from the second one, and
//! nothing ties two exports of the same chat together. This is the other half:
//! every message a run reads is merged into that chat's `<title>.sqlite`, with
//! its media bytes, and what Telegram stops returning stays there marked with
//! the date it was noticed missing.
//!
//! One file per chat rather than one for the account: a database is the thing
//! most likely to be copied to a drive or handed to somebody, and a file that
//! silently carried every other conversation exported into the same folder is
//! not what an export of one chat should be. Every table is still keyed by
//! `chat_id`, so the schema is unchanged and a file holding two chats reads
//! correctly — see [`Store::holds_another_chat`], which is how the exporter
//! keeps two from sharing one by accident.
//!
//! **No Telegram types.** Like [`tgx_html`], this layer takes serialised maps —
//! the message payload exactly as `result.json` carries it, minus the
//! presentation-only `_p` key — and knows nothing of the wire. That is what
//! lets the parity harness replay a recorded export through the store and diff
//! what comes back out, which is the only way a store can be proved to keep
//! what it was given. A `grammers-*` dependency here would end that, and
//! `tgx-parity`'s `layering.rs` fails the build if one appears.
//!
//! It renders nothing. Both outputs still come from the one map that
//! `tgx-tg/src/output.rs` builds; this is a fourth *output*, not a fourth
//! writer.

pub mod store;

pub use store::{merge_volatile, Error, Merge, Store, StoredMessage, FILE_EXT, SCHEMA_VERSION};
