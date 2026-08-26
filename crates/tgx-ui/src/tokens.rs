//! The design tokens, taken from `file-analyser/web/assets/css/analyser.css`.
//!
//! Swiss/International: black on white, hairline rules, square corners, one
//! red. The values are the site's `:root` and `:root[data-theme="dark"]`
//! blocks, copied rather than re-derived so the two stay recognisably one
//! design.
//!
//! **Both appearances carry the same keys**, enforced by the type rather than
//! by a test — a palette is a struct, so a colour that exists in light and not
//! in dark will not compile.
//!
//! This is deliberately *not* a port of the Qt `theme.py`. Roughly a third of
//! that file's comments are apologies for what a Qt stylesheet cannot express —
//! letter-spacing, transitions, shadows — and GPUI can express all three, so
//! the tokens come from the stylesheet directly.

use gpui::{hsla, Hsla};

/// One appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// The page itself, behind everything.
    pub bg: Hsla,
    /// Body text.
    pub fg: Hsla,
    /// Secondary text.
    pub muted: Hsla,
    /// The 1px rule that does all the dividing. **Pure ink in light, mid grey
    /// in dark** — a dark theme with black hairlines shows nothing at all.
    pub hairline: Hsla,
    /// The softer divider.
    pub rule: Hsla,
    /// A raised fill.
    pub surface: Hsla,
    /// The one red.
    pub accent: Hsla,
    /// Text on the accent.
    pub accent_fg: Hsla,
    /// The grid backdrop's ink.
    pub grid: Hsla,
}

/// Convert a `#rrggbb` literal to GPUI's colour type.
///
/// Written as a `const fn`-shaped helper over the hex the stylesheet actually
/// contains, so the tokens below read as the CSS does and a transcription error
/// is visible on the line it happens.
fn hex(rgb: u32) -> Hsla {
    let r = ((rgb >> 16) & 0xff) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xff) as f32 / 255.0;
    let b = (rgb & 0xff) as f32 / 255.0;
    rgb_to_hsla(r, g, b, 1.0)
}

fn rgba(rgb: u32, alpha: f32) -> Hsla {
    let r = ((rgb >> 16) & 0xff) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xff) as f32 / 255.0;
    let b = (rgb & 0xff) as f32 / 255.0;
    rgb_to_hsla(r, g, b, alpha)
}

