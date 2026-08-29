//! Rendering one ordinary message into the tree: the header, the reply
//! quote, the media block and the text.
//!
//! Split from the writer itself because the two do different jobs. The parent
//! module owns the page lifecycle -- opening, paginating, flushing, closing --
//! and this owns turning a single serialised message map into markup.
//!
//! Same rule as the parent: **no Telegram types here**. This module sees only
//! already-serialised maps, which is what lets the html leg replay a real
//! export through it with no connection.

use super::*;

impl HtmlWriter {
    pub(super) fn render_default(
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
