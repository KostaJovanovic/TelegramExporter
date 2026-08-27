//! The converter, driven against synthetic TL objects.
//!
//! **This is not a substitute for a live run**, and it should not be read as
//! one: it proves the mapping given an input, not that Telegram sends that
//! input. What it does buy is coverage of the two modules the parity harness
//! structurally cannot reach — `convert.rs` and `plan.rs` — with the shapes
//! most likely to break them.
//!
//! Every case below is drawn from something the reference export or the Python
//! notes record as having actually happened.

mod fixtures;

use fixtures::*;
use grammers_tl_types as tl;
use serde_json::Value;
use tgx_media::names::NameBook;
use tgx_media::topics::{topic_id_for, ReplyHeader, GENERAL_TOPIC_ID};
use tgx_tg::config::Settings;
use tgx_tg::convert::{base_message, base_service, NameBook as Names, UserFacts};
use tgx_tg::plan;

fn names() -> Names {
    let mut n = Names::default();
    n.learn(UserFacts {
        id: 1,
        first: "Ivana",
        last: "",
        username: "",
        ..Default::default()
    });
    n.learn(UserFacts {
        id: 2,
        first: "Nada",
        last: "Gavrilovic",
        username: "",
        ..Default::default()
    });
    n
}

// ---------------------------------------------------------------- text ----

#[test]
fn an_emoji_before_a_bold_run_does_not_shift_it() {
    // The invariant that silently corrupts formatting in every message with an
    // emoji: Telegram counts offsets in UTF-16 code units.
    let mut m = message(1, 1_766_071_072);
    m.message = "👍 bold tail".into();
    m.entities = Some(vec![tl::enums::MessageEntity::Bold(
        tl::types::MessageEntityBold {
            offset: 3,
            length: 4,
        },
    )]);
    let out = base_message(&m, &names());
    let segs = out["text_entities"].as_array().unwrap();
    let bold: Vec<&Value> = segs.iter().filter(|s| s["type"] == "bold").collect();
    assert_eq!(bold.len(), 1);
    assert_eq!(bold[0]["text"], "bold");
}

#[test]
fn a_hostile_offset_inside_a_surrogate_pair_does_not_kill_the_message() {
    // A segment holding half a pair cannot be encoded as UTF-8, and one
    // reaching a writer took down the export of an entire chat.
    let mut m = message(1, 1_766_071_072);
    m.message = "a👍b".into();
    m.entities = Some(vec![tl::enums::MessageEntity::Bold(
        tl::types::MessageEntityBold {
            offset: 2, // between the halves
            length: 1,
        },
    )]);
    let out = base_message(&m, &names());
    let joined: String = out["text_entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["text"].as_str())
        .collect();
    assert!(joined.contains('a') && joined.contains('b'));
    // And the whole payload survives being encoded.
    let encoded = serde_json::to_string(&Value::Object(out)).unwrap();
    assert!(encoded.is_char_boundary(encoded.len()));
}

#[test]
fn an_unformatted_message_writes_text_as_a_bare_string() {
    let mut m = message(1, 1_766_071_072);
    m.message = "sad cemo mi to da resimo".into();
    let out = base_message(&m, &names());
    assert!(out["text"].is_string());
    assert_eq!(out["text"], "sad cemo mi to da resimo");
}

// -------------------------------------------------------------- shape ----

#[test]
fn an_ordinary_message_gains_no_key_desktop_would_not_write() {
    // The invariant that keeps the HTML leg at 4 of 4: a branch that fired
    // without its source field would diff on every page.
    let m = message(69, 1_766_071_367);
    let out = base_message(&m, &names());
    let keys: Vec<&str> = out.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec![
            "id",
            "type",
            "date",
            "date_unixtime",
            "from",
            "from_id",
            "text",
            "text_entities"
        ]
    );
}

