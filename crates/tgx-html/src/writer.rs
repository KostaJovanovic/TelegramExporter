//! The streaming writer: `messages.html`, `messages2.html`, …
//!
//! Takes already-serialised message maps rather than Telegram objects, so the
//! HTML and the JSON can never drift apart and the writer is testable with no
//! connection. **Keep it that way** — the moment this module needs a TL type,
//! the two outputs can disagree and the parity harness stops being able to
//! replay a real export through it.

use crate::escape::{esc, message_number, safe_href};
use crate::inline::render_entities;
use crate::join::{parse_date, JoinKey, JoinState};
use crate::media::{file_row, is_placeholder, row_fields};
use crate::page::{close_page, open_page, page_name, utc_suffix, PageChrome};
use crate::preview::MIN_INLINE_PHOTO;
use crate::reactions;
use crate::service::service_text;
use crate::tree::{a, Tree};
use crate::userpic::{userpic, Presentation};
use chrono::{Datelike, NaiveDateTime, Timelike};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub struct HtmlWriter {
    root: PathBuf,
    chrome: PageChrome,
    page_size: usize,
    utc_suffix: String,

    tree: Option<Tree>,
    page: usize,
    on_page: usize,

    last_day: Option<String>,
    join: JoinState,
    last_group: Option<String>,
    /// Which page each message landed on, so a reply can link across pages.
    /// Replies only ever point backwards, so the target is already recorded by
    /// the time one is rendered.
    page_of: HashMap<i64, usize>,
    divider: i64,
}

impl HtmlWriter {
    pub fn new(root: impl AsRef<Path>, title: &str, page_size: usize) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            chrome: PageChrome {
                title: title.to_string(),
                back_href: None,
            },
            page_size: page_size.max(1),
            utc_suffix: utc_suffix(),
            tree: None,
            page: 0,
            on_page: 0,
            last_day: None,
            join: JoinState::default(),
            last_group: None,
            page_of: HashMap::new(),
            divider: 0,
        }
    }

    pub fn with_back_href(mut self, href: Option<String>) -> Self {
        self.chrome.back_href = href;
        self
    }

    fn ensure_page(&mut self) -> io::Result<()> {
        if self.tree.is_some() && self.on_page < self.page_size {
            return Ok(());
        }
        if self.tree.is_some() {
            self.flush(true)?;
        }
        self.page += 1;
        self.on_page = 0;
        // A page break always starts a fresh block and re-states its date.
        self.join.reset();
        self.last_group = None;
        self.last_day = None;
        let mut t = Tree::new();
        open_page(&mut t, &self.chrome, self.page);
        self.tree = Some(t);
        Ok(())
    }

    fn flush(&mut self, has_next: bool) -> io::Result<()> {
        let Some(mut t) = self.tree.take() else {
            return Ok(());
        };
        close_page(&mut t, self.page, has_next);
        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(page_name(self.page));
        std::fs::write(path, t.into_string())?;
        Ok(())
    }

    /// Finish, writing at least one page even for an empty chat.
    ///
    /// Also lays out `css/`, `js/` and `images/` beside the pages. Desktop's
    /// stylesheet references `../images/*.png` for every media-type icon, so
    /// the images are not optional decoration — without them every file row
    /// loses its glyph and the pagination arrows disappear.
    pub fn close(&mut self) -> io::Result<usize> {
        if self.tree.is_none() && self.page == 0 {
            self.ensure_page()?;
        }
        self.flush(false)?;
        crate::assets::write_assets(&self.root)?;
        Ok(self.page)
    }

    fn title_attr(&self, value: Option<&Value>) -> String {
        match parse_date(value) {
            Some(dt) => format!(
                "{:02}.{:02}.{} {:02}:{:02}:{:02} {}",
                dt.day(),
                dt.month(),
                dt.year(),
                dt.hour(),
                dt.minute(),
                dt.second(),
                self.utc_suffix
            ),
            None => String::new(),
        }
    }

    /// An `<a>` pointing at another message, in whichever form fits.
    ///
    /// Same page, or never exported at all: a script call. Another page: a
    /// plain cross-file link, because the script cannot reach it. Measured on
    /// the reference: 1,704 same-page and 11 missing targets take the first
    /// form, 1,513 cross-page targets the second, no exceptions.
    fn message_link(&self, target: Option<&Value>) -> Option<String> {
        let target = target?;
        let Some(number) = message_number(target) else {
            // Not a whole number: no link at all. The id goes straight into an
            // inline script call and escaping is not enough there.
            return Some("this message".to_string());
        };
        match self.page_of.get(&number) {
            Some(&page) if page != self.page => Some(format!(
                "<a href=\"{}#go_to_message{number}\">this message</a>",
                page_name(page)
            )),
            _ => Some(format!(
                "<a href=\"#go_to_message{number}\" onclick=\"return GoToMessage({number})\">this message</a>"
            )),
        }
    }

    pub fn add(&mut self, m: &Map<String, Value>) -> io::Result<()> {
        self.ensure_page()?;
        let p = Presentation::of(m);
        let dt = parse_date(m.get("date"));

        // --- date divider --------------------------------------------------
        if let Some(dt) = dt {
            let day = dt.format("%Y-%m-%d").to_string();
            if self.last_day.as_deref() != Some(day.as_str()) {
                self.divider += 1;
                let divider = self.divider;
                let label = format!(
                    "{} {} {}",
                    dt.day(),
                    MONTHS[(dt.month() - 1) as usize],
                    dt.year()
                );
                let t = self.tree.as_mut().expect("page is open");
                t.open(
                    "div",
                    &[
                        a("class", "message service"),
                        a("id", format!("message-{divider}")),
                    ],
                );
                t.leaf("div", &esc(&label), &[a("class", "body details")]);
                t.close("div");
                self.last_day = Some(day);
                self.join.broke();
            }
        }

        // --- the message itself --------------------------------------------
        if m.get("type").and_then(Value::as_str) == Some("service") {
            let link = self.message_link(m.get("message_id"));
            let body = service_text(m, link.as_deref());
            let id = m.get("id").map(num_str).unwrap_or_default();
            let t = self.tree.as_mut().expect("page is open");
            // Not escaped here: Tree escapes every attribute value it writes,
            // so doing it twice turned an "&" in an id into "&amp;amp;".
            t.open(
                "div",
                &[
                    a("class", "message service"),
                    a("id", format!("message{id}")),
                ],
            );
            t.leaf("div", &body, &[a("class", "body details")]);
            t.close("div");
            self.join.broke();
            self.last_group = None;
        } else {
            let key = JoinKey::of(m);
            let is_forward = m.contains_key("forwarded_from");
            let joined = self.join.joined(&key, dt, is_forward);
            self.render_default(m, &p, dt, joined);
            self.join.advance(key, dt);
            self.last_group = p.str("group").map(String::from);
        }

        if let Some(id) = m.get("id").and_then(Value::as_i64) {
            self.page_of.insert(id, self.page);
        }
        self.on_page += 1;
        Ok(())
    }
}

