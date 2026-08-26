//! The JSON leg: re-emit a reference `result.json` and byte-diff it.
//!
//! The Python original never tested this direction — its harness replayed
//! `result.json` through the *HTML* writer only, so the JSON emitter itself was
//! covered by unit tests that encoded what we believed Desktop did. This leg
//! checks the belief.
//!
//! It tests four things at once, independently of everything upstream:
//!
//! * indent width and the pretty-printing shape,
//! * raw-UTF-8 output and every escaping decision,
//! * the deliberate `reactions` over-indent,
//! * and, because a re-key is applied and asserted to be a no-op, that our
//!   `ORDER` table really is Desktop's own sequence.

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use tgx_format::{json, order};

pub fn run(topics: &[PathBuf]) -> Result<u32> {
    let mut failures = 0u32;
    for topic in topics {
        let name = topic.file_name().unwrap_or_default().to_string_lossy();
        println!("  {name}");
        match check(topic) {
            Ok(Report::Exact { messages, bytes }) => {
                println!("    result.json: identical ({messages} messages, {bytes} bytes)");
            }
            Ok(Report::Differs {
                detail,
                differing,
                total,
            }) => {
                failures += 1;
                println!("    result.json: {differing} differing lines of {total}");
                println!("      {detail}");
            }
            Ok(Report::OrderDrift { keys }) => {
                failures += 1;
                println!("    result.json: ORDER disagrees with the reference");
                for k in keys {
                    println!("      {k}");
                }
            }
            Err(e) => {
                failures += 1;
                println!("    result.json: {e:#}");
            }
        }
    }
    let n = topics.len() as u32;
    println!("\n{} of {n} topics reproduced exactly", n - failures);
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
    OrderDrift {
        keys: Vec<String>,
    },
}

fn check(topic: &Path) -> Result<Report> {
    let path = topic.join("result.json");
    let theirs =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    // Desktop writes LF. Reading as text on Windows does not translate, but be
    // explicit so a CRLF reference cannot masquerade as a formatting bug.
    let theirs = theirs.replace("\r\n", "\n");

    let parsed: Value =
        serde_json::from_str(&theirs).with_context(|| format!("parsing {}", path.display()))?;
    let obj = parsed.as_object().context("result.json is not an object")?;

    // --- the ORDER check ----------------------------------------------------
    // Re-keying each message through our table must be a no-op against a real
    // export. If it is not, our ORDER is wrong and every line after the first
    // offending key would diff.
    let messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .context("no messages array")?;
    let mut drift: Vec<String> = Vec::new();
    for m in messages {
        let Some(map) = m.as_object() else { continue };
        let before: Vec<&String> = map.keys().collect();
        let reordered = order::ordered(map);
        let after: Vec<&String> = reordered.keys().collect();
        if before != after {
            let id = map.get("id").map(|v| v.to_string()).unwrap_or_default();
            drift.push(format!(
                "message {id}: desktop {before:?} but ORDER gives {after:?}"
            ));
            if drift.len() >= 5 {
                break;
            }
        }
    }
    if !drift.is_empty() {
        return Ok(Report::OrderDrift { keys: drift });
    }

    // --- the emitter check --------------------------------------------------
    let mut header = Map::new();
    for (k, v) in obj.iter() {
        if k != "messages" {
            header.insert(k.clone(), v.clone());
        }
    }
    let mut ours = json::header_prelude(&header);
    for (i, m) in messages.iter().enumerate() {
        let map = m.as_object().context("a message is not an object")?;
        if i > 0 {
            ours.push_str(",\n");
        }
        ours.push_str(&json::message_block(map));
    }
    ours.push_str(&json::footer());

    if ours == theirs {
        return Ok(Report::Exact {
            messages: messages.len(),
            bytes: theirs.len(),
        });
    }
    Ok(Report::Differs {
        detail: crate::first_difference(&theirs, &ours)
            .unwrap_or_else(|| "differs only in trailing bytes".into()),
        differing: crate::differing_lines(&theirs, &ours),
        total: theirs.lines().count(),
    })
}
