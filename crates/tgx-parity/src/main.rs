//! The one check that cannot be faked.
//!
//! Take Desktop's own export, feed its data back through our writers, and
//! compare what we produce with what Desktop produced from the same input.
//! Everything that differs is ours to explain.
//!
//! ```text
//! tgx-parity json "N:\telegram export\UA KOLAB TELEGRAM"
//! tgx-parity html "N:\telegram export\UA KOLAB TELEGRAM"
//! ```
//!
//! Exit status is the number of topics that did not match exactly.
//!
//! **This exists before the code it judges.** In the Python original the
//! equivalent harness arrived late and immediately found five media-naming bugs
//! that no unit test had caught — because a unit test encodes what you believed,
//! and this encodes what Desktop actually did.

mod html_leg;
mod json_leg;
mod media_leg;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(failures) => {
            if failures > 255 {
                std::process::ExitCode::FAILURE
            } else {
                std::process::ExitCode::from(failures as u8)
            }
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<u32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("{}", USAGE);
        bail!("need a leg and an export root");
    }
    let (leg, root) = (args[0].as_str(), PathBuf::from(&args[1]));
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }
    let topics = topic_folders(&root)?;
    if topics.is_empty() {
        bail!(
            "no export folders with a result.json under {}",
            root.display()
        );
    }

    match leg {
        "json" => json_leg::run(&topics),
        "html" => html_leg::run(&topics),
        "media" => media_leg::run(&topics),
        other => bail!("unknown leg {other:?}; expected one of: json, html, media"),
    }
}

const USAGE: &str = "usage: tgx-parity <json|html|media> <export root>";

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
