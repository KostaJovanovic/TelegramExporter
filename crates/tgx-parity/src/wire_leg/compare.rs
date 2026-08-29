//! The diff, and the tallies it fills in.
//!
//! **Topics pair by title, not by folder name.** Desktop uses the bare title
//! and we prefix the topic id, so `caskanje` and `0001 - caskanje` never met:
//! eight folders came back "only in ours" and the leg reported what looked
//! like a total export failure, having compared zero messages.
//!
//! Three tallies matter, and each was added because its absence hid a real
//! defect:
//!
//! * `absent` -- a field the reference writes that we never write at all.
//!   Deliberately **outside** the MAY_DRIFT allowance, which had been scoring
//!   963 missing `reactions` as honest run-to-run drift.
//! * `dangling` -- a media path in our JSON with no file behind it. 1,546 of
//!   these were invisible to the leg on the first real run.
//! * `extra` -- a key we write that the reference does not, so a payload
//!   invented rather than measured gets scored instead of ignored.

use super::detail::*;
use super::*;

#[derive(Default)]
pub(super) struct Totals {
    pub(super) shared: usize,
    pub(super) missing: usize,
    pub(super) extra: usize,
    pub(super) skips: usize,
    pub(super) skips_agreed: usize,
    pub(super) field_mismatches: usize,
    pub(super) drifted: usize,
    pub(super) edited: usize,
    pub(super) skips_by_key: BTreeMap<String, (usize, usize)>,
    pub(super) skip_shapes: BTreeMap<String, usize>,
    pub(super) renames: BTreeMap<String, (String, String, usize)>,
    pub(super) absent: BTreeMap<String, usize>,
    pub(super) extra_fields: BTreeMap<String, usize>,
    pub(super) dangling: BTreeMap<String, usize>,
    pub(super) stated: BTreeMap<String, usize>,
}

impl Totals {
    pub(super) fn add(&mut self, r: &Report) {
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
pub(super) struct Report {
    pub(super) shared: usize,
    pub(super) missing: Vec<i64>,
    pub(super) extra: Vec<i64>,
    pub(super) skips: usize,
    pub(super) skips_agreed: usize,
    pub(super) skips_by_key: BTreeMap<String, (usize, usize)>,
    pub(super) skip_shapes: BTreeMap<String, usize>,
    pub(super) skip_examples: Vec<String>,
    pub(super) mismatched_messages: usize,
    pub(super) by_field: BTreeMap<String, usize>,
    pub(super) examples: Vec<String>,
    pub(super) drifted: usize,
    pub(super) edited: usize,
    /// Peer key → (our name, the reference's name, how many messages).
    pub(super) renames: BTreeMap<String, (String, String, usize)>,
    pub(super) absent: BTreeMap<String, usize>,
    pub(super) extra_fields: BTreeMap<String, usize>,
    pub(super) dangling: BTreeMap<String, usize>,
    pub(super) dangling_examples: Vec<String>,
    /// Dangling paths the export itself names in `missing_media.txt`.
    pub(super) stated: BTreeMap<String, usize>,
}

impl Report {
    pub(super) fn clean(&self) -> bool {
        self.missing.is_empty()
            && self.extra.is_empty()
            && self.mismatched_messages == 0
            && self.skips == self.skips_agreed
            && self.absent.is_empty()
            && self.extra_fields.is_empty()
            && self.dangling.is_empty()
    }

    pub(super) fn print(&self) {
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

pub(super) fn head(ids: &[i64]) -> String {
    let shown: Vec<String> = ids.iter().take(10).map(i64::to_string).collect();
    if ids.len() > shown.len() {
        format!("{} …", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

pub(super) fn compare(ours_root: &Path, theirs: &Path) -> Result<Report> {
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
