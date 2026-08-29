//! Media planning: **filenames are decided before bytes are fetched.**
//!
//! `plan` returns the JSON fields *and* the download job. The path is assigned
//! synchronously, so the JSON and HTML stream out immediately while a bounded
//! pool fetches behind them.
//!
//! Everything about *naming* lives in [`tgx_media::names`] and is pinned at
//! 830 of 836 against a real export. This module's job is only to read a TL
//! object well enough to hand that code the right facts — the kind, the name
//! Telegram supplied, the size, the dimensions.

use crate::config::Settings;
use grammers_client::media::{Media, Photo};
use grammers_tl_types as tl;
use serde_json::{json, Map, Value};
use tgx_media::names::{kind_for, layout, sanitize_extension, NameBook};

/// A file to fetch once the message has already been written out.
#[derive(Debug, Clone)]
pub struct DownloadJob {
    /// Relative to the output root.
    pub dest: String,
    /// Telegram's own thumbnail, if this item has one.
    ///
    /// `<full name>_thumb.jpg`, and the value the JSON's `thumbnail` carries.
    pub thumb_dest: Option<String>,
    /// The inline preview, which is a **different file**: `<stem>_thumb<ext>`.
    ///
    /// Desktop renders this one itself and references it only from the HTML —
    /// it appears in no JSON field, which is why it is easy to miss that a real
    /// export has both `clip.mp4_thumb.jpg` and `clip_thumb.mp4` sitting beside
    /// `clip.mp4`. `names::claim_rendered_preview` has always known how to name
    /// it and nothing ever called it.
    pub preview_dest: Option<String>,
    pub size: i64,
    /// Bytes to write straight out instead of downloading anything.
    ///
    /// Set for a stripped thumbnail, which arrives **inside the message**, so
    /// the job goes through the same pool purely to keep the disk write off the
    /// read loop — there is no request behind it.
    pub inline_bytes: Option<Vec<u8>>,
    /// Which message this belongs to, for `missing_media.txt`.
    pub message_id: i64,
    /// This file was already written by an earlier message carrying the same
    /// Telegram id, so `dest` is a path to fetch *nothing* into.
    ///
    /// The thumbnail and the preview are still outstanding: their names collide
    /// with the first message's and take a ` (1)` suffix, which is why the
    /// reference export contains `file_1@19-12-2025_14-51-08_thumb (1).webp`
    /// beside a single `file_1@19-12-2025_14-51-08.webp`.
    pub already_saved: bool,
}

/// Desktop's verbatim placeholder strings for a file it did not save.
pub const NOT_INCLUDED: &str = "(File not included. Change data exporting settings to download.)";
pub const TOO_LARGE: &str =
    "(File exceeds maximum size. Change data exporting settings to download.)";

/// What a message's media is, in the vocabulary `tgx-media` speaks.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaFacts {
    /// Telegram's own id for this photo or document, or 0 if it has none.
    ///
    /// The identity a repeat is recognised by. It is known before a byte is
    /// fetched, which is what lets the *name* be decided the way Desktop decides
    /// it — see [`NameBook::saved_at`].
    pub id: i64,
    /// The `_LAYOUT` kind — `photos`, `video_files`, `stickers`, …
    pub kind: &'static str,
    /// The JSON `media_type`, which can disagree with the folder: a WebM video
    /// sticker is filed under `video_files/` and still reports `sticker`.
    pub media_type: Option<&'static str>,
    /// The name Telegram sent, if any. A photo never has one.
    pub file_name: Option<String>,
    pub mime_type: String,
    pub size: i64,
    pub width: i64,
    pub height: i64,
    pub duration: i64,
    pub sticker_emoji: Option<String>,
    pub performer: Option<String>,
    pub title: Option<String>,
    /// Telegram's own thumbnail size, known before the download because the
    /// size table arrives with the document.
    pub thumb_size: i64,
    pub has_thumb: bool,
    /// The ~180-byte blur preview embedded in the message itself.
    pub stripped: Option<Vec<u8>>,
    pub spoiler: bool,
}

/// The `(width, height, bytes)` of the size a download will actually fetch.
///
/// **Selected the way grammers selects it, not the way a reader would.** This
/// used to take the largest by *area*, while `Downloadable for Photo` builds its
/// `InputPhotoFileLocation` from `thumbs().max_by_key(|x| x.size())` — the
/// largest by *bytes*. The two disagree whenever Telegram serves a
/// `photoSizeProgressive`, which advertises a bigger picture in fewer bytes: we
/// recorded the progressive entry's dimensions and then downloaded a plain
/// `photoSize`, so `result.json` described a file that was not on disk. Four
/// photos in the reference, and in three of them the *lower*-resolution file was
/// the larger one — so the mismatch was not even consistently in one direction.
///
/// Nothing catches this. The media leg diffs names, the json leg replays
/// Desktop's own numbers, and the file itself is never opened.
///
/// [`thumb_bytes`] already says this for thumbnails: one observable, used by
/// both ends. This is the same rule for the photo itself.
fn photo_size(photo: &tl::types::Photo) -> (i64, i64, i64) {
    photo
        .sizes
        .iter()
        .max_by_key(|s| download_rank(s))
        .map(size_entry)
        .unwrap_or((0, 0, 0))
}