#[test]
fn every_emitted_key_is_one_the_order_table_knows() {
    // An unranked key sorts after `reactions`, which is visible in a diff —
    // but it should never happen, so pin it here rather than discover it in a
    // 256,780-line comparison.
    let mut m = message(1, 1_766_071_072);
    m.message = "x".into();
    m.edit_date = Some(1_766_071_400);
    m.post_author = Some("Editor".into());
    m.via_bot_id = Some(7);
    m.reply_to = Some(reply_to(Some(42), Some(77), true));
    let out = base_message(&m, &names());
    for key in out.keys() {
        assert!(
            tgx_format::order::rank(key) < tgx_format::order::tail_rank(),
            "{key} is not in ORDER"
        );
    }
}

#[test]
fn a_service_message_still_carries_the_empty_text_pair() {
    let s = service(66, 1_766_071_072, tl::enums::MessageAction::Empty);
    let out = base_service(&s, &names());
    assert_eq!(out["type"], "service");
    assert_eq!(out["text"], "");
    assert_eq!(out["text_entities"], serde_json::json!([]));
}

#[test]
fn the_from_name_is_the_trimmed_one_and_the_key_is_typed() {
    let m = message(1, 1_766_071_072);
    let out = base_message(&m, &names());
    assert_eq!(out["from"], "Ivana", "result.json carries the trimmed name");
    assert_eq!(out["from_id"], "user1", "and a typed key, never a bare int");
}

// ------------------------------------------------------------- routing ----

#[test]
fn topic_routing_matches_the_four_documented_cases() {
    // Drawn straight from a real forum's message shapes.
    let cases: [(i64, bool, Option<ReplyHeader>, i64); 4] = [
        // The service message that creates a topic names its own topic.
        (66, true, None, 66),
        // No reply header at all is General.
        (5, false, None, GENERAL_TOPIC_ID),
        // A forum reply routes to its top id.
        (
            5,
            false,
            Some(ReplyHeader {
                forum_topic: true,
                reply_to_top_id: Some(12),
                reply_to_msg_id: Some(77),
            }),
            12,
        ),
        // A plain reply with no forum flag is General — the case that looks
        // wrong and is not.
        (
            5,
            false,
            Some(ReplyHeader {
                forum_topic: false,
                reply_to_top_id: Some(12),
                reply_to_msg_id: Some(77),
            }),
            GENERAL_TOPIC_ID,
        ),
    ];
    for (id, creates, reply, want) in cases {
        assert_eq!(topic_id_for(id, creates, reply), want, "message {id}");
    }
}

// --------------------------------------------------------------- media ----

fn sticker_attrs(alt: &str) -> Vec<tl::enums::DocumentAttribute> {
    vec![tl::enums::DocumentAttribute::Sticker(
        tl::types::DocumentAttributeSticker {
            mask: false,
            alt: alt.into(),
            stickerset: tl::enums::InputStickerSet::Empty,
            mask_coords: None,
        },
    )]
}

#[test]
fn a_webm_sticker_is_filed_with_the_videos_and_still_reports_sticker() {
    // Four files in the reference are exactly this, and routing on media_type
    // alone put them in stickers/ *and* gave them that folder's collision
    // counter — one rule, ten wrong filenames.
    let doc = document(1, "video/webm", 1024, sticker_attrs("6️⃣"));
    let facts = plan::classify(&doc_media(doc, false), false).expect("classified");
    assert_eq!(facts.kind, "video_files", "the folder follows the shape");
    assert_eq!(
        facts.media_type,
        Some("sticker"),
        "the media_type follows what Telegram says"
    );

    let mut book = NameBook::new();
    let (fields, _) = plan::plan(&facts, 1, "s", &mut book, &Settings::default());
    assert!(
        fields["file"].as_str().unwrap().starts_with("video_files/"),
        "got {}",
        fields["file"]
    );
    assert_eq!(fields["media_type"], "sticker");
}

#[test]
fn a_webp_sticker_stays_in_the_sticker_folder() {
    let doc = document(2, "image/webp", 512, sticker_attrs("🫥"));
    let facts = plan::classify(&doc_media(doc, false), false).unwrap();
    assert_eq!(facts.kind, "stickers");
}

