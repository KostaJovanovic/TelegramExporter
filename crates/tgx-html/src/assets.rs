//! Telegram Desktop's own export assets, **copied verbatim from a real export**.
//!
//! `assets/` holds Desktop's `css/style.css`, `js/script.js` and the 42 PNGs
//! its stylesheet references, taken byte for byte from an export Desktop itself
//! produced.
//!
//! An earlier version of the Python original carried a hand-written
//! reproduction of the stylesheet: 6.2 KB against Desktop's 43 KB, with no
//! images at all, so every `media_file` / `media_photo` / `media_voice` row
//! rendered as a blank square. **Reproducing a stylesheet by eye cannot
//! converge**; shipping the real one is both smaller work and exact.
//!
//! They are embedded in the binary rather than shipped beside it, so there is
//! no `_MEIPASS` branch, no spec file and no `datas` list — the whole class of
//! "the frozen build cannot find its assets" bug does not exist here.

use std::io;
use std::path::Path;

/// Desktop's stylesheet.
pub const STYLE_CSS: &str = include_str!("../assets/style.css");

/// Desktop's script — `GoToMessage`, `ShowSpoiler`, `ShowHashtag`, and the rest
/// of the handlers the writer emits calls to.
pub const SCRIPT_JS: &str = include_str!("../assets/script.js");

/// The 42 media-type icons the stylesheet references as `../images/*.png`.
///
/// **Not optional decoration**: without them every file row loses its coloured
/// glyph and the pagination arrows disappear.
pub const IMAGES: &[(&str, &[u8])] = &[
    ("back.png", include_bytes!("../assets/images/back.png")),
    (
        "back@2x.png",
        include_bytes!("../assets/images/back@2x.png"),
    ),
    (
        "media_call.png",
        include_bytes!("../assets/images/media_call.png"),
    ),
    (
        "media_call@2x.png",
        include_bytes!("../assets/images/media_call@2x.png"),
    ),
    (
        "media_contact.png",
        include_bytes!("../assets/images/media_contact.png"),
    ),
    (
        "media_contact@2x.png",
        include_bytes!("../assets/images/media_contact@2x.png"),
    ),
    (
        "media_file.png",
        include_bytes!("../assets/images/media_file.png"),
    ),
    (
        "media_file@2x.png",
        include_bytes!("../assets/images/media_file@2x.png"),
    ),
    (
        "media_game.png",
        include_bytes!("../assets/images/media_game.png"),
    ),
    (
        "media_game@2x.png",
        include_bytes!("../assets/images/media_game@2x.png"),
    ),
    (
        "media_location.png",
        include_bytes!("../assets/images/media_location.png"),
    ),
    (
        "media_location@2x.png",
        include_bytes!("../assets/images/media_location@2x.png"),
    ),
    (
        "media_music.png",
        include_bytes!("../assets/images/media_music.png"),
    ),
    (
        "media_music@2x.png",
        include_bytes!("../assets/images/media_music@2x.png"),
    ),
    (
        "media_photo.png",
        include_bytes!("../assets/images/media_photo.png"),
    ),
    (
        "media_photo@2x.png",
        include_bytes!("../assets/images/media_photo@2x.png"),
    ),
    (
        "media_shop.png",
        include_bytes!("../assets/images/media_shop.png"),
    ),
    (
        "media_shop@2x.png",
        include_bytes!("../assets/images/media_shop@2x.png"),
    ),
    (
        "media_video.png",
        include_bytes!("../assets/images/media_video.png"),
    ),
    (
        "media_video@2x.png",
        include_bytes!("../assets/images/media_video@2x.png"),
    ),
    (
        "media_voice.png",
        include_bytes!("../assets/images/media_voice.png"),
    ),
    (
        "media_voice@2x.png",
        include_bytes!("../assets/images/media_voice@2x.png"),
    ),
    (
        "section_calls.png",
        include_bytes!("../assets/images/section_calls.png"),
    ),
    (
        "section_calls@2x.png",
        include_bytes!("../assets/images/section_calls@2x.png"),
    ),
    (
        "section_chats.png",
        include_bytes!("../assets/images/section_chats.png"),
    ),
    (
        "section_chats@2x.png",
        include_bytes!("../assets/images/section_chats@2x.png"),
    ),
    (
        "section_contacts.png",
        include_bytes!("../assets/images/section_contacts.png"),
    ),
    (
        "section_contacts@2x.png",
        include_bytes!("../assets/images/section_contacts@2x.png"),
    ),
    (
        "section_frequent.png",
        include_bytes!("../assets/images/section_frequent.png"),
    ),
    (
        "section_frequent@2x.png",
        include_bytes!("../assets/images/section_frequent@2x.png"),
    ),
    (
        "section_music.png",
        include_bytes!("../assets/images/section_music.png"),
    ),
    (
        "section_music@2x.png",
        include_bytes!("../assets/images/section_music@2x.png"),
    ),
    (
        "section_other.png",
        include_bytes!("../assets/images/section_other.png"),
    ),
    (
        "section_other@2x.png",
        include_bytes!("../assets/images/section_other@2x.png"),
    ),
    (
        "section_photos.png",
        include_bytes!("../assets/images/section_photos.png"),
    ),
    (
        "section_photos@2x.png",
        include_bytes!("../assets/images/section_photos@2x.png"),
    ),
    (
        "section_sessions.png",
        include_bytes!("../assets/images/section_sessions.png"),
    ),
    (
        "section_sessions@2x.png",
        include_bytes!("../assets/images/section_sessions@2x.png"),
    ),
    (
        "section_stories.png",
        include_bytes!("../assets/images/section_stories.png"),
    ),
    (
        "section_stories@2x.png",
        include_bytes!("../assets/images/section_stories@2x.png"),
    ),
    (
        "section_web.png",
        include_bytes!("../assets/images/section_web.png"),
    ),
    (
        "section_web@2x.png",
        include_bytes!("../assets/images/section_web@2x.png"),
    ),
];

