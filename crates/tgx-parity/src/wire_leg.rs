//! The wire leg: our own live export against the reference run.
//!
//! The other three legs all start from Desktop's `result.json` and work
//! forward. That pins everything downstream of the wire byte for byte — and
//! proves nothing whatsoever about `convert.rs`, which turns a TL object into
//! that JSON in the first place. There is no recorded TL message to replay, so
//! the only oracle for the wire is a second export of the same chat.
//!
//! This diffs two export trees:
//!
//! ```text
//! tgx-parity wire "Exports\UA KOLAB TELEGRAM" "N:\telegram export\UA KOLAB"
//! ```
//!
//! Three questions, in the order they matter:
//!
//! 1. **Did we get every message?** A missing id is a paging bug and makes
//!    every other number meaningless.
//! 2. **Did we make the same size decision?** Whether an attachment was
//!    downloaded, skipped as too large, or excluded by settings is a decision
//!    the exporter makes at runtime from the size limit — the one piece of
//!    behaviour that no replay can reach, because the reference records only
//!    the outcome.
//! 3. **Does each message say the same thing?** Compared field by field, so a
//!    difference names itself instead of arriving as a wall of JSON.
//!
//! Two exports of the same chat are never *identical* and should not be
//! expected to be: they were taken at different times, so edits, deletions,
//! new reactions and view counts all legitimately differ. Fields that drift
//! for those reasons are reported separately from fields that must not.

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Fields that must agree for the same id in two exports of one chat.
///
/// Deliberately not "every key". `edited`, `reactions`, `views` and `forwards`
/// change with time and their disagreeing says nothing about the converter.
const MUST_MATCH: &[&str] = &[
    "type",
    "date",
    "date_unixtime",
    "from",
    "from_id",
    "actor",
    "actor_id",
    "action",
    "text",
    "media_type",
    "mime_type",
    "file_name",
    "sticker_emoji",
    "duration_seconds",
    "width",
    "height",
    "reply_to_message_id",
    "forwarded_from",
    "photo_file_size",
    "file_size",
];

/// Fields that differ between two runs for honest reasons, reported apart.
const MAY_DRIFT: &[&str] = &[
    "edited",
    "edited_unixtime",
    "reactions",
    "views",
    "forwards",
];

pub fn run(ours: &Path, theirs: &Path) -> Result<u32> {
    let mut mine = topics_by_name(ours)?;
    let ref_ = topics_by_name(theirs)?;
    anyhow::ensure!(!mine.is_empty(), "no topics under {}", ours.display());
    anyhow::ensure!(!ref_.is_empty(), "no topics under {}", theirs.display());

    // A non-forum chat is one folder named after the chat, and a chat can be
    // renamed between two runs. With exactly one topic a side there is nothing
    // to be ambiguous about, so pair them and say so rather than reporting two
    // half-empty topics that are plainly the same one.
    if mine.len() == 1 && ref_.len() == 1 {
        let (a, b) = (mine.keys().next().unwrap(), ref_.keys().next().unwrap());
        if a != b {
            println!("  pairing {a:?} with {b:?} — one topic a side");
            let only = mine.remove(&a.clone()).expect("just read the key");
            mine.insert(b.clone(), only);
        }
    }

    let mut failures = 0u32;
    let names: BTreeSet<&String> = mine.keys().chain(ref_.keys()).collect();

    let mut totals = Totals::default();
    for name in names {
        println!("  {name}");
        match (mine.get(name), ref_.get(name)) {
            (Some(a), Some(b)) => {
                let r = compare(a, b)?;
                r.print();
                totals.add(&r);
                if !r.clean() {
                    failures += 1;
                }
            }
            (Some(_), None) => {
                failures += 1;
                println!("    only in ours — the reference has no such topic");
            }
            (None, Some(_)) => {
                failures += 1;
                println!("    only in the reference — we did not export this topic");
            }
            (None, None) => unreachable!("the name came from one of the two"),
        }
    }

    println!();
    println!(
        "ids:          {} matched, {} missing from ours, {} extra",
        totals.shared, totals.missing, totals.extra
    );
    println!(
        "size skips:   {} decisions, {} agreed",
        totals.skips, totals.skips_agreed
    );
    // Broken out by field because the roadmap's headline number — 1,786 —
    // counts `file` placeholders alone, and a summary that only showed the
    // total would look like it disagreed with it.
    for (key, (n, agreed)) in &totals.skips_by_key {
        println!("              {key}: {n}, {agreed} agreed");
    }
    println!(
        "fields:       {} messages compared, {} disagreed on a field that must match",
        totals.shared, totals.field_mismatches
    );
    if totals.drifted > 0 {
        println!(
            "              {} differ only in {} — two runs, two points in time",
            totals.drifted,
            MAY_DRIFT.join("/")
        );
    }
    if !totals.absent.is_empty() {
        let n: usize = totals.absent.values().sum();
        println!("absent:       {n} fields the reference writes and we never do");
        for (key, n) in &totals.absent {
            println!("              {key}: {n}");
        }
    }
    if !totals.dangling.is_empty() {
        let n: usize = totals.dangling.values().sum();
        println!("dangling:     {n} media paths in our JSON with no file behind them");
        for (key, n) in &totals.dangling {
            println!("              {key}: {n}");
        }
    }
    Ok(failures)
}

