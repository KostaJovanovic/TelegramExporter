//! Run all three legs against the committed corpus, if there is one.
//!
//! The legs are a command you remember to run against a drive letter. This is
//! the same code as an ordinary `cargo test`, so a regression in the emitter
//! fails the build rather than waiting for someone to think of the oracle.
//!
//! **What happens when `reference/` is absent depends on who is asking.**
//!
//! * With `TGX_REQUIRE_CORPUS` set — which `save.bat test` does — a missing
//!   corpus is a hard failure. That is the machine that owns the export, so
//!   "the oracle did not run" is a broken setup, not a fact of life.
//! * Without it, the legs skip. `cargo test --all` has to work on a machine
//!   that has never seen the export, and the corpus is real chat history and is
//!   gitignored (see `src/corpus.rs`), so on CI it can never be present.
//!
//! The skip used to rely on an `eprintln!` being read, which it is not: libtest
//! captures stdout *and* stderr for a passing test and prints neither. So the
//! notice now only claims to be visible where it is — under `--nocapture`,
//! which CI passes, alongside a step that raises a GitHub annotation.

use std::path::PathBuf;
use tgx_parity::{corpus, html_leg, json_leg, media_leg, topic_folders};

/// What a run should do when `MANIFEST.txt` is not where it should be.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// The corpus is there; run the leg.
    Run,
    /// No corpus and none demanded; skip, saying so.
    Skip,
    /// No corpus on a machine that said it has one.
    Fail,
}

/// Split out from [`corpus_dir`] so the matrix is testable without moving the
/// corpus around underneath a running suite.
fn decide(manifest_present: bool, require: Option<&str>) -> Decision {
    if manifest_present {
        return Decision::Run;
    }
    // Any non-empty value except "0" counts as set, so `TGX_REQUIRE_CORPUS=0`
    // is an escape hatch rather than a synonym for "yes".
    match require {
        Some(v) if !v.is_empty() && v != "0" => Decision::Fail,
        _ => Decision::Skip,
    }
}

fn missing_message(dir: &std::path::Path) -> String {
    format!(
        "no corpus at {} — the json, html and media legs are NOT covered by this run.\n\
         Cut one with:  cargo run -p tgx-parity -- corpus \"<export root>\"\n\
         (or: save.bat corpus)",
        dir.display()
    )
}

/// `Some(dir)` when a corpus is there, `None` after saying why not — unless
/// `TGX_REQUIRE_CORPUS` is set, in which case there is no `None`.
fn corpus_dir() -> Option<PathBuf> {
    let dir = corpus::default_dir();
    let require = std::env::var("TGX_REQUIRE_CORPUS").ok();
    match decide(dir.join("MANIFEST.txt").is_file(), require.as_deref()) {
        Decision::Run => Some(dir),
        Decision::Skip => {
            eprintln!("{}", missing_message(&dir));
            None
        }
        Decision::Fail => panic!(
            "TGX_REQUIRE_CORPUS is set, so a missing corpus is a failure.\n{}",
            missing_message(&dir)
        ),
    }
}

/// Every leg starts here: a corpus with no topics in it compares nothing, and
/// a comparison of nothing must not read as a comparison that agreed.
fn topics_or_skip() -> Option<Vec<PathBuf>> {
    let dir = corpus_dir()?;
    let topics = topic_folders(&dir).expect("reading the corpus");
    assert!(
        !topics.is_empty(),
        "the corpus at {} holds no topics",
        dir.display()
    );
    Some(topics)
}

#[test]
fn a_present_corpus_always_runs() {
    assert_eq!(decide(true, None), Decision::Run);
    assert_eq!(decide(true, Some("1")), Decision::Run);
}

#[test]
fn a_missing_corpus_skips_by_default() {
    assert_eq!(decide(false, None), Decision::Skip);
    assert_eq!(decide(false, Some("")), Decision::Skip);
    assert_eq!(decide(false, Some("0")), Decision::Skip);
}

#[test]
fn a_missing_corpus_fails_when_the_machine_says_it_has_one() {
    assert_eq!(decide(false, Some("1")), Decision::Fail);
    assert_eq!(decide(false, Some("yes")), Decision::Fail);
}

#[test]
fn corpus_is_present_or_explains_itself() {
    // Deliberately not an assertion when the corpus is merely absent. Its job
    // is to put the skip notice in the test output on a machine that has no
    // export, so "0 failed" is never mistaken for "everything was checked".
    let _ = corpus_dir();
}

#[test]
fn the_corpus_matches_its_manifest() {
    let Some(dir) = corpus_dir() else { return };
    let checked = corpus::verify(&dir).expect("corpus does not match MANIFEST.txt");
    assert!(checked > 0, "the manifest names no files");
}

#[test]
fn every_result_json_is_reproduced_byte_for_byte() {
    let Some(topics) = topics_or_skip() else {
        return;
    };
    let failures = json_leg::run(&topics).expect("running the json leg");
    assert_eq!(failures, 0, "{failures} topics did not re-emit exactly");
}

#[test]
fn every_html_page_is_reproduced_exactly() {
    let Some(topics) = topics_or_skip() else {
        return;
    };
    let failures = html_leg::run(&topics).expect("running the html leg");
    assert_eq!(failures, 0, "{failures} topics did not replay exactly");
}

#[test]
fn media_names_land_where_desktop_put_them() {
    let Some(topics) = topics_or_skip() else {
        return;
    };
    // The leg knows its own ceiling — the six custom emoji a JSON replay cannot
    // see, named individually in `media_leg::KNOWN_UNMATCHED` — and returns 1
    // for anything else, including a run that compared nothing. Discarding that
    // made the assertion decorative: the count could go to 0 of 836 and the
    // suite would stay green, because the number lives in stdout and cargo
    // captures it.
    let failures = media_leg::run(&topics).expect("running the media leg");
    assert_eq!(failures, 0, "media names did not match Desktop's");
}