/// Does Desktop render an inline preview for this sticker?
///
/// Measured from the reference: every one of its 66 `.webp` stickers has a
/// `_thumb.webp` beside it and not one of its eight `.tgs` files has anything.
/// A `.tgs` is a gzipped Lottie animation, so there is no still frame to write
/// without an animation renderer — which is presumably why Desktop, which does
/// have one for display, still writes no preview file.
fn previewable_sticker(dest: &str) -> bool {
    dest.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("webp"))
}

/// `grammers_client::types::PhotoSize::size`, reproduced.
///
/// Not `size_entry`'s third field, which is the size we *report*: grammers
/// counts a stripped size as its bytes plus the 622-byte JPEG header it would be
/// expanded with, and that offset decides ties. Mirrored rather than
/// approximated, because the whole point of the function is that our choice and
/// the downloader's choice cannot differ.
fn download_rank(s: &tl::enums::PhotoSize) -> i64 {
    use tl::enums::PhotoSize as P;
    match s {
        P::Empty(_) => 0,
        P::Size(v) => v.size as i64,
        P::PhotoCachedSize(v) => v.bytes.len() as i64,
        P::PhotoStrippedSize(v) => {
            if v.bytes.len() < 3 || v.bytes[0] != 0x01 {
                0
            } else {
                v.bytes.len() as i64 + 622
            }
        }
        P::Progressive(v) => v.sizes.iter().copied().max().unwrap_or(0) as i64,
        P::PhotoPathSize(v) => v.bytes.len() as i64,
    }
}

fn size_entry(s: &tl::enums::PhotoSize) -> (i64, i64, i64) {
    use tl::enums::PhotoSize as P;
    match s {
        P::Size(v) => (v.w as i64, v.h as i64, v.size as i64),
        P::PhotoCachedSize(v) => (v.w as i64, v.h as i64, v.bytes.len() as i64),
        P::PhotoStrippedSize(v) => (0, 0, v.bytes.len() as i64),
        // PhotoSizeProgressive advertises several sizes; the largest is what a
        // full download costs.
        P::Progressive(v) => (
            v.w as i64,
            v.h as i64,
            v.sizes.iter().copied().max().unwrap_or(0) as i64,
        ),
        P::Empty(_) => (0, 0, 0),
        P::PhotoPathSize(_) => (0, 0, 0),
    }
}

/// The embedded blur preview, if this size table carries one.
fn stripped_of(sizes: &[tl::enums::PhotoSize]) -> Option<Vec<u8>> {
    for s in sizes {
        if let tl::enums::PhotoSize::PhotoStrippedSize(v) = s {
            if !v.bytes.is_empty() {
                return tgx_media::stripped::expand(&v.bytes);
            }
        }
    }
    None
}

/// Byte size of the thumbnail a download would fetch.
///
/// **Selected on byte size, and so is the download.** `download::largest_thumb`
/// has to pick the same entry out of grammers' `PhotoSize`, which exposes
/// `size()` but no dimensions — so choosing here by area and there by bytes
/// would let `thumbnail_file_size` describe a different file from the one on
/// disk. One observable, used by both.
///
/// Stripped sizes are excluded outright rather than merely not setting `any`:
/// they report `0 × 0`, so under an area comparison a stripped entry could win
/// against a real thumbnail and report the blur preview's length as the
/// thumbnail's size.
fn thumb_bytes(thumbs: &[tl::enums::PhotoSize]) -> (i64, bool) {
    let mut best = 0i64;
    let mut any = false;
    for s in thumbs {
        if matches!(s, tl::enums::PhotoSize::PhotoStrippedSize(_)) {
            continue;
        }
        any = true;
        let (_, _, size) = size_entry(s);
        best = best.max(size);
    }
    (best, any)
}

/// Read a message's media into [`MediaFacts`].
///
/// `follow_webpage` decides whether a link preview's image counts as media.
/// **Desktop never saves one**; measured against a reference export, treating
/// it as media added 21 photos and shifted every later `photo_N` by one.
pub fn classify(media: &tl::enums::MessageMedia, follow_webpage: bool) -> Option<MediaFacts> {
    use tl::enums::MessageMedia as M;
    match media {
        M::Photo(m) => {
            let tl::enums::Photo::Photo(photo) = m.photo.as_ref()? else {
                return None;
            };
            let (w, h, size) = photo_size(photo);
            Some(MediaFacts {
                id: photo.id,
                kind: "photos",
                media_type: None,
                file_name: None,
                mime_type: "image/jpeg".into(),
                size,
                width: w,
                height: h,
                duration: 0,
                sticker_emoji: None,
                performer: None,
                title: None,
                thumb_size: 0,
                has_thumb: false,
                stripped: stripped_of(&photo.sizes),
                spoiler: m.spoiler,
            })
        }
        M::Document(m) => {
            let tl::enums::Document::Document(doc) = m.document.as_ref()? else {
                return None;
            };
            Some(document_facts(doc, m.spoiler))
        }
        M::WebPage(m) if follow_webpage => {
            let tl::enums::WebPage::Page(page) = &m.webpage else {
                return None;
            };
            if let Some(tl::enums::Document::Document(doc)) = page.document.as_ref() {
                return Some(document_facts(doc, false));
            }
            if let Some(tl::enums::Photo::Photo(photo)) = page.photo.as_ref() {
                let (w, h, size) = photo_size(photo);
                return Some(MediaFacts {
                    id: photo.id,
                    kind: "photos",
                    media_type: None,
                    file_name: None,
                    mime_type: "image/jpeg".into(),
                    size,
                    width: w,
                    height: h,
                    duration: 0,
                    sticker_emoji: None,
                    performer: None,
                    title: None,
                    thumb_size: 0,
                    has_thumb: false,
                    stripped: stripped_of(&photo.sizes),
                    spoiler: false,
                });
            }
            None
        }
        _ => None,
    }
}

