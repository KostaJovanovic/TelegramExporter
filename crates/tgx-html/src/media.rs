//! Media rows: the coloured block Desktop draws for anything it does not inline.
//!
//! Two rules here were measured against the reference and are easy to get
//! wrong in a way that reads as plausible:
//!
//! * **Durations are shown for video, round video, voice and audio — not for
//!   animations.** An animation is a silent loop, so Desktop prints its size
//!   alone even though it shares the `media_video` styling with real videos.
//! * **Sizes are shown everywhere except voice messages**, which are described
//!   by their length alone.

use crate::escape::{esc, safe_href};
use crate::tree::{a, Tree};
use serde_json::{Map, Value};
use tgx_format::size::{human_duration, human_size};

/// Desktop's HTML wording for a file it did not save.
///
/// The JSON keeps the longer parenthesised form; these are the shorter
/// sentences shown inside a media row.
const TOO_LARGE_HTML: &str = "Exceeds maximum size, change data exporting settings to download.";
const NOT_INCLUDED_HTML: &str = "Not included, change data exporting settings to download.";
const UNAVAILABLE_HTML: &str = "Unavailable, please try again later.";

/// Every placeholder Desktop writes into `file` / `photo` starts with this.
pub const PLACEHOLDER_PREFIX: &str = "(File";

/// Media Desktop shows as a coloured row rather than an inline preview.
///
/// `media_type -> (row css class, fixed title)`. A `None` title means the row
/// titles itself from the file name.
fn row_style(media_type: &str) -> (&'static str, Option<&'static str>) {
    match media_type {
        "video_file" => ("media_video", Some("Video file")),
        "video_message" => ("media_video", Some("Video message")),
        "animation" => ("media_video", Some("Animation")),
        "voice_message" => ("media_voice_message", Some("Voice message")),
        "audio_file" => ("media_audio_file", None),
        "sticker" => ("media_photo", Some("Sticker")),
        "dice" => ("media_file", Some("Dice")),
        _ => ("media_file", None),
    }
}

pub fn placeholder_text(value: &str) -> &'static str {
    let lowered = value.to_lowercase();
    if lowered.contains("exceeds maximum size") {
        TOO_LARGE_HTML
    } else if lowered.contains("not included") {
        NOT_INCLUDED_HTML
    } else {
        UNAVAILABLE_HTML
    }
}

pub fn is_placeholder(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|s| s.starts_with(PLACEHOLDER_PREFIX))
}

/// One media row's content.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub css: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub href: Option<String>,
}

fn s(m: &Map<String, Value>, k: &str) -> String {
    m.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn i(m: &Map<String, Value>, k: &str) -> i64 {
    match m.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(t)) => t.parse().unwrap_or(0),
        _ => 0,
    }
}

