//! The media leg: replay the reference's own message sequence through the
//! naming rules and compare against the filenames Desktop actually wrote.
//!
//! This is the check that found five bugs in the Python implementation, none of
//! which any unit test had caught — because `tools/diff_reference.py` replays
//! `result.json` through the *HTML writer* and so never touches the naming.
//!
//! **What it can and cannot see.** The reference JSON records the final path
//! but not Telegram's document id, so a file Desktop saved once and pointed
//! several messages at is invisible as a repeat until its path shows up twice.
//! Repeats are therefore detected from the reference's own paths and reported
//! separately rather than being scored as naming failures.

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tgx_media::names::{sanitize_extension, NameBook};

/// The ceiling, named rather than counted.
///
/// Custom emoji share `stickers/` and its collision suffixes but are referenced
/// from `document_id` rather than from a message's `file`, so a replay driven by
/// `result.json` cannot see them: `sticker (55).webp` is a custom emoji, which
/// is why the next message's sticker is `(56)`. A real export does reserve those
/// names — this harness simply has no way to know about them. One invisible
/// reservation shifts every later sticker in the topic by one, which is why the
/// six are consecutive.
///
/// This used to be `exact >= total - 6`, and a count is the wrong shape for it:
/// six *new* mismatches elsewhere would have scored exactly the same as these
/// six, and the leg exists to catch that class. Naming them means any seventh
/// mismatch, or a different sixth, fails.
const KNOWN_UNMATCHED: &[(&str, &str)] = &[
    ("7392", "stickers/sticker (56).webp"),
    ("7395", "stickers/sticker (57).webp"),
    ("7422", "stickers/sticker (58).webp"),
    ("7423", "stickers/sticker (59).webp"),
    ("7841", "stickers/sticker (60).webp"),
    ("7930", "stickers/sticker (61).webp"),
];

pub fn run(topics: &[PathBuf]) -> Result<u32> {
    let mut total = 0usize;
    let mut exact = 0usize;
    let mut repeats = 0usize;
    let mut wrong: Vec<Mismatch> = Vec::new();

    for topic in topics {
        let name = topic.file_name().unwrap_or_default().to_string_lossy();
        let r = check(topic)?;
        println!(
            "  {name}: {}/{} exact ({} repeats)",
            r.exact, r.total, r.repeats
        );
        total += r.total;
        exact += r.exact;
        repeats += r.repeats;
        wrong.extend(r.wrong);
    }

    if !wrong.is_empty() {
        println!("\nfirst mismatches:");
        for w in wrong.iter().take(15) {
            println!("  {}", w.describe());
        }
        if wrong.len() > 15 {
            println!("  ... and {} more", wrong.len() - 15);
        }
    }
    println!("\n{exact} of {total} filenames reproduced exactly ({repeats} repeats)");

    // A run that compared nothing is not a run that agreed about everything.
    // Pointed at an empty directory this used to compute 0 >= 0 and report the
    // known ceiling, so the harness's own failure read as the harness passing.
    if total == 0 {
        println!("NO filenames were compared — the corpus is empty or unreadable");
        return Ok(1);
    }

    let unexplained = unexplained(&wrong);
    if unexplained.is_empty() {
        println!(
            "at the known ceiling: the {} unmatched are custom emoji, invisible to a JSON replay",
            wrong.len()
        );
        return Ok(0);
    }
    println!(
        "\n{} mismatch(es) are NOT the known custom-emoji cascade — this is a regression:",
        unexplained.len()
    );
    for w in unexplained.iter().take(15) {
        println!("  {}", w.describe());
    }
    Ok(1)
}

/// Mismatches that the custom-emoji exception does not account for.
fn unexplained(wrong: &[Mismatch]) -> Vec<&Mismatch> {
    wrong
        .iter()
        .filter(|w| {
            !KNOWN_UNMATCHED
                .iter()
                .any(|(id, want)| *id == w.id && *want == w.want)
        })
        .collect()
}

struct Mismatch {
    id: String,
    want: String,
    got: String,
}

impl Mismatch {
    fn describe(&self) -> String {
        format!("{}: desktop {:?}, ours {:?}", self.id, self.want, self.got)
    }
}

#[derive(Default)]
struct Report {
    total: usize,
    exact: usize,
    repeats: usize,
    wrong: Vec<Mismatch>,
}

/// Strip Desktop's `chats/chat_<id>/topic_<n>/` prefix. Our layout is flat, so
/// only the last two components are comparable.
fn tail(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2..].join("/")
    } else {
        path.to_string()
    }
}

