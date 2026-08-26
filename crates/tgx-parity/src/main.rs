//! The one check that cannot be faked.
//!
//! Take Desktop's own export, feed its data back through our writers, and
//! compare what we produce with what Desktop produced from the same input.
//! Everything that differs is ours to explain.
//!
//! ```text
//! tgx-parity json   "N:\telegram export\UA KOLAB TELEGRAM"
//! tgx-parity html   "N:\telegram export\UA KOLAB TELEGRAM"
//! tgx-parity media  "N:\telegram export\UA KOLAB TELEGRAM"
//! tgx-parity wire   "Exports\UA KOLAB TELEGRAM" "N:\telegram export\UA KOLAB"
//! tgx-parity corpus "N:\telegram export\UA KOLAB TELEGRAM" reference
//! ```
//!
//! Exit status is the number of topics that did not match exactly.
//!
//! **This exists before the code it judges.** In the Python original the
//! equivalent harness arrived late and immediately found five media-naming bugs
//! that no unit test had caught — because a unit test encodes what you believed,
//! and this encodes what Desktop actually did.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use tgx_parity::{corpus, html_leg, json_leg, media_leg, topic_folders, wire_leg};

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

    // The wire leg is the odd one out: it compares two export trees rather
    // than replaying one, so it takes its second root before the topic scan.
    if leg == "wire" {
        let theirs = args
            .get(2)
            .map(PathBuf::from)
            .context("usage: tgx-parity wire <our export> <reference export>")?;
        if !theirs.is_dir() {
            bail!("{} is not a directory", theirs.display());
        }
        return wire_leg::run(&root, &theirs);
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
        // The workspace's own `reference/` is where the corpus test looks, so
        // it is the default destination: the documented command and the tested
        // path cannot drift apart.
        "corpus" => {
            let out = args
                .get(2)
                .map(PathBuf::from)
                .unwrap_or_else(tgx_parity::corpus::default_dir);
            corpus::build(&root, &topics, &out)
        }
        other => bail!("unknown leg {other:?}; expected one of: json, html, media, wire, corpus"),
    }
}

const USAGE: &str = "\
usage: tgx-parity <leg> <export root> [args]

  json    re-emit each result.json and byte-diff it
  html    replay each result.json through our writer and diff the pages
  media   replan every attachment's file name and diff the tree
  wire    diff our own export against a reference run: ids, size decisions,
          and every field a converter bug would change
            tgx-parity wire <our export> <reference export>
  corpus  copy the text half of the export into a small standalone corpus
          (default destination: the workspace's reference/)";
