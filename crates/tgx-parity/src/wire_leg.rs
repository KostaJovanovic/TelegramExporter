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
//!
//! **This leg has been wrong three times**, which is why it now has tests of
//! its own. It paired topics by folder name and so compared nothing; it scored
//! 963 missing `reactions` as honest drift; and it printed `"downloaded"` for a
//! field that was not in the message at all. Each was invisible in exactly the
//! way the bugs it hunts are invisible — a harness that fails open reports the
//! same thing as a program that works.

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

/// Resolved *display names*, not identifiers.
///
/// These stay in [`MUST_MATCH`] — they are what caught 206 people exporting as
/// `""` — but a disagreement between two non-empty names is a person having
/// renamed their profile, not a converter fault. Counted per message it turns
/// one rename into a mismatch on every message that person ever sent, and since
/// the report prints five examples per bucket, a rename storm pushes a real
/// `text` or `media_type` failure off the page. So they are bucketed by peer
/// instead: a rename is uniform across one peer's messages, and an empty name
/// is not.
const DISPLAY_NAMES: &[&str] = &["from", "actor", "forwarded_from"];

/// Keys we write on purpose that Desktop's format has no place for.
///
/// Everything else we emit and the reference does not is a finding — that is
/// the mirror of `absent`, and it was missing entirely. Message #509's
/// link-preview `file`/`file_name`/`mime_type` belong to that class and no leg
/// could see them, for exactly the reason `absent` could not: a replay is
/// driven by the reference, so a key only *we* invent is a key it never asks
/// about.
const OURS_BY_DESIGN: &[&str] = &["stripped_thumbnail"];

