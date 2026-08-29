//! Enforces the two dependency rules CLAUDE.md and the README both describe
//! as "enforced by the build" — which, until this file existed, they were not.
//! `cargo test --all` reads every crate's `Cargo.toml` as plain TOML text and
//! fails loudly, naming the rule, if a forbidden dependency has crept in.
//!
//! This lives in `tgx-parity` because it is the harness crate: it already
//! depends on nothing that would make it awkward to also carry the checks
//! that keep the *other* crates honest.

use std::path::Path;

fn manifest(crate_dir: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(crate_dir)
        .join("Cargo.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Deliberately not a real TOML parser — pulling in one just for two
/// dependency checks would be a heavier fix than the problem it solves. Every
/// section whose header ends in `dependencies]` is one Cargo actually reads
/// as a dependency table (`[dependencies]`, `[dev-dependencies]`,
/// `[build-dependencies]`, and the `[target.'cfg(...)'.*-dependencies]` forms
/// this workspace uses for `winresource`), so tracking "am I inside one of
/// those" line by line is enough to catch every place a crate name can
/// appear as a key.
fn dependency_names(manifest_toml: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_deps_section = false;
    for raw_line in manifest_toml.lines() {
        let line = raw_line.trim();
        if let Some(header) = line.strip_prefix('[') {
            let header = header.trim_end_matches(']');
            in_deps_section = header.ends_with("dependencies");
            continue;
        }
        if !in_deps_section {
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            let key = key.trim().trim_matches('"');
            if !key.is_empty() {
                names.push(key.to_string());
            }
        }
    }
    names
}

/// `tgx-html` writes Desktop's markup from serialised maps and MUST NOT know
/// about Telegram's wire types — that is what lets the parity harness replay
/// a recorded `result.json` through it with no connection at all. A
/// `grammers-*` dependency here would mean the writer had started depending
/// on wire shapes instead of the JSON schema, and the html leg would no
/// longer be proving what it claims to prove.
#[test]
fn tgx_html_does_not_depend_on_grammers() {
    let names = dependency_names(&manifest("tgx-html"));
    let offenders: Vec<&String> = names.iter().filter(|n| n.starts_with("grammers")).collect();
    assert!(
        offenders.is_empty(),
        "tgx-html/Cargo.toml depends on {offenders:?} — tgx-html must not depend on \
         grammers-tl-types (or any grammers-* crate). It writes Desktop's pages from \
         serialised maps with no knowledge of Telegram's wire types; that separation is \
         what lets the parity harness replay a recorded result.json through it with no \
         network connection. See CLAUDE.md's Architecture section."
    );
}

/// `tgx-app` is the window. It reaches Telegram only through `tgx-tg`, which
/// owns the client, the engine and every typed error the UI is allowed to see.
/// A direct `grammers-*` dependency lets a wire type reach the widgets, and the
/// seam that keeps GPUI on the main thread and tokio on its own stops being the
/// only way across.
///
/// This assertion was missing while `tgx-app/Cargo.toml` carried an unused
/// `grammers-client`, so the manifest and CLAUDE.md disagreed and the test that
/// exists to catch exactly that did not look.
#[test]
fn tgx_app_does_not_depend_on_grammers() {
    let names = dependency_names(&manifest("tgx-app"));
    let offenders: Vec<&String> = names.iter().filter(|n| n.starts_with("grammers")).collect();
    assert!(
        offenders.is_empty(),
        "tgx-app/Cargo.toml depends on {offenders:?} — the window must reach Telegram only \
         through tgx-tg, which owns the client and the typed errors the UI may see. A direct \
         grammers-* dependency lets a wire type reach the widgets. See CLAUDE.md's \
         Architecture section."
    );
}

/// `tgx-parity` is the oracle: it replays *recorded* data — a real Desktop
/// export, or the corpus cut from one — through our own writers and diffs
/// the result. If it depended on `tgx-tg` it could reach for a live client
/// instead of replaying fixtures, and the harness would stop being something
/// that runs offline, deterministically, on a machine with no signed-in
/// account.
#[test]
fn tgx_parity_does_not_depend_on_tgx_tg() {
    let names = dependency_names(&manifest("tgx-parity"));
    assert!(
        !names.iter().any(|n| n == "tgx-tg"),
        "tgx-parity/Cargo.toml depends on tgx-tg — the oracle must only ever replay \
         recorded data (a real Desktop export, or the corpus cut from one) through our \
         writers. Depending on tgx-tg would let it reach for a live client instead of a \
         fixture, and the harness would no longer run offline and deterministically. \
         See CLAUDE.md's Architecture section."
    );
}