/// The handle the download pool must be given for this message's media.
///
/// **Not `Message::media()`, and that difference cost 21 files a run.** For a
/// link preview grammers answers `Media::WebPage`, and its own
/// `Downloadable for Media` maps that variant to `None` — so `iter_download`
/// hands back an iterator pre-loaded with `io::Error("media not
/// downloadable")` before any request leaves the process. The planner had
/// already named the image in `result.json` and in the HTML by then, so every
/// link-preview photo burned all five retries on a permanent refusal and landed
/// in `missing_media.txt`, one hundred percent of them, every run. Nothing
/// distinguished it from a flaky network because the reason was discarded; see
/// [`crate::download::MissingFile`].
///
/// It reads the page in **the same order [`classify`] does** — document, then
/// photo. If the two ever disagree the pool downloads bytes the JSON does not
/// describe, which is worse than the gap this fixes.
///
/// `None` here is not an error: it means there is nothing to fetch, and
/// `run_one` records the paths the JSON promised as a stated gap.
pub fn downloadable(media: &tl::enums::MessageMedia, follow_webpage: bool) -> Option<Media> {
    let tl::enums::MessageMedia::WebPage(m) = media else {
        return Media::from_raw(media.clone());
    };
    if !follow_webpage {
        return None;
    }
    let tl::enums::WebPage::Page(page) = &m.webpage else {
        return None;
    };
    if let Some(doc) = page.document.as_ref() {
        if matches!(doc, tl::enums::Document::Document(_)) {
            // Rebuilt as the media it would have been on its own message, so
            // `Media::from_raw` applies the same sticker/document split it
            // applies everywhere else.
            return Media::from_raw(tl::enums::MessageMedia::Document(
                tl::types::MessageMediaDocument {
                    nopremium: false,
                    spoiler: false,
                    video: false,
                    round: false,
                    voice: false,
                    document: Some(doc.clone()),
                    alt_documents: None,
                    video_cover: None,
                    video_timestamp: None,
                    ttl_seconds: None,
                },
            ));
        }
    }
    if let Some(photo) = page.photo.as_ref() {
        if matches!(photo, tl::enums::Photo::Photo(_)) {
            return Some(Media::Photo(Photo::from_raw(photo.clone())));
        }
    }
    None
}

fn document_facts(doc: &tl::types::Document, spoiler: bool) -> MediaFacts {
    use tl::enums::DocumentAttribute as A;

    let mut file_name = None;
    let mut width = 0i64;
    let mut height = 0i64;
    let mut duration = 0i64;
    let mut round = false;
    let mut animated = false;
    let mut is_sticker = false;
    let mut is_voice = false;
    let mut has_video = false;
    let mut sticker_emoji = None;
    let mut performer = None;
    let mut title = None;

    for a in &doc.attributes {
        match a {
            A::Filename(v) => file_name = Some(v.file_name.clone()),
            A::Video(v) => {
                has_video = true;
                width = v.w as i64;
                height = v.h as i64;
                duration = v.duration as i64;
                round = v.round_message;
            }
            A::ImageSize(v) => {
                width = v.w as i64;
                height = v.h as i64;
            }
            A::Animated => animated = true,
            A::Sticker(v) => {
                is_sticker = true;
                sticker_emoji = Some(v.alt.clone());
            }
            A::Audio(v) => {
                duration = v.duration as i64;
                is_voice = v.voice;
                performer = v.performer.clone();
                title = v.title.clone();
            }
            _ => {}
        }
    }

    // The JSON media_type and the folder are decided separately: the folder
    // follows the file's *shape* (see `kind_for`), the media_type follows what
    // Telegram says it is. A WebM video sticker differs on exactly this.
    let declared: &'static str = if round {
        "video_message"
    } else if is_sticker {
        "sticker"
    } else if animated {
        "animation"
    } else if is_voice {
        "voice_message"
    } else if has_video {
        "video_file"
    } else {
        ""
    };
    let kind = kind_for(declared, &doc.mime_type);
    let (thumb_size, has_thumb) = thumb_bytes(doc.thumbs.as_deref().unwrap_or(&[]));

    MediaFacts {
        id: doc.id,
        kind,
        media_type: if declared.is_empty() {
            None
        } else {
            Some(declared)
        },
        file_name,
        mime_type: doc.mime_type.clone(),
        size: doc.size,
        width,
        height,
        duration,
        sticker_emoji,
        performer,
        title,
        thumb_size,
        has_thumb,
        stripped: stripped_of(doc.thumbs.as_deref().unwrap_or(&[])),
        spoiler,
    }
}

