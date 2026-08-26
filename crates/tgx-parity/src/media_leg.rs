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

pub fn run(topics: &[PathBuf]) -> Result<u32> {
    let mut total = 0usize;
    let mut exact = 0usize;
    let mut repeats = 0usize;
    let mut wrong: Vec<String> = Vec::new();

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
            println!("  {w}");
        }
        if wrong.len() > 15 {
            println!("  ... and {} more", wrong.len() - 15);
        }
    }
    println!("\n{exact} of {total} filenames reproduced exactly ({repeats} repeats)");

    // The ceiling is 830 of 836, and the six are not a defect. Custom emoji
    // share `stickers/` and its collision suffixes but are referenced from
    // `document_id` rather than from a message's `file`, so a replay driven by
    // `result.json` cannot see them: `sticker (55).webp` is a custom emoji,
    // which is why the next message's sticker is `(56)`. A real export does
    // reserve those names — this harness simply has no way to know about them.
    let expected_ceiling = total.saturating_sub(6);
    if exact >= expected_ceiling {
        println!(
            "at the known ceiling: the {} unmatched are custom emoji, invisible to a JSON replay",
            total - exact
        );
        return Ok(0);
    }
    println!("BELOW the known ceiling of {expected_ceiling}/{total} — this is a regression");
    Ok(1)
}

#[derive(Default)]
struct Report {
    total: usize,
    exact: usize,
    repeats: usize,
    wrong: Vec<String>,
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
            r.wrong.push(format!(
                "{}: desktop {:?}, ours {:?}",
                m.get("id").map(|v| v.to_string()).unwrap_or_default(),
                want,
                got
            ));
            seen.insert(want.clone(), got);
        }
    }
    Ok(r)
}
