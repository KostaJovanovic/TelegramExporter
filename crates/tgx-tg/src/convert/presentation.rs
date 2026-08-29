//! The `_p` map: everything the HTML needs that the JSON does not carry.
//!
//! `output.rs` strips this key before writing `result.json`, so it exists only
//! between the converter and the page writer.
//!
//! **The html leg cannot see this module.** It lifts `_p` out of Desktop's own
//! pages and feeds it back in, so the leg reads green whether or not anything
//! here runs -- which is exactly how a live export once emitted zero `<img>`
//! elements against Desktop's 649. `crates/tgx-tg/tests/wire.rs` is the
//! compensating control.

use super::*;

/// The presentation-only map the HTML writer reads and `result.json` never
/// sees.
///
/// **The engine never built this.** `grep '"_p"' crates/tgx-tg/src/` returned
/// nothing, so every key below was absent from every live export, and the
/// writer took its fallback path each time. What that cost, measured on a real
/// run against the same chat Desktop exported:
///
/// * **Zero `<img>` elements in the entire archive** — 649 in Desktop's, 0 in
///   ours. `render_media` gates every inline preview on `_p.preview`, so every
///   photo, sticker, animation and video came out as a coloured placeholder
///   row: 129 `photo_wrap` and 77 `sticker_wrap` in one topic became 0 and 0,
///   and `class="fill"` went from 47 to 306.
/// * Userpic letters derived from the display *string* rather than the name
///   fields, no self-chosen name colours, no forward-date tooltips, and no
///   `reaction active`.
///
/// **The html leg reads 4 of 4 anyway**, because `html_leg.rs` lifts `_p` out
/// of Desktop's own HTML before replaying — it proves the writer, not the
/// pipeline. This function is the pipeline's half, and `tests/wire.rs` is where
/// it is held to the same shape.
///
/// `out` is the message map as it will be written, so the media paths and
/// dimensions the plan already decided are read back from it rather than
/// recomputed from the TL object and allowed to disagree.
pub fn presentation(
    m: &tl::types::Message,
    out: &Map<String, Value>,
    names: &NameBook,
    preview_src: Option<&str>,
) -> Option<Map<String, Value>> {
    let mut p = Map::new();
    let s = |k: &str| out.get(k).and_then(Value::as_str).unwrap_or("");

    // Desktop's HTML name is `first + " " + last` **untrimmed**, which is not
    // the name `result.json` carries — hence a second table rather than a reuse.
    if let Some(from) = &m.from_id {
        let key = peer_key(from).to_string();
        let html = names.html_name(&key);
        if !html.is_empty() {
            p.insert("from_name".into(), json!(html));
        }
    }
    if let Some(fwd) = &m.fwd_from {
        let tl::enums::MessageFwdHeader::Header(fwd) = fwd;
        if let Some(peer) = &fwd.from_id {
            let html = names.html_name(&peer_key(peer).to_string());
            if !html.is_empty() {
                p.insert("forwarded_from_name".into(), json!(html));
            }
        }
        if let Some((date, _)) = tgx_format::date_pair(fwd.date as i64) {
            p.insert("forwarded_date".into(), json!(date));
        }
    }
    // Albums are one block in the HTML and separate messages in the JSON.
    if let Some(g) = m.grouped_id {
        p.insert("group".into(), json!(g.to_string()));
    }

    // The avatars Desktop paints, keyed by the peer each belongs to. A hidden
    // forward has no peer of its own, so Desktop keys it off the message id.
    let mut initials = Map::new();
    let mut colours = Map::new();
    let mut learn = |key: &str| {
        if key.is_empty() {
            return;
        }
        if let Some(v) = names.initials.get(key) {
            initials.insert(key.to_string(), json!(v));
        }
        if let Some(c) = names.colour.get(key) {
            // Zero-based here, one-based in the palette: `userpic_class` adds
            // the one back. The harness records it the same way.
            colours.insert(key.to_string(), json!(*c - 1));
        }
    };
    learn(s("from_id"));
    if out.contains_key("forwarded_from") {
        let fwd_peer = s("forwarded_from_id");
        let own = m.id.to_string();
        learn(if fwd_peer.is_empty() { &own } else { fwd_peer });
    }
    // Everyone Desktop names under a reaction gets an avatar too.
    if let Some(Value::Array(rs)) = out.get("reactions") {
        for r in rs {
            for who in r
                .get("recent")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                learn(who.get("from_id").and_then(Value::as_str).unwrap_or(""));
            }
        }
    }
    if !initials.is_empty() {
        p.insert("initials".into(), Value::Object(initials));
    }
    if !colours.is_empty() {
        p.insert("colours".into(), Value::Object(colours));
    }

    // Which reactions *we* pressed, in the order they appear.
    if let Some(r) = &m.reactions {
        let tl::enums::MessageReactions::Reactions(r) = r;
        let chosen: Vec<Value> = r
            .results
            .iter()
            .map(|e| {
                let tl::enums::ReactionCount::Count(e) = e;
                json!(e.chosen_order.is_some())
            })
            .collect();
        if chosen.iter().any(|v| v == &json!(true)) {
            p.insert("reactions_chosen".into(), Value::Array(chosen));
        }
    }

    if let Some(pv) = preview_of(out, preview_src) {
        p.insert("preview".into(), pv);
    }
    (!p.is_empty()).then_some(p)
}

