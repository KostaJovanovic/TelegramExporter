//! Filenames, and the claiming discipline that keeps two files off one path.
//!
//! Every rule here was measured against a real run rather than reasoned about,
//! and four of them are counter-intuitive enough that a plausible-looking
//! implementation gets them wrong:
//!
//! 1. **A name is reserved for every message carrying media, written or not.**
//!    Reading `photo_N` as "the Nth photo-bearing message" matches 557 of 557
//!    in the reference; reading it as "the Nth photo actually written" matches
//!    64 and misses 493. One file skipped for size shifts every name after it.
//! 2. **Each synthesised prefix has its own counter.** `photo_`, `video_`,
//!    `audio_`, else `file_`. The proof is `video_6` on the only unnamed video
//!    Desktop wrote — the five before it were skipped for size and still took
//!    numbers — and `audio_1..3` on voice messages that come *after* two named
//!    `audio.ogg` files. One shared counter gave those three `file_3..5`.
//! 3. **A file Telegram named leaves every counter alone** but is still
//!    *claimed*, and so is the `_thumb.jpg` beside it.
//! 4. **The collision suffix is claimed only when the file is written**, unlike
//!    the counter. Claiming it up front drops the match rate from 830/836 to
//!    809/836.

use std::collections::{HashMap, HashSet};

/// Characters Windows refuses in a path component.
fn is_illegal(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20
}

fn reserved_stems() -> &'static HashSet<String> {
    use std::sync::OnceLock;
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| {
        let mut s: HashSet<String> = ["CON", "PRN", "AUX", "NUL"]
            .iter()
            .map(|x| x.to_string())
            .collect();
        for i in 1..=9 {
            s.insert(format!("COM{i}"));
            s.insert(format!("LPT{i}"));
        }
        s
    })
}

/// Make a Telegram-supplied filename safe on Windows.
pub fn sanitize_filename(name: &str, fallback: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if is_illegal(c) { '_' } else { c })
        .collect();
    let trimmed = replaced.trim().trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    // Windows reserves CON, CON.txt and CON.txt.bak alike — it is the part
    // before the *first* dot that matters, not the last.
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    let candidate = if reserved_stems().contains(&stem.to_uppercase()) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    };
    // Trimmed *after* the cut, not only before it. Truncating a long name can
    // land the boundary on a dot, and Windows silently drops a trailing one —
    // so the file on disk was "abc" while result.json pointed at "abc.", a
    // dangling reference that an existence check still answered true for and
    // that therefore never reached missing_media.txt.
    let cut: String = candidate.chars().take(180).collect();
    let out = cut.trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

/// Sanitise an extension, **including** the extension.
///
/// Raw, it injects separators — and creating parent directories then makes the
/// junk dirs — plus Windows-illegal characters that make the download fail,
/// letting a sender guarantee their file is never saved.
pub fn sanitize_extension(raw: &str) -> String {
    let tail = raw.rsplit('.').next().unwrap_or("");
    let cleaned: String = tail.chars().filter(|c| !is_illegal(*c)).take(16).collect();
    if cleaned.is_empty() {
        String::new()
    } else {
        format!(".{cleaned}")
    }
}

/// Which folder each kind of media lands in, and its default extension.
///
/// Desktop files anything carrying a video track under `video_files` —
/// animations and video stickers included. This export has no `animations/`
/// folder at all.
pub fn layout(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "photos" => ("photos", ".jpg"),
        "video_files" => ("video_files", ".mp4"),
        "voice_messages" => ("voice_messages", ".ogg"),
        "video_messages" => ("round_video_messages", ".mp4"),
        "stickers" => ("stickers", ".webp"),
        "animations" => ("video_files", ".mp4"),
        // A stripped thumbnail is not a document Telegram sent — falling
        // through to "files" here handed it the same file_N counter real
        // unnamed documents use, so plan.rs's own comment claiming it "gets
        // its own counter and its own folder" was false. Verified live: a
        // real export had thumbnails/file_10@... shifting every later
        // files/ name by one.
        "thumbnails" => ("thumbnails", ".jpg"),
        _ => ("files", ""),
    }
}

