//! `export_results.html`: the front door of a forum export.
//!
//! One page listing every topic, and the target of the `&lsaquo; <chat>` link
//! that [`crate::page::open_page`] puts in every topic page's header. A Desktop
//! export has no counterpart — Desktop names its topic folders `topic_12` under
//! `chats/chat_<id>/` and there is nothing worth indexing — so this page is not
//! covered by the parity legs and nothing downstream of them noticed when it
//! stopped being written at all. It had: on the real export at
//! `N:\telegram export\UA KOLAB RUST` all nine topic pages carried the back
//! link and the file it points at was simply absent, so every page in the
//! archive opened with a dead link out of it.
//!
//! **Built only from classes Desktop's own stylesheet defines**, which is not a
//! small constraint. An earlier attempt at this page used `section_chats` and
//! `topics_list`, invented to match a hand-written stylesheet that was later
//! replaced by Desktop's real one; neither class exists there — 0 rules each —
//! so the page rendered as unstyled text.
//! [`tests::every_class_this_page_uses_exists_in_desktops_stylesheet`] checks
//! each one against [`crate::assets::STYLE_CSS`], the same way the original's
//! offline suite does.

use crate::assets::write_assets;
use crate::escape::{esc, safe_href};
use crate::tree::{a, Tree};
use std::io;
use std::path::Path;

/// Desktop's list-page userpic diameter, in px. Both the box and the initials'
/// line height take it, which is what centres the letters.
const USERPIC_SIZE: u32 = 50;

/// Files an export may or may not have produced, and the label each gets in the
/// `sections` block. Probed on disk rather than passed in, because whether a
/// run produced `participants.json` is a fact about the folder, not something a
/// caller should have to remember to report — and a section linking to a file
/// that was never written is the same dead link this module exists to fix.
const EXTRAS: [(&str, &str); 3] = [
    ("participants.json", "Members"),
    ("scheduled.json", "Scheduled messages"),
    ("missing_media.txt", "Media that failed to download"),
];

/// One row of the index.
///
/// Everything here is display-ready text except [`IndexEntry::href`], which is
/// scheme-checked on the way out. Nothing is pre-escaped: strings arrive raw and
/// this module escapes them, so a caller cannot escape twice by mistake.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// Relative target, e.g. `0001 - ćaskanje/messages.html`. Folder names come
    /// from a topic title, so this is attacker-controlled like everything else.
    pub href: String,
    pub title: String,
    /// The one or two letters painted in the userpic.
    pub initials: String,
    /// Picks `userpic1`..`userpic8`. Written verbatim, as
    /// [`crate::userpic::userpic_class`] does: Desktop emits an out-of-range
    /// index rather than clamping it, and an unstyled circle is the honest
    /// rendering of a colour its stylesheet has no rule for.
    pub colour: i64,
    /// State markers, already joined: `pinned`, `pinned, closed`.
    ///
    /// A closed or hidden topic is still exported — Telegram returns them and an
    /// archive should hold them — but the page has to say so, or a reader cannot
    /// tell why one stopped.
    pub detail: Option<String>,
    /// `opened by UA KOLAB, 14.12.2025`.
    pub subname: Option<String>,
    pub messages: usize,
}

/// `1 message`, `0 messages`, `3811 messages`.
///
/// English pluralisation is Desktop's, and the summary line is assembled from
/// this in the caller, so it lives here rather than in `tgx-tg`.
pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Write `export_results.html` into `dir`, with the assets it references.
///
/// `summary` is the caller's one-line description of the group — `4 topics,
/// 6643 messages, 43 members` — and `about` an optional second line, used for
/// the invite link or the group description. Both are plain text and escaped
/// here.
///
/// The assets go down first, exactly as the topic pages do it
/// ([`crate::writer::HtmlWriter::close`] calls the same function): this page
/// links `css/style.css`, and an index written into a folder whose export
/// failed before its pages were flushed would otherwise be an unstyled wall of
/// text. `write_assets` skips images that are already there at the right size,
/// so calling it once more per export costs nothing.
pub fn write_index(
    dir: &Path,
    chat_title: &str,
    summary: &str,
    about: Option<&str>,
    entries: &[IndexEntry],
) -> io::Result<()> {
    write_assets(dir)?;
    let extras: Vec<(&str, &str)> = EXTRAS
        .iter()
        .filter(|(name, _)| dir.join(name).exists())
        .copied()
        .collect();
    std::fs::write(
        dir.join("export_results.html"),
        render(chat_title, summary, about, entries, &extras),
    )
}

