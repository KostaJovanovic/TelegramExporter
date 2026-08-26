//! Media naming, folder layout and stripped thumbnails.
//!
//! No Telegram types anywhere: the caller fills in what it learned off the
//! wire, which keeps every rule here testable with no connection and is why
//! the naming can be replayed against a reference export.

pub mod jpeg_header;
pub mod names;
pub mod stripped;
pub mod topics;

pub use names::{
    layout, media_type, sanitize_extension, sanitize_filename, synth_prefix, NameBook,
};
pub use stripped::expand as expand_stripped;
pub use topics::{topic_dirname, topic_id_for, ReplyHeader, GENERAL_TITLE, GENERAL_TOPIC_ID};
