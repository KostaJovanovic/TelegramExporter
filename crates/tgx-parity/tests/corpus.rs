//! Run all three legs against the committed corpus, if there is one.
//!
//! The legs are a command you remember to run against a drive letter. This is
//! the same code as an ordinary `cargo test`, so a regression in the emitter
//! fails the build rather than waiting for someone to think of the oracle.
//!
//! **It skips rather than fails when `reference/` is absent**, and that is a
//! deliberate, uncomfortable choice. A test that cannot find its data and
//! passes anyway is the classic way a suite goes quietly green — so the skip
//! prints exactly what it did not check and how to produce it, and
//! [`corpus_is_present_or_explains_itself`] makes the absence itself visible
//! in the output rather than silent.
//!
//! The alternative — fail without a corpus — would break `cargo test` on every
//! machine that has never seen the export, including CI. The corpus is real
//! chat history and is gitignored; see `src/corpus.rs`.

use std::path::PathBuf;
use tgx_parity::{corpus, html_leg, json_leg, media_leg, topic_folders};

/// `Some(dir)` when a corpus is there, `None` after saying why not.
fn corpus_dir() -> Option<PathBuf> {
    let dir = corpus::default_dir();
    if dir.join("MANIFEST.txt").is_file() {
        return Some(dir);
    }
    eprintln!(
        "no corpus at {} — the json, html and media legs are NOT covered by this run.\n\
         Cut one with:  cargo run -p tgx-parity -- corpus \"<export root>\"",
        dir.display()
    );
    None
}

#[test]
fn corpus_is_present_or_explains_itself() {
    // Deliberately not an assertion. Its job is to put the skip notice in the
    // test output on a machine that has no export, so "0 failed" is never
    // mistaken for "everything was checked".
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
    let Some(dir) = corpus_dir() else { return };
    let topics = topic_folders(&dir).expect("reading the corpus");
    assert!(!topics.is_empty(), "the corpus holds no topics");
    let failures = json_leg::run(&topics).expect("running the json leg");
    assert_eq!(failures, 0, "{failures} topics did not re-emit exactly");
}

#[test]
fn every_html_page_is_reproduced_exactly() {
    let Some(dir) = corpus_dir() else { return };
    let topics = topic_folders(&dir).expect("reading the corpus");
    let failures = html_leg::run(&topics).expect("running the html leg");
    assert_eq!(failures, 0, "{failures} topics did not replay exactly");
}

#[test]
fn media_names_land_where_desktop_put_them() {
    let Some(dir) = corpus_dir() else { return };
    let topics = topic_folders(&dir).expect("reading the corpus");
    // Not asserted at zero. The media leg's known ceiling is the custom-emoji
    // repeats documented in ROADMAP.md — 830 of 836 names on the reference —
    // so this guards the *shape* of the run, and the burn-down number stays in
    // the leg's own printed output where a change to it is visible.
    let _ = media_leg::run(&topics).expect("running the media leg");
}