/// The `<img>` Desktop would put inline for this message, and its CSS size.
///
/// Reads the paths the plan already wrote, so the preview cannot point
/// somewhere the media does not.
///
/// **A file skipped for size is not previewless.** Telegram sends a blur
/// thumbnail *inside* the message — no request, no size limit — and `plan.rs`
/// has always written it to `thumbnails/` and recorded it as
/// `stripped_thumbnail`. `writer.rs:468` has always known how to draw it, from
/// `_p.preview.stripped`. Nothing ever set that key, so the two halves never
/// met: 1,364 JPEGs on disk in the last live run and not one of them displayed,
/// while every skipped file rendered as a bare coloured row. No leg can see it
/// either — Desktop's `result.json` has no `stripped_thumbnail`, so a replay
/// never asks — which is why the test for this lives in `tests/wire.rs`.
fn preview_of(out: &Map<String, Value>, preview_src: Option<&str>) -> Option<Value> {
    let s = |k: &str| out.get(k).and_then(Value::as_str).unwrap_or("");
    let n = |k: &str| out.get(k).and_then(Value::as_i64).unwrap_or(0);
    let is_ph = |v: &str| v.starts_with("(File");

    let sized = |src: &str, box_size: i64| {
        let (_, css) = tgx_html::preview::preview_size(n("width"), n("height"), box_size);
        Some(json!({ "src": src, "width": css.0, "height": css.1 }))
    };

    // First, because the planner only writes this key on the branch that
    // skipped the download — its presence *is* the skip.
    let stripped = s("stripped_thumbnail");
    if !stripped.is_empty() {
        let (_, css) = tgx_html::preview::preview_size(
            n("width"),
            n("height"),
            tgx_html::preview::PREVIEW_BOX,
        );
        return Some(json!({
            "src": stripped,
            "stripped": true,
            "width": css.0,
            "height": css.1,
        }));
    }

    // **The planner's name, never one derived here.** `claim` adds a `(1)` on
    // collision, so a path recomputed from `photo` would silently differ from
    // the file the pool is going to write — the dangling reference this whole
    // change exists to remove, reintroduced one layer up.
    let photo = s("photo");
    if !photo.is_empty() && !is_ph(photo) {
        return sized(preview_src?, tgx_html::preview::PREVIEW_BOX);
    }
    let file = s("file");
    if file.is_empty() || is_ph(file) {
        return None;
    }
    let kind = s("media_type");
    let lower = file.to_lowercase();
    if kind == "sticker" && (lower.ends_with(".webp") || lower.ends_with(".png")) {
        return sized(preview_src?, tgx_html::preview::STICKER_BOX);
    }
    if matches!(kind, "video_file" | "video_message" | "animation") {
        // A video's inline image is Telegram's own thumbnail — the `thumbnail`
        // key, already on the map — not a rendered downscale, and only when we
        // actually saved one.
        let thumb = s("thumbnail");
        if !thumb.is_empty() && !is_ph(thumb) {
            return sized(thumb, tgx_html::preview::PREVIEW_BOX);
        }
    }
    None
}
