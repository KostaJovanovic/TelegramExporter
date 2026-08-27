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

    fn render_default(
        &mut self,
        m: &Map<String, Value>,
        p: &Presentation,
        dt: Option<NaiveDateTime>,
        joined: bool,
    ) {
        let name = p
            .str("from_name")
            .map(String::from)
            .or_else(|| m.get("from").and_then(Value::as_str).map(String::from))
            .unwrap_or_default();
        let from_id = m.get("from_id").map(num_str).unwrap_or_default();
        let id = m.get("id").map(num_str).unwrap_or_default();

        let forwarded = m.contains_key("forwarded_from");
        let fwd_name = p
            .str("forwarded_from_name")
            .map(String::from)
            .or_else(|| {
                m.get("forwarded_from")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .unwrap_or_default();

        // A joined forward still repeats the "forwarded from X" header unless
        // it belongs to the same album as the message above it. Measured on
        // 573 joined forwards: 480 drop the header and 93 keep it, and neither
        // the send gap nor the original date splits them — 53 adjacent pairs
        // share an original timestamp to the second and both carry a header.
        let group = p.str("group").map(String::from);
        let same_album = group.is_some() && group == self.last_group;
        let show_header = forwarded && (!joined || !same_album);

        let date_title = self.title_attr(m.get("date"));
        let fwd_stamp_title = self.title_attr(p.get("forwarded_date"));
        let reply_link = self.message_link(m.get("reply_to_message_id"));

        let t = self.tree.as_mut().expect("page is open");
        t.open(
            "div",
            &[
                a(
                    "class",
                    format!(
                        "message default clearfix{}",
                        if joined { " joined" } else { "" }
                    ),
                ),
                a("id", format!("message{id}")),
            ],
        );
        if !joined {
            userpic(t, p, &name, &from_id, 42, "");
        }

        t.open("div", &[a("class", "body")]);
        if let Some(dt) = dt {
            t.leaf(
                "div",
                &esc(&format!("{:02}:{:02}", dt.hour(), dt.minute())),
                &[
                    a("class", "pull_right date details"),
                    a("title", date_title),
                ],
            );
        }
        if !joined && !name.is_empty() {
            let via = m.get("via_bot").and_then(Value::as_str).unwrap_or("");
            let extra = if via.is_empty() {
                String::new()
            } else {
                format!(" <span class=\"details\">via {}</span>", esc(via))
            };
            t.leaf(
                "div",
                &format!("{}{extra}", esc(&name)),
                &[a("class", "from_name")],
            );
        }

        if show_header {
            // A forward from someone who hides their account carries no peer,
            // and Desktop colours that avatar from the *message* id instead.
            let fwd_peer = m
                .get("forwarded_from_id")
                .map(num_str)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| id.clone());
            userpic(t, p, &fwd_name, &fwd_peer, 42, "forwarded");
        }

        if forwarded {
            t.open("div", &[a("class", "forwarded body")]);
            if show_header {
                let span = match parse_date(p.get("forwarded_date")) {
                    Some(shown) => format!(
                        "<span class=\"date details\" title=\"{}\"> {}</span>",
                        esc(&fwd_stamp_title),
                        esc(&format!(
                            "{:02}.{:02}.{} {:02}:{:02}:{:02}",
                            shown.day(),
                            shown.month(),
                            shown.year(),
                            shown.hour(),
                            shown.minute(),
                            shown.second()
                        ))
                    ),
                    None => String::new(),
                };
                t.leaf(
                    "div",
                    &format!("{}{span}", esc(&fwd_name)),
                    &[a("class", "from_name")],
                );
            }
        }

        render_reply(t, m, reply_link.as_deref());
        render_media(t, m, p);

        let text_html = render_entities(m.get("text_entities").or_else(|| m.get("text")));
        if !text_html.is_empty() {
            t.leaf("div", &text_html, &[a("class", "text")]);
        }

        if forwarded {
            t.close("div"); // forwarded body
        }

        reactions::render(t, m, p);
        t.close("div"); // body
        t.close("div"); // message
    }
}

fn num_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn int_of(m: &Map<String, Value>, k: &str) -> i64 {
    match m.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn render_reply(t: &mut Tree, m: &Map<String, Value>, link: Option<&str>) {
    if m.get("reply_to_message_id").is_none() {
        return;
    }
    // Telegram marks a reply whose target lives outside this history, and
    // Desktop does not pretend it can link there.
    if m.contains_key("reply_to_peer_id") {
        t.leaf(
            "div",
            "In reply to a message in another chat",
            &[a("class", "reply_to details")],
        );
        return;
    }
    let link = link.unwrap_or("this message");
    t.leaf(
        "div",
        &format!("In reply to {link}"),
        &[a("class", "reply_to details")],
    );
    // Replying to a *fragment* quotes it. The reply id alone points at the
    // whole message, so without the quote there is no record of which part was
    // being answered. Desktop keeps neither.
    let quote = m
        .get("reply_to_quote_entities")
        .or_else(|| m.get("reply_to_quote"));
    let quote_html = render_entities(quote);
    if !quote_html.is_empty() {
        t.leaf(
            "div",
            &quote_html,
            &[
                a("class", "reply_to details"),
                a(
                    "style",
                    "border-left: 2px solid #3892db; padding-left: 6px; margin-left: 2px",
                ),
            ],
        );
    }
}

fn preview_of<'a>(p: &Presentation<'a>) -> Option<&'a Map<String, Value>> {
    p.get("preview")?.as_object()
}