mod body;

fn num_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn render_one(v: Value) -> String {
        let dir = std::env::temp_dir().join(format!(
            "tgx-writer-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = HtmlWriter::new(&dir, "t", 1000);
        w.add(&obj(v)).unwrap();
        w.close().unwrap();
        let out = std::fs::read_to_string(dir.join("messages.html")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn a_date_divider_opens_the_history() {
        let out = render_one(json!({
            "id": 1, "type": "message", "date": "2025-12-18T16:17:52",
            "from": "A", "from_id": "user1", "text": "hi"
        }));
        assert!(out.contains("id=\"message-1\""), "got:\n{out}");
        assert!(out.contains("18 December 2025"), "got:\n{out}");
    }

    #[test]
    fn a_service_message_renders_its_sentence() {
        let out = render_one(json!({
            "id": 66, "type": "service", "date": "2025-12-18T16:17:52",
            "actor": "kosta kosta", "actor_id": "user1",
            "action": "topic_created", "title": "bitno pročitaj"
        }));
        assert!(out.contains("id=\"message66\""), "got:\n{out}");
        assert!(
            out.contains("kosta kosta created topic &laquo;bitno pročitaj&raquo;"),
            "got:\n{out}"
        );
    }

    #[test]
    fn the_time_is_zero_padded_and_the_tooltip_carries_the_zone() {
        let out = render_one(json!({
            "id": 1, "type": "message", "date": "2025-12-18T09:05:02",
            "from": "A", "from_id": "user1", "text": "x"
        }));
        assert!(out.contains("\n09:05\n"), "got:\n{out}");
        assert!(out.contains("18.12.2025 09:05:02 UTC"), "got:\n{out}");
    }

    #[test]
    fn a_reply_on_the_same_page_uses_the_script_form() {
        let dir = std::env::temp_dir().join("tgx-writer-reply-same");
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = HtmlWriter::new(&dir, "t", 1000);
        w.add(&obj(
            json!({ "id": 5, "type": "message", "date": "2025-12-18T16:00:00",
                           "from": "A", "from_id": "user1", "text": "first" }),
        ))
        .unwrap();
        w.add(&obj(
            json!({ "id": 6, "type": "message", "date": "2025-12-18T16:00:01",
                           "from": "B", "from_id": "user2", "text": "second",
                           "reply_to_message_id": 5 }),
        ))
        .unwrap();
        w.close().unwrap();
        let out = std::fs::read_to_string(dir.join("messages.html")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            out.contains("onclick=\"return GoToMessage(5)\""),
            "got:\n{out}"
        );
    }

    #[test]
    fn a_reply_across_a_page_boundary_uses_a_plain_link() {
        let dir = std::env::temp_dir().join("tgx-writer-reply-cross");
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = HtmlWriter::new(&dir, "t", 1); // one message per page
        w.add(&obj(
            json!({ "id": 5, "type": "message", "date": "2025-12-18T16:00:00",
                           "from": "A", "from_id": "user1", "text": "first" }),
        ))
        .unwrap();
        w.add(&obj(
            json!({ "id": 6, "type": "message", "date": "2025-12-18T16:00:01",
                           "from": "B", "from_id": "user2", "text": "second",
                           "reply_to_message_id": 5 }),
        ))
        .unwrap();
        w.close().unwrap();
        let out = std::fs::read_to_string(dir.join("messages2.html")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            out.contains("href=\"messages.html#go_to_message5\""),
            "got:\n{out}"
        );
        assert!(!out.contains("GoToMessage"), "got:\n{out}");
    }

    #[test]
    fn a_reply_to_another_chat_gets_no_link_at_all() {
        let out = render_one(json!({
            "id": 6, "type": "message", "date": "2025-12-18T16:00:01",
            "from": "B", "from_id": "user2", "text": "x",
            "reply_to_message_id": 5, "reply_to_peer_id": "channel9"
        }));
        assert!(
            out.contains("In reply to a message in another chat"),
            "got:\n{out}"
        );
        assert!(!out.contains("<a "), "got:\n{out}");
    }

    #[test]
    fn a_hostile_reply_target_drops_the_link_rather_than_interpolating() {
        let out = render_one(json!({
            "id": 6, "type": "message", "date": "2025-12-18T16:00:01",
            "from": "B", "from_id": "user2", "text": "x",
            "reply_to_message_id": "5); alert(1); //"
        }));
        assert!(!out.contains("alert(1)"), "got:\n{out}");
        assert!(out.contains("In reply to this message"), "got:\n{out}");
    }

    #[test]
    fn a_second_message_from_the_same_sender_joins() {
        let dir = std::env::temp_dir().join("tgx-writer-join");
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = HtmlWriter::new(&dir, "t", 1000);
        for (id, when) in [(1, "16:00:00"), (2, "16:00:30")] {
            w.add(&obj(json!({ "id": id, "type": "message",
                               "date": format!("2025-12-18T{when}"),
                               "from": "A", "from_id": "user1", "text": "x" })))
                .unwrap();
        }
        w.close().unwrap();
        let out = std::fs::read_to_string(dir.join("messages.html")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(out.matches("message default clearfix joined").count(), 1);
        // The joined message has no userpic and no from_name of its own.
        assert_eq!(out.matches("userpic_wrap").count(), 1);
        assert_eq!(out.matches("class=\"from_name\"").count(), 1);
    }

    #[test]
    fn pagination_splits_and_links_both_ways() {
        let dir = std::env::temp_dir().join("tgx-writer-pages");
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = HtmlWriter::new(&dir, "t", 2);
        for id in 1..=5 {
            w.add(&obj(json!({ "id": id, "type": "message",
                               "date": "2025-12-18T16:00:00",
                               "from": "A", "from_id": "user1", "text": "x" })))
                .unwrap();
        }
        let pages = w.close().unwrap();
        assert_eq!(pages, 3);
        let p1 = std::fs::read_to_string(dir.join("messages.html")).unwrap();
        let p2 = std::fs::read_to_string(dir.join("messages2.html")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(p1.contains("Next messages"));
        assert!(!p1.contains("Previous messages"));
        assert!(p2.contains("Previous messages") && p2.contains("Next messages"));
    }

    #[test]
    fn an_empty_chat_still_produces_a_page() {
        let dir = std::env::temp_dir().join("tgx-writer-empty");
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = HtmlWriter::new(&dir, "t", 1000);
        let pages = w.close().unwrap();
        assert_eq!(pages, 1);
        assert!(dir.join("messages.html").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sticker_renders_as_an_inline_image() {
        let out = render_one(json!({
            "id": 1, "type": "message", "date": "2025-12-18T16:00:00",
            "from": "A", "from_id": "user1",
            "file": "stickers/sticker.webp", "media_type": "sticker",
            "width": 512, "height": 512,
            "_p": { "preview": { "src": "stickers/sticker_thumb.webp",
                                 "width": 192, "height": 192 } }
        }));
        assert!(
            out.contains("class=\"sticker_wrap clearfix pull_left\""),
            "got:\n{out}"
        );
        assert!(
            out.contains("style=\"width: 192px; height: 192px\""),
            "got:\n{out}"
        );
    }

    #[test]
    fn a_preview_src_goes_through_safe_href_like_every_other_url() {
        // Not reachable from a message today — the src is a path our own
        // planner built — but `escape.rs` allows no exception, and this is the
        // <img> path that used to be one.
        let out = render_one(json!({
            "id": 1, "type": "message", "date": "2025-12-18T16:00:00",
            "from": "A", "from_id": "user1",
            "file": "stickers/sticker.webp", "media_type": "sticker",
            "width": 512, "height": 512,
            "_p": { "preview": { "src": "javascript:alert(1)",
                                 "width": 192, "height": 192 } }
        }));
        assert!(!out.contains("javascript:"), "got:\n{out}");
        assert!(out.contains("src=\"\""), "got:\n{out}");
        // A host-relative target resolves to a UNC path under file://.
        let unc = render_one(json!({
            "id": 1, "type": "message", "date": "2025-12-18T16:00:00",
            "from": "A", "from_id": "user1",
            "file": "stickers/sticker.webp", "media_type": "sticker",
            "width": 512, "height": 512,
            "_p": { "preview": { "src": "//evil.example/x.png",
                                 "width": 192, "height": 192 } }
        }));
        assert!(!unc.contains("evil.example"), "got:\n{unc}");
    }

    #[test]
    fn an_ordinary_preview_src_is_unchanged() {
        // safe_href trims and scheme-checks; it must not touch a real path,
        // spaces included — Desktop writes "stickers/sticker (55).webp".
        let out = render_one(json!({
            "id": 1, "type": "message", "date": "2025-12-18T16:00:00",
            "from": "A", "from_id": "user1",
            "file": "stickers/sticker (55).webp", "media_type": "sticker",
            "width": 512, "height": 512,
            "_p": { "preview": { "src": "stickers/sticker (55).webp",
                                 "width": 192, "height": 192 } }
        }));
        assert!(
            out.contains("src=\"stickers/sticker (55).webp\""),
            "got:\n{out}"
        );
    }

    #[test]
    fn a_video_shows_its_play_button_and_duration() {
        let out = render_one(json!({
            "id": 1, "type": "message", "date": "2025-12-18T16:00:00",
            "from": "A", "from_id": "user1",
            "file": "video_files/a.mp4", "media_type": "video_file",
            "duration_seconds": 65, "width": 640, "height": 480,
            "_p": { "preview": { "src": "t.jpg", "width": 260, "height": 195 } }
        }));
        assert!(out.contains("class=\"video_play\""), "got:\n{out}");
        assert!(out.contains("01:05"), "got:\n{out}");
    }

    #[test]
    fn a_small_photo_becomes_a_row_not_an_inline_image() {
        // 260x74: below MIN_INLINE_PHOTO in one direction.
        let out = render_one(json!({
            "id": 1, "type": "message", "date": "2025-12-18T16:00:00",
            "from": "A", "from_id": "user1",
            "photo": "photos/photo_1.jpg", "width": 260, "height": 74,
            "photo_file_size": 3079,
            "_p": { "preview": { "src": "photos/photo_1_thumb.jpg",
                                 "width": 260, "height": 74 } }
        }));
        assert!(out.contains("media_photo"), "got:\n{out}");
        assert!(!out.contains("photo_wrap"), "got:\n{out}");
        assert!(out.contains("260×74"), "got:\n{out}");
    }

    #[test]
    fn a_page_break_restates_the_date_and_breaks_the_block() {
        let dir = std::env::temp_dir().join("tgx-writer-restate");
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = HtmlWriter::new(&dir, "t", 1);
        for id in 1..=2 {
            w.add(&obj(json!({ "id": id, "type": "message",
                               "date": "2025-12-18T16:00:00",
                               "from": "A", "from_id": "user1", "text": "x" })))
                .unwrap();
        }
        w.close().unwrap();
        let p2 = std::fs::read_to_string(dir.join("messages2.html")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(p2.contains("18 December 2025"), "got:\n{p2}");
        assert!(
            !p2.contains("joined"),
            "a page break must start a fresh block"
        );
    }
}
