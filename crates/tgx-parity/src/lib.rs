//! The oracle harness, as a library so its legs can also run from a test.
//!
//! The binary is the everyday face of this crate — you point it at
//! `N:\telegram export\…` and it prints a burn-down. But a harness that only
//! exists as a command is a harness that stops running the moment the drive
//! holding the reference goes away, and "the reference is on one machine" is
//! the standing risk in the roadmap. So the legs live here, and
//! `tests/corpus.rs` runs the same code against a small committed corpus.

pub mod corpus;
pub mod html_leg;
pub mod json_leg;
pub mod media_leg;

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Every folder under `root` holding a `result.json`, sorted.
///
/// Desktop writes `chats/chat_<id>/`; our own exporter writes one folder per
/// topic directly under the root. Both are found without a flag, which is what
/// lets the harness run against either.
pub fn topic_folders(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect(root, &mut out, 0)?;
    out.sort();
    Ok(out)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) -> Result<()> {
    if depth > 3 {
        return Ok(());
    }
    if dir.join("result.json").is_file() {
        out.push(dir.to_path_buf());
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out, depth + 1)?;
        }
    }
    Ok(())
}

/// Report the first differing line between two texts, with a little context.
///
/// Deliberately line-based and deliberately truncated: a format bug produces
/// thousands of identical diffs and the first one is the whole story.
pub fn first_difference(theirs: &str, ours: &str) -> Option<String> {
    let a: Vec<&str> = theirs.lines().collect();
    let b: Vec<&str> = ours.lines().collect();
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y {
            return Some(format!(
                "line {}:\n      desktop: {:?}\n      ours   : {:?}",
                i + 1,
                x,
                y
            ));
        }
    }
    if a.len() != b.len() {
        return Some(format!(
            "line count {} (desktop) vs {} (ours)",
            a.len(),
            b.len()
        ));
    }
    None
}

/// How many lines differ in total, for the burn-down number.
pub fn differing_lines(theirs: &str, ours: &str) -> usize {
    let a: Vec<&str> = theirs.lines().collect();
    let b: Vec<&str> = ours.lines().collect();
    let common = a.iter().zip(b.iter()).filter(|(x, y)| x != y).count();
    common + a.len().abs_diff(b.len())
}