/// The prefix Desktop synthesises a name from, per folder.
///
/// `round_video_messages` has no instance in the reference; it is filed with
/// the videos because that is the shape it is, and it is the only entry here
/// that is inferred rather than measured.
pub fn synth_prefix(subdir: &str) -> &'static str {
    match subdir {
        "photos" => "photo",
        "video_files" | "round_video_messages" => "video",
        "voice_messages" => "audio",
        // Matches the Python original: N:\telegram export\UA KOLAB
        // PYTHON\0001 - ćaskanje\thumbnails\thumb_10@08-03-2026_23-17-42.jpg.
        "thumbnails" => "thumb",
        _ => "file",
    }
}

/// Which `layout` kind a document belongs to.
///
/// **The folder follows the file's *shape*, not its `media_type`.** A WebM
/// video sticker goes to `video_files/` while still reporting
/// `"media_type": "sticker"` in the JSON — measured on four files in the
/// reference, each `mime_type: video/webm` with `media_type: sticker`, all four
/// of which Desktop filed under `video_files/`.
///
/// Routing on `media_type` alone puts them in `stickers/` and, worse, gives
/// them the `stickers/` collision counter, so every later sticker shifts.
pub fn kind_for(media_type: &str, mime_type: &str) -> &'static str {
    let is_video = mime_type.starts_with("video/");
    match media_type {
        // A round video message keeps its own folder whatever its container.
        "video_message" => "video_messages",
        "voice_message" => "voice_messages",
        "video_file" | "animation" => "video_files",
        // The shape rule: an animated sticker is a video and is filed as one.
        "sticker" if is_video => "video_files",
        "sticker" => "stickers",
        _ if is_video => "video_files",
        _ => "files",
    }
}

/// The JSON `media_type` for a kind, if it has one.
pub fn media_type(kind: &str) -> Option<&'static str> {
    match kind {
        "video_files" => Some("video_file"),
        "voice_messages" => Some("voice_message"),
        "video_messages" => Some("video_message"),
        "stickers" => Some("sticker"),
        "animations" => Some("animation"),
        _ => None,
    }
}

/// Assigns Desktop-style names within **one output folder**.
///
/// One instance per folder, i.e. one per topic when topics are split, so each
/// topic gets its own `photo_1`, `photo_2`… exactly as a standalone Desktop
/// export of that conversation would.
#[derive(Debug, Default)]
pub struct NameBook {
    /// One counter per synthesised prefix, not one shared `file_N`.
    synth: HashMap<String, u64>,
    used: HashSet<String>,
}