pub fn run(ours: &Path, theirs: &Path) -> Result<u32> {
    let mut mine = topics_by_name(ours)?;
    let ref_ = topics_by_name(theirs)?;
    anyhow::ensure!(!mine.is_empty(), "no topics under {}", ours.display());
    anyhow::ensure!(!ref_.is_empty(), "no topics under {}", theirs.display());

    // A non-forum chat is one folder named after the chat, and a chat can be
    // renamed between two runs. With exactly one topic a side there is nothing
    // to be ambiguous about, so pair them and say so rather than reporting two
    // half-empty topics that are plainly the same one.
    //
    // **But only when they are the same chat.** Keyed on nothing but "there is
    // one of each", pointing the leg at the wrong directory produced a full
    // field-by-field comparison of two unrelated chats — thousands of
    // mismatches that read as a catastrophic converter regression rather than
    // as a typo in an argument. `result.json`'s own `id` is what settles it:
    // it survives a rename, and it is the same number on both sides of a real
    // pair.
    if mine.len() == 1 && ref_.len() == 1 {
        let (a, b) = (mine.keys().next().unwrap(), ref_.keys().next().unwrap());
        if a != b {
            let (ia, ib) = (chat_id(&mine[a]), chat_id(&ref_[b]));
            anyhow::ensure!(
                ia.is_some() && ia == ib,
                "{a:?} and {b:?} are the only topics but are different chats \
                 (ids {ia:?} and {ib:?}) — is one of the two paths wrong?"
            );
            println!("  pairing {a:?} with {b:?} — one topic a side, same chat id");
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
    // And by *which way* they disagreed. A uniform block — every disagreement
    // the same shape — is our settings differing from the reference run's, not
    // the converter being wrong; a scatter across shapes is the converter. The
    // leg cannot read either run's settings (no export records them), so the
    // best it can do is make the difference between the two readable.
    for (shape, n) in &totals.skip_shapes {
        println!("              {n} × {shape}");
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
    if totals.edited > 0 {
        println!(
            "              {} were edited between the runs, so their text is exempt",
            totals.edited
        );
    }
    if !totals.renames.is_empty() {
        let n: usize = totals.renames.values().map(|(_, _, n)| n).sum();
        println!(
            "              {n} differ only in display name ({} distinct people)",
            totals.renames.len()
        );
        for (peer, (a, b, n)) in totals.renames.iter().take(10) {
            println!("              {peer}: ours {a:?}, reference {b:?} ({n})");
        }
    }
    if !totals.absent.is_empty() {
        let n: usize = totals.absent.values().sum();
        println!("absent:       {n} fields the reference writes and we never do");
        for (key, n) in &totals.absent {
            println!("              {key}: {n}");
        }
    }
    if !totals.extra_fields.is_empty() {
        let n: usize = totals.extra_fields.values().sum();
        println!("extra:        {n} fields we write and the reference does not");
        for (key, n) in &totals.extra_fields {
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
    if !totals.stated.is_empty() {
        let n: usize = totals.stated.values().sum();
        println!("stated gaps:  {n} of those are named in missing_media.txt — not a defect");
        for (key, n) in &totals.stated {
            println!("              {key}: {n}");
        }
    }
    Ok(failures)
}

/// The chat id inside a topic's `result.json`.
fn chat_id(topic: &Path) -> Option<i64> {
    let raw = std::fs::read_to_string(topic.join("result.json")).ok()?;
    serde_json::from_str::<Value>(&raw)
        .ok()?
        .get("id")?
        .as_i64()
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
    edited: usize,
    skips_by_key: BTreeMap<String, (usize, usize)>,
    skip_shapes: BTreeMap<String, usize>,
    renames: BTreeMap<String, (String, String, usize)>,
    absent: BTreeMap<String, usize>,
    extra_fields: BTreeMap<String, usize>,
    dangling: BTreeMap<String, usize>,
    stated: BTreeMap<String, usize>,
}

impl Totals {
    fn add(&mut self, r: &Report) {
        for (key, (n, agreed)) in &r.skips_by_key {
            let slot = self.skips_by_key.entry(key.clone()).or_default();
            slot.0 += n;
            slot.1 += agreed;
        }
        for (key, n) in &r.skip_shapes {
            *self.skip_shapes.entry(key.clone()).or_default() += n;
        }
        for (key, n) in &r.absent {
            *self.absent.entry(key.clone()).or_default() += n;
        }
        for (key, n) in &r.extra_fields {
            *self.extra_fields.entry(key.clone()).or_default() += n;
        }
        for (key, n) in &r.dangling {
            *self.dangling.entry(key.clone()).or_default() += n;
        }
        for (key, n) in &r.stated {
            *self.stated.entry(key.clone()).or_default() += n;
        }
        for (peer, (a, b, n)) in &r.renames {
            let slot = self
                .renames
                .entry(peer.clone())
                .or_insert_with(|| (a.clone(), b.clone(), 0));
            slot.2 += n;
        }
        self.shared += r.shared;
        self.missing += r.missing.len();
        self.extra += r.extra.len();
        self.skips += r.skips;
        self.skips_agreed += r.skips_agreed;
        self.field_mismatches += r.mismatched_messages;
        self.drifted += r.drifted;
        self.edited += r.edited;
    }
}

#[derive(Default)]
struct Report {
    shared: usize,
    missing: Vec<i64>,
    extra: Vec<i64>,
    skips: usize,
    skips_agreed: usize,
    skips_by_key: BTreeMap<String, (usize, usize)>,
    skip_shapes: BTreeMap<String, usize>,
    skip_examples: Vec<String>,
    mismatched_messages: usize,
    by_field: BTreeMap<String, usize>,
    examples: Vec<String>,
    drifted: usize,
    edited: usize,
    /// Peer key → (our name, the reference's name, how many messages).
    renames: BTreeMap<String, (String, String, usize)>,
    absent: BTreeMap<String, usize>,
    extra_fields: BTreeMap<String, usize>,
    dangling: BTreeMap<String, usize>,
    dangling_examples: Vec<String>,
    /// Dangling paths the export itself names in `missing_media.txt`.
    stated: BTreeMap<String, usize>,
}

impl Report {
    fn clean(&self) -> bool {
        self.missing.is_empty()
            && self.extra.is_empty()
            && self.mismatched_messages == 0
            && self.skips == self.skips_agreed
            && self.absent.is_empty()
            && self.extra_fields.is_empty()
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
            if !self.stated.is_empty() {
                let n: usize = self.stated.values().sum();
                println!("    {n} media are missing and said so in missing_media.txt");
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
            for (shape, n) in &self.skip_shapes {
                println!("      {n} × {shape}");
            }
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
        if !self.renames.is_empty() {
            let n: usize = self.renames.values().map(|(_, _, n)| n).sum();
            println!(
                "    {n} differ only in display name ({} distinct people)",
                self.renames.len()
            );
        }
        if !self.absent.is_empty() {
            println!("    fields the reference writes and we never do:");
            for (field, n) in &self.absent {
                println!("      {field}: {n}");
            }
        }
        if !self.extra_fields.is_empty() {
            println!("    fields we write and the reference does not:");
            for (field, n) in &self.extra_fields {
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
        if !self.stated.is_empty() {
            let n: usize = self.stated.values().sum();
            println!("    (a further {n} are named in missing_media.txt — a stated gap)");
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
    let stated = stated_gaps(ours_root);

    let ids_a: BTreeSet<i64> = a.keys().copied().collect();
    let ids_b: BTreeSet<i64> = b.keys().copied().collect();

    let mut report = Report {
        missing: ids_b.difference(&ids_a).copied().collect(),
        extra: ids_a.difference(&ids_b).copied().collect(),
        ..Report::default()
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
            let (x, y) = (decision(m, key), decision(n, key));
            if x.is_skip() || y.is_skip() {
                report.skips += 1;
                let slot = report.skips_by_key.entry(key.to_string()).or_default();
                slot.0 += 1;
                if x == y {
                    report.skips_agreed += 1;
                    slot.1 += 1;
                } else {
                    // The *shape* of the disagreement, not the instance: "every
                    // one of the 62 is ours=saved / reference=too_large on
                    // `photo`" says "our size limit differed", which is not a
                    // defect. A scatter across shapes says the converter is.
                    *report
                        .skip_shapes
                        .entry(format!(
                            "{key}: ours {} / reference {}",
                            x.class(),
                            y.class()
                        ))
                        .or_default() += 1;
                    if report.skip_examples.len() < 5 {
                        report.skip_examples.push(format!(
                            "{id} {key}: ours {}, reference {}",
                            x.describe(),
                            y.describe()
                        ));
                    }
                }
            }
        }

        // --- a path we wrote down but never wrote out -----------------------
        // Only ours is checked: the reference may legitimately be the text-only
        // corpus, whose media tree was never copied. A name in the JSON with no
        // file behind it is a broken image in the HTML and a lost attachment,
        // and no replay leg can see it — the three that diff names all diff
        // *names*, and a name matches whether or not the bytes arrived.
        //
        // A path the export **named in `missing_media.txt`** is a different
        // thing and is counted apart. Filenames are decided before bytes are
        // fetched — that is the design, and it is where the speed comes from —
        // so a failed download must leave a reference to a file that is not
        // there. The archive's honesty comes from saying so, and scoring a
        // stated gap as a dangling reference would keep this leg red forever on
        // any run with one failed download.
        for key in ["photo", "file", "thumbnail", "stripped_thumbnail"] {
            let Some(v) = m.get(key).and_then(Value::as_str) else {
                continue;
            };
            // Desktop's size-limit note occupies the field a path would.
            if v.starts_with('(') || ours_root.join(v).is_file() {
                continue;
            }
            if stated.contains(v) {
                *report.stated.entry(key.to_string()).or_default() += 1;
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
        // --- and the mirror, which did not exist ----------------------------
        // A key only we write is exactly as invisible to a replay as a key only
        // the reference writes, and for the same reason.
        for key in m.keys() {
            if n.contains_key(key)
                || OURS_BY_DESIGN.contains(&key.as_str())
                // A reaction that arrived after the reference run was taken is
                // new activity, not an invented field.
                || MAY_DRIFT.contains(&key.as_str())
            {
                continue;
            }
            *report.extra_fields.entry(key.clone()).or_default() += 1;
        }

        // --- the fields -----------------------------------------------------
        // An edit is *why* text changes, and `edited` is the field that says an
        // edit happened. Requiring `text` while permitting `edited` to drift
        // asks the two runs to disagree about when a message was last changed
        // and agree about what it now says.
        let edited_differs = MAY_DRIFT
            .iter()
            .filter(|f| f.starts_with("edited"))
            .any(|f| m.get(*f) != n.get(*f));
        let mut was_edited = false;
        let mut bad: Vec<&str> = Vec::new();
        let mut renamed = false;

        for field in MUST_MATCH {
            let (x, y) = (m.get(*field), n.get(*field));
            if x == y {
                continue;
            }
            if *field == "text" && edited_differs {
                was_edited = true;
                continue;
            }
            if DISPLAY_NAMES.contains(field) && is_rename(x, y) {
                renamed = true;
                let key = peer_key(n, field);
                let slot = report.renames.entry(key).or_insert_with(|| {
                    (
                        x.and_then(Value::as_str).unwrap_or_default().to_string(),
                        y.and_then(Value::as_str).unwrap_or_default().to_string(),
                        0,
                    )
                });
                slot.2 += 1;
                continue;
            }
            bad.push(field);
        }
        if was_edited {
            report.edited += 1;
        }
        if bad.is_empty() {
            if !renamed && !was_edited && MAY_DRIFT.iter().any(|f| m.get(*f) != n.get(*f)) {
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

/// Two non-empty names for the same person: somebody renamed their profile.
///
/// An empty name on either side is **not** a rename — that is the 206-people
/// bug this field is in [`MUST_MATCH`] to catch, and it must keep failing.
fn is_rename(x: Option<&Value>, y: Option<&Value>) -> bool {
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
fn peer_key(reference: &Map<String, Value>, field: &str) -> String {
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
fn stated_gaps(topic: &Path) -> BTreeSet<String> {
    let Ok(body) = std::fs::read_to_string(topic.join("missing_media.txt")) else {
        return BTreeSet::new();
    };
    body.lines()
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
enum Decision<'a> {
    /// The key is not in the message.
    Absent,
    /// A path: the bytes were saved.
    Saved,
    /// One of Desktop's parenthesised notes, verbatim.
    Skipped(&'a str),
}

impl Decision<'_> {
    fn is_skip(&self) -> bool {
        matches!(self, Decision::Skipped(_))
    }

    fn describe(&self) -> String {
        match self {
            Decision::Absent => "absent".into(),
            Decision::Saved => "downloaded".into(),
            Decision::Skipped(why) => format!("{why:?}"),
        }
    }

    /// The class of decision, for grouping. Substring tests rather than equality
    /// against Desktop's exact sentences, matching `tgx_html::media`.
    fn class(&self) -> &'static str {
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

fn decision<'a>(m: &'a Map<String, Value>, key: &str) -> Decision<'a> {
    match m.get(key).and_then(Value::as_str) {
        None => Decision::Absent,
        Some(v) if v.starts_with('(') => Decision::Skipped(v),
        Some(_) => Decision::Saved,
    }
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

    /// A scratch root that is emptied first, so a rerun cannot see last run's
    /// files — this leg's whole business is whether a file is on disk.
    fn root(tag: &str) -> PathBuf {
        let r = std::env::temp_dir().join(format!("tgx-wire-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(&r).unwrap();
        r
    }

    /// Write a one-topic export tree and hand back its root.
    fn tree(tag: &str, folder: &str, name: &str, messages: Value) -> PathBuf {
        tree_with_id(tag, folder, name, 1, messages)
    }

    fn tree_with_id(tag: &str, folder: &str, name: &str, id: i64, messages: Value) -> PathBuf {
        let r = root(tag);
        write_topic(&r, folder, name, id, messages);
        r
    }

    fn write_topic(r: &Path, folder: &str, name: &str, id: i64, messages: Value) {
        let topic = r.join(folder);
        std::fs::create_dir_all(&topic).unwrap();
        let body = json!({"name": name, "type": "public_supergroup", "id": id,
                          "messages": messages});
        std::fs::write(topic.join("result.json"), body.to_string()).unwrap();
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
    fn one_topic_a_side_pairs_only_when_it_is_the_same_chat() {
        // The special case exists for a renamed non-forum chat. Without the id
        // check it also fired for two entirely different chats, so a wrong path
        // on the command line produced a full comparison of unrelated messages
        // rather than "that is not the same chat".
        let msg = json!([{"id": 1, "type": "message", "text": "hello"}]);
        let ours = tree_with_id("pair-ours", "Old Name", "Old Name", 77, msg.clone());
        let same = tree_with_id("pair-same", "New Name", "New Name", 77, msg.clone());
        let other = tree_with_id("pair-other", "Someone Else", "Someone Else", 99, msg);

        assert_eq!(run(&ours, &same).unwrap(), 0, "a rename must still pair");
        let err = run(&ours, &other).unwrap_err().to_string();
        assert!(err.contains("different chats"), "{err}");

        for r in [&ours, &same, &other] {
            let _ = std::fs::remove_dir_all(r);
        }
    }

    #[test]
    fn run_reports_a_clean_pair_of_trees_as_clean() {
        // `run` itself had no test at all: every unit test drove `compare`,
        // `skip_reason` or `brief`. The thing between the converter and a silent
        // wire regression was the one part not exercised end to end.
        let msgs = json!([
            {"id": 1, "type": "message", "from": "A", "from_id": "user1", "text": "one"},
            {"id": 2, "type": "message", "from": "B", "from_id": "user2", "text": "two"},
        ]);
        let ours = root("run-clean-ours");
        write_topic(&ours, "0001 - t", "t", 5, msgs.clone());
        let theirs = root("run-clean-theirs");
        write_topic(&theirs, "t", "t", 5, msgs);
        assert_eq!(run(&ours, &theirs).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&ours);
        let _ = std::fs::remove_dir_all(&theirs);
    }

    #[test]
    fn run_fails_a_topic_we_did_not_export_and_one_we_invented() {
        let msgs = json!([{"id": 1, "type": "message", "text": "x"}]);
        let ours = root("run-topics-ours");
        write_topic(&ours, "0001 - a", "a", 5, msgs.clone());
        write_topic(&ours, "0002 - invented", "invented", 5, msgs.clone());
        let theirs = root("run-topics-theirs");
        write_topic(&theirs, "a", "a", 5, msgs.clone());
        write_topic(&theirs, "missed", "missed", 5, msgs);
        // One topic only in ours, one only in the reference.
        assert_eq!(run(&ours, &theirs).unwrap(), 2);
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
    fn a_field_only_we_write_is_caught_too() {
        // The mirror of the above, and it did not exist. Message #509 exported
        // a link preview's *document* as a 14 MB video the sender never sent;
        // the reference has no `file` key there at all, so nothing compared it.
        let one = |v: Value| Value::Array(vec![v]);
        let ours = tree(
            "extra-ours",
            "t",
            "t",
            one(json!({"id": 1, "type": "message", "text": "", "file_name": "invented.mp4"})),
        );
        let theirs = tree(
            "extra-theirs",
            "t",
            "t",
            one(json!({"id": 1, "type": "message", "text": ""})),
        );
        let r = compare(&ours.join("t"), &theirs.join("t")).unwrap();
        assert_eq!(r.extra_fields.get("file_name"), Some(&1));
        assert!(!r.clean());
        let _ = std::fs::remove_dir_all(&ours);
        let _ = std::fs::remove_dir_all(&theirs);
    }

    #[test]
    fn a_key_we_write_on_purpose_is_not_an_extra_field() {
        // `stripped_thumbnail` is ours by design and `reactions` arriving after
        // the reference run is new activity. Neither is an invented field, and
        // scoring them as one would make the new tally useless on day one.
        let one = |v: Value| Value::Array(vec![v]);
        let ours = tree(
            "extra-ok-ours",
            "t",
            "t",
            one(json!({"id": 1, "type": "message", "text": "",
                       "stripped_thumbnail": "(inline)", "reactions": []})),
        );
        let theirs = tree(
            "extra-ok-theirs",
            "t",
            "t",
            one(json!({"id": 1, "type": "message", "text": ""})),
        );
        let r = compare(&ours.join("t"), &theirs.join("t")).unwrap();
        assert!(r.extra_fields.is_empty(), "{:?}", r.extra_fields);
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
    fn a_gap_the_export_declared_is_not_a_dangling_reference() {
        // Filenames are chosen before the bytes are fetched, so a failed
        // download *must* leave a reference to a file that is not there. The
        // archive's honesty is missing_media.txt, and scoring what it names as
        // a dangling path keeps the leg red forever on any run with one failed
        // download.
        let one = |v: Value| Value::Array(vec![v]);
        let msg = one(json!({"id": 1, "type": "message", "file": "files/gone.bin"}));
        let ours = tree("stated-ours", "t", "t", msg.clone());
        let theirs = tree("stated-theirs", "t", "t", msg);
        std::fs::write(
            ours.join("t").join("missing_media.txt"),
            "These files are referenced by this export but could not be saved.\n\nfiles/gone.bin\n",
        )
        .unwrap();
        let r = compare(&ours.join("t"), &theirs.join("t")).unwrap();
        assert!(r.dangling.is_empty(), "{:?}", r.dangling);
        assert_eq!(r.stated.get("file"), Some(&1));
        assert!(r.clean(), "a stated gap must not fail the leg");
        let _ = std::fs::remove_dir_all(&ours);
        let _ = std::fs::remove_dir_all(&theirs);
    }

    #[test]
    fn a_rename_is_bucketed_by_person_and_an_empty_name_is_not() {
        // One person renaming their profile turns every message they ever sent
        // into a mismatch, and the report prints five examples per bucket — so
        // a rename storm pushes a real failure off the page. But `from` is in
        // MUST_MATCH because it caught 206 people exporting as "", and that must
        // keep failing.
        let msgs = |name: &str| {
            json!([
                {"id": 1, "type": "message", "from": name, "from_id": "user7", "text": "a"},
                {"id": 2, "type": "message", "from": name, "from_id": "user7", "text": "b"},
            ])
        };
        let ours = tree("rename-ours", "t", "t", msgs("Tam Fmk"));
        let theirs = tree("rename-theirs", "t", "t", msgs("Tamara Blokade"));
        let r = compare(&ours.join("t"), &theirs.join("t")).unwrap();
        assert_eq!(r.mismatched_messages, 0, "a rename is not a converter bug");
        assert_eq!(r.renames.len(), 1, "two messages, one person");
        assert_eq!(r.renames["user7"].2, 2);
        assert!(r.clean());

        // An empty name is a different animal and still fails.
        let blank = tree("rename-blank", "t", "t", msgs(""));
        let r = compare(&blank.join("t"), &theirs.join("t")).unwrap();
        assert_eq!(r.mismatched_messages, 2);
        assert!(r.renames.is_empty());

        for d in [&ours, &theirs, &blank] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn an_edited_message_may_say_something_different() {
        // `text` is required and `edited` may drift — but an edit is *why* text
        // changes, so that pair asks the two runs to disagree about when a
        // message was last changed and agree about what it now says.
        let one = |text: &str, edited: &str| {
            json!([{"id": 1, "type": "message", "text": text,
                    "edited": edited, "edited_unixtime": "1"}])
        };
        let ours = tree("edit-ours", "t", "t", one("after", "2026-08-27T01:00:00"));
        let theirs = tree(
            "edit-theirs",
            "t",
            "t",
            one("before", "2026-01-01T01:00:00"),
        );
        let r = compare(&ours.join("t"), &theirs.join("t")).unwrap();
        assert_eq!(r.mismatched_messages, 0);
        assert_eq!(r.edited, 1);
        assert!(r.clean());

        // Text differing with no edit recorded is still a failure.
        let same_edit = tree("edit-same", "t", "t", one("after", "2026-01-01T01:00:00"));
        let r = compare(&same_edit.join("t"), &theirs.join("t")).unwrap();
        assert_eq!(r.mismatched_messages, 1);

        for d in [&ours, &theirs, &same_edit] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn an_absent_field_is_not_reported_as_downloaded() {
        // The example line rendered `None` as the word "downloaded", and
        // `skip_reason` returned `None` both for a path and for a key that was
        // not there. So a message with no `photo` at all was reported as a
        // photo we had downloaded — the leg inventing the other side's answer.
        let m = map(json!({
            "file": "(File exceeds maximum size. Change data exporting settings to download.)",
            "photo": "photos/photo_1@01-01-2020_00-00-00.jpg",
        }));
        assert!(decision(&m, "file").is_skip());
        assert_eq!(decision(&m, "photo"), Decision::Saved);
        assert_eq!(decision(&m, "thumbnail"), Decision::Absent);
        assert_eq!(decision(&m, "thumbnail").describe(), "absent");
        assert_eq!(decision(&m, "photo").describe(), "downloaded");
        assert_eq!(decision(&m, "file").class(), "too_large");
        let n = map(
            json!({"file": "(File not included. Change data exporting settings to download.)"}),
        );
        assert_eq!(decision(&n, "file").class(), "not_included");
    }

    #[test]
    fn size_disagreements_are_grouped_by_shape() {
        // A uniform block is our settings differing from the reference run's,
        // which is not a defect; a scatter across shapes is the converter,
        // which is. The leg cannot read either run's settings, so making the
        // difference readable is the most it can honestly do.
        let one = |v: Value| Value::Array(vec![v]);
        let ours = tree(
            "shape-ours",
            "t",
            "t",
            one(json!({"id": 1, "type": "message", "file": "files/a.bin"})),
        );
        std::fs::create_dir_all(ours.join("t").join("files")).unwrap();
        std::fs::write(ours.join("t").join("files").join("a.bin"), b"x").unwrap();
        let theirs = tree(
            "shape-theirs",
            "t",
            "t",
            one(json!({"id": 1, "type": "message",
                       "file": "(File exceeds maximum size. Change data exporting settings to download.)"})),
        );
        let r = compare(&ours.join("t"), &theirs.join("t")).unwrap();
        assert_eq!(
            r.skip_shapes.get("file: ours saved / reference too_large"),
            Some(&1),
            "{:?}",
            r.skip_shapes
        );
        let _ = std::fs::remove_dir_all(&ours);
        let _ = std::fs::remove_dir_all(&theirs);
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
        // The display names stay required. Bucketing changes how a
        // disagreement is *reported*, never whether it is looked at.
        for field in DISPLAY_NAMES {
            assert!(MUST_MATCH.contains(field), "{field} stopped being checked");
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
