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
    let mine = topics_by_name(ours)?;
    let ref_ = topics_by_name(theirs)?;
    anyhow::ensure!(!mine.is_empty(), "no topics under {}", ours.display());
    anyhow::ensure!(!ref_.is_empty(), "no topics under {}", theirs.display());

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
}

impl Totals {
    fn add(&mut self, r: &Report) {
        for (key, (n, agreed)) in &r.skips_by_key {
            let slot = self.skips_by_key.entry(key.clone()).or_default();
            slot.0 += n;
            slot.1 += agreed;
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
}

impl Report {
    fn clean(&self) -> bool {
        self.missing.is_empty()
            && self.extra.is_empty()
            && self.mismatched_messages == 0
            && self.skips == self.skips_agreed
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

fn compare(ours: &Path, theirs: &Path) -> Result<Report> {
    let a = messages_by_id(ours)?;
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

/// Topic folders keyed by folder name, so two trees pair up by topic.
///
/// A forum export is one folder per topic and the names come from the topic
/// titles, so they line up across runs. When both sides hold exactly one topic
/// they are paired regardless of name — a non-forum chat is named after the
/// chat, and the two runs may have used different output roots.
fn topics_by_name(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let folders = crate::topic_folders(root)?;
    let mut out = BTreeMap::new();
    for f in &folders {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
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