/// The page itself, as a string. Separate from the write so the tests can read
/// the markup without going near a filesystem.
fn render(
    chat_title: &str,
    summary: &str,
    about: Option<&str>,
    entries: &[IndexEntry],
    extras: &[(&str, &str)],
) -> String {
    let mut t = Tree::new();
    t.text("<!DOCTYPE html>");
    t.open("html", &[]);
    t.open("head", &[]);
    t.void("meta", &[a("charset", "utf-8")]);
    // The chat's own title, not Desktop's fixed `Exported Data`: this page is
    // the one a reader opens first and its tab is how they tell two exports
    // apart. Emitted as text, not a tag line — see `tree`'s blank-line rule.
    t.text(&format!("<title>{}</title>", esc(chat_title)));
    t.void(
        "meta",
        &[
            a("content", "width=device-width, initial-scale=1.0"),
            a("name", "viewport"),
        ],
    );
    t.void(
        "link",
        &[a("href", "css/style.css"), a("rel", "stylesheet")],
    );
    t.close("head");
    // No `js/script.js` and no `onload="CheckLocation();"`, unlike a topic page.
    // Nothing on this page calls a handler — there are no spoilers, no
    // `GoToMessage` links and no map thumbnails — and the specimen this page
    // was modelled on carries neither.
    t.open("body", &[]);
    t.open("div", &[a("class", "page_wrap")]);

    t.open("div", &[a("class", "page_header")]);
    t.open("div", &[a("class", "content")]);
    t.leaf("div", &esc(chat_title), &[a("class", "text bold")]);
    t.close("div");
    t.close("div");

    t.open("div", &[a("class", "page_body list_page")]);
    t.leaf("div", &esc(summary), &[a("class", "page_about details")]);
    if let Some(text) = about.filter(|s| !s.is_empty()) {
        t.leaf("div", &esc(text), &[a("class", "page_about details")]);
    }

    t.open("div", &[a("class", "entry_list")]);
    for entry in entries {
        write_entry(&mut t, entry);
    }
    t.close("div");

    if !extras.is_empty() {
        t.open("div", &[a("class", "sections")]);
        for (name, label) in extras {
            t.open(
                "a",
                &[
                    a("class", "section block_link other"),
                    a("href", safe_href(name).unwrap_or_else(|| "#".to_string())),
                ],
            );
            t.leaf("div", &esc(label), &[a("class", "label bold")]);
            t.close("a");
        }
        t.close("div");
    }

    t.close("div"); // page_body
    t.close("div"); // page_wrap
    t.close("body");
    t.close("html");
    t.into_string()
}