fn rgb_to_hsla(r: f32, g: f32, b: f32, a: f32) -> Hsla {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d.abs() < f32::EPSILON {
        return hsla(0.0, 0.0, l, a);
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < f32::EPSILON {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if (max - g).abs() < f32::EPSILON {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    hsla(h, s, l, a)
}

impl Palette {
    /// `:root` — the light appearance.
    pub fn light() -> Self {
        Self {
            bg: hex(0xffffff),
            fg: hex(0x0a0a0a),
            muted: hex(0x6b6b6b),
            hairline: hex(0x0a0a0a),
            rule: hex(0xe6e6e6),
            surface: hex(0xf4f4f4),
            accent: hex(0xe60023),
            accent_fg: hex(0xffffff),
            grid: rgba(0x000000, 0.07),
        }
    }

    /// `:root[data-theme="dark"]`.
    pub fn dark() -> Self {
        Self {
            bg: hex(0x0a0a0a),
            fg: hex(0xe8e8e8),
            muted: hex(0x888888),
            hairline: hex(0x333333),
            rule: hex(0x262626),
            surface: hex(0x141414),
            accent: hex(0xff3347),
            accent_fg: hex(0xffffff),
            grid: rgba(0xffffff, 0.12),
        }
    }

    pub fn named(name: &str) -> Self {
        // Anything else falls back to the default, so an edited settings file
        // cannot leave the app unreadable.
        if name == "light" {
            Self::light()
        } else {
            Self::dark()
        }
    }
}

/// The type scale, at the clamp ceilings the stylesheet declares.
pub mod type_scale {
    use gpui::{px, Pixels};

    pub const MEGA: Pixels = px(200.0);
    pub const HUGE: Pixels = px(112.0);
    pub const H1: Pixels = px(56.0);
    pub const H2: Pixels = px(28.0);
    pub const H3: Pixels = px(16.0);
    pub const BODY: Pixels = px(16.0);
    pub const SMALL: Pixels = px(13.0);
    pub const TINY: Pixels = px(11.0);
    pub const MICRO: Pixels = px(10.0);
}

/// Rhythm and tracking.
pub mod rhythm {
    /// `--lh-tight`
    pub const LINE_TIGHT: f32 = 1.2;
    /// `--lh-body`
    pub const LINE_BODY: f32 = 1.5;
    /// `--lh-prose`
    pub const LINE_PROSE: f32 = 1.65;
    /// `--ls-caps`, in em. Qt could not express this at all.
    pub const TRACK_CAPS: f32 = 0.08;
    /// `--ls-micro`, in em.
    pub const TRACK_MICRO: f32 = 0.15;
}

/// Durations, in milliseconds.
pub mod motion {
    pub const FAST: u64 = 120;
    pub const SNAPPY: u64 = 150;
    pub const BASE: u64 = 200;
    pub const SLOW: u64 = 300;
    /// One full cycle of the drifting grid backdrop, in seconds.
    pub const GRID_DRIFT_SECONDS: f32 = 24.0;
}

/// Metrics.
pub mod metrics {
    use gpui::{px, Pixels};

    /// `--gap`
    pub const GAP: Pixels = px(24.0);
    /// `--radius: 0`. Square corners are the design, not a default.
    pub const RADIUS: Pixels = px(0.0);
    /// The grid backdrop's tile.
    pub const GRID_TILE: Pixels = px(72.0);
    /// `--nav-offset`
    pub const NAV_HEIGHT: Pixels = px(60.0);
    /// Below this the layout stops being merely tight: the Chat column goes.
    /// It still fits a 1366x768 laptop under its taskbar.
    pub const MIN_WINDOW: (f32, f32) = (900.0, 620.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_appearances_are_genuinely_different() {
        let (l, d) = (Palette::light(), Palette::dark());
        assert_ne!(l.bg, d.bg);
        assert_ne!(l.fg, d.fg);
        assert_ne!(l.accent, d.accent);
    }

    #[test]
    fn a_dark_hairline_is_not_black() {
        // A dark theme with black hairlines shows nothing at all. In light it
        // is pure ink; in dark it is a mid grey.
        let (l, d) = (Palette::light(), Palette::dark());
        assert_eq!(l.hairline, l.fg, "light hairlines are the ink colour");
        assert_ne!(d.hairline, d.bg);
        assert!(
            d.hairline.l > d.bg.l,
            "the dark hairline must be lighter than the page"
        );
        assert!(d.hairline.l < d.fg.l, "and darker than the text");
    }

    #[test]
    fn hex_conversion_round_trips_the_greys() {
        // #ffffff and #0a0a0a are achromatic: saturation must be zero, or the
        // whole palette drifts toward a colour cast.
        let white = hex(0xffffff);
        assert!((white.l - 1.0).abs() < 1e-4, "got {white:?}");
        assert!(white.s.abs() < 1e-4, "white gained saturation: {white:?}");
        let near_black = hex(0x0a0a0a);
        assert!(
            (near_black.l - 10.0 / 255.0).abs() < 1e-3,
            "got {near_black:?}"
        );
        assert!(near_black.s.abs() < 1e-4);
    }

    #[test]
    fn the_accent_really_is_red() {
        let a = Palette::light().accent; // #e60023
                                         // Hue near 0 (or 1) and strongly saturated.
        assert!(a.h < 0.03 || a.h > 0.97, "hue was {}", a.h);
        assert!(a.s > 0.9, "saturation was {}", a.s);
    }

    #[test]
    fn the_grid_backdrop_is_translucent_and_flips_polarity() {
        let (l, d) = (Palette::light(), Palette::dark());
        assert!(l.grid.a < 0.2 && d.grid.a < 0.2);
        // Dark ink on light, light ink on dark.
        assert!(l.grid.l < 0.5);
        assert!(d.grid.l > 0.5);
    }

    #[test]
    fn an_unknown_theme_name_falls_back_rather_than_breaking() {
        assert_eq!(Palette::named("chartreuse"), Palette::dark());
        assert_eq!(Palette::named("light"), Palette::light());
        assert_eq!(Palette::named("dark"), Palette::dark());
    }

    #[test]
    fn corners_are_square_and_the_minimum_window_fits_a_laptop() {
        assert_eq!(metrics::RADIUS, gpui::px(0.0));
        let (w, h) = metrics::MIN_WINDOW;
        assert_eq!((w, h), (900.0, 620.0));
        assert!(h < 768.0, "must fit a 1366x768 laptop under its taskbar");
    }

    #[test]
    fn the_type_scale_descends() {
        use type_scale::*;
        let steps = [MEGA, HUGE, H1, H2, BODY, SMALL, TINY, MICRO];
        for pair in steps.windows(2) {
            assert!(pair[0] >= pair[1], "{:?} then {:?}", pair[0], pair[1]);
        }
    }
}