/// Lay out `css/`, `js/` and `images/` beside an export's pages.
pub fn write_assets(root: &Path) -> io::Result<()> {
    std::fs::create_dir_all(root.join("css"))?;
    std::fs::create_dir_all(root.join("js"))?;
    let images = root.join("images");
    std::fs::create_dir_all(&images)?;

    std::fs::write(root.join("css").join("style.css"), STYLE_CSS)?;
    std::fs::write(root.join("js").join("script.js"), SCRIPT_JS)?;
    for (name, bytes) in IMAGES {
        let target = images.join(name);
        // Skip a file that is already there at the right size, so re-running an
        // export over an existing folder is cheap.
        if let Ok(meta) = std::fs::metadata(&target) {
            if meta.len() == bytes.len() as u64 {
                continue;
            }
        }
        std::fs::write(target, bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stylesheet_is_desktops_not_a_reproduction() {
        // The hand-written one was 6.2 KB against Desktop's 43 KB.
        assert!(
            STYLE_CSS.len() > 40_000,
            "style.css is {} bytes — that is a reproduction, not the real one",
            STYLE_CSS.len()
        );
        // And it really does reference the images.
        assert!(STYLE_CSS.contains("../images/"), "no image references");
    }

    #[test]
    fn the_script_carries_the_handlers_the_writer_emits() {
        // Every one of these appears in markup tgx-html generates; a missing
        // handler is a dead link in every export.
        for handler in [
            "GoToMessage",
            "ShowSpoiler",
            "ShowHashtag",
            "ShowCashtag",
            "ShowBotCommand",
            "ShowMentionName",
            "CheckLocation",
        ] {
            assert!(SCRIPT_JS.contains(handler), "script.js has no {handler}");
        }
    }

    #[test]
    fn every_image_is_present_and_non_empty() {
        assert_eq!(IMAGES.len(), 42, "the icon set changed size");
        for (name, bytes) in IMAGES {
            assert!(!bytes.is_empty(), "{name} is empty");
            assert_eq!(&bytes[1..4], b"PNG", "{name} is not a PNG");
        }
    }

    #[test]
    fn writing_assets_lays_out_the_three_folders() {
        let dir = std::env::temp_dir().join(format!("tgx-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_assets(&dir).unwrap();
        assert!(dir.join("css/style.css").is_file());
        assert!(dir.join("js/script.js").is_file());
        assert!(dir.join("images/media_photo.png").is_file());
        // Idempotent: a second run over the same folder is fine.
        write_assets(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
