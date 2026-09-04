//! The database output: one SQLite file that accumulates across runs.
//!
//! The folder export is a fresh, complete pass into a new directory every
//! time, byte for byte in Telegram Desktop's format. A message deleted on
//! Telegram between two exports is simply absent from the second one, and
//! nothing ties two exports of the same chat together. This is the other half:
//! every message a run reads is merged into `telegram.sqlite` beside the
//! exports, with its media bytes, and what Telegram stops returning stays here
//! marked with the date it was noticed missing.
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
