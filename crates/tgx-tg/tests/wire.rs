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
use tgx_tg::convert::{base_message, base_service, NameBook as Names};
use tgx_tg::plan;

fn names() -> Names {
    let mut n = Names::default();
    n.learn_user_parts(1, "Ivana", "", "", false, None);
    n.learn_user_parts(2, "Nada", "Gavrilovic", "", false, None);
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