impl NameBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve `name` in `subdir`, suffixing `" (1)"` on a clash.
    ///
    /// Returns the path relative to the output root.
    pub fn claim(&mut self, subdir: &str, name: &str) -> String {
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) => (s.to_string(), Some(e.to_string())),
            None => (name.to_string(), None),
        };
        let mut candidate = name.to_string();
        let mut i = 1;
        while self
            .used
            .contains(&format!("{subdir}/{candidate}").to_lowercase())
        {
            candidate = match &ext {
                Some(e) => format!("{stem} ({i}).{e}"),
                None => format!("{stem} ({i})"),
            };
            i += 1;
        }
        self.used
            .insert(format!("{subdir}/{candidate}").to_lowercase());
        format!("{subdir}/{candidate}")
    }

    /// Is this path already spoken for?
    pub fn is_claimed(&self, path: &str) -> bool {
        self.used.contains(&path.to_lowercase())
    }

    /// Decide the file name, advancing the synthesised counter if needed.
    ///
    /// Called for **every** message carrying media, including ones that will
    /// never be written — Desktop advances the counter here rather than when it
    /// saves, so a photo skipped for size still consumes a `photo_N` and
    /// everything after it shifts up.
    ///
    /// `given` is the name Telegram sent, if any. A photo never has one.
    pub fn reserve_name(
        &mut self,
        kind: &str,
        given: Option<&str>,
        stamp: &str,
        ext_hint: &str,
    ) -> String {
        let (subdir, default_ext) = layout(kind);
        let given = if kind == "photos" { None } else { given };

        if let Some(g) = given.filter(|g| !g.is_empty()) {
            // A file Telegram named needs nothing synthesised, and leaves every
            // counter alone: audio.ogg came before audio_1@…
            let mut name = sanitize_filename(g, "file");
            if !name.contains('.') {
                let ext = if ext_hint.is_empty() {
                    default_ext
                } else {
                    ext_hint
                };
                name.push_str(ext);
            }
            return name;
        }

        let prefix = synth_prefix(subdir).to_string();
        let n = self.synth.entry(prefix.clone()).or_insert(0);
        *n += 1;
        let n = *n;
        let ext = if kind == "photos" {
            ".jpg".to_string()
        } else if ext_hint.is_empty() {
            default_ext.to_string()
        } else {
            ext_hint.to_string()
        };
        format!("{prefix}_{n}@{stamp}{ext}")
    }

    /// The counter value a prefix has reached, for tests and reporting.
    pub fn counter(&self, prefix: &str) -> u64 {
        self.synth.get(prefix).copied().unwrap_or(0)
    }

    /// Telegram's **own** thumbnail: `clip.mp4` -> `clip.mp4_thumb.jpg`.
    ///
    /// This is the one that reaches the JSON as `thumbnail`. Note it appends to
    /// the *full* name, extension included — it is not a sibling of the file,
    /// it is the file's name with a suffix.
    ///
    /// **Claimed, not string-appended.** The path used to be built by
    /// concatenation and never registered, so a document a sender had named
    /// `clip.mp4_thumb.jpg` landed exactly where `clip.mp4`'s thumbnail was
    /// about to be written and one of the two was lost.
    pub fn claim_telegram_thumb(&mut self, path: &str) -> String {
        let (subdir, file) = path.rsplit_once('/').unwrap_or(("", path));
        self.claim(subdir, &format!("{file}_thumb.jpg"))
    }

    /// Desktop's **rendered** downscale: `clip.mp4` -> `clip_thumb.mp4`.
    ///
    /// Used only as the `<img>` in the HTML, never written to the JSON. Both
    /// this and [`Self::claim_telegram_thumb`] exist on disk in a real export,
    /// and conflating them loses one of the two.
    pub fn claim_rendered_preview(&mut self, path: &str) -> String {
        let (subdir, file) = path.rsplit_once('/').unwrap_or(("", path));
        let (stem, ext) = match file.rsplit_once('.') {
            Some((s, e)) => (s.to_string(), format!(".{e}")),
            None => (file.to_string(), String::new()),
        };
        self.claim(subdir, &format!("{stem}_thumb{ext}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn illegal_characters_become_underscores() {
        assert_eq!(
            sanitize_filename("a<b>c:d\"e/f\\g|h?i*j", "x"),
            "a_b_c_d_e_f_g_h_i_j"
        );
    }

    #[test]
    fn windows_reserved_names_are_escaped_on_the_first_dot() {
        // CON, CON.txt and CON.txt.bak are all reserved.
        assert_eq!(sanitize_filename("CON", "x"), "_CON");
        assert_eq!(sanitize_filename("CON.txt", "x"), "_CON.txt");
        assert_eq!(sanitize_filename("CON.txt.bak", "x"), "_CON.txt.bak");
        assert_eq!(sanitize_filename("com4.mp3", "x"), "_com4.mp3");
        // A name that merely starts with those letters is fine.
        assert_eq!(sanitize_filename("CONTRACT.pdf", "x"), "CONTRACT.pdf");
    }

    #[test]
    fn a_trailing_dot_is_stripped_after_truncation_not_only_before() {
        // Windows silently drops a trailing dot, so the file on disk would be
        // "abc" while the JSON pointed at "abc." — a dangling reference an
        // existence check still answers true for.
        let long = format!("{}.", "a".repeat(179));
        let out = sanitize_filename(&long, "x");
        assert!(!out.ends_with('.'), "got {out}");
        assert_eq!(out.chars().count(), 179);
    }

    #[test]
    fn an_empty_or_all_illegal_name_falls_back() {
        assert_eq!(sanitize_filename("", "fallback"), "fallback");
        assert_eq!(sanitize_filename("   ", "fallback"), "fallback");
        assert_eq!(sanitize_filename("...", "fallback"), "fallback");
    }

    #[test]
    fn the_extension_is_sanitised_too() {
        // Raw, this injects separators and makes the download fail — letting a
        // sender guarantee their file is never saved.
        assert_eq!(sanitize_extension("evil../../x"), ".x");
        assert_eq!(sanitize_extension("a.mp4"), ".mp4");
        assert_eq!(sanitize_extension("a.we:rd"), ".werd");
        assert_eq!(sanitize_extension("noext"), ".noext");
        assert_eq!(sanitize_extension(""), "");
    }

    #[test]
    fn a_long_extension_is_capped() {
        let out = sanitize_extension(&format!("a.{}", "x".repeat(100)));
        assert!(out.len() <= 17, "got {out}");
    }

    #[test]
    fn every_media_bearing_message_takes_a_number_written_or_not() {
        // Rule 1: reading photo_N as "the Nth photo-bearing message" is what
        // matches 557 of 557; "the Nth written" matches 64.
        let mut b = NameBook::new();
        let a = b.reserve_name("photos", None, "01-01-2025", "");
        let _skipped = b.reserve_name("photos", None, "01-01-2025", "");
        let c = b.reserve_name("photos", None, "01-01-2025", "");
        assert_eq!(a, "photo_1@01-01-2025.jpg");
        assert_eq!(
            c, "photo_3@01-01-2025.jpg",
            "a skipped file must still count"
        );
    }

    #[test]
    fn each_prefix_has_its_own_counter() {
        // Rule 2, and the measured proof: audio_1..3 on voice messages that
        // come after named documents, not file_3..5.
        let mut b = NameBook::new();
        b.reserve_name("files", Some("audio.ogg"), "s", "");
        b.reserve_name("files", Some("other.pdf"), "s", "");
        let v1 = b.reserve_name("voice_messages", None, "s", "");
        let v2 = b.reserve_name("voice_messages", None, "s", "");
        assert_eq!(v1, "audio_1@s.ogg");
        assert_eq!(v2, "audio_2@s.ogg");
        // And a photo is on a different counter again.
        assert_eq!(b.reserve_name("photos", None, "s", ""), "photo_1@s.jpg");
    }

    #[test]
    fn a_stripped_thumbnail_does_not_consume_the_file_counter() {
        // The bug: layout("thumbnails") fell through to ("files", ""), so
        // synth_prefix returned "file" and a stripped thumbnail took
        // file_1@stamp.jpg — the same counter an unnamed document uses.
        // The next real document then became file_2 instead of file_1,
        // shifting every later name in files/. Verified live: a real export
        // had thumbnails/file_10@28-01-2026_22-36-41.jpg.
        let mut b = NameBook::new();
        let thumb = b.reserve_name("thumbnails", None, "s", "");
        let doc = b.reserve_name("files", None, "s", "");
        assert_eq!(thumb, "thumb_1@s.jpg");
        assert_eq!(doc, "file_1@s", "the thumbnail must not have taken file_1");
        assert_eq!(b.counter("thumb"), 1);
        assert_eq!(b.counter("file"), 1);
    }

    #[test]
    fn video_numbering_survives_skipped_videos() {
        // The measured case: video_6 is the only unnamed video Desktop wrote;
        // the five before it were skipped for size and still took numbers.
        let mut b = NameBook::new();
        for _ in 0..5 {
            b.reserve_name("video_files", None, "s", "");
        }
        assert_eq!(
            b.reserve_name("video_files", None, "s", ""),
            "video_6@s.mp4"
        );
    }

    #[test]
    fn a_named_file_leaves_every_counter_alone() {
        // Rule 3.
        let mut b = NameBook::new();
        b.reserve_name("photos", None, "s", ""); // photo_1
        b.reserve_name("files", Some("report.pdf"), "s", "");
        b.reserve_name("video_files", Some("clip.mp4"), "s", "");
        assert_eq!(b.counter("photo"), 1);
        assert_eq!(b.counter("video"), 0);
        assert_eq!(b.counter("file"), 0);
    }

    #[test]
    fn a_photo_never_takes_a_supplied_name() {
        let mut b = NameBook::new();
        // Only photos are synthesised; a name here must be ignored.
        assert_eq!(
            b.reserve_name("photos", Some("given.png"), "s", ""),
            "photo_1@s.jpg"
        );
    }

    #[test]
    fn a_webm_sticker_is_filed_as_a_video() {
        // Measured: four files in the reference are mime_type video/webm with
        // media_type sticker, and Desktop put all four in video_files/.
        // Routing on media_type alone also hands them the stickers/ collision
        // counter, which shifts every later sticker — so this one rule
        // accounted for ten wrong filenames, not four.
        assert_eq!(kind_for("sticker", "video/webm"), "video_files");
        assert_eq!(kind_for("sticker", "image/webp"), "stickers");
    }

    #[test]
    fn shape_routing_covers_the_other_kinds() {
        assert_eq!(kind_for("video_file", "video/mp4"), "video_files");
        assert_eq!(kind_for("animation", "video/mp4"), "video_files");
        assert_eq!(kind_for("voice_message", "audio/ogg"), "voice_messages");
        assert_eq!(kind_for("video_message", "video/mp4"), "video_messages");
        assert_eq!(kind_for("", "application/pdf"), "files");
        // A bare document that is really a video still lands with the videos.
        assert_eq!(kind_for("", "video/mp4"), "video_files");
    }

    #[test]
    fn animations_are_filed_with_the_videos() {
        // This export has no animations/ folder at all.
        assert_eq!(layout("animations").0, "video_files");
        assert_eq!(layout("video_messages").0, "round_video_messages");
    }

    #[test]
    fn a_collision_earns_a_numbered_suffix() {
        let mut b = NameBook::new();
        assert_eq!(b.claim("stickers", "sticker.webp"), "stickers/sticker.webp");
        assert_eq!(
            b.claim("stickers", "sticker.webp"),
            "stickers/sticker (1).webp"
        );
        assert_eq!(
            b.claim("stickers", "sticker.webp"),
            "stickers/sticker (2).webp"
        );
    }

    #[test]
    fn claiming_is_case_insensitive_because_windows_is() {
        let mut b = NameBook::new();
        b.claim("files", "Report.PDF");
        assert_eq!(b.claim("files", "report.pdf"), "files/report (1).pdf");
    }

    #[test]
    fn an_extensionless_collision_suffixes_at_the_end() {
        let mut b = NameBook::new();
        b.claim("files", "README");
        assert_eq!(b.claim("files", "README"), "files/README (1)");
    }

    #[test]
    fn a_telegram_thumbnail_is_claimed_not_string_appended() {
        // The bug: a document named clip.mp4_thumb.jpg landed exactly where
        // clip.mp4's thumbnail was about to go, and one of the two was lost.
        let mut b = NameBook::new();
        let doc = b.claim("video_files", "clip.mp4_thumb.jpg");
        let thumb = b.claim_telegram_thumb("video_files/clip.mp4");
        assert_eq!(doc, "video_files/clip.mp4_thumb.jpg");
        assert_ne!(thumb, doc, "the thumbnail overwrote a real document");
        assert_eq!(thumb, "video_files/clip.mp4_thumb (1).jpg");
    }

    #[test]
    fn the_two_thumbnails_are_different_files() {
        // Both exist on disk in a real export: <full name>_thumb.jpg is
        // Telegram's own and reaches the JSON; <stem>_thumb<ext> is the
        // downscale Desktop renders and shows in the HTML. Conflating them
        // loses one of the two.
        let mut b = NameBook::new();
        let tg = b.claim_telegram_thumb("video_files/clip.mp4");
        let rendered = b.claim_rendered_preview("video_files/clip.mp4");
        assert_eq!(tg, "video_files/clip.mp4_thumb.jpg");
        assert_eq!(rendered, "video_files/clip_thumb.mp4");
        assert_ne!(tg, rendered);
    }

    #[test]
    fn different_folders_do_not_collide() {
        let mut b = NameBook::new();
        assert_eq!(b.claim("photos", "a.jpg"), "photos/a.jpg");
        assert_eq!(b.claim("files", "a.jpg"), "files/a.jpg");
    }
}
