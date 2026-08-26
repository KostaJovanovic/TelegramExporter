//! Page lifecycle: the chrome around the message stream, and pagination.
//!
//! Desktop paginates into `messages.html`, `messages2.html`, … at 1,000
//! messages a page. The reference's four topics split at 1020 / 1046 / 1025 /
//! 840 — those are not page sizes, they are where the *last* message before the
//! boundary landed, and they come out right only if the counter advances once
//! per message including service messages and date dividers do **not** count.

use crate::escape::esc;
use crate::tree::{a, Tree};

/// `messages.html`, then `messages2.html`, `messages3.html`, …
pub fn page_name(index: usize) -> String {
    if index <= 1 {
        "messages.html".to_string()
    } else {
        format!("messages{index}.html")
    }
}

/// The local zone's **standard** UTC offset, formatted as Desktop writes it.
///
/// Every tooltip carries this, the same one all year. Measured on the
/// reference: 3,752 tooltips all read `UTC+01:00` although the timestamps span
/// both +01:00 and +02:00 and the export was taken during summer time — so it
/// is neither the offset at export time nor the offset at message time.
///
/// Python read `time.timezone`, which is defined as the non-DST offset. chrono
/// has no equivalent, so it is recovered as the **smaller** of the January and
/// July offsets, which is standard time in both hemispheres: northern zones are
/// on DST in July (larger), southern ones in January (larger).
pub fn utc_suffix() -> String {
    use chrono::{Datelike, Local, TimeZone};
    let year = Local::now().year();
    let probe = |month: u32| -> i32 {
        Local
            .with_ymd_and_hms(year, month, 15, 12, 0, 0)
            .single()
            .map(|d| d.offset().local_minus_utc())
            .unwrap_or(0)
    };
    let seconds = probe(1).min(probe(7));
    let minutes = seconds / 60;
    let sign = if minutes >= 0 { '+' } else { '-' };
    format!(
        "UTC{sign}{:02}:{:02}",
        minutes.abs() / 60,
        minutes.abs() % 60
    )
}

/// Everything a page needs to render its header.
pub struct PageChrome {
    pub title: String,
    /// When set, the header title is a link back to a forum's index page.
    /// A Desktop export has no counterpart — splitting by topic is this app's
    /// whole point, so the index is the page a reader opens first.
    pub back_href: Option<String>,
}

/// Write the opening chrome: doctype through `<div class="history">`.
///
/// Reproduced byte for byte from the reference; see `tree::tests`.
pub fn open_page(t: &mut Tree, chrome: &PageChrome, page: usize) {
    t.text("<!DOCTYPE html>");
    t.open("html", &[]);
    t.open("head", &[]);
    t.void("meta", &[a("charset", "utf-8")]);
    t.text("<title>Exported Data</title>");
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
    t.open(
        "script",
        &[a("src", "js/script.js"), a("type", "text/javascript")],
    );
    t.close("script");
    t.close("head");
    t.open("body", &[a("onload", "CheckLocation();")]);
    t.open("div", &[a("class", "page_wrap")]);
    t.open("div", &[a("class", "page_header")]);
    t.open("div", &[a("class", "content")]);
    // The trailing space is Desktop's: its header slot always ends with one.
    match &chrome.back_href {
        Some(href) => t.leaf(
            "a",
            &format!("&lsaquo; {} ", esc(&chrome.title)),
            &[a("class", "text bold"), a("href", href.clone())],
        ),
        None => t.leaf(
            "div",
            &format!("{} ", esc(&chrome.title)),
            &[a("class", "text bold")],
        ),
    }
    t.close("div");
    t.close("div");
    t.open("div", &[a("class", "page_body chat_page")]);
    t.open("div", &[a("class", "history")]);

    if page > 1 {
        t.leaf(
            "a",
            "Previous messages",
            &[
                a("class", "pagination block_link"),
                a("href", page_name(page - 1)),
            ],
        );
    }
}

/// Write the closing chrome.
pub fn close_page(t: &mut Tree, page: usize, has_next: bool) {
    if has_next {
        t.leaf(
            "a",
            "Next messages",
            &[
                a("class", "pagination block_link"),
                a("href", page_name(page + 1)),
            ],
        );
    }
    t.close("div"); // history
    t.close("div"); // page_body
    t.close("div"); // page_wrap
    t.close("body");
    t.close("html");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_page_is_not_numbered() {
        assert_eq!(page_name(0), "messages.html");
        assert_eq!(page_name(1), "messages.html");
        assert_eq!(page_name(2), "messages2.html");
        assert_eq!(page_name(9), "messages9.html");
    }

    #[test]
    fn utc_suffix_has_desktops_shape() {
        let s = utc_suffix();
        assert!(s.starts_with("UTC"), "got {s}");
        assert_eq!(s.len(), 9, "got {s}"); // UTC+01:00
        assert!(s[3..4].contains('+') || s[3..4].contains('-'), "got {s}");
    }

    #[test]
    fn the_header_title_ends_with_desktops_trailing_space() {
        let mut t = Tree::new();
        let chrome = PageChrome {
            title: "bitno pročitaj".into(),
            back_href: None,
        };
        open_page(&mut t, &chrome, 1);
        assert!(
            t.as_str().contains("bitno pročitaj \n"),
            "the trailing space is Desktop's and it is load-bearing:\n{}",
            t.as_str()
        );
    }

    #[test]
    fn a_later_page_links_backwards() {
        let mut t = Tree::new();
        let chrome = PageChrome {
            title: "x".into(),
            back_href: None,
        };
        open_page(&mut t, &chrome, 3);
        assert!(t.as_str().contains("Previous messages"));
        assert!(t.as_str().contains("href=\"messages2.html\""));
    }

    #[test]
    fn the_first_page_has_no_previous_link() {
        let mut t = Tree::new();
        let chrome = PageChrome {
            title: "x".into(),
            back_href: None,
        };
        open_page(&mut t, &chrome, 1);
        assert!(!t.as_str().contains("Previous messages"));
    }

    #[test]
    fn a_page_with_a_successor_links_forwards() {
        let mut t = Tree::new();
        close_page(&mut t, 1, true);
        assert!(t.as_str().contains("href=\"messages2.html\""));
        let mut t2 = Tree::new();
        close_page(&mut t2, 1, false);
        assert!(!t2.as_str().contains("Next messages"));
    }
}
