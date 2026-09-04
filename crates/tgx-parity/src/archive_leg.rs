//! The archive leg: merge a real export into the store twice, read it back,
//! and byte-diff it against Desktop's own `result.json`.
//!
//! The other three legs prove that a *writer* reproduces Desktop. This one
//! proves that nothing is lost on the way through the database — which is a
//! different claim, and the only one that matters for an output whose whole
//! purpose is to still be right in a year.
//!
//! It tests four things at once:
//!
//! * **the round trip**: a payload put in comes back out as the same bytes,
//!   with key order intact — the `preserve_order` feature is load-bearing here
//!   and nothing else in the suite would notice if it were dropped;
//! * **that a re-read changes nothing**: run B merges every message run A
//!   already stored, through `merge_volatile`, and every one of them must come
//!   back `Unchanged` with the `versions` table still empty. A single
//!   `Changed` means a re-export would rewrite history it already had right;
//! * **that `order::ordered` really is a no-op on a real export**, because
//!   `merge_volatile` ends on it — the json leg asserts the same thing from the
//!   other side, and here it is what makes a merged message still emit exactly;
//! * **that topics of one chat do not collide.** All four corpus topics share
//!   one chat id and one store, which is the shape a forum export really has:
//!   `messages` is keyed `(chat_id, id)`, so a topic that leaked into another's
//!   read-back would show up as a diff rather than as nobody's problem.

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use tgx_archive::store::{Merge, Store};
use tgx_format::json;

/// How much of each topic run A stores. The rest arrives only in run B, so the
/// file exercises both paths through `merge` in the order a real second export
/// meets them: a long stretch of already-stored messages, then new ones.
const FIRST_PASS: f64 = 0.6;

struct Topic {
    name: String,
    /// Synthetic, and distinct per folder: the reference's headers carry no
    /// `topic_id`. It only has to separate the four in `messages_of`.
    topic_id: i64,
    /// Every header key but `messages`.
    header: Map<String, Value>,
    messages: Vec<Map<String, Value>>,
    /// The file, verbatim, with CRLF normalised — what we must reproduce.
    text: String,
}

pub fn run(topics: &[PathBuf]) -> Result<u32> {
    let parsed = topics
        .iter()
        .enumerate()
        .map(|(i, dir)| read(dir, i as i64 + 1))
        .collect::<Result<Vec<_>>>()?;

    let chat_id = match parsed.first() {
        Some(t) => t.header.get("id").and_then(Value::as_i64).unwrap_or(0),
        None => return Ok(0),
    };

    let path = temp_file();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building a runtime for the store")?;
    let failures = runtime.block_on(replay(&path, chat_id, &parsed))?;
    // Only on success: a failing run leaves the file behind to open.
    if failures == 0 {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    } else {
        println!("\n  the store is at {}", path.display());
    }
    Ok(failures)
}

async fn replay(path: &Path, chat_id: i64, topics: &[Topic]) -> Result<u32> {
    let store = Store::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let kind = topics
        .first()
        .and_then(|t| t.header.get("type").and_then(Value::as_str))
        .unwrap_or("public_supergroup");
    store.upsert_chat(chat_id, "corpus", kind, 1).await?;
    for t in topics {
        store
            .upsert_topic(chat_id, t.topic_id, &t.name, &head_of(&t.header), 1)
            .await?;
    }

    // --- run A: the first 60% of every topic --------------------------------
    store.begin_batch().await?;
    for t in topics {
        for m in &t.messages[..first_pass_len(t.messages.len())] {
            store
                .merge(chat_id, id_of(m)?, Some(t.topic_id), m, 100)
                .await?;
        }
    }
    store.commit_batch().await?;

    // --- run B: all of it, the way the engine does it -----------------------
    //
    // A message already stored goes through `merge_volatile` first, exactly as
    // `engine.rs` will: that is the call that decides whether a second export
    // is a no-op or a rewrite, and it is worth as much of this leg as the
    // round trip is.
    let mut failures = 0u32;
    store.begin_batch().await?;
    for t in topics {
        let boundary = first_pass_len(t.messages.len());
        for (i, m) in t.messages.iter().enumerate() {
            let id = id_of(m)?;
            let merged = match store.get(chat_id, id).await? {
                Some(old) => tgx_archive::merge_volatile(&old.payload, m),
                None => m.clone(),
            };
            let what = store
                .merge(chat_id, id, Some(t.topic_id), &merged, 200)
                .await?;
            let wanted = if i < boundary {
                Merge::Unchanged
            } else {
                Merge::Inserted
            };
            if what != wanted {
                failures += 1;
                println!(
                    "  {}: message {id} came back {what:?}, expected {wanted:?} — a re-read \
                     must not change a message Telegram did not change",
                    t.name
                );
                break;
            }
        }
    }
    store.commit_batch().await?;

    // Nothing was edited between the two runs, so nothing may have been
    // versioned. This is the assertion that catches a `merge_volatile` which
    // reorders keys: it would report `Changed` on every message, and the diff
    // below would still pass because the payload is the same map.
    let versions = store.version_count(chat_id).await?;
    if versions != 0 {
        failures += 1;
        println!(
            "  {versions} messages were versioned by a re-read that changed nothing — \
             merge_volatile is not stable"
        );
    }

    // --- read back and diff -------------------------------------------------
    for t in topics {
        println!("  {}", t.name);
        match check(&store, chat_id, t).await? {
            Report::Exact { messages, bytes } => {
                println!("    result.json: identical ({messages} messages, {bytes} bytes)");
            }
            Report::Differs {
                detail,
                differing,
                total,
            } => {
                failures += 1;
                println!("    result.json: {differing} differing lines of {total}");
                println!("      {detail}");
            }
            Report::OutOfOrder { detail } => {
                failures += 1;
                println!("    result.json: {detail}");
            }
        }
    }
    let n = topics.len() as u32;
    println!(
        "\n{} of {n} topics read back exactly",
        n.saturating_sub(failures)
    );
    Ok(failures)
}

