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

/// `tgx-archive` is the database output. It takes the message payload as a
/// serialised map — exactly what `result.json` carries, minus `_p` — and knows
/// nothing of Telegram's wire types, for the same reason `tgx-html` does not:
/// **the store must be replayable by the harness from recorded data.** The
/// archive leg merges a real Desktop export into a store twice and reads
/// Desktop's own bytes back out, and it can only do that offline if nothing in
/// the store needs a connection to exist.
#[test]
fn tgx_archive_does_not_depend_on_grammers() {
    let names = dependency_names(&manifest("tgx-archive"));
    let offenders: Vec<&String> = names.iter().filter(|n| n.starts_with("grammers")).collect();
    assert!(
        offenders.is_empty(),
        "tgx-archive/Cargo.toml depends on {offenders:?} — the store must not depend on \
         grammers-tl-types (or any grammers-* crate). It takes the same serialised map the \
         JSON and HTML writers take, so the harness can replay a recorded result.json \
         through it with no network connection; that is what the archive leg proves. \
         See CLAUDE.md's Architecture section."
    );
}

/// The other half of the rule above, and the one that is easier to break by
/// accident: `tgx-tg` owns the engine, the client and every typed error, so
/// reaching for it from the store is the obvious shortcut the moment the store
/// wants a `Settings` or a `Progress`. It would also make the store
/// unreachable from `tgx-parity`, which must never depend on `tgx-tg` — so the
/// archive leg would stop compiling, which is a late and confusing way to
/// learn the layer was crossed.
#[test]
fn tgx_archive_does_not_depend_on_tgx_tg() {
    let names = dependency_names(&manifest("tgx-archive"));
    assert!(
        !names.iter().any(|n| n == "tgx-tg"),
        "tgx-archive/Cargo.toml depends on tgx-tg — the store sits *below* the engine, not \
         beside it. Anything it needs from a run is passed in as a value. Depending on \
         tgx-tg would also put it out of reach of tgx-parity, which may not depend on \
         tgx-tg, and the archive leg would have nowhere to live. See CLAUDE.md's \
         Architecture section."
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

/// CLAUDE.md says "tgx-app: the window. Depends only on tgx-ui + tgx-tg", and
/// until this test existed that was a sentence rather than a rule.
///
/// It is worth a check of its own because the pressure to break it is real and
/// looks harmless every time: the window needs one constant or one type from a
/// lower crate — the database file name, say — and adding the path dependency
/// is a one-line change that compiles. Then the window knows about the store's
/// vocabulary, and the seam that keeps every Telegram and storage concern
/// behind `tgx-tg` has a second hole in it. Whatever the window needs, `tgx-tg`
/// re-exports or does on its behalf.
#[test]
fn tgx_app_depends_only_on_tgx_ui_and_tgx_tg() {
    let names = dependency_names(&manifest("tgx-app"));
    let ours: Vec<&String> = names.iter().filter(|n| n.starts_with("tgx-")).collect();
    let strays: Vec<&&String> = ours
        .iter()
        .filter(|n| n.as_str() != "tgx-ui" && n.as_str() != "tgx-tg")
        .collect();
    assert!(
        strays.is_empty(),
        "tgx-app/Cargo.toml depends on {strays:?} — the window may name only tgx-ui and \
         tgx-tg. Anything it needs from a lower crate comes through tgx-tg, which owns the \
         client, the engine, the store and every typed error the UI is allowed to see. \
         See CLAUDE.md's Architecture section."
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