fn join_nonempty(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

/// `(css class, title, description, status, href)` for a media row, or `None`
/// when the message carries nothing that gets one.
pub fn row_fields(m: &Map<String, Value>) -> Option<Row> {
    let media_type = s(m, "media_type");

    // --- photo -------------------------------------------------------------
    if let Some(photo) = m.get("photo") {
        let placeholder = is_placeholder(Some(photo));
        // A photo row leads with its pixel dimensions: "2560×1706, 1.0 MB".
        let dims = if i(m, "width") > 0 && i(m, "height") > 0 {
            format!("{}×{}", i(m, "width"), i(m, "height"))
        } else {
            String::new()
        };
        // The byte size joins the dimensions only when the file was *not*
        // saved. A photo on disk but too small to inline shows its dimensions
        // alone — measured, 260×74 with a known 3,079-byte size.
        let size = if placeholder {
            human_size(i(m, "photo_file_size"))
        } else {
            String::new()
        };
        let path = photo.as_str().unwrap_or_default().to_string();
        return Some(Row {
            css: "media_photo".into(),
            title: "Photo".into(),
            description: if placeholder {
                placeholder_text(&path).into()
            } else {
                String::new()
            },
            status: join_nonempty(&[dims, size]),
            href: if placeholder { None } else { Some(path) },
        });
    }

    // --- document ----------------------------------------------------------
    if let Some(file) = m.get("file") {
        let placeholder = is_placeholder(Some(file));
        let (css, fixed) = row_style(&media_type);
        let title = match fixed {
            Some(t) => t.to_string(),
            None => {
                let base = if !s(m, "title").is_empty() {
                    s(m, "title")
                } else if !s(m, "file_name").is_empty() {
                    s(m, "file_name")
                } else {
                    "File".into()
                };
                let performer = s(m, "performer");
                if performer.is_empty() {
                    base
                } else {
                    format!("{performer} - {base}")
                }
            }
        };
        let status = if media_type == "sticker" {
            s(m, "sticker_emoji")
        } else {
            // Only a row that is *about* a duration shows one.
            let timed = matches!(
                media_type.as_str(),
                "video_file" | "video_message" | "voice_message" | "audio_file"
            );
            // A voice message is described by its length alone.
            let sized = media_type != "voice_message";
            join_nonempty(&[
                if timed {
                    human_duration(i(m, "duration_seconds"))
                } else {
                    String::new()
                },
                if sized {
                    human_size(i(m, "file_size"))
                } else {
                    String::new()
                },
            ])
        };
        let path = file.as_str().unwrap_or_default().to_string();
        return Some(Row {
            css: css.into(),
            title,
            description: if placeholder {
                placeholder_text(&path).into()
            } else {
                String::new()
            },
            status,
            href: if placeholder { None } else { Some(path) },
        });
    }

    // --- contact -----------------------------------------------------------
    if let Some(Value::Object(c)) = m.get("contact_information") {
        let name = join_space(&[s(c, "first_name"), s(c, "last_name")]);
        return Some(Row {
            css: "media_contact".into(),
            title: if name.is_empty() {
                "Contact".into()
            } else {
                name
            },
            description: String::new(),
            status: s(c, "phone_number"),
            href: None,
        });
    }

    // --- location ----------------------------------------------------------
    if let Some(Value::Object(loc)) = m.get("location_information") {
        let lat = loc.get("latitude").map(num_str).unwrap_or_default();
        let lon = loc.get("longitude").map(num_str).unwrap_or_default();
        let live = m.contains_key("live_location_period_seconds");
        let css = if live {
            "media_live_location"
        } else {
            "media_location"
        };
        let place = s(m, "place_name");
        let title = if !place.is_empty() {
            place
        } else if live {
            "Live location".into()
        } else {
            "Location".into()
        };
        return Some(Row {
            css: css.into(),
            title,
            description: s(m, "address"),
            status: format!("{lat}, {lon}"),
            href: Some(format!(
                "https://maps.google.com/maps?q={lat},{lon}&ll={lat},{lon}&z=16"
            )),
        });
    }

    // --- game --------------------------------------------------------------
    if !s(m, "game_title").is_empty() {
        return Some(Row {
            css: "media_game".into(),
            title: s(m, "game_title"),
            description: s(m, "game_description"),
            status: String::new(),
            href: m.get("game_link").and_then(Value::as_str).map(String::from),
        });
    }

    // --- invoice -----------------------------------------------------------
    if let Some(Value::Object(inv)) = m.get("invoice_information") {
        let title = s(inv, "title");
        return Some(Row {
            css: "media_invoice".into(),
            title: if title.is_empty() {
                "Invoice".into()
            } else {
                title
            },
            description: s(inv, "description"),
            status: format!("{} {}", s(inv, "amount"), s(inv, "currency"))
                .trim()
                .to_string(),
            href: None,
        });
    }

    None
}

fn join_space(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

fn num_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Emit one media row.
///
/// The href is **vetted here rather than trusted from the caller**. Most of
/// what reaches this is a path this app generated, but not all of it: a link
/// preview's `url` is copied verbatim off the wire, and an archive opens as a
/// local file, so an unvetted scheme runs with that origin. A rejected target
/// degrades to the unlinked `<div>` form, which is the same shape Desktop
/// already uses for a file it did not save — so parity is unaffected.
pub fn file_row(t: &mut Tree, row: &Row) {
    let href = row.href.as_deref().and_then(safe_href);
    t.open("div", &[a("class", "media_wrap clearfix")]);
    match &href {
        Some(h) => t.open(
            "a",
            &[
                a(
                    "class",
                    format!("media clearfix pull_left block_link {}", row.css),
                ),
                a("href", h.clone()),
            ],
        ),
        None => t.open(
            "div",
            &[a("class", format!("media clearfix pull_left {}", row.css))],
        ),
    }
    // Desktop leaves this one bare — the css class goes on the outer element.
    t.open("div", &[a("class", "fill pull_left")]);
    t.close("div");
    t.open("div", &[a("class", "body")]);
    t.leaf("div", &esc(&row.title), &[a("class", "title bold")]);
    if !row.description.is_empty() {
        t.leaf("div", &esc(&row.description), &[a("class", "description")]);
    }
    t.leaf("div", &esc(&row.status), &[a("class", "status details")]);
    t.close("div");
    t.close(if href.is_some() { "a" } else { "div" });
    t.close("div");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn an_animation_shows_its_size_but_no_duration() {
        // It shares media_video styling with real videos, and that is exactly
        // why the duration rule is easy to get wrong.
        let m = obj(json!({
            "file": "video_files/a.mp4", "media_type": "animation",
            "duration_seconds": 12, "file_size": 1048576
        }));
        let row = row_fields(&m).unwrap();
        assert_eq!(row.css, "media_video");
        assert_eq!(row.title, "Animation");
        assert_eq!(row.status, "1.0 MB");
        assert!(!row.status.contains("00:12"), "got {}", row.status);
    }

    #[test]
    fn a_real_video_shows_both() {
        let m = obj(json!({
            "file": "video_files/a.mp4", "media_type": "video_file",
            "duration_seconds": 12, "file_size": 1048576
        }));
        assert_eq!(row_fields(&m).unwrap().status, "00:12, 1.0 MB");
    }

    #[test]
    fn a_voice_message_shows_its_length_alone() {
        // Its byte size says nothing a listener wants.
        let m = obj(json!({
            "file": "voice_messages/a.ogg", "media_type": "voice_message",
            "duration_seconds": 65, "file_size": 1048576
        }));
        let row = row_fields(&m).unwrap();
        assert_eq!(row.css, "media_voice_message");
        assert_eq!(row.status, "01:05");
    }

    #[test]
    fn a_saved_photo_shows_dimensions_without_a_size() {
        // Measured: 260x74 with a known 3079-byte size shows dimensions alone.
        let m = obj(json!({
            "photo": "photos/photo_1.jpg", "width": 260, "height": 74,
            "photo_file_size": 3079
        }));
        let row = row_fields(&m).unwrap();
        assert_eq!(row.status, "260×74");
        assert_eq!(row.href.as_deref(), Some("photos/photo_1.jpg"));
    }

    #[test]
    fn a_skipped_photo_gains_its_size_and_loses_its_link() {
        let m = obj(json!({
            "photo": "(File exceeds maximum size. Change data exporting settings to download.)",
            "width": 2560, "height": 1706, "photo_file_size": 1940744
        }));
        let row = row_fields(&m).unwrap();
        assert_eq!(row.status, "2560×1706, 1.8 MB");
        assert_eq!(row.href, None);
        assert_eq!(row.description, TOO_LARGE_HTML);
    }

    #[test]
    fn placeholder_wording_matches_desktops_three_sentences() {
        assert_eq!(
            placeholder_text("(File exceeds maximum size. ...)"),
            TOO_LARGE_HTML
        );
        assert_eq!(
            placeholder_text("(File not included. ...)"),
            NOT_INCLUDED_HTML
        );
        assert_eq!(placeholder_text("(File unavailable...)"), UNAVAILABLE_HTML);
    }

    #[test]
    fn an_audio_file_titles_itself_from_performer_and_title() {
        let m = obj(json!({
            "file": "files/a.mp3", "media_type": "audio_file",
            "title": "Song", "performer": "Band", "file_size": 1024
        }));
        assert_eq!(row_fields(&m).unwrap().title, "Band - Song");
    }

    #[test]
    fn an_unnamed_document_falls_back_to_file() {
        let m = obj(json!({ "file": "files/x", "media_type": "unknown_kind" }));
        let row = row_fields(&m).unwrap();
        assert_eq!(row.title, "File");
        assert_eq!(row.css, "media_file");
    }

    #[test]
    fn a_message_with_no_media_gets_no_row() {
        assert_eq!(row_fields(&obj(json!({ "id": 1, "text": "hi" }))), None);
    }

    #[test]
    fn a_dangerous_href_degrades_to_the_unlinked_form() {
        // A link preview's url is copied verbatim off the wire.
        let mut t = Tree::new();
        file_row(
            &mut t,
            &Row {
                css: "media_file".into(),
                title: "x".into(),
                description: String::new(),
                status: String::new(),
                href: Some("javascript:alert(1)".into()),
            },
        );
        assert!(!t.as_str().contains("<a "), "got:\n{}", t.as_str());
        assert!(!t.as_str().contains("javascript"), "got:\n{}", t.as_str());
    }

    #[test]
    fn a_relative_path_with_a_space_still_links() {
        let mut t = Tree::new();
        file_row(
            &mut t,
            &Row {
                css: "media_photo".into(),
                title: "Sticker".into(),
                description: String::new(),
                status: String::new(),
                href: Some("stickers/sticker (55).webp".into()),
            },
        );
        assert!(
            t.as_str().contains("href=\"stickers/sticker (55).webp\""),
            "got:\n{}",
            t.as_str()
        );
    }

    #[test]
    fn the_fill_div_stays_bare() {
        // Desktop puts the css class on the outer element and leaves
        // <div class="fill pull_left"> alone.
        let mut t = Tree::new();
        file_row(
            &mut t,
            &Row {
                css: "media_video".into(),
                title: "Video file".into(),
                description: String::new(),
                status: String::new(),
                href: None,
            },
        );
        assert!(t.as_str().contains("<div class=\"fill pull_left\">"));
    }

    #[test]
    fn an_empty_status_still_emits_its_div() {
        // Desktop always writes the status row, even blank.
        let mut t = Tree::new();
        file_row(
            &mut t,
            &Row {
                css: "media_file".into(),
                title: "x".into(),
                description: String::new(),
                status: String::new(),
                href: None,
            },
        );
        assert!(t.as_str().contains("class=\"status details\""));
    }
}