enum Report {
    Exact {
        messages: usize,
        bytes: usize,
    },
    Differs {
        detail: String,
        differing: usize,
        total: usize,
    },
    OutOfOrder {
        detail: String,
    },
}

async fn check(store: &Store, chat_id: i64, t: &Topic) -> Result<Report> {
    let stored = store.messages_of(chat_id, t.topic_id).await?;

    // `messages_of` is `ORDER BY id`, and the diff below silently depends on
    // that being the order the file is in. It is, for a one-pass oldest-first
    // export — but when it is not, the failure is a first-difference thousands
    // of lines in that names nothing. So say so directly.
    let theirs: Vec<i64> = t.messages.iter().map(id_of).collect::<Result<Vec<_>>>()?;
    let ours: Vec<i64> = stored.iter().map(|m| m.id).collect();
    if theirs != ours {
        let at = theirs
            .iter()
            .zip(ours.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(theirs.len().min(ours.len()));
        return Ok(Report::OutOfOrder {
            detail: format!(
                "the file is not in message-id order ({} messages in the file, {} stored; \
                 first disagreement at position {at})",
                theirs.len(),
                ours.len()
            ),
        });
    }

    let mut ours = json::header_prelude(&t.header);
    for (i, m) in stored.iter().enumerate() {
        if i > 0 {
            ours.push_str(",\n");
        }
        ours.push_str(&json::message_block(&m.payload));
    }
    ours.push_str(&json::footer());

    if ours == t.text {
        return Ok(Report::Exact {
            messages: stored.len(),
            bytes: t.text.len(),
        });
    }
    Ok(Report::Differs {
        detail: crate::first_difference(&t.text, &ours)
            .unwrap_or_else(|| "differs only in trailing bytes".into()),
        differing: crate::differing_lines(&t.text, &ours),
        total: t.text.lines().count(),
    })
}

fn read(dir: &Path, topic_id: i64) -> Result<Topic> {
    let path = dir.join("result.json");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?
        .replace("\r\n", "\n");
    let parsed: Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let obj = parsed
        .as_object()
        .context("result.json is not an object")?
        .clone();

    let messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .context("no messages array")?
        .iter()
        .map(|m| m.as_object().cloned().context("a message is not an object"))
        .collect::<Result<Vec<_>>>()?;

    let mut header = Map::new();
    for (k, v) in obj.iter() {
        if k != "messages" {
            header.insert(k.clone(), v.clone());
        }
    }
    Ok(Topic {
        name: dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        topic_id,
        header,
        messages,
        text,
    })
}

/// Desktop's three header keys are the chat's, not the topic's; whatever else
/// a header carries is what `Output::new` was handed as `extra_head`.
fn head_of(header: &Map<String, Value>) -> Map<String, Value> {
    header
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "name" | "type" | "id"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn id_of(m: &Map<String, Value>) -> Result<i64> {
    m.get("id")
        .and_then(Value::as_i64)
        .context("a message has no id")
}

fn first_pass_len(total: usize) -> usize {
    (total as f64 * FIRST_PASS) as usize
}

fn temp_file() -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("tgx-parity-archive-{}.sqlite", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}
