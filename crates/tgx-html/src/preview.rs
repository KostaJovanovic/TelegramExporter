//! Inline preview sizing.
//!
//! Reproduced exactly because the result is written into the `style` attribute
//! of every inline preview, and rounding differently shows up as a one-pixel
//! difference on most images — which is thousands of diffing lines.

/// Desktop scales an inline preview to fit this box.
pub const PREVIEW_BOX: i64 = 260;

/// Stickers get a smaller one.
pub const STICKER_BOX: i64 = 192;

/// A photo smaller than this in either direction is drawn as a media row rather
/// than inlined.
///
/// The reference pins the threshold only loosely: its one row case is 260×74
/// and its smallest inlined photo is 188×232, so the cut sits somewhere in
/// (74, 188]. 100 is Desktop's usual minimum-photo constant.
pub const MIN_INLINE_PHOTO: i64 = 100;

/// Qt's `QSize::scaled(box, box, KeepAspectRatio)`, with integer division.
pub fn fit_box(width: i64, height: i64, box_size: i64) -> (i64, i64) {
    if width <= 0 || height <= 0 {
        return (box_size, box_size);
    }
    let rw = box_size * width / height;
    if rw <= box_size {
        (rw, box_size)
    } else {
        (box_size, box_size * height / width)
    }
}

/// `(file pixels, css pixels)` for an inline preview.
///
/// Desktop stores the preview at **twice** its displayed size for high-density
/// screens, but **never upscales**: a 480×320 photo is kept at 480×320 and
/// shown at 240×160, not stretched to fill the 520-pixel box. Missing that
/// clamp put one image in the reference a full 20 pixels wide.
pub fn preview_size(width: i64, height: i64, box_size: i64) -> ((i64, i64), (i64, i64)) {
    let (w, h) = (width.max(0), height.max(0));
    let mut px = fit_box(w, h, box_size * 2);
    if w > 0 && w < px.0 {
        px = (w, h);
    }
    (px, ((px.0 / 2).max(1), (px.1 / 2).max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_photo_is_never_upscaled() {
        // The measured case: 480x320 shows at 240x160, not stretched to the
        // 520-pixel box. Dropping the clamp is what made one reference image
        // twenty pixels wide.
        let (file, css) = preview_size(480, 320, PREVIEW_BOX);
        assert_eq!(file, (480, 320));
        assert_eq!(css, (240, 160));
    }

    #[test]
    fn a_large_photo_fits_the_doubled_box() {
        let (file, css) = preview_size(2560, 1706, PREVIEW_BOX);
        // 520-wide box, aspect kept by integer division.
        assert_eq!(file.0, 520);
        assert_eq!(file.1, 520 * 1706 / 2560);
        assert_eq!(css, (260, file.1 / 2));
    }

    #[test]
    fn integer_division_not_rounding() {
        // 520 * 1706 / 2560 = 346.5 -> 346, not 347. One pixel here is
        // thousands of diffing lines.
        assert_eq!(fit_box(2560, 1706, 520), (520, 346));
    }

    #[test]
    fn a_tall_image_is_bounded_by_height() {
        let (w, h) = fit_box(100, 400, 200);
        assert_eq!(h, 200);
        assert_eq!(w, 50);
    }

    #[test]
    fn css_size_never_collapses_to_zero() {
        // A 1px-tall image halves to 0, which would emit `height: 0px`.
        let (_, css) = preview_size(1, 1, PREVIEW_BOX);
        assert!(css.0 >= 1 && css.1 >= 1, "got {css:?}");
    }

    #[test]
    fn missing_dimensions_fall_back_to_the_box() {
        assert_eq!(fit_box(0, 0, 200), (200, 200));
        assert_eq!(fit_box(-5, 10, 200), (200, 200));
    }

    /// Would Desktop inline a photo of this size, or draw it as a row?
    ///
    /// Mirrors the decision in `writer::render_media` so the threshold can be
    /// tested against the measured cases rather than against itself.
    fn inlines(width: i64, height: i64) -> bool {
        width.min(height) >= MIN_INLINE_PHOTO
    }

    #[test]
    fn the_inline_cut_sits_inside_the_measured_bracket() {
        // The reference's one row case is 260x74 and its smallest inlined
        // photo is 188x232, so the threshold must fall in (74, 188]. A const
        // assertion rather than a runtime one: it is a statement about the
        // constant, so it should fail the build, not a test run.
        const _: () = assert!(MIN_INLINE_PHOTO > 74 && MIN_INLINE_PHOTO <= 188);
        // It must also classify both measured cases correctly. Asserted through
        // the decision function, not against the constants themselves — the
        // first version of this test compared two literals and so could not
        // fail whatever the threshold was.
        assert!(!inlines(260, 74), "the measured row case must not inline");
        assert!(
            inlines(188, 232),
            "the measured inline case must not become a row"
        );
        // Orientation must not matter.
        assert!(!inlines(74, 260));
        assert!(inlines(232, 188));
    }
}