fn render_media(t: &mut Tree, m: &Map<String, Value>, p: &Presentation) {
    let preview = preview_of(p);
    let media_type = m
        .get("media_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let style = preview
        .map(|pv| {
            format!(
                "width: {}px; height: {}px",
                int_of(pv, "width"),
                int_of(pv, "height")
            )
        })
        .unwrap_or_default();
    // `escape.rs` states the rule absolutely: every URL that reaches markup goes
    // through `safe_href`. This one did not. **It is not reachable from hostile
    // input today** — the preview `src` is a path our own media planner built,
    // and the parity harness lifts it out of a reference export — so this closes
    // the gap between the rule and the code rather than a live hole. The five
    // `<img>` sites below all read this one binding, so there is no second place
    // for the next one to be forgotten.
    let src = preview
        .and_then(|pv| pv.get("src"))
        .and_then(Value::as_str)
        .and_then(safe_href)
        .unwrap_or_default();

    // A file we did not save, whose blur preview came free inside the message.
    // Both are drawn: the image shows what the file was, the row below it still
    // says why it is not here.
    if let Some(pv) = preview {
        if pv.get("stripped").and_then(Value::as_bool) == Some(true) {
            t.open("div", &[a("class", "media_wrap clearfix")]);
            t.open("div", &[a("class", "photo_wrap clearfix pull_left")]);
            t.void(
                "img",
                &[
                    a("class", "photo"),
                    a("src", src.clone()),
                    a("style", style.clone()),
                ],
            );
            t.close("div");
            t.close("div");
            if let Some(row) = row_fields(m) {
                file_row(t, &row);
            }
            return;
        }
    }

    let photo = m.get("photo");
    let file = m.get("file");

    if let (Some(photo), Some(_)) = (photo, preview) {
        let big_enough = int_of(m, "width").min(int_of(m, "height")) >= MIN_INLINE_PHOTO;
        if !is_placeholder(Some(photo)) && big_enough {
            t.open("div", &[a("class", "media_wrap clearfix")]);
            let href = photo.as_str().and_then(safe_href).unwrap_or_default();
            t.open(
                "a",
                &[a("class", "photo_wrap clearfix pull_left"), a("href", href)],
            );
            t.void(
                "img",
                &[a("class", "photo"), a("src", src), a("style", style)],
            );
            t.close("a");
            t.close("div");
            return;
        }
    }

    if let (Some(file), Some(_)) = (file, preview) {
        if !is_placeholder(Some(file)) {
            let href = file.as_str().and_then(safe_href).unwrap_or_default();
            match media_type.as_str() {
                "sticker" => {
                    t.open("div", &[a("class", "media_wrap clearfix")]);
                    t.open(
                        "a",
                        &[
                            a("class", "sticker_wrap clearfix pull_left"),
                            a("href", href),
                        ],
                    );
                    t.void(
                        "img",
                        &[a("class", "sticker"), a("src", src), a("style", style)],
                    );
                    t.close("a");
                    t.close("div");
                    return;
                }
                "animation" => {
                    t.open("div", &[a("class", "media_wrap clearfix")]);
                    t.open(
                        "a",
                        &[
                            a("class", "animated_wrap clearfix pull_left"),
                            a("href", href),
                        ],
                    );
                    t.open("div", &[a("class", "video_play_bg")]);
                    t.leaf("div", "GIF", &[a("class", "gif_play")]);
                    t.close("div");
                    t.void(
                        "img",
                        &[a("class", "animated"), a("src", src), a("style", style)],
                    );
                    t.close("a");
                    t.close("div");
                    return;
                }
                "video_file" | "video_message" => {
                    t.open("div", &[a("class", "media_wrap clearfix")]);
                    t.open(
                        "a",
                        &[
                            a("class", "video_file_wrap clearfix pull_left"),
                            a("href", href),
                        ],
                    );
                    t.open("div", &[a("class", "video_play_bg")]);
                    t.open("div", &[a("class", "video_play")]);
                    t.close("div");
                    t.close("div");
                    let duration = tgx_format::size::human_duration(int_of(m, "duration_seconds"));
                    if !duration.is_empty() {
                        t.leaf("div", &esc(&duration), &[a("class", "video_duration")]);
                    }
                    t.void(
                        "img",
                        &[a("class", "video_file"), a("src", src), a("style", style)],
                    );
                    t.close("a");
                    t.close("div");
                    return;
                }
                _ => {}
            }
        }
    }

    // A poll sits after every inline form and before the generic row, which is
    // where Desktop puts it.
    if let Some(Value::Object(poll)) = m.get("poll") {
        crate::poll::render(t, poll);
        return;
    }

    if let Some(row) = row_fields(m) {
        file_row(t, &row);
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
