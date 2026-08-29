//! The typefaces, embedded in the binary and handed to the text system.
//!
//! `fonts/*.ttf` are a **build artefact, checked in rather than generated**.
//! They are Geist and Geist Mono with Latin and Cyrillic *merged into one file
//! per weight*, because a toolkit with no equivalent of CSS `unicode-range`
//! cannot stitch two files into one family. Do not
//! regenerate or subset them: the merge is the reason a Russian or Ukrainian
//! chat title renders in the design's typeface instead of dropping to a
//! system fallback halfway through a word.
//!
//! **The family names below are the names inside the files, not names we
//! chose.** `add_fonts` registers each face under whatever its `name` table
//! says, and `font_family()` asked for anything else falls silently back to the
//! platform UI font — a window that looks exactly like one where this module
//! was never called at all. That is the failure this file exists to prevent, so
//! [`register`] does not trust `add_fonts` returning `Ok`; it asks the text
//! system which families it now has and reports the mismatch by name.

use std::borrow::Cow;

/// The text face. Every string in the window is set in this.
pub const SANS: &str = "Geist";

/// The figure face. The design sets **every number** in it — counts, sizes,
/// progress, timestamps — so that a column of digits stays a column while it
/// ticks upward, which a proportional face cannot do.
pub const MONO: &str = "Geist Mono";

/// The three sans weights and the two mono weights, in one array so that
/// `register` and the tests below can never disagree about what is embedded.
///
/// All five are handed to `add_fonts` in a single call. On Windows each call
/// rebuilds the custom font collection, so registering in dribs and drabs is
/// wasted work at best; one call is also one failure to report.
const EMBEDDED: [(&str, &[u8]); 5] = [
    (
        "Geist-Regular",
        include_bytes!("../fonts/Geist-Regular.ttf"),
    ),
    ("Geist-Medium", include_bytes!("../fonts/Geist-Medium.ttf")),
    (
        "Geist-SemiBold",
        include_bytes!("../fonts/Geist-SemiBold.ttf"),
    ),
    (
        "GeistMono-Regular",
        include_bytes!("../fonts/GeistMono-Regular.ttf"),
    ),
    (
        "GeistMono-Medium",
        include_bytes!("../fonts/GeistMono-Medium.ttf"),
    ),
];

/// Why the window is about to be set in the wrong typeface.
///
/// Both variants are recoverable — the export runs fine in Segoe UI — so this
/// is returned rather than panicked, and the caller reports it and carries on.
/// The point is only that the user is told, because the symptom on its own
/// (slightly wrong-looking text) is not something anyone reports as a bug.
#[derive(Debug)]
pub enum FontError {
    /// The platform text system refused the bytes outright.
    Rejected(String),
    /// The bytes loaded, but not under the names the design asks for. The
    /// families actually registered are listed, because that list *is* the
    /// fix: whatever it contains is what [`SANS`] and [`MONO`] should say.
    WrongFamilyNames {
        missing: Vec<&'static str>,
        registered: Vec<String>,
    },
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(why) => {
                write!(f, "the embedded typefaces could not be loaded: {why}")
            }
            Self::WrongFamilyNames {
                missing,
                registered,
            } => write!(
                f,
                "the embedded typefaces loaded but not as {}; \
                 the text system now offers {:?}. The window will use the \
                 system font instead.",
                missing.join(" and "),
                registered
            ),
        }
    }
}

impl std::error::Error for FontError {}

/// Register the design's typefaces with GPUI's text system.
///
/// Call once, before the first window opens. Registering later is not an
/// error, but any text already laid out keeps the face it was measured with.
pub fn register(cx: &mut gpui::App) -> Result<(), FontError> {
    let text_system = cx.text_system().clone();
    text_system
        .add_fonts(EMBEDDED.iter().map(|(_, b)| Cow::Borrowed(*b)).collect())
        .map_err(|e| FontError::Rejected(e.to_string()))?;

    // **`add_fonts` returning `Ok` only means the bytes parsed.** It says
    // nothing about the names they landed under, and a wrong name is invisible
    // at every later step: `font_family("Geist")` on an unknown family does not
    // error, it silently resolves to the platform UI font. `all_font_names`
    // includes the custom collection, so this is the one moment where the
    // mismatch can still be caught and named.
    let available = text_system.all_font_names();
    let missing: Vec<&'static str> = [SANS, MONO]
        .into_iter()
        .filter(|wanted| !available.iter().any(|have| have == wanted))
        .collect();
    if !missing.is_empty() {
        return Err(FontError::WrongFamilyNames {
            missing,
            // Only the Geist-ish entries: `available` is every font installed
            // on the machine, and a diagnostic nobody will read is no better
            // than no diagnostic.
            registered: available
                .into_iter()
                .filter(|name| name.starts_with("Geist"))
                .collect(),
        });
    }
    Ok(())
}