/// One row: userpic, title, state markers, who opened it, how many messages.
fn write_entry(t: &mut Tree, entry: &IndexEntry) {
    // A refused target becomes `#` rather than being dropped, so the row still
    // renders — but it must never become a link the browser will follow. An
    // export folder is named from a topic title, and `safe_href` is what keeps
    // `//host/x` from resolving to a UNC path when the archive is opened as
    // `file://`.
    let href = safe_href(&entry.href).unwrap_or_else(|| "#".to_string());
    t.open(
        "a",
        &[a("class", "entry block_link clearfix"), a("href", href)],
    );

    t.open("div", &[a("class", "pull_left userpic_wrap")]);
    t.open(
        "div",
        &[
            a("class", format!("userpic userpic{}", entry.colour)),
            a(
                "style",
                format!("width: {USERPIC_SIZE}px; height: {USERPIC_SIZE}px"),
            ),
        ],
    );
    t.leaf(
        "div",
        &esc(&entry.initials),
        &[
            a("class", "initials"),
            a("style", format!("line-height: {USERPIC_SIZE}px")),
        ],
    );
    t.close("div");
    t.close("div");

    t.open("div", &[a("class", "body")]);
    t.leaf("div", &esc(&entry.title), &[a("class", "name bold")]);
    if let Some(detail) = entry.detail.as_deref().filter(|s| !s.is_empty()) {
        t.leaf("div", &esc(detail), &[a("class", "details_entry details")]);
    }
    if let Some(subname) = entry.subname.as_deref().filter(|s| !s.is_empty()) {
        t.leaf("div", &esc(subname), &[a("class", "subname details")]);
    }
    t.leaf(
        "div",
        &esc(&plural(entry.messages, "message")),
        &[a("class", "info details")],
    );
    t.close("div"); // body
    t.close("a");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(href: &str, title: &str) -> IndexEntry {
        IndexEntry {
            href: href.to_string(),
            title: title.to_string(),
            initials: "ć".to_string(),
            colour: 8,
            detail: Some("pinned".to_string()),
            subname: Some("opened by UA KOLAB, 14.12.2025".to_string()),
            messages: 3811,
        }
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tgx-index-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_page_has_every_landmark_the_specimen_has() {
        // **This page has no oracle.** Desktop writes no `export_results.html`
        // at all, because it cannot split a forum into topics — so no leg can
        // judge it. Checked against a specimen export instead, which is what a
        // reader of the archive actually sees.
        let entries = [
            entry("0001 - ćaskanje/messages.html", "ćaskanje"),
            entry("0012 - foto video/messages.html", "foto video"),
        ];
        let out = render(
            "UA KOLAB",
            "4 topics, 6643 messages, 43 members",
            Some("https://t.me/+9rwWjprcqyowNDlk"),
            &entries,
            &[("participants.json", "Members")],
        );

        for landmark in [
            "<!DOCTYPE html>\n<html>",
            "<title>UA KOLAB</title>",
            "<link href=\"css/style.css\" rel=\"stylesheet\"/>",
            "<div class=\"page_wrap\">",
            "<div class=\"page_header\">",
            "<div class=\"text bold\">",
            "<div class=\"page_body list_page\">",
            "<div class=\"page_about details\">",
            "4 topics, 6643 messages, 43 members",
            "https://t.me/+9rwWjprcqyowNDlk",
            "<div class=\"entry_list\">",
            "<div class=\"pull_left userpic_wrap\">",
            "<div class=\"userpic userpic8\" style=\"width: 50px; height: 50px\">",
            "<div class=\"initials\" style=\"line-height: 50px\">",
            "<div class=\"name bold\">",
            "<div class=\"details_entry details\">",
            "<div class=\"subname details\">",
            "<div class=\"info details\">\n3811 messages\n",
            "<div class=\"sections\">",
            "<a class=\"section block_link other\" href=\"participants.json\">",
            "<div class=\"label bold\">\nMembers\n",
        ] {
            assert!(out.contains(landmark), "no {landmark:?} in:\n{out}");
        }

        // One row per topic, and the rows are the entries handed in.
        assert_eq!(
            out.matches("<a class=\"entry block_link clearfix\"")
                .count(),
            2
        );
        assert!(out.contains("href=\"0001 - ćaskanje/messages.html\""));
        assert!(out.contains("href=\"0012 - foto video/messages.html\""));
        // Desktop's list page has no script and no onload; ours must not gain one.
        assert!(
            !out.contains("script"),
            "the index needs no handlers:\n{out}"
        );
        assert!(out.ends_with("</html>\n"));
    }

    #[test]
    fn a_hostile_title_or_target_cannot_inject_markup() {
        // Both a topic title and the folder named from it are attacker-chosen,
        // and this archive opens as a local file, so anything that survives
        // into the page executes with that file's origin.
        let mut e = entry("javascript:alert(1)", "<img src=x onerror=alert(1)>");
        e.initials = "<b>".to_string();
        e.detail = Some("\" onmouseover=\"alert(1)".to_string());
        e.subname = Some("opened by </div><script>alert(1)</script>".to_string());
        let out = render(
            "</title><script>alert(1)</script>",
            "1 topic</div><script>alert(1)</script>",
            Some("javascript:alert(1)"),
            &[e],
            &[],
        );

        // The test is that no *tag* and no *attribute* survives — not that the
        // words do. `onerror=alert(1)` is still in the page, and has to be: it
        // is part of a topic's title and the reader should see the title they
        // were given. What must not exist is a `<` that opens it.
        assert!(!out.contains("<script"), "markup survived:\n{out}");
        assert!(!out.contains("<img"), "markup survived:\n{out}");
        assert!(!out.contains("<b>"), "markup survived:\n{out}");
        assert!(
            !out.contains("onmouseover=\""),
            "an attribute broke out:\n{out}"
        );
        assert!(out.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(out.contains("&lt;/title&gt;&lt;script&gt;"));
        assert!(out.contains("&quot; onmouseover=&quot;alert(1)"));
        // The refused scheme becomes `#`, so the row still renders and the link
        // goes nowhere. The same string is also handed in as `about`, where it
        // is text and survives verbatim — that is the distinction: `safe_href`
        // guards targets, `esc` guards text, and neither does the other's job.
        assert!(out.contains("href=\"#\""), "{out}");
        assert!(
            !out.contains("href=\"javascript:"),
            "a scheme survived:\n{out}"
        );
        assert!(out.contains("\njavascript:alert(1)\n"), "{out}");
    }

    #[test]
    fn a_forum_with_no_topics_still_produces_a_whole_page() {
        // An export stopped before its first topic finished still gets an index,
        // because every page already written links back to it.
        let out = render("UA KOLAB", "0 topics, 0 messages", None, &[], &[]);
        assert!(out.contains("<div class=\"entry_list\">"));
        assert!(!out.contains("class=\"entry block_link"));
        assert!(!out.contains("class=\"sections\""));
        // An absent `about` line leaves exactly one summary div, not an empty
        // second one.
        assert_eq!(out.matches("class=\"page_about details\"").count(), 1);
        // Still balanced: the body and both wrapper divs close.
        assert_eq!(out.matches("<div").count(), out.matches("</div>").count());
        assert!(out.ends_with("</body>\n\n</html>\n"), "{out}");
    }

    #[test]
    fn writing_lays_the_file_down_beside_the_assets_it_links() {
        let dir = tmp("assets");
        write_index(
            &dir,
            "UA KOLAB",
            "1 topic, 7 messages",
            None,
            &[entry("a/messages.html", "a")],
        )
        .unwrap();
        let page = std::fs::read_to_string(dir.join("export_results.html")).unwrap();
        assert!(page.contains("<div class=\"entry_list\">"));
        // The stylesheet the page links has to actually be there.
        assert!(dir.join("css/style.css").is_file());
        assert!(dir.join("images/media_photo.png").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_section_appears_only_for_a_file_that_exists() {
        let dir = tmp("extras");
        write_index(&dir, "UA KOLAB", "1 topic", None, &[]).unwrap();
        let before = std::fs::read_to_string(dir.join("export_results.html")).unwrap();
        assert!(!before.contains("participants.json"), "{before}");

        std::fs::write(dir.join("participants.json"), "[]").unwrap();
        write_index(&dir, "UA KOLAB", "1 topic", None, &[]).unwrap();
        let after = std::fs::read_to_string(dir.join("export_results.html")).unwrap();
        assert!(after.contains("href=\"participants.json\""), "{after}");
        assert!(after.contains("Members"));
        // The two files that were not written stay unlinked.
        assert!(!after.contains("scheduled.json"), "{after}");
        assert!(!after.contains("missing_media.txt"), "{after}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_message_is_singular() {
        assert_eq!(plural(0, "message"), "0 messages");
        assert_eq!(plural(1, "message"), "1 message");
        assert_eq!(plural(4, "topic"), "4 topics");
    }

    #[test]
    fn every_class_this_page_uses_exists_in_desktops_stylesheet() {
        // The original's first index invented `section_chats` and `topics_list`
        // to match a stylesheet that was later thrown away for Desktop's real
        // one. Both had 0 rules there, so the page rendered as unstyled text and
        // nothing failed. Desktop's stylesheet is the vocabulary; this test is
        // the dictionary check.
        let css = crate::assets::STYLE_CSS;
        for class in [
            "page_wrap",
            "page_header",
            "content",
            "text",
            "bold",
            "page_body",
            "list_page",
            "page_about",
            "details",
            "entry_list",
            "entry",
            "block_link",
            "clearfix",
            "pull_left",
            "userpic_wrap",
            "userpic",
            "userpic8",
            "initials",
            "body",
            "name",
            "details_entry",
            "subname",
            "info",
            "sections",
            "section",
            "other",
            "label",
        ] {
            assert!(
                css.contains(&format!(".{class}")),
                "Desktop's stylesheet defines no .{class} — that row renders unstyled"
            );
        }
    }
}