#[test]
fn a_voice_message_is_routed_and_timed() {
    let doc = document(
        3,
        "audio/ogg",
        2048,
        vec![tl::enums::DocumentAttribute::Audio(
            tl::types::DocumentAttributeAudio {
                voice: true,
                duration: 65,
                title: None,
                performer: None,
                waveform: None,
            },
        )],
    );
    let facts = plan::classify(&doc_media(doc, false), false).unwrap();
    assert_eq!(facts.kind, "voice_messages");
    assert_eq!(facts.media_type, Some("voice_message"));
    assert_eq!(facts.duration, 65);
}

#[test]
fn a_photo_reads_its_largest_size() {
    let facts = plan::classify(&photo_media(photo(4, 2560, 1706, 1_940_744), false), false)
        .expect("classified");
    assert_eq!(facts.kind, "photos");
    assert_eq!((facts.width, facts.height), (2560, 1706));
    assert_eq!(facts.size, 1_940_744);
    assert_eq!(facts.file_name, None, "a photo never carries a name");
}

#[test]
fn a_link_preview_is_not_media_unless_asked_for() {
    // Desktop never saves one; treating it as media added 21 photos to the
    // reference and shifted every later photo_N.
    let doc = document(5, "image/jpeg", 100, vec![]);
    let page = tl::enums::MessageMedia::WebPage(tl::types::MessageMediaWebPage {
        force_large_media: false,
        force_small_media: false,
        manual: false,
        safe: false,
        webpage: tl::enums::WebPage::Empty(tl::types::WebPageEmpty { id: 1, url: None }),
    });
    assert!(plan::classify(&page, false).is_none());
    let _ = doc;
}

#[test]
fn a_skipped_file_still_advances_the_counter_and_keeps_its_thumbnail_record() {
    // Both halves of the rule that matched 557 of 557 photos and 1,287 of
    // 1,786 skipped thumbnails.
    let settings = Settings {
        size_limit_mb: 1,
        ..Settings::default()
    };
    let mut book = NameBook::new();

    let small = plan::classify(&photo_media(photo(1, 100, 100, 500), false), false).unwrap();
    let huge = plan::classify(
        &photo_media(photo(2, 4000, 3000, 50 * 1024 * 1024), false),
        false,
    )
    .unwrap();

    let (a, _) = plan::plan(&small, 1, "s", &mut book, &settings);
    let (skipped, job) = plan::plan(&huge, 2, "s", &mut book, &settings);
    let (c, _) = plan::plan(&small, 3, "s", &mut book, &settings);

    assert_eq!(a["photo"], "photos/photo_1@s.jpg");
    assert_eq!(skipped["photo"], plan::TOO_LARGE);
    assert!(job.is_none());
    assert_eq!(
        c["photo"], "photos/photo_3@s.jpg",
        "the skipped file must still take its number"
    );
}

#[test]
fn a_spoiler_is_recorded_and_an_ordinary_file_gains_no_key() {
    let doc = document(6, "image/jpeg", 100, vec![]);
    let hidden = plan::classify(&doc_media(doc.clone(), true), false).unwrap();
    let plain = plan::classify(&doc_media(doc, false), false).unwrap();
    assert!(hidden.spoiler);
    assert!(!plain.spoiler);

    let mut book = NameBook::new();
    let (h, _) = plan::plan(&hidden, 1, "s", &mut book, &Settings::default());
    let (p, _) = plan::plan(&plain, 2, "s", &mut book, &Settings::default());
    assert_eq!(h["spoiler"], true);
    assert!(!p.contains_key("spoiler"));
}