/// The sans, with the OpenType features the design asks for.
///
/// **`ss01` and `cv11` are not wired, and cannot be.** GPUI *can* express
/// OpenType features — `gpui::Font::features` reaches DirectWrite as an
/// `IDWriteTypography` and is applied to every laid-out line — but these
/// particular files do not contain those tables. The merge that gave them
/// Cyrillic dropped the stylistic and character-variant sets; `GSUB`/`GPOS`
/// hold only `ccmp dnom frac kern liga locl mark mkmk numr pnum tnum`. Asking
/// DirectWrite for an absent feature is a no-op, not an error, so naming them
/// here would look applied and do nothing. Restoring them means rebuilding the
/// fonts upstream, where the weights are merged, not adding a tag to this list.
///
/// `tnum` is present and is set: the design puts figures in tables and beside
/// each other in the queue, and proportional digits make a count that changes
/// shift its neighbours sideways on every repaint.
pub fn sans() -> gpui::Font {
    let mut font = gpui::font(SANS);
    font.features = gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]));
    font
}

/// The mono, for numbers.
///
/// No features: `tnum` is not in these files either, and would be redundant in
/// a face whose every advance is already the same width.
pub fn mono() -> gpui::Font {
    gpui::font(MONO)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `name` table's typographic family (id 16) if the font has one, else
    /// the legacy family (id 1).
    ///
    /// Written out by hand rather than pulled in as a dependency: this is the
    /// only thing in the crate that needs to read a font file, and the whole
    /// point is to check the constants against the *bytes we ship*, which a
    /// parser sharing GPUI's own opinions could not do independently.
    fn family_name(ttf: &[u8]) -> Option<String> {
        let be16 = |at: usize| -> Option<usize> {
            Some(u16::from_be_bytes(ttf.get(at..at + 2)?.try_into().ok()?) as usize)
        };
        let be32 = |at: usize| -> Option<usize> {
            Some(u32::from_be_bytes(ttf.get(at..at + 4)?.try_into().ok()?) as usize)
        };

        let table_count = be16(4)?;
        let mut name_table = None;
        for i in 0..table_count {
            let record = 12 + i * 16;
            if ttf.get(record..record + 4)? == b"name" {
                name_table = Some(be32(record + 8)?);
                break;
            }
        }
        let name_table = name_table?;
        let count = be16(name_table + 2)?;
        let strings = name_table + be16(name_table + 4)?;

        // Prefer id 16 over id 1: with 16 present, id 1 is the *per-weight*
        // name ("Geist Medium"), and grouping the weights under one family is
        // exactly what a font collection does.
        let mut best: Option<(usize, String)> = None;
        for i in 0..count {
            let record = name_table + 6 + i * 12;
            let (platform, encoding) = (be16(record)?, be16(record + 2)?);
            let name_id = be16(record + 6)?;
            if platform != 3 || encoding != 1 || (name_id != 1 && name_id != 16) {
                continue; // Windows/UCS-2 only; the Mac records repeat these.
            }
            let (len, offset) = (be16(record + 8)?, be16(record + 10)?);
            let bytes = ttf.get(strings + offset..strings + offset + len)?;
            let text: String = bytes
                .chunks_exact(2)
                .map(|p| u16::from_be_bytes([p[0], p[1]]))
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|u| char::from_u32(u as u32))
                .collect();
            if best.as_ref().is_none_or(|(id, _)| name_id > *id) {
                best = Some((name_id, text));
            }
        }
        best.map(|(_, text)| text)
    }

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
    fn the_family_constants_are_the_names_inside_the_files() {
        // The whole failure mode this module guards against, checked without a
        // window: if a constant and its file disagree, `font_family` falls back
        // to the system face and nothing else in the build complains.
        for (file, bytes) in EMBEDDED {
            let found = family_name(bytes).expect("no usable name table");
            let expected = if file.starts_with("GeistMono") {
                MONO
            } else {
                SANS
            };
            assert_eq!(found, expected, "{file} calls itself {found:?}");
        }
    }

    #[test]
    fn the_sans_and_the_mono_are_two_different_families() {
        // They are separate files with separate names, and the design leans on
        // that: "every number in the mono" means nothing if asking for the mono
        // hands back the sans.
        assert_ne!(SANS, MONO);
    }

    #[test]
    fn the_requested_features_actually_exist_in_the_files() {
        // A feature tag that the font does not contain is silently ignored by
        // every text system there is, so the only way to keep `sans()` honest
        // is to look. `ss01` and `cv11` are deliberately absent from the list
        // below and from the fonts; see `sans()` for why.
        let (_, sans_bytes) = EMBEDDED[0];
        for (tag, _) in sans().features.tag_value_list() {
            assert!(
                contains_feature(sans_bytes, tag),
                "Geist-Regular has no {tag:?} table, so requesting it does nothing"
            );
        }
    }

    /// Whether a `GSUB`/`GPOS` feature tag appears anywhere in the font.
    ///
    /// A substring scan of the whole file, not a parse: feature tags are
    /// four ASCII bytes and a false positive would need those exact bytes
    /// elsewhere, which for `tnum` is vanishingly unlikely and would only ever
    /// make this test *pass* wrongly, never fail a good font.
    fn contains_feature(ttf: &[u8], tag: &str) -> bool {
        ttf.windows(4).any(|w| w == tag.as_bytes())
    }
}
