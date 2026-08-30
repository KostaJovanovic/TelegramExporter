//! The typefaces, embedded in the binary and handed to the text system.
//!
//! `fonts/*.ttf` are a **build artefact, checked in rather than generated**.
//! They are Geist and Geist Mono with Latin and Cyrillic *merged into one file
//! per weight*, because a toolkit with no equivalent of CSS `unicode-range`
//! cannot stitch two files into one family. Do not regenerate or subset them:
//! the merge is the reason a Russian or Ukrainian chat title renders in the
//! design's typeface instead of dropping to a system fallback halfway through a
//! word.
//!
//! **The family keys below are ours, not the files'.** This is the one thing
//! the swap away from GPUI genuinely simplified. A platform text system reads
//! the `name` table out of the file and registers each face under whatever it
//! finds there, so asking for `"Geist"` when the file called itself something
//! else fell back silently to the platform UI font — a window indistinguishable
//! from one where this module was never called. Guarding that took a hand-rolled
//! `name`-table parser, a two-variant error type and a post-registration
//! readback. egui takes a key we choose, so the mismatch it all defended against
//! cannot occur, and every line of it is gone.
//!
//! **`tnum` is no longer requested, and no longer needs to be.** GPUI could
//! reach DirectWrite with OpenType features; egui's text stack does not shape
//! with them. The design already sets **every number** in the mono — counts,
//! sizes, progress, timestamps — and a monospaced face is tabular by
//! construction, so the one place tabular figures actually mattered still has
//! them. What is lost is tabular digits in *sans* runs, which the design has
//! none of.

use eframe::egui::{FontData, FontDefinitions, FontFamily, FontId};

/// The text face key. Every string in the window is set in this.
pub const SANS: &str = "geist";

/// The figure face key. The design sets **every number** in it — counts, sizes,
/// progress, timestamps — so that a column of digits stays a column while it
/// ticks upward, which a proportional face cannot do.
pub const MONO: &str = "geist-mono";

/// The face headings and emphasis are set in.
pub const MEDIUM: &str = "geist-medium";

/// The heaviest weight, for the one or two places that need to outrank MEDIUM.
pub const SEMIBOLD: &str = "geist-semibold";

/// The mono's heavier weight, for a figure that has to stand out from the
/// column it sits in without leaving the face.
pub const MONO_MEDIUM: &str = "geist-mono-medium";

/// The three sans weights and the two mono weights, in one array so that
/// [`definitions`] and the tests below can never disagree about what is
/// embedded.
const EMBEDDED: [(&str, &[u8]); 5] = [
    (SANS, include_bytes!("../fonts/Geist-Regular.ttf")),
    (MEDIUM, include_bytes!("../fonts/Geist-Medium.ttf")),
    (SEMIBOLD, include_bytes!("../fonts/Geist-SemiBold.ttf")),
    (MONO, include_bytes!("../fonts/GeistMono-Regular.ttf")),
    (
        MONO_MEDIUM,
        include_bytes!("../fonts/GeistMono-Medium.ttf"),
    ),
];

/// Every face, under the keys the rest of the design asks for.
///
/// Built from [`FontDefinitions::empty`] rather than `default`, which is what
/// `default_fonts = false` in the manifest is for: egui otherwise ships its own
/// Ubuntu/Hack pair and merges them in as fallbacks, so a missing glyph would
/// silently render in a face that is not part of this design.
pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    for (key, bytes) in EMBEDDED {
        fonts
            .font_data
            .insert(key.to_owned(), FontData::from_static(bytes).into());
    }
    fonts
        .families
        .insert(FontFamily::Proportional, vec![SANS.to_owned()]);
    fonts
        .families
        .insert(FontFamily::Monospace, vec![MONO.to_owned()]);
    for key in [MEDIUM, SEMIBOLD, MONO_MEDIUM] {
        fonts
            .families
            .insert(FontFamily::Name(key.into()), vec![key.to_owned()]);
    }
    fonts
}

/// The sans, at a size.
pub fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// The mono, for numbers.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// The face the headings are set in.
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MEDIUM.into()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// The mono's heavier weight.
pub fn mono_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MONO_MEDIUM.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_file_is_a_real_truetype_font() {
        // Catches a copy that produced an empty file or a Git LFS pointer —
        // both of which compile, ship, and fail only as a wrong-looking window.
        for (name, bytes) in EMBEDDED {
            assert!(
                bytes.len() > 20_000,
                "{name} is {} bytes, which is not a font",
                bytes.len()
            );
            assert!(
                matches!(&bytes[..4], b"\x00\x01\x00\x00" | b"true" | b"OTTO"),
                "{name} does not start with a TrueType or OpenType signature"
            );
        }
    }

    #[test]
    fn every_family_the_design_asks_for_has_bytes_behind_it() {
        // egui falls back to an empty glyph set for a family it does not have,
        // which draws as a window with no text in it at all. This is the
        // replacement for the old `name`-table readback: the failure is now a
        // key that was never registered rather than a name that did not match,
        // and unlike that one it is decidable without a window.
        let fonts = definitions();
        for id in [
            sans(16.0),
            mono(16.0),
            medium(16.0),
            semibold(16.0),
            mono_medium(16.0),
        ] {
            let keys = fonts
                .families
                .get(&id.family)
                .unwrap_or_else(|| panic!("{:?} is not registered", id.family));
            assert!(!keys.is_empty(), "{:?} has no face behind it", id.family);
            for key in keys {
                assert!(fonts.font_data.contains_key(key), "{key} has no bytes");
            }
        }
    }

    #[test]
    fn no_face_outside_this_design_is_registered() {
        // `default_fonts` is off in the manifest so that egui's own Ubuntu and
        // Hack are not merged in as fallbacks. If that feature came back, a
        // missing glyph would quietly render in a typeface nobody chose, which
        // is the same class of failure the old `name`-table guard existed for.
        let fonts = definitions();
        assert_eq!(fonts.font_data.len(), EMBEDDED.len());
        for key in fonts.font_data.keys() {
            assert!(key.starts_with("geist"), "{key} is not part of this design");
        }
    }

    #[test]
    fn the_sans_and_the_mono_are_two_different_families() {
        // They are separate files with separate keys, and the design leans on
        // that: "every number in the mono" means nothing if asking for the mono
        // hands back the sans.
        assert_ne!(SANS, MONO);
        assert_ne!(sans(16.0).family, mono(16.0).family);
    }
}
