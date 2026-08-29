//! Reading a tree, pairing what is in it, and classifying one difference.
//!
//! A difference is classified before it is counted: a rename is not a
//! mismatch, a gap the export stated is not a dangling path, and a value that
//! moved between two runs is not a field we never wrote.

use super::*;

/// Two non-empty names for the same person: somebody renamed their profile.
///
/// An empty name on either side is **not** a rename — that is the 206-people
/// bug this field is in [`MUST_MATCH`] to catch, and it must keep failing.
pub(super) fn is_rename(x: Option<&Value>, y: Option<&Value>) -> bool {
    let (Some(a), Some(b)) = (x.and_then(Value::as_str), y.and_then(Value::as_str)) else {
        return false;
    };
    !a.trim().is_empty() && !b.trim().is_empty()
}

/// Which person a display-name disagreement belongs to.
///
/// A rename is uniform across one peer's messages; anything that is not is a
/// different bug wearing the same clothes, and bucketing on the id is what tells
/// them apart. `forwarded_from` carries no id of its own, so it buckets on the
/// name the reference gave.
pub(super) fn peer_key(reference: &Map<String, Value>, field: &str) -> String {
    let id = match field {
        "from" => reference.get("from_id"),
        "actor" => reference.get("actor_id"),
        _ => None,
    };
    if let Some(id) = id.and_then(Value::as_str) {
        return id.to_string();
    }
    match reference.get(field).and_then(Value::as_str) {
        Some(name) => format!("{field}={name}"),
        None => format!("{field}=?"),
    }
}

/// Paths the export itself declared it could not save.
///
/// A path occupies a whole line and starts in column zero; an **indented** line
/// is the reason for the path above it (`tgx_tg::download::write_missing`).
/// Filtering on the indent rather than on a separator inside the line is what
/// keeps the split unambiguous when a Telegram filename contains the separator.
pub(super) fn stated_gaps(topic: &Path) -> BTreeSet<String> {
    let Ok(body) = std::fs::read_to_string(topic.join("missing_media.txt")) else {
        return BTreeSet::new();
    };
    body.lines()
        .filter(|l| !l.starts_with(char::is_whitespace))
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("These files are referenced"))
        .map(str::to_string)
        .collect()
}

/// What one export said happened to one media field.
///
/// Three states, and the leg used to have two. `skip_reason` returned `None`
/// for "the key is absent" *and* for "the key holds a path", and the example
/// line then rendered that `None` as the word `"downloaded"` — so a message
/// with no `photo` key at all was reported as a photo we had downloaded. That
/// is the wire leg inventing the other side's answer, in the one report whose
/// entire job is to say what the other side actually holds.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Decision<'a> {
    /// The key is not in the message.
    Absent,
    /// A path: the bytes were saved.
    Saved,
    /// One of Desktop's parenthesised notes, verbatim.
    Skipped(&'a str),
}

impl Decision<'_> {
    pub(super) fn is_skip(&self) -> bool {
        matches!(self, Decision::Skipped(_))
    }

    pub(super) fn describe(&self) -> String {
        match self {
            Decision::Absent => "absent".into(),
            Decision::Saved => "downloaded".into(),
            Decision::Skipped(why) => format!("{why:?}"),
        }
    }

    /// The class of decision, for grouping. Substring tests rather than equality
    /// against Desktop's exact sentences, matching `tgx_html::media`.
    pub(super) fn class(&self) -> &'static str {
        match self {
            Decision::Absent => "absent",
            Decision::Saved => "saved",
            Decision::Skipped(why) => {
                let lowered = why.to_ascii_lowercase();
                if lowered.contains("exceeds maximum size") {
                    "too_large"
                } else if lowered.contains("not included") {
                    "not_included"
                } else {
                    "unavailable"
                }
            }
        }
    }
}

pub(super) fn decision<'a>(m: &'a Map<String, Value>, key: &str) -> Decision<'a> {
    match m.get(key).and_then(Value::as_str) {
        None => Decision::Absent,
        Some(v) if v.starts_with('(') => Decision::Skipped(v),
        Some(_) => Decision::Saved,
    }
}

pub(super) fn brief(v: Option<&Value>) -> String {
    let Some(v) = v else { return "absent".into() };
    let s = v.to_string();
    if s.chars().count() > 60 {
        let cut: String = s.chars().take(57).collect();
        format!("{cut}…")
    } else {
        s
    }
}

pub(super) fn messages_by_id(topic: &Path) -> Result<BTreeMap<i64, Map<String, Value>>> {
    let path = topic.join("result.json");
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let data: Value = serde_json::from_str(&raw)?;
    let messages = data
        .get("messages")
        .and_then(Value::as_array)
        .with_context(|| format!("{} has no messages array", path.display()))?;
    let mut out = BTreeMap::new();
    for m in messages {
        let Some(map) = m.as_object() else { continue };
        let Some(id) = map.get("id").and_then(Value::as_i64) else {
            continue;
        };
        out.insert(id, map.clone());
    }
    Ok(out)
}

/// Topic folders keyed by the `name` **inside** `result.json`, not by folder.
///
/// A forum export is one folder per topic, but the two sides do not name those
/// folders alike: Desktop uses the bare topic title, we prefix the topic id, so
/// `ćaskanje` and `0001 - ćaskanje` are one topic under two names. Keying on the
/// folder made every real comparison degenerate — against the reference run all
/// eight folders came back "only in ours" and "we did not export this topic",
/// so the leg reported eight failures having compared no message at all. That
/// reads like a total export failure rather than a harness that cannot pair its
/// inputs. The title inside the file is the thing both sides agree on.
pub(super) fn topics_by_name(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let folders = crate::topic_folders(root)?;
    let mut out = BTreeMap::new();
    for f in &folders {
        let folder = f
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = std::fs::read_to_string(f.join("result.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.get("name")?.as_str().map(str::to_string))
            .unwrap_or(folder);
        out.insert(name, f.clone());
    }
    Ok(out)
}
