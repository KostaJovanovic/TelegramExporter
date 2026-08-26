//! Cutting a standalone corpus out of the reference export.
//!
//! The reference lives on one drive on one machine. Every byte-exactness claim
//! in this project rests on it, which makes "`N:\` disappears" the risk that
//! quietly downgrades the whole harness from *verified* to *asserted* — the
//! legs would still run, find no reference, and exit.
//!
//! What actually needs to survive is small. An export is ~278 MB, but the
//! oracle is only the text half: `result.json` and `messages*.html`. Media is
//! the bulk and the media leg never reads a byte of it — it diffs *names*
//! against the tree recorded in the JSON. So the corpus copies the text and
//! leaves 270-odd MB behind.
//!
//! **Both halves are whole-topic.** Neither leg can take a slice: the HTML
//! writer's pagination and message joining are cumulative across a topic, so
//! page 3 is only reproducible if pages 1 and 2 were written first. Trimming
//! to "a few interesting messages" would produce a corpus that cannot be
//! compared to anything.
//!
//! Hence the survey printed at the end: it names what each topic covers and
//! what would be *lost* by leaving it out, so the choice of which whole topics
//! to keep is made on coverage rather than on size.
//!
//! ## This is real chat history
//!
//! A corpus is a verbatim copy of real messages from real people. `reference/`
//! is in `.gitignore` for that reason. Committing it is a deliberate act with
//! a consequence that git makes permanent, and it is not this tool's to make.

use anyhow::{Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The workspace's `reference/`, which is where the corpus test looks.
pub fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference")
}

const MANIFEST: &str = "MANIFEST.txt";

/// Copy the text half of every topic under `root` into `out`, then write a
/// manifest of what was copied.
pub fn build(root: &Path, topics: &[PathBuf], out: &Path) -> Result<u32> {
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;

    let mut entries: Vec<(String, u64, String)> = Vec::new();
    let mut surveys: Vec<(String, Survey)> = Vec::new();

    for topic in topics {
        let rel = topic.strip_prefix(root).unwrap_or(topic);
        let name = rel.to_string_lossy().replace('\\', "/");
        let dest = out.join(rel);
        std::fs::create_dir_all(&dest).with_context(|| format!("creating {}", dest.display()))?;

        let mut files = vec![topic.join("result.json")];
        files.extend(text_pages(topic));
        for src in &files {
            let file = src.file_name().context("a file with no name")?;
            let dst = dest.join(file);
            std::fs::copy(src, &dst)
                .with_context(|| format!("copying {} -> {}", src.display(), dst.display()))?;
            let bytes = std::fs::read(&dst)?;
            entries.push((
                format!("{name}/{}", file.to_string_lossy()),
                bytes.len() as u64,
                hex(&bytes),
            ));
        }

        surveys.push((name, survey(&topic.join("result.json"))?));
    }

    entries.sort();
    write_manifest(out, &entries)?;

    let total: u64 = entries.iter().map(|(_, n, _)| n).sum();
    println!(
        "\n{} files, {:.1} MB, in {}",
        entries.len(),
        total as f64 / 1_048_576.0,
        out.display()
    );
    report(&surveys);
    println!(
        "\n`reference/` is gitignored: it is verbatim chat history. Committing it \
         is a deliberate choice, and git keeps it for good."
    );
    Ok(0)
}

/// `messages.html`, `messages2.html`, … — the pages the HTML leg diffs.
fn text_pages(topic: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(topic)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("messages") && n.ends_with(".html"))
        })
        .collect();
    out.sort();
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn write_manifest(out: &Path, entries: &[(String, u64, String)]) -> Result<()> {
    let mut text = String::from(
        "# Cut by `tgx-parity corpus`. Every line is: sha256, bytes, path.\n\
         #\n\
         # The hashes are not paranoia about corruption. They are here because the\n\
         # tempting way to close a failing diff is to edit the reference, and a\n\
         # reference edited to agree with us is no longer an oracle — it is our own\n\
         # output wearing Desktop's name. The corpus test refuses a corpus that\n\
         # does not hash to this file.\n\n",
    );
    for (path, bytes, sha) in entries {
        text.push_str(&format!("{sha}  {bytes:>9}  {path}\n"));
    }
    std::fs::write(out.join(MANIFEST), text)
        .with_context(|| format!("writing {}", out.join(MANIFEST).display()))?;
    Ok(())
}