#[test]
fn media_fields_do_not_disturb_desktops_key_order() {
    // The media fields are merged into an already-ordered map, so re-ordering
    // afterwards is what keeps `text`/`text_entities` trailing.
    let facts = plan::classify(&photo_media(photo(1, 800, 600, 1000), false), false).unwrap();
    let mut book = NameBook::new();
    let (fields, _) = plan::plan(&facts, 1, "s", &mut book, &Settings::default());

    let mut m = message(1, 1_766_071_072);
    m.message = "caption".into();
    let mut out = base_message(&m, &names());
    for (k, v) in fields {
        out.insert(k, v);
    }
    let out = tgx_format::order::ordered(&out);
    let keys: Vec<&str> = out.keys().map(String::as_str).collect();
    let photo_at = keys.iter().position(|k| *k == "photo").unwrap();
    let text_at = keys.iter().position(|k| *k == "text").unwrap();
    assert!(photo_at < text_at, "media must precede the text: {keys:?}");
    assert_eq!(*keys.last().unwrap(), "text_entities");
}

// --------------------------------------------------- the presentation ----

/// Assemble a message exactly as `engine::payload` does, `_p` included.
fn payload_with_presentation(
    m: &tl::types::Message,
    media: &tl::enums::MessageMedia,
) -> serde_json::Map<String, Value> {
    let facts = plan::classify(media, false).expect("classified");
    let mut book = NameBook::new();
    let (fields, job) = plan::plan(&facts, m.id as i64, "s", &mut book, &Settings::default());
    let preview_src = job.as_ref().and_then(|j| j.preview_dest.clone());
    let mut out = base_message(m, &names());
    for (k, v) in fields {
        out.insert(k, v);
    }
    let mut out = tgx_format::order::ordered(&out);
    if let Some(p) = tgx_tg::convert::presentation(m, &out, &names(), preview_src.as_deref()) {
        out.insert("_p".into(), Value::Object(p));
    }
    out
}