/// Assign a path and build the JSON fields.
///
/// **A name is reserved for every message carrying media, written or not** —
/// `reserve_name` runs before the skip checks, because Desktop advances its
/// counter at this point rather than when it saves. One file skipped for size
/// shifts every name after it.
pub fn plan(
    facts: &MediaFacts,
    message_id: i64,
    stamp: &str,
    book: &mut NameBook,
    settings: &Settings,
) -> (Map<String, Value>, Option<DownloadJob>) {
    let mut fields = Map::new();
    let (subdir, _) = layout(facts.kind);

    let ext_hint = facts
        .file_name
        .as_deref()
        .map(sanitize_extension)
        .unwrap_or_default();

    // Reserve first. This is the rule that matched 557 of 557 photos.
    let name = book.reserve_name(facts.kind, facts.file_name.as_deref(), stamp, &ext_hint);

    // --- would we save it? ------------------------------------------------
    let enabled = settings.media_kinds.iter().any(|k| k == facts.kind)
        // Both video-shaped kinds live under the same folder.
        || (facts.kind == "video_files"
            && settings.media_kinds.iter().any(|k| k == "animations"));
    let too_large = settings
        .size_limit_bytes()
        .is_some_and(|limit| facts.size > limit);
    let write = settings.download_media && enabled && !too_large;

    let is_photo = facts.kind == "photos";
    let placeholder = if !settings.download_media || !enabled {
        NOT_INCLUDED
    } else {
        TOO_LARGE
    };

    // **A repeat reuses the path and claims nothing.** Desktop saves a file once
    // and points every later message at it; note that the *counter* still moved,
    // above — `media_leg` replays exactly that order against the reference and
    // reproduces 830 of 836 names, which it could not do if a repeat consumed no
    // number. What a repeat must not do is take a second name.
    let reused = book.saved_at(facts.id).map(str::to_string);
    let path = match (&reused, write) {
        (Some(existing), _) => Some(existing.clone()),
        // The collision suffix is claimed **only when the file is written**,
        // unlike the counter. Claiming it up front drops the match rate from
        // 830/836 to 809/836.
        (None, true) => {
            let claimed = book.claim(subdir, &name);
            book.remember(facts.id, &claimed);
            Some(claimed)
        }
        (None, false) => None,
    };

    // --- the JSON fields ---------------------------------------------------
    if let Some(mt) = facts.media_type {
        fields.insert("media_type".into(), json!(mt));
    }
    if is_photo {
        fields.insert(
            "photo".into(),
            json!(path.clone().unwrap_or_else(|| placeholder.to_string())),
        );
        if facts.size > 0 {
            fields.insert("photo_file_size".into(), json!(facts.size));
        }
    } else {
        fields.insert(
            "file".into(),
            json!(path.clone().unwrap_or_else(|| placeholder.to_string())),
        );
        if let Some(n) = &facts.file_name {
            fields.insert("file_name".into(), json!(n));
        }
        if facts.size > 0 {
            fields.insert("file_size".into(), json!(facts.size));
        }
        if !facts.mime_type.is_empty() {
            fields.insert("mime_type".into(), json!(facts.mime_type));
        }
    }

    // Telegram's own thumbnail. **A file skipped for size keeps its thumbnail
    // *key*, but not a path** — 1,287 of the 1,786 skipped files in the
    // reference carry `"thumbnail": "(File exceeds maximum size…)"`, the same
    // placeholder as the file itself, and the 499 that do not are exactly those
    // with no thumbnail. A skipped *photo* never gets one, 62 of 62.
    //
    // This read the reference as "keeps its thumbnail record" and wrote a real
    // path for all 1,287. That is wrong twice: it disagrees with Desktop, and
    // it promises a file the skip means we are never going to fetch — the
    // dangling-reference failure `download.rs` argues against, and invisible to
    // `missing_media.txt` because no job was ever queued to fail.
    let mut thumb_dest = None;
    if facts.has_thumb && !is_photo {
        match &path {
            Some(dest) => {
                let claimed = book.claim_telegram_thumb(dest);
                fields.insert("thumbnail".into(), json!(claimed.clone()));
                if facts.thumb_size > 0 {
                    fields.insert("thumbnail_file_size".into(), json!(facts.thumb_size));
                }
                thumb_dest = Some(claimed);
            }
            // Skipped. Desktop still records that a thumbnail exists, with the
            // same note it put in `file`, and reserves no name for it — but it
            // still writes the **size**, in all 1,544 cases: 257 saved and
            // 1,287 skipped. Writing it only on the saved branch made this one
            // key 1,287 of the last live run's 1,290 `absent` findings, which
            // is to say nearly every field the reference had and we did not.
            None => {
                fields.insert("thumbnail".into(), json!(placeholder));
                if facts.thumb_size > 0 {
                    fields.insert("thumbnail_file_size".into(), json!(facts.thumb_size));
                }
            }
        }
    }

    if facts.width > 0 {
        fields.insert("width".into(), json!(facts.width));
    }
    if facts.height > 0 {
        fields.insert("height".into(), json!(facts.height));
    }
    if facts.duration > 0 {
        fields.insert("duration_seconds".into(), json!(facts.duration));
    }
    // `Some("")` is not the same as `Some("👍")`. A sticker whose document
    // carries an empty `alt` produced `"sticker_emoji": ""` on 11 messages in
    // the last live run and on none in the reference — Desktop omits the key
    // rather than writing a blank one, which is what every other optional field
    // here already does.
    if let Some(e) = facts.sticker_emoji.as_deref().filter(|e| !e.is_empty()) {
        fields.insert("sticker_emoji".into(), json!(e));
    }
    if let Some(p) = &facts.performer {
        fields.insert("performer".into(), json!(p));
    }
    if let Some(t) = &facts.title {
        fields.insert("title".into(), json!(t));
    }
    // Beyond Desktop's format, and gated on a key it never writes.
    if facts.spoiler {
        fields.insert("spoiler".into(), json!(true));
    }

    // --- the download ------------------------------------------------------
    // The inline preview, for the two kinds Desktop shows one for that are not
    // videos. A video's inline image is Telegram's own thumbnail, already
    // claimed above; a photo's and a sticker's is a downscale of the file
    // itself, on its own name. Claimed here rather than derived later because
    // `claim` may add a `(1)` on collision, and a preview computed from the
    // path afterwards would miss it and point at nothing.
    //
    // **Not every sticker gets one.** Desktop previews a still sticker and not
    // an animated one: its export of this chat carries `sticker (1)_thumb.webp`
    // for all 66 WebP stickers and no `_thumb` whatsoever beside any of the
    // eight `AnimatedSticker*.tgs`, which is decisive because a `.tgs` is a
    // gzipped Lottie animation and no browser will draw one in an `<img>`. We
    // claimed a name for every sticker, so an animated one got
    // `AnimatedSticker_thumb.tgs` — an extra file Desktop does not write, under
    // a name that promises an image and holds a Lottie.
    let preview_dest = match (&path, facts.kind) {
        (Some(dest), "photos") => Some(book.claim_rendered_preview(dest)),
        (Some(dest), "stickers") if previewable_sticker(dest) => {
            Some(book.claim_rendered_preview(dest))
        }
        _ => None,
    };

    let job = match &path {
        Some(dest) => Some(DownloadJob {
            dest: dest.clone(),
            thumb_dest,
            preview_dest,
            size: facts.size,
            inline_bytes: None,
            message_id,
            // The bytes are already on disk under this name; only the thumbnail
            // and the preview, which took fresh names, still have to be fetched.
            already_saved: reused.is_some(),
        }),
        // A file we did not save, whose blur preview came free inside the
        // message. **A skipped file must not consume a `photo_N`**, so the
        // stripped thumbnail gets its own counter and its own folder.
        None => facts.stripped.as_ref().map(|bytes| {
            let n = book.reserve_name("thumbnails", None, stamp, ".jpg");
            let dest = book.claim("thumbnails", &n);
            fields.insert("stripped_thumbnail".into(), json!(dest.clone()));
            DownloadJob {
                dest,
                thumb_dest: None,
                preview_dest: None,
                size: bytes.len() as i64,
                inline_bytes: Some(bytes.clone()),
                message_id,
                already_saved: false,
            }
        }),
    };

    (fields, job)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings::default()
    }

    fn plain(t: &str, w: i32, h: i32, size: i32) -> tl::enums::PhotoSize {
        tl::enums::PhotoSize::Size(tl::types::PhotoSize {
            r#type: t.to_string(),
            w,
            h,
            size,
        })
    }

    fn progressive(t: &str, w: i32, h: i32, sizes: Vec<i32>) -> tl::enums::PhotoSize {
        tl::enums::PhotoSize::Progressive(tl::types::PhotoSizeProgressive {
            r#type: t.to_string(),
            w,
            h,
            sizes,
        })
    }

    fn photo_with(sizes: Vec<tl::enums::PhotoSize>) -> tl::types::Photo {
        tl::types::Photo {
            has_stickers: false,
            id: 1,
            access_hash: 2,
            file_reference: Vec::new(),
            date: 0,
            sizes,
            video_sizes: None,
            dc_id: 1,
        }
    }

    #[test]
    fn the_same_file_sent_twice_is_saved_once() {
        // Desktop saves a file once and points every later message at it. We
        // were strictly one-to-one: 46 references in the reference collapse to
        // 20 basenames there, nine of them redundant photos worth 3.33 MB, and
        // each surplus name shifted 74 file and 74 thumbnail paths after it.
        //
        // The counter still advances — `media_leg` replays exactly that order
        // and reproduces 830 of 836 names, which it could not do otherwise —
        // so the *second* photo is the one that keeps `photo_1.jpg`, and a
        // third, different photo lands on `photo_3.jpg` with `photo_2` spent.
        let mut book = NameBook::new();
        let mut facts = photo_facts(1000);
        facts.id = 77;

        let (first, job) = plan(&facts, 1, "stamp", &mut book, &settings());
        assert_eq!(first["photo"], "photos/photo_1@stamp.jpg");
        assert!(!job.expect("a job").already_saved);

        let (second, job) = plan(&facts, 2, "stamp", &mut book, &settings());
        assert_eq!(
            second["photo"], "photos/photo_1@stamp.jpg",
            "a repeat reuses it"
        );
        assert!(
            job.expect("a job").already_saved,
            "a repeat must not fetch the bytes again"
        );

        let mut other = photo_facts(2000);
        other.id = 78;
        let (third, _) = plan(&other, 3, "stamp", &mut book, &settings());
        assert_eq!(
            third["photo"], "photos/photo_3@stamp.jpg",
            "the repeat still spent photo_2"
        );
    }

    #[test]
    fn a_file_telegram_gave_no_id_is_never_treated_as_a_repeat() {
        // `id: 0` is "unknown", not "the same one again". Collapsing those
        // would point every unidentified file at the first one's path.
        let mut book = NameBook::new();
        let facts = photo_facts(1000);
        assert_eq!(facts.id, 0);

        let (first, _) = plan(&facts, 1, "stamp", &mut book, &settings());
        let (second, job) = plan(&facts, 2, "stamp", &mut book, &settings());
        assert_eq!(first["photo"], "photos/photo_1@stamp.jpg");
        assert_eq!(second["photo"], "photos/photo_2@stamp.jpg");
        assert!(!job.expect("a job").already_saved);
    }

    #[test]
    fn only_a_still_sticker_gets_an_inline_preview() {
        // Counted in the reference export's own HTML: 66 `_thumb.webp` beside
        // its 66 `.webp` stickers, and nothing at all beside any of its eight
        // `AnimatedSticker*.tgs`. We claimed a name for every sticker, so an
        // animated one produced `AnimatedSticker_thumb.tgs` — a file Desktop
        // does not write, under a name promising an image and holding a gzipped
        // Lottie no browser will draw.
        assert!(previewable_sticker("stickers/sticker (1).webp"));
        assert!(previewable_sticker(
            "stickers/file_1@19-12-2025_14-51-08.WEBP"
        ));
        assert!(!previewable_sticker("stickers/AnimatedSticker.tgs"));
        assert!(!previewable_sticker("stickers/AnimatedSticker (7).tgs"));
        // A name with no extension has nothing to promise either.
        assert!(!previewable_sticker("stickers/sticker"));
    }

    #[test]
    fn a_photo_is_measured_by_the_size_that_will_be_downloaded() {
        // grammers builds its `InputPhotoFileLocation` from
        // `thumbs().max_by_key(|x| x.size())` — the largest by *bytes*. Picking
        // the largest by *area* here meant the JSON described the progressive
        // entry and the pool fetched the plain one: four photos in the
        // reference, and in three of them the lower-resolution file was the
        // larger file, so the mismatch did not even lean one way.
        //
        // Bigger picture, fewer bytes — which is what progressive is for.
        let photo = photo_with(vec![
            plain("x", 800, 600, 90_000),
            progressive("y", 2560, 1706, vec![1_000, 20_000, 60_000]),
        ]);
        assert_eq!(photo_size(&photo), (800, 600, 90_000));

        // And when the progressive entry really is the heaviest, it wins.
        let photo = photo_with(vec![
            plain("x", 800, 600, 30_000),
            progressive("y", 2560, 1706, vec![1_000, 20_000, 60_000]),
        ]);
        assert_eq!(photo_size(&photo), (2560, 1706, 60_000));
    }

    #[test]
    fn a_blur_placeholder_never_wins_the_measurement() {
        // A stripped size reports 0x0, so under the old area comparison it could
        // not win — under a byte comparison it can, if the real sizes are tiny.
        // grammers counts it as its bytes plus the 622-byte header it would be
        // expanded with, and `download_rank` mirrors that exactly so our choice
        // and the downloader's cannot differ even here.
        let stripped = tl::enums::PhotoSize::PhotoStrippedSize(tl::types::PhotoStrippedSize {
            r#type: "i".to_string(),
            bytes: vec![0x01; 180],
        });
        assert_eq!(download_rank(&stripped), 802);
        assert_eq!(download_rank(&plain("x", 90, 60, 803)), 803);

        let photo = photo_with(vec![stripped, plain("x", 90, 60, 803)]);
        assert_eq!(photo_size(&photo), (90, 60, 803));
    }

    fn photo_facts(size: i64) -> MediaFacts {
        MediaFacts {
            id: 0,
            kind: "photos",
            media_type: None,
            file_name: None,
            mime_type: "image/jpeg".into(),
            size,
            width: 2560,
            height: 1706,
            duration: 0,
            sticker_emoji: None,
            performer: None,
            title: None,
            thumb_size: 0,
            has_thumb: false,
            stripped: None,
            spoiler: false,
        }
    }

    #[test]
    fn a_skipped_file_still_consumes_its_number() {
        // The rule that matched 557 of 557 photos: the counter advances when
        // the name is reserved, not when the file is written.
        let mut book = NameBook::new();
        let s = Settings {
            size_limit_mb: 1,
            ..settings()
        };
        let (a, _) = plan(&photo_facts(100), 1, "s", &mut book, &s);
        // 5 MB: over the 1 MB cap, so skipped.
        let (skipped, _) = plan(&photo_facts(5 * 1024 * 1024), 2, "s", &mut book, &s);
        let (c, _) = plan(&photo_facts(100), 3, "s", &mut book, &s);

        assert_eq!(a["photo"], "photos/photo_1@s.jpg");
        assert_eq!(skipped["photo"], TOO_LARGE);
        assert_eq!(
            c["photo"], "photos/photo_3@s.jpg",
            "the skip must still count"
        );
    }

    #[test]
    fn a_skipped_photo_keeps_its_size_and_loses_its_path() {
        let mut book = NameBook::new();
        let s = Settings {
            size_limit_mb: 1,
            ..settings()
        };
        let (f, job) = plan(&photo_facts(5 * 1024 * 1024), 1, "s", &mut book, &s);
        assert_eq!(f["photo"], TOO_LARGE);
        assert_eq!(f["photo_file_size"], 5 * 1024 * 1024);
        assert!(job.is_none(), "nothing to download");
    }

    #[test]
    fn a_disabled_kind_says_not_included_not_too_large() {
        // The two placeholders are different strings and Desktop distinguishes
        // them: one is a size decision, the other a settings decision.
        let mut book = NameBook::new();
        let s = Settings {
            media_kinds: vec!["files".into()],
            ..settings()
        };
        let (f, _) = plan(&photo_facts(10), 1, "s", &mut book, &s);
        assert_eq!(f["photo"], NOT_INCLUDED);
    }

    #[test]
    fn a_stripped_thumbnail_survives_a_skip_on_its_own_counter() {
        // 1,848 of the reference's 2,684 media messages get a preview this way
        // — and it must not consume a photo_N.
        let mut book = NameBook::new();
        let s = Settings {
            size_limit_mb: 1,
            ..settings()
        };
        let mut facts = photo_facts(5 * 1024 * 1024);
        facts.stripped = Some(vec![0xff, 0xd8, 1, 2, 3]);
        let (f, job) = plan(&facts, 1, "s", &mut book, &s);

        assert_eq!(f["photo"], TOO_LARGE);
        let job = job.expect("the blur preview is still written");
        assert!(job.dest.starts_with("thumbnails/"), "got {}", job.dest);
        assert!(
            job.inline_bytes.is_some(),
            "no request should be made for it"
        );
        // The photo counter is untouched by the thumbnail's own.
        assert_eq!(book.counter("photo"), 1);
    }

    #[test]
    fn a_document_records_its_name_size_and_mime() {
        let mut book = NameBook::new();
        let facts = MediaFacts {
            id: 0,
            kind: "files",
            media_type: None,
            file_name: Some("report.pdf".into()),
            mime_type: "application/pdf".into(),
            size: 2048,
            width: 0,
            height: 0,
            duration: 0,
            sticker_emoji: None,
            performer: None,
            title: None,
            thumb_size: 0,
            has_thumb: false,
            stripped: None,
            spoiler: false,
        };
        let (f, job) = plan(&facts, 1, "s", &mut book, &settings());
        assert_eq!(f["file"], "files/report.pdf");
        assert_eq!(f["file_name"], "report.pdf");
        assert_eq!(f["file_size"], 2048);
        assert_eq!(f["mime_type"], "application/pdf");
        assert_eq!(job.unwrap().dest, "files/report.pdf");
        // A named file leaves every synthesised counter alone.
        assert_eq!(book.counter("file"), 0);
    }

    #[test]
    fn a_sticker_with_no_emoji_does_not_get_an_empty_one() {
        // `Some("")` is not `Some("👍")`. A document whose `alt` is empty wrote
        // `"sticker_emoji": ""` on 11 messages in the last live run, against
        // zero in the whole reference — Desktop omits the key rather than
        // writing a blank one, which is what every other optional field in this
        // function already does.
        let mut book = NameBook::new();
        let base = MediaFacts {
            id: 0,
            kind: "stickers",
            media_type: Some("sticker"),
            file_name: Some("sticker.webp".into()),
            mime_type: "image/webp".into(),
            size: 1024,
            width: 512,
            height: 512,
            duration: 0,
            sticker_emoji: Some(String::new()),
            performer: None,
            title: None,
            thumb_size: 0,
            has_thumb: false,
            stripped: None,
            spoiler: false,
        };
        let (f, _) = plan(&base, 1, "s", &mut book, &settings());
        assert!(
            f.get("sticker_emoji").is_none(),
            "an empty alt must not become an empty key: {:?}",
            f.get("sticker_emoji")
        );

        let with = MediaFacts {
            id: 0,
            sticker_emoji: Some("👍".into()),
            ..base
        };
        let (f, _) = plan(&with, 2, "s", &mut book, &settings());
        assert_eq!(f["sticker_emoji"], "👍");
    }

    #[test]
    fn a_skipped_document_records_its_thumbnail_as_skipped_too() {
        // 1,287 of the reference's 1,786 skipped files carry a `thumbnail`
        // key — and its value is the *placeholder*, the same note Desktop put
        // in `file`, not a path. Re-measured against the reference:
        //
        //   1287  file skipped, thumbnail = placeholder
        //    499  file skipped, no thumbnail key
        //      0  file skipped, thumbnail = a path
        //
        // This test previously asserted the path, which is where the belief
        // came from that a skipped file "keeps its thumbnail record". It kept
        // the key, not the file — and asserting the path made the export
        // promise 1,287 thumbnails it had no job queued to fetch.
        let mut book = NameBook::new();
        let s = Settings {
            size_limit_mb: 1,
            ..settings()
        };
        let facts = MediaFacts {
            id: 0,
            kind: "video_files",
            media_type: Some("video_file"),
            file_name: Some("clip.mp4".into()),
            mime_type: "video/mp4".into(),
            size: 50 * 1024 * 1024,
            width: 1920,
            height: 1080,
            duration: 30,
            sticker_emoji: None,
            performer: None,
            title: None,
            thumb_size: 4096,
            has_thumb: true,
            stripped: None,
            spoiler: false,
        };
        let (f, job) = plan(&facts, 1, "s", &mut book, &s);
        assert_eq!(f["file"], TOO_LARGE);
        assert_eq!(f["thumbnail"], TOO_LARGE);
        // The size **is** written, and this assertion used to say the opposite
        // — "there is no file for it to be the size of", which sounds right and
        // is not what Desktop does. Re-measured against the reference:
        //
        //    257  thumbnail saved,   thumbnail_file_size present
        //   1287  thumbnail skipped, thumbnail_file_size present
        //      0  thumbnail present, thumbnail_file_size absent
        //
        // The size describes the thumbnail Telegram *has*, not the one we
        // fetched, so a skip does not make it unknown. This single key was
        // 1,287 of the last live run's 1,290 missing fields — and the test that
        // should have caught it asserted the defect instead.
        assert_eq!(f["thumbnail_file_size"], 4096);
        assert!(job.is_none(), "the file itself is not fetched");
        // And no name was reserved for a thumbnail nobody will fetch.
        assert_eq!(book.counter("thumb"), 0);
    }

    #[test]
    fn a_saved_document_queues_its_thumbnail_for_download() {
        // The other half of the same bug: `thumb_dest` was written by the plan
        // and read by nothing, so a `thumbnail` path reached `result.json` with
        // no download behind it — 1,559 dangling references in the last live
        // export, none of them in `missing_media.txt` because no job existed to
        // fail. The plan's job must carry the destination.
        let mut book = NameBook::new();
        let facts = MediaFacts {
            id: 0,
            kind: "video_files",
            media_type: Some("video_file"),
            file_name: Some("clip.mp4".into()),
            mime_type: "video/mp4".into(),
            size: 1024,
            width: 1920,
            height: 1080,
            duration: 30,
            sticker_emoji: None,
            performer: None,
            title: None,
            thumb_size: 4096,
            has_thumb: true,
            stripped: None,
            spoiler: false,
        };
        let (f, job) = plan(&facts, 1, "s", &mut book, &settings());
        assert_eq!(f["file"], "video_files/clip.mp4");
        assert_eq!(f["thumbnail"], "video_files/clip.mp4_thumb.jpg");
        assert_eq!(f["thumbnail_file_size"], 4096);
        let job = job.expect("a saved file is fetched");
        assert_eq!(
            job.thumb_dest.as_deref(),
            Some("video_files/clip.mp4_thumb.jpg"),
            "the JSON named a thumbnail the pool was never told to fetch"
        );
    }

    #[test]
    fn a_spoiler_is_recorded_but_only_when_set() {
        let mut book = NameBook::new();
        let mut facts = photo_facts(10);
        let (plain, _) = plan(&facts, 1, "s", &mut book, &settings());
        assert!(
            !plain.contains_key("spoiler"),
            "an ordinary photo gains no key"
        );

        facts.spoiler = true;
        let (hidden, _) = plan(&facts, 2, "s", &mut book, &settings());
        assert_eq!(hidden["spoiler"], true);
    }

    #[test]
    fn a_webm_sticker_is_filed_as_video_and_still_reports_sticker() {
        // The measured case the media leg caught: folder and media_type
        // deliberately disagree.
        assert_eq!(kind_for("sticker", "video/webm"), "video_files");
        assert_eq!(
            tgx_media::names::media_type("video_files"),
            Some("video_file")
        );
    }
}