/// Read the manifest back and re-hash every file it names.
///
/// Returns the number of files checked. Errors if one is missing, changed, or
/// if the corpus holds a text file the manifest does not mention — a file that
/// arrived after the cut is exactly as suspicious as one that changed.
pub fn verify(dir: &Path) -> Result<usize> {
    let manifest = std::fs::read_to_string(dir.join(MANIFEST))
        .with_context(|| format!("reading {}", dir.join(MANIFEST).display()))?;

    let mut named = BTreeSet::new();
    let mut checked = 0usize;
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Split the two fixed fields off the front and take the whole rest of
        // the line as the path. Splitting on whitespace throughout would cut
        // `bitno pročitaj/result.json` in half — topic names are chat titles,
        // and chat titles have spaces in them.
        let (sha, rest) = line
            .split_once(char::is_whitespace)
            .context("a manifest line with no hash")?;
        let (bytes, path) = rest
            .trim_start()
            .split_once(char::is_whitespace)
            .context("a manifest line with no size")?;
        let path = path.trim_start();
        anyhow::ensure!(!path.is_empty(), "a manifest line with no path");
        let full = dir.join(path);
        let actual = std::fs::read(&full)
            .with_context(|| format!("{path} is in the manifest but not in the corpus"))?;
        anyhow::ensure!(
            actual.len().to_string() == bytes,
            "{path} is {} bytes, manifest says {bytes}",
            actual.len()
        );
        anyhow::ensure!(
            hex(&actual) == sha,
            "{path} does not match the manifest — a reference edited to agree \
             with us is not a reference"
        );
        named.insert(path.to_string());
        checked += 1;
    }

    for topic in crate::topic_folders(dir)? {
        let rel = topic.strip_prefix(dir).unwrap_or(&topic);
        let name = rel.to_string_lossy().replace('\\', "/");
        let mut files = vec![topic.join("result.json")];
        files.extend(text_pages(&topic));
        for f in files {
            let key = format!(
                "{name}/{}",
                f.file_name().unwrap_or_default().to_string_lossy()
            );
            anyhow::ensure!(!named.insert(key.clone()), "{key} is not in the manifest");
        }
    }
    Ok(checked)
}

// --- coverage ---------------------------------------------------------------

/// What shapes a topic contains — the thing that decides whether dropping it
/// costs anything.
#[derive(Default)]
pub struct Survey {
    pub messages: usize,
    pub tags: BTreeSet<String>,
}

fn survey(result_json: &Path) -> Result<Survey> {
    let raw = std::fs::read_to_string(result_json)
        .with_context(|| format!("reading {}", result_json.display()))?;
    let data: Value = serde_json::from_str(&raw)?;
    let messages = data
        .get("messages")
        .and_then(Value::as_array)
        .context("no messages array")?;

    let mut s = Survey {
        messages: messages.len(),
        ..Default::default()
    };
    for m in messages {
        let Some(map) = m.as_object() else { continue };
        let str_of = |k: &str| map.get(k).and_then(Value::as_str).unwrap_or("");

        if str_of("type") == "service" {
            s.tags.insert(format!("action:{}", str_of("action")));
        } else {
            let media = str_of("media_type");
            if !media.is_empty() {
                s.tags.insert(format!("media:{media}"));
            } else if map.contains_key("photo") {
                s.tags.insert("media:photo".into());
            } else if map.contains_key("file") {
                s.tags.insert("media:file".into());
            }
        }
        for key in [
            "reactions",
            "forwarded_from",
            "reply_to_message_id",
            "poll",
            "location_information",
            "contact_information",
            "edited",
            "via_bot",
            "inline_bot_buttons",
        ] {
            if map.contains_key(key) {
                s.tags.insert(key.to_string());
            }
        }
        if let Some(entities) = map.get("text_entities").and_then(Value::as_array) {
            for e in entities {
                if let Some(t) = e.get("type").and_then(Value::as_str) {
                    s.tags.insert(format!("entity:{t}"));
                }
            }
        }
    }
    Ok(s)
}

/// Print each topic's coverage, and — the useful column — what only it has.
fn report(surveys: &[(String, Survey)]) {
    let mut owners: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, s) in surveys {
        for tag in &s.tags {
            *owners.entry(tag.as_str()).or_default() += 1;
        }
    }
    println!("\ncoverage");
    for (name, s) in surveys {
        let only: Vec<&str> = s
            .tags
            .iter()
            .map(String::as_str)
            .filter(|t| owners.get(t) == Some(&1))
            .collect();
        println!("  {name}: {} messages, {} shapes", s.messages, s.tags.len());
        if only.is_empty() {
            println!("    nothing unique — every shape here is also somewhere else");
        } else {
            println!("    only here: {}", only.join(", "));
        }
    }
}