/// Render one message through the real HTML writer and hand back the page.
fn render(out: &serde_json::Map<String, Value>, tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("tgx-pres-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = tgx_html::writer::HtmlWriter::new(&dir, "t", 1000);
    w.add(out).unwrap();
    w.close().unwrap();
    let page = std::fs::read_to_string(dir.join("messages.html")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    page
}

#[test]
fn a_photo_renders_as_an_inline_image_and_not_a_coloured_row() {
    // The engine never built `_p`, and `render_media` gates every inline
    // preview on it — so a real export contained **zero** `<img>` elements
    // anywhere, against 649 in Desktop's export of the same chat. Every photo,
    // sticker, animation and video came out as a placeholder row.
    //
    // The html leg could not see this: it lifts `_p` out of Desktop's own HTML
    // before replaying, so it proves the writer and never the pipeline.
    let mut m = message(1, 1_766_071_072);
    m.message = "caption".into();
    let media = photo_media(photo(1, 800, 600, 1000), false);
    let out = payload_with_presentation(&m, &media);

    let p = out["_p"].as_object().expect("a presentation map");
    let preview = p["preview"].as_object().expect("a preview");
    assert_eq!(preview["src"], "photos/photo_1@s_thumb.jpg");
    // 800x600 into the 260 box, stored at twice the displayed size.
    assert_eq!(preview["width"], 260);
    assert_eq!(preview["height"], 195);

    let page = render(&out, "photo");
    assert!(
        page.contains("class=\"photo_wrap"),
        "no inline image: {page}"
    );
    assert!(page.contains("<img class=\"photo\""), "no <img>: {page}");
    assert!(
        page.contains("photos/photo_1@s_thumb.jpg"),
        "the preview does not point at the thumbnail"
    );
}

#[test]
fn a_file_skipped_for_size_still_renders_as_a_row() {
    // The other half of the same rule: a preview must not be invented for a
    // file we did not save, or the page gains a broken image where Desktop
    // deliberately draws a coloured row.
    let s = Settings {
        size_limit_mb: 1,
        ..Settings::default()
    };
    let media = photo_media(photo(1, 800, 600, 50 * 1024 * 1024), false);
    let facts = plan::classify(&media, false).unwrap();
    let mut book = NameBook::new();
    let (fields, _) = plan::plan(&facts, 1, "s", &mut book, &s);
    let mut out = base_message(&message(1, 1_766_071_072), &names());
    for (k, v) in fields {
        out.insert(k, v);
    }
    let out = tgx_format::order::ordered(&out);
    let p = tgx_tg::convert::presentation(&message(1, 1_766_071_072), &out, &names(), None);
    assert!(
        p.as_ref().and_then(|p| p.get("preview")).is_none(),
        "a skipped file must not claim a preview"
    );
}

#[test]
fn a_skipped_file_with_a_blur_thumbnail_shows_it() {
    // The half of that rule nobody wrote. Telegram sends a blur thumbnail
    // *inside* the message — no request, no size limit — and `plan.rs` has
    // always written it to `thumbnails/` as `stripped_thumbnail`, while
    // `writer.rs:468` has always known how to draw it from
    // `_p.preview.stripped`. Nothing ever set that key, so the two halves never
    // met: 1,364 JPEGs on disk in the last live run and not one displayed.
    //
    // No parity leg can see this. Desktop's `result.json` carries no
    // `stripped_thumbnail`, so a replay never asks about it, which is why the
    // test lives here rather than in tgx-parity.
    let s = Settings {
        size_limit_mb: 1,
        ..Settings::default()
    };
    let media = photo_media(photo(1, 800, 600, 50 * 1024 * 1024), false);
    let mut facts = plan::classify(&media, false).unwrap();
    // A real stripped size arrives as Telegram's 3-byte-header form; `plan`
    // stores whatever `stripped_of` expanded, and only its presence matters
    // here.
    facts.stripped = Some(vec![0xff, 0xd8, 1, 2, 3]);
    let mut book = NameBook::new();
    let (fields, job) = plan::plan(&facts, 1, "s", &mut book, &s);
    assert!(
        job.as_ref()
            .is_some_and(|j| j.dest.starts_with("thumbnails/")),
        "the plan must still queue the blur write"
    );

    let mut out = base_message(&message(1, 1_766_071_072), &names());
    for (k, v) in fields {
        out.insert(k, v);
    }
    let mut out = tgx_format::order::ordered(&out);
    let p = tgx_tg::convert::presentation(&message(1, 1_766_071_072), &out, &names(), None)
        .expect("a presentation map");
    let preview = p["preview"].as_object().expect("a preview");
    assert_eq!(preview["stripped"], true);
    assert_eq!(preview["src"], out["stripped_thumbnail"]);
    // 800x600 into the 260 box, same sizing as any other inline image.
    assert_eq!(preview["width"], 260);
    assert_eq!(preview["height"], 195);

    // And it reaches the page as an <img>, with the row still below it saying
    // why the file is not here.
    out.insert("_p".into(), Value::Object(p));
    let page = render(&out, "stripped");
    assert!(page.contains("<img class=\"photo\""), "no <img>: {page}");
    assert!(page.contains("thumbnails/"), "wrong src: {page}");
    assert!(
        page.contains("Exceeds maximum size"),
        "the row must still say why: {page}"
    );
    // Both, in that order: the image shows what the file was, the row below it
    // still says why it is not here. This is what `writer.rs:468` was written
    // for and had never once been reached.
    let img = page.find("<img class=\"photo\"").expect("an image");
    let row = page.find("Exceeds maximum size").expect("a row");
    assert!(img < row, "the row must sit below the preview");
}

#[test]
fn the_presentation_map_never_reaches_result_json() {
    // `_p` is the one key in the map that Desktop does not write. If it ever
    // leaked, every message in `result.json` would gain a key and the json leg
    // would stop being a parity check at all.
    let mut m = message(1, 1_766_071_072);
    m.message = "caption".into();
    let media = photo_media(photo(1, 800, 600, 1000), false);
    let out = payload_with_presentation(&m, &media);
    assert!(out.contains_key("_p"), "the fixture is meant to have one");

    let dir = std::env::temp_dir().join(format!("tgx-pres-json-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut o = tgx_tg::output::Output::new(
        &dir,
        "t",
        "public_supergroup",
        1,
        &Settings::default(),
        None,
        None,
    )
    .unwrap();
    o.add(&out).unwrap();
    o.close().unwrap();
    let raw = std::fs::read_to_string(dir.join("result.json")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !raw.contains("_p"),
        "the presentation map leaked into the JSON"
    );
}
