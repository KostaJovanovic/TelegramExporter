//! Every switch in `Settings` must be read by something that exports.
//!
//! **This test exists because five of them were not.** `full_reactions`,
//! `chat_metadata`, `invite_links`, `refresh_polls` and `scheduled_messages`
//! were painted as checkboxes in the settings panel, written to
//! `settings.json`, loaded back on the next run — and read by no code that
//! produces output. Turning one on did nothing whatsoever, and nothing in the
//! build, the lint or the suite said so, because a struct field that is
//! written and never read is not dead code.
//!
//! That is the worst shape a defect can take here: it is invisible from the
//! inside, invisible from the tests, and from the outside it looks exactly like
//! a feature that works. The only way it surfaced was somebody comparing a real
//! export against the Python original's and noticing the reaction lists were
//! short.
//!
//! So the check is mechanical rather than a matter of remembering: parse the
//! field names out of `Settings`, and require each to appear in code that is
//! neither the definition nor the panel that toggles it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Source with `//` comments removed, so a field named only in prose does not
/// count as wired. `chat_concurrency` is discussed at length in two comments
/// and used by nothing, which is exactly the confusion this avoids.
fn code_of(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The `pub name:` fields of `struct Settings`.
fn settings_fields(src: &str) -> BTreeSet<String> {
    let start = src.find("pub struct Settings").expect("struct Settings");
    let body = &src[start..];
    let end = body.find("\n}").expect("the struct ends");
    body[..end]
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("pub ")?;
            let name = rest.split(':').next()?.trim();
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
                .then(|| name.to_string())
        })
        .collect()
}

#[test]
fn every_setting_is_read_by_something_that_exports() {
    let ws = workspace();
    let config = ws.join("crates/tgx-tg/src/config.rs");
    let fields = settings_fields(&std::fs::read_to_string(&config).expect("reading config.rs"));
    assert!(fields.len() > 15, "only found {} fields", fields.len());

    // The definition itself, and the panel whose entire job is to name every
    // switch. A field mentioned only in those two is a switch that does
    // nothing.
    let excluded = ["config.rs", "settings_form.rs", "settings.rs"];

    let mut files = Vec::new();
    rust_files(&ws.join("crates/tgx-tg/src"), &mut files);
    rust_files(&ws.join("crates/tgx-app/src"), &mut files);
    let haystack: String = files
        .iter()
        .filter(|p| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            !excluded.contains(&name.as_str())
        })
        .map(|p| code_of(p))
        .collect::<Vec<_>>()
        .join("\n");

    // Read through a method rather than by name. `size_limit_mb` is only ever
    // reached via `size_limit_bytes()`, which is the right way round — the
    // multiplication belongs with the field, not at every call site.
    let via_method = [("size_limit_mb", "size_limit_bytes")];

    let mut dead: Vec<&str> = Vec::new();
    for f in &fields {
        let named = haystack.contains(&format!(".{f}"));
        let by_method = via_method
            .iter()
            .any(|(field, method)| field == f && haystack.contains(method));
        if !named && !by_method {
            dead.push(f);
        }
    }

    assert!(
        dead.is_empty(),
        "these settings are offered to the user and read by nothing:\n  {}\n\
         A switch that does nothing is worse than a missing one: it is\n\
         indistinguishable from a working feature until someone diffs the\n\
         output against another exporter's.",
        dead.join("\n  ")
    );
}
