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
//! export against another export of the same chat and noticing the reaction
//! lists were short.
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
        let named = reads_settings_field(&haystack, f);
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

/// Is this field read **on a `Settings`**, rather than merely named somewhere?
///
/// The first version of this test asked `haystack.contains(&format!(".{f}"))`,
/// which two things satisfy that should not:
///
/// * **another type's field of the same name.** `theme` is a `Settings` field
///   *and* a `Palette` concern; `phone`, `title` and `id` are the shape of name
///   that recurs. Any unrelated `.theme` would have marked the setting wired,
///   so a switch could be disconnected and this test would still pass — which
///   is precisely the failure it exists to prevent, wearing its own clothes.
/// * **a longer field name.** `.page_size` is a prefix of `.page_size_hint`.
///
/// So the receiver must end in `settings` — `settings.x`, `self.settings.x`,
/// `export_settings.x` — and the field must end on a word boundary. Measured
/// before the rule was tightened: of the 25 fields, 24 are read through a
/// receiver literally named `settings` once `config.rs`, `settings_form.rs` and
/// `settings.rs` are excluded, and the 25th is `size_limit_mb`, which the
/// `via_method` table already covers. If a future call site binds a `Settings`
/// to something not ending in `settings`, this test will name that field as
/// dead — and the fix is to rename the binding or extend `via_method`, not to
/// loosen the match back to a bare `.field`.
fn reads_settings_field(haystack: &str, field: &str) -> bool {
    let needle = format!("settings.{field}");
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(i) = haystack[from..].find(&needle) {
        let start = from + i;
        let end = start + needle.len();
        // `settings.page_size` must not be satisfied by `.page_size_hint`.
        let bounded = bytes.get(end).is_none_or(|c| !ident(*c));
        // And the receiver has to *end* at "settings": `my_settings` counts,
        // `resettings` does not, because the character before it is a letter.
        let receiver_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        if bounded && receiver_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

#[test]
fn a_field_name_on_an_unrelated_struct_is_not_a_wired_setting() {
    // The blind spot in the rule this test used to apply. `theme` is a
    // `Settings` field and also something a `Palette` has, so a bare `.theme`
    // anywhere in the workspace marked the switch wired — meaning the guard
    // against a disconnected setting could itself be satisfied by an unrelated
    // struct.
    assert!(!reads_settings_field("let c = palette.theme;", "theme"));
    assert!(reads_settings_field("let c = settings.theme;", "theme"));
    assert!(reads_settings_field("self.settings.theme.clone()", "theme"));
    assert!(reads_settings_field("export_settings.theme", "theme"));

    // A longer field must not satisfy a shorter one.
    assert!(!reads_settings_field(
        "settings.page_size_hint",
        "page_size"
    ));
    assert!(reads_settings_field("settings.page_size)", "page_size"));

    // And the receiver has to end at "settings", not merely contain it.
    assert!(!reads_settings_field("resettings.theme", "theme"));
}