/// `2025-12-18T16:17:52` -> `18-12-2025_16-17-52`.
fn stamp(date: &str) -> String {
    let (d, t) = match date.split_once('T') {
        Some(x) => x,
        None => return String::new(),
    };
    let ymd: Vec<&str> = d.split('-').collect();
    if ymd.len() != 3 {
        return String::new();
    }
    format!("{}-{}-{}_{}", ymd[2], ymd[1], ymd[0], t.replace(':', "-"))
}

/// Which `_LAYOUT` kind a reference message describes.
fn kind_of(m: &serde_json::Map<String, Value>) -> Option<&'static str> {
    if m.contains_key("photo") {
        return Some("photos");
    }
    if !m.contains_key("file") {
        return None;
    }
    // The folder follows the file's shape, not its media_type — see
    // `tgx_media::names::kind_for`.
    Some(tgx_media::names::kind_for(
        m.get("media_type").and_then(Value::as_str).unwrap_or(""),
        m.get("mime_type").and_then(Value::as_str).unwrap_or(""),
    ))
}

fn check(topic: &Path) -> Result<Report> {
    let raw = std::fs::read_to_string(topic.join("result.json"))
        .with_context(|| format!("reading {}/result.json", topic.display()))?;
    let data: Value = serde_json::from_str(&raw)?;
    let messages = data
        .get("messages")
        .and_then(Value::as_array)
        .context("no messages array")?;

    let mut book = NameBook::new();
    let mut r = Report::default();
    // Reference path -> the path we produced for it the first time we saw it.
    let mut seen: HashMap<String, String> = HashMap::new();

    for msg in messages {
        let Some(m) = msg.as_object() else { continue };
        let Some(kind) = kind_of(m) else { continue };

        let recorded = m
            .get("photo")
            .or_else(|| m.get("file"))
            .and_then(Value::as_str)
            .unwrap_or("");
        // A placeholder still consumed a name, but Desktop wrote no path for
        // it, so there is nothing to compare against — it is counted only by
        // its effect on the numbering of everything after it.
        let placeholder = recorded.starts_with("(File");

        let given = m.get("file_name").and_then(Value::as_str);
        let stamp = m
            .get("date")
            .and_then(Value::as_str)
            .map(stamp)
            .unwrap_or_default();
        let ext_hint = given.map(sanitize_extension).unwrap_or_default();

        let name = book.reserve_name(kind, given, &stamp, &ext_hint);

        if placeholder {
            continue;
        }
        let want = tail(recorded);

        // Desktop saves a file once and points every later message at it. The
        // reference's own paths are the only signal we have for that.
        if let Some(previous) = seen.get(&want) {
            r.repeats += 1;
            r.total += 1;
            if *previous == want {
                r.exact += 1;
            }
            continue;
        }

        let (subdir, _) = tgx_media::names::layout(kind);
        let got = book.claim(subdir, &name);

        r.total += 1;
        if got == want {
            r.exact += 1;
            seen.insert(want, got);
        } else {
            r.wrong.push(Mismatch {
                id: m.get("id").map(|v| v.to_string()).unwrap_or_default(),
                want: want.clone(),
                got: got.clone(),
            });
            seen.insert(want.clone(), got);
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mismatch(id: &str, want: &str) -> Mismatch {
        Mismatch {
            id: id.into(),
            want: want.into(),
            got: "stickers/sticker (1).webp".into(),
        }
    }

    #[test]
    fn the_known_custom_emoji_cascade_is_explained() {
        let wrong: Vec<Mismatch> = KNOWN_UNMATCHED
            .iter()
            .map(|(id, want)| mismatch(id, want))
            .collect();
        assert!(unexplained(&wrong).is_empty());
    }

    #[test]
    fn a_seventh_mismatch_is_not_explained() {
        // The count-based ceiling could not tell these apart: six known plus one
        // new still left `exact` one short, and one known replaced by one new
        // left it exactly at the ceiling and passed.
        let mut wrong: Vec<Mismatch> = KNOWN_UNMATCHED
            .iter()
            .map(|(id, want)| mismatch(id, want))
            .collect();
        wrong.push(mismatch("9999", "video_files/clip.mp4"));
        assert_eq!(unexplained(&wrong).len(), 1);
    }

    #[test]
    fn a_known_id_with_a_different_path_is_not_explained() {
        let wrong = vec![mismatch("7392", "photos/photo_1@01-01-2025_00-00-00.jpg")];
        assert_eq!(unexplained(&wrong).len(), 1);
    }
}