#[derive(Default)]
struct Totals {
    shared: usize,
    missing: usize,
    extra: usize,
    skips: usize,
    skips_agreed: usize,
    field_mismatches: usize,
    drifted: usize,
    skips_by_key: BTreeMap<String, (usize, usize)>,
    absent: BTreeMap<String, usize>,
    dangling: BTreeMap<String, usize>,
}

impl Totals {
    fn add(&mut self, r: &Report) {
        for (key, (n, agreed)) in &r.skips_by_key {
            let slot = self.skips_by_key.entry(key.clone()).or_default();
            slot.0 += n;
            slot.1 += agreed;
        }
        for (key, n) in &r.absent {
            *self.absent.entry(key.clone()).or_default() += n;
        }
        for (key, n) in &r.dangling {
            *self.dangling.entry(key.clone()).or_default() += n;
        }
        self.shared += r.shared;
        self.missing += r.missing.len();
        self.extra += r.extra.len();
        self.skips += r.skips;
        self.skips_agreed += r.skips_agreed;
        self.field_mismatches += r.mismatched_messages;
        self.drifted += r.drifted;
    }
}

struct Report {
    shared: usize,
    missing: Vec<i64>,
    extra: Vec<i64>,
    skips: usize,
    skips_agreed: usize,
    skips_by_key: BTreeMap<String, (usize, usize)>,
    skip_examples: Vec<String>,
    mismatched_messages: usize,
    by_field: BTreeMap<String, usize>,
    examples: Vec<String>,
    drifted: usize,
    absent: BTreeMap<String, usize>,
    dangling: BTreeMap<String, usize>,
    dangling_examples: Vec<String>,
}

impl Report {
    fn clean(&self) -> bool {
        self.missing.is_empty()
            && self.extra.is_empty()
            && self.mismatched_messages == 0
            && self.skips == self.skips_agreed
            && self.absent.is_empty()
            && self.dangling.is_empty()
    }

    fn print(&self) {
        if self.clean() {
            println!(
                "    {} ids, all present; {} size decisions, all agreed",
                self.shared, self.skips
            );
            if self.drifted > 0 {
                println!("    {} differ only in time-varying fields", self.drifted);
            }
            return;
        }
        if !self.missing.is_empty() {
            println!(
                "    {} ids missing from ours: {}",
                self.missing.len(),
                head(&self.missing)
            );
        }
        if !self.extra.is_empty() {
            println!(
                "    {} ids we have and the reference does not: {}",
                self.extra.len(),
                head(&self.extra)
            );
        }
        if self.skips != self.skips_agreed {
            println!(
                "    {} of {} size decisions disagree",
                self.skips - self.skips_agreed,
                self.skips
            );
            for e in &self.skip_examples {
                println!("      {e}");
            }
        }
        if self.mismatched_messages > 0 {
            println!(
                "    {} of {} messages disagree on a field that must match",
                self.mismatched_messages, self.shared
            );
            for (field, n) in &self.by_field {
                println!("      {field}: {n}");
            }
            for e in &self.examples {
                println!("      {e}");
            }
        }
        if !self.absent.is_empty() {
            println!("    fields the reference writes and we never do:");
            for (field, n) in &self.absent {
                println!("      {field}: {n}");
            }
        }
        if !self.dangling.is_empty() {
            println!("    media we name in the JSON but did not put on disk:");
            for (field, n) in &self.dangling {
                println!("      {field}: {n}");
            }
            for e in &self.dangling_examples {
                println!("      {e}");
            }
        }
    }
}

fn head(ids: &[i64]) -> String {
    let shown: Vec<String> = ids.iter().take(10).map(i64::to_string).collect();
    if ids.len() > shown.len() {
        format!("{} …", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

fn compare(ours_root: &Path, theirs: &Path) -> Result<Report> {
    let a = messages_by_id(ours_root)?;
    let b = messages_by_id(theirs)?;

    let ids_a: BTreeSet<i64> = a.keys().copied().collect();
    let ids_b: BTreeSet<i64> = b.keys().copied().collect();

    let mut report = Report {
        shared: 0,
        missing: ids_b.difference(&ids_a).copied().collect(),
        extra: ids_a.difference(&ids_b).copied().collect(),
        skips: 0,
        skips_agreed: 0,
        skips_by_key: BTreeMap::new(),
        skip_examples: Vec::new(),
        mismatched_messages: 0,
        by_field: BTreeMap::new(),
        examples: Vec::new(),
        drifted: 0,
        absent: BTreeMap::new(),
        dangling: BTreeMap::new(),
        dangling_examples: Vec::new(),
    };

    for id in ids_a.intersection(&ids_b) {
        let (m, n) = (&a[id], &b[id]);
        report.shared += 1;

        // --- the size decision ---------------------------------------------
        // Desktop records a skip in the field the file would have occupied:
        // `"file": "(File exceeds maximum size…)"`. Two exports agree when
        // both skipped or both downloaded — the *path* differs by folder
        // history, so only the decision is compared, never the name.
        for key in ["file", "photo", "thumbnail"] {
            let (x, y) = (skip_reason(m, key), skip_reason(n, key));
            if x.is_some() || y.is_some() {
                report.skips += 1;
                let slot = report.skips_by_key.entry(key.to_string()).or_default();
                slot.0 += 1;
                if x == y {
                    report.skips_agreed += 1;
                    slot.1 += 1;
                } else if report.skip_examples.len() < 5 {
                    report.skip_examples.push(format!(
                        "{id} {key}: ours {:?}, reference {:?}",
                        x.unwrap_or("downloaded"),
                        y.unwrap_or("downloaded")
                    ));
                }
            }
        }

        // --- a path we wrote down but never wrote out -----------------------
        // Only ours is checked: the reference may legitimately be the text-only
        // corpus, whose media tree was never copied. A name in the JSON with no
        // file behind it is a broken image in the HTML and a lost attachment,
        // and no replay leg can see it — the three that diff names all diff
        // *names*, and a name matches whether or not the bytes arrived.
        for key in ["photo", "file", "thumbnail", "stripped_thumbnail"] {
            let Some(v) = m.get(key).and_then(Value::as_str) else {
                continue;
            };
            // Desktop's size-limit note occupies the field a path would.
            if v.starts_with('(') || ours_root.join(v).is_file() {
                continue;
            }
            *report.dangling.entry(key.to_string()).or_default() += 1;
            if report.dangling_examples.len() < 5 {
                report.dangling_examples.push(format!("{id} {key}: {v}"));
            }
        }

        // --- a field the reference writes and we never do -------------------
        // `reactions` sits in MAY_DRIFT because its *value* moves between two
        // runs. That licence must not extend to its absence: an exporter that
        // emits no reactions at all would otherwise score as honest drift,
        // which is exactly what happened — 963 messages carried reactions in
        // the reference, none in ours, and the leg called it "differ only in
        // time-varying fields".
        for key in n.keys() {
            if !m.contains_key(key) {
                *report.absent.entry(key.clone()).or_default() += 1;
            }
        }

        // --- the fields -----------------------------------------------------
        let mut bad: Vec<&str> = Vec::new();
        for field in MUST_MATCH {
            let (x, y) = (m.get(*field), n.get(*field));
            if x != y {
                bad.push(field);
            }
        }
        if bad.is_empty() {
            if MAY_DRIFT.iter().any(|f| m.get(*f) != n.get(*f)) {
                report.drifted += 1;
            }
            continue;
        }
        report.mismatched_messages += 1;
        for field in &bad {
            *report.by_field.entry((*field).to_string()).or_default() += 1;
        }
        if report.examples.len() < 5 {
            let field = bad[0];
            report.examples.push(format!(
                "{id} {field}: ours {}, reference {}",
                brief(m.get(field)),
                brief(n.get(field))
            ));
        }
    }
    Ok(report)
}

/// `Some(reason)` when this field holds one of Desktop's parenthesised
/// placeholders instead of a path.
fn skip_reason<'a>(m: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    let v = m.get(key)?.as_str()?;
    v.starts_with('(').then_some(v)
}

fn brief(v: Option<&Value>) -> String {
    let Some(v) = v else { return "absent".into() };
    let s = v.to_string();
    if s.chars().count() > 60 {
        let cut: String = s.chars().take(57).collect();
        format!("{cut}…")
    } else {
        s
    }
}

fn messages_by_id(topic: &Path) -> Result<BTreeMap<i64, Map<String, Value>>> {
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
fn topics_by_name(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    /// Write a one-topic export tree and hand back its root.
    fn tree(tag: &str, folder: &str, name: &str, messages: Value) -> PathBuf {
        let root = std::env::temp_dir().join(format!("tgx-wire-{tag}-{}", std::process::id()));
        let topic = root.join(folder);
        std::fs::create_dir_all(&topic).unwrap();
        let body = json!({"name": name, "type": "public_supergroup", "id": 1,
                          "messages": messages});
        std::fs::write(topic.join("result.json"), body.to_string()).unwrap();
        root
    }

    #[test]
    fn a_topic_is_paired_by_its_title_not_its_folder() {
        // Desktop writes `ćaskanje`; we write `0001 - ćaskanje`. Keyed on the
        // folder these never met, and the leg called a clean export eight
        // failures without comparing one message.
        let ours = tree("ours", "0001 - ćaskanje", "ćaskanje", json!([]));
        let theirs = tree("theirs", "ćaskanje", "ćaskanje", json!([]));
        let a = topics_by_name(&ours).unwrap();
        let b = topics_by_name(&theirs).unwrap();
        assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&ours);
        let _ = std::fs::remove_dir_all(&theirs);
    }

    #[test]
    fn a_field_we_never_write_is_not_scored_as_drift() {
        // `reactions` may drift in *value* between two runs. It may not go
        // missing: an exporter that emits none at all scored 963 messages as
        // "differ only in time-varying fields" and the leg stayed green.
        let msg = |extra: Value| {
            let mut m = map(json!({"id": 1, "type": "message", "text": ""}));
            for (k, v) in extra.as_object().unwrap() {
                m.insert(k.clone(), v.clone());
            }
            Value::Array(vec![Value::Object(m)])
        };
        let ours = tree("drift-ours", "t", "t", msg(json!({})));
        let theirs = tree(
            "drift-theirs",
            "t",
            "t",
            msg(json!({"reactions": [{"type": "emoji", "count": 1}]})),
        );
        let r = compare(&ours.join("t"), &theirs.join("t")).unwrap();
        assert_eq!(r.absent.get("reactions"), Some(&1), "absence went unseen");
        assert!(!r.clean(), "a field we never write must fail the leg");
        let _ = std::fs::remove_dir_all(&ours);
        let _ = std::fs::remove_dir_all(&theirs);
    }

    #[test]
    fn a_media_path_with_no_file_behind_it_is_caught() {
        // Every replay leg diffs *names*, and a name matches whether or not the
        // bytes arrived. 1,546 thumbnail paths in a live export pointed at
        // nothing and all three legs were green.
        let one = |v: Value| Value::Array(vec![v]);
        let ours = tree(
            "dangle-ours",
            "t",
            "t",
            one(json!({"id": 1, "type": "message", "thumbnail": "files/a.jpg_thumb.jpg"})),
        );
        let theirs = tree(
            "dangle-theirs",
            "t",
            "t",
            one(json!({"id": 1, "type": "message", "thumbnail": "files/a.jpg_thumb.jpg"})),
        );
        let r = compare(&ours.join("t"), &theirs.join("t")).unwrap();
        assert_eq!(r.dangling.get("thumbnail"), Some(&1));
        assert!(!r.clean());
        let _ = std::fs::remove_dir_all(&ours);
        let _ = std::fs::remove_dir_all(&theirs);
    }

    #[test]
    fn a_placeholder_is_a_skip_and_a_path_is_not() {
        let m = map(json!({
            "file": "(File exceeds maximum size. Change data exporting settings to download.)",
            "photo": "photos/photo_1@01-01-2020_00-00-00.jpg",
        }));
        assert!(skip_reason(&m, "file").is_some());
        assert_eq!(skip_reason(&m, "photo"), None);
        assert_eq!(skip_reason(&m, "thumbnail"), None);
    }

    #[test]
    fn time_varying_fields_are_not_in_the_must_match_set() {
        // Two exports of one chat are taken at different moments. A reaction
        // added in between is not a converter bug, and counting it as one
        // would bury the failures that are.
        for field in MAY_DRIFT {
            assert!(
                !MUST_MATCH.contains(field),
                "{field} cannot be both required and allowed to drift"
            );
        }
    }

    #[test]
    fn the_things_a_wire_bug_would_break_are_required() {
        for field in ["text", "date", "from_id", "media_type", "action", "type"] {
            assert!(MUST_MATCH.contains(&field), "{field} is not checked");
        }
    }

    #[test]
    fn brief_truncates_without_splitting_a_character() {
        let long = json!("ћ".repeat(200));
        let out = brief(Some(&long));
        assert!(out.chars().count() <= 61, "{out}");
        assert!(out.ends_with('…'));
    }
}
