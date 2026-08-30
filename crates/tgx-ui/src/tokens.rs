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
//! The tokens come from the design's stylesheet directly, rather than from any
//! intermediate theme file: letter-spacing, transitions and shadows are all
//! expressible here, so there is nothing to work around and nothing to
//! reinterpret.
//!
//! **Two appearances, not one.** TelegramAnalyser, which shares this palette,
//! is dark-only by decision — "a second is a second design to keep in step".
//! Here there is already a `theme` setting that a user can change and that is
//! written to disk, so both have to exist regardless; what that costs is paid
//! by [`Palette::named`] rather than by a switch at each call site.

use eframe::egui::Color32;

/// One appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The page itself, behind everything.
    pub bg: Color32,
    /// Body text.
    pub fg: Color32,
    /// Secondary text.
    pub muted: Color32,
    /// The 1px rule that does all the dividing. **Pure ink in light, mid grey
    /// in dark** — a dark theme with black hairlines shows nothing at all.
    pub hairline: Color32,
    /// The softer divider.
    pub rule: Color32,
    /// A raised fill.
    pub surface: Color32,
    /// The one red.
    pub accent: Color32,
    /// Text on the accent.
    pub accent_fg: Color32,
}

/// A `#rrggbb` literal, written the way the stylesheet writes it.
///
/// `const fn`, so every palette below is a compile-time constant and a
/// transcription error is visible on the line it happens. Under GPUI this had
/// to convert to HSL first, which is why it could not be `const` and why three
/// of the tests below used to assert things about hue and saturation rather
/// than about the colour anyone typed.
pub const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
    )
}

impl Palette {
    /// `:root` — the light appearance.
    pub const fn light() -> Self {
        Self {
            bg: hex(0xffffff),
            fg: hex(0x0a0a0a),
            muted: hex(0x6b6b6b),
            hairline: hex(0x0a0a0a),
            rule: hex(0xe6e6e6),
            surface: hex(0xf4f4f4),
            accent: hex(0xe60023),
            accent_fg: hex(0xffffff),
        }
    }

    /// `:root[data-theme="dark"]`.
    pub const fn dark() -> Self {
        Self {
            bg: hex(0x0a0a0a),
            fg: hex(0xe8e8e8),
            muted: hex(0x888888),
            hairline: hex(0x333333),
            rule: hex(0x262626),
            surface: hex(0x141414),
            accent: hex(0xff3347),
            accent_fg: hex(0xffffff),
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

    /// Is this the light appearance?
    ///
    /// egui's `Visuals` carries a `dark_mode` flag that some widgets read to
    /// pick their own contrast, so it has to be told which of the two this is
    /// rather than inferring it from the fills.
    pub fn is_light(&self) -> bool {
        *self == Self::light()
    }
}

/// The type scale, at the clamp ceilings the stylesheet declares.
///
/// Points rather than `gpui::Pixels`: egui sizes text in points and scales the
/// whole UI by the display's DPI, so these are the same numbers in a type the
/// toolkit accepts directly.
pub mod type_scale {
    pub const MEGA: f32 = 200.0;
    pub const HUGE: f32 = 112.0;
    pub const H1: f32 = 56.0;
    pub const H2: f32 = 28.0;
    pub const H3: f32 = 16.0;
    pub const BODY: f32 = 16.0;
    pub const SMALL: f32 = 13.0;
    pub const TINY: f32 = 11.0;
    pub const MICRO: f32 = 10.0;
}

/// The **window's** scale. Three roles, and the names are the roles.
///
/// [`type_scale`] above is the stylesheet's, and a stylesheet describes a web
/// page whose body is 16px on a viewport a metre wide. A window is read closer
/// and holds four panels at once, so it takes its own sizes.
///
/// **Named for what they are for, not for how big they are**, and that is the
/// point rather than a nicety. Two earlier passes ended with 14, 13 and 11 all
/// in play — because a call site that wanted something a shade quieter reached
/// for `SMALL`, and one that wanted something quieter still reached for
/// `MICRO`. A point of difference is not a hierarchy; it is noise that reads as
/// carelessness. There is no size here for "slightly less than the last one",
/// so the question cannot be asked.
///
/// The steps are wide on purpose: 20 / 14 / 11 is 1.4× and 1.3×, either of which
/// is visible across a room.
pub mod window {
    /// The one large size, for an empty state's headline. Nothing else.
    ///
    /// A window with four panels has no room for a second display size, and a
    /// heading that has to be large to be found is a heading in the wrong place.
    pub const DISPLAY: f32 = 20.0;

    /// **Anything that is text**: a chat title, a setting, a log line, a button,
    /// a sentence in a dialog. If a person reads it rather than scans it, it is
    /// this size, in the sans.
    pub const READING: f32 = 14.0;

    /// **Anything that names something rather than being it**: an eyebrow, a
    /// column header, a row's caption, a count, the status line.
    ///
    /// Always in the mono — uppercase and tracked when it is a label, plain when
    /// it is a figure or small print. See `components::caps`, `figure`, `meta`.
    pub const LABEL: f32 = 11.0;
}

/// The vertical rhythm. **Three steps, all multiples of eight.**
///
/// Earlier passes spaced by eye — 6, 8, 10, 12, 14, 18 and 22 all appeared, and
/// several of them within one panel. The result is that nothing groups: a gap
/// that means "these two belong together" and a gap that means "a new section
/// starts here" were two points apart and read as the same gap.
pub mod space {
    /// Inside one thing: a label and the control it names.
    pub const TIGHT: f32 = 8.0;
    /// Between things of the same kind: one settings row and the next.
    pub const STEP: f32 = 16.0;
    /// Between groups: one settings section and the next, a panel's title and
    /// its body. **This is what replaced most of the rules.**
    pub const BREAK: f32 = 32.0;
}

/// Rhythm and tracking.
pub mod rhythm {
    /// `--lh-tight`
    pub const LINE_TIGHT: f32 = 1.2;
    /// `--lh-body`
    pub const LINE_BODY: f32 = 1.5;
    /// `--lh-prose`
    pub const LINE_PROSE: f32 = 1.65;
    /// `--ls-caps`, in em.
    pub const TRACK_CAPS: f32 = 0.08;
    /// `--ls-micro`, in em.
    pub const TRACK_MICRO: f32 = 0.15;
}

// The stylesheet's `--dur-fast .12s` through `--dur-slow .3s` are **not** here.
// Nothing in this app moves: the one thing that ever did was the drifting grid
// backdrop, removed by decision (see `ROADMAP.md`, Phase 6), and a duration
// table with no caller is a token that looks applied and is not — which is
// exactly what `rhythm::TRACK_*` was for as long as nothing letterspaced.
// `analyser.css` is the source if motion is wanted back.

/// Metrics.
pub mod metrics {
    /// `--gap`
    pub const GAP: f32 = 24.0;
    /// `--radius: 0`. Square corners are the design, not a default — and
    /// egui's default widget is a rounded raised button with a shadow, so this
    /// is a value that has to be applied rather than merely declared. See
    /// `theme::install`.
    pub const RADIUS: f32 = 0.0;
    /// `--nav-offset`
    pub const NAV_HEIGHT: f32 = 60.0;
    /// Below this the layout stops being merely tight: the Chat column goes.
    /// It still fits a 1366x768 laptop under its taskbar.
    pub const MIN_WINDOW: (f32, f32) = (900.0, 620.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sum of a colour's channels, which is enough to order greys.
    fn lum(c: Color32) -> u32 {
        c.r() as u32 + c.g() as u32 + c.b() as u32
    }

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
        // is pure ink; in dark it is a mid grey, lighter than the page and
        // darker than the text.
        let (l, d) = (Palette::light(), Palette::dark());
        assert_eq!(l.hairline, l.fg, "light hairlines are the ink colour");
        assert_ne!(d.hairline, d.bg);
        assert!(lum(d.hairline) > lum(d.bg));
        assert!(lum(d.hairline) < lum(d.fg));
        assert!(
            lum(d.rule) < lum(d.hairline),
            "the softer divider is softer"
        );
    }

    #[test]
    fn hex_reads_the_literal_the_stylesheet_contains() {
        // The whole point of writing them as `0xrrggbb`: what comes out is what
        // was typed. Under GPUI this had to round-trip through HSL, so the test
        // could only check that white stayed unsaturated.
        assert_eq!(hex(0xffffff), Color32::from_rgb(255, 255, 255));
        assert_eq!(hex(0x0a0a0a), Color32::from_rgb(10, 10, 10));
        assert_eq!(hex(0xe60023), Color32::from_rgb(0xe6, 0x00, 0x23));
    }

    #[test]
    fn the_accent_really_is_red() {
        for a in [Palette::light().accent, Palette::dark().accent] {
            assert!(a.r() > a.g() && a.r() > a.b(), "{a:?}");
        }
    }

    #[test]
    fn an_unknown_theme_name_falls_back_rather_than_breaking() {
        assert_eq!(Palette::named("chartreuse"), Palette::dark());
        assert_eq!(Palette::named("light"), Palette::light());
        assert_eq!(Palette::named("dark"), Palette::dark());
    }

    #[test]
    fn each_appearance_knows_which_one_it_is() {
        // egui's `Visuals::dark_mode` is read by widgets that pick their own
        // contrast, so getting this backwards is not cosmetic.
        assert!(Palette::light().is_light());
        assert!(!Palette::dark().is_light());
    }

    #[test]
    fn corners_are_square_and_the_minimum_window_fits_a_laptop() {
        assert_eq!(metrics::RADIUS, 0.0);
        let (w, h) = metrics::MIN_WINDOW;
        assert_eq!((w, h), (900.0, 620.0));
        assert!(h < 768.0, "must fit a 1366x768 laptop under its taskbar");
    }

    #[test]
    fn no_two_window_sizes_are_close_enough_to_be_mistaken() {
        // The failure this scale exists to prevent: a call site that wants
        // something a shade quieter picks the next size down, and the window
        // ends up carrying three sizes a point apart, which reads as
        // carelessness rather than as hierarchy. A quarter is the floor for a
        // step anyone can see.
        let steps = [window::DISPLAY, window::READING, window::LABEL];
        for pair in steps.windows(2) {
            assert!(
                pair[0] >= pair[1] * 1.25,
                "{} and {} are too close to tell apart",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn the_rhythm_is_one_grid_and_every_step_is_a_multiple_of_it() {
        // Spacing by eye is what produced 6, 8, 10, 12, 14, 18 and 22 in one
        // window. On a grid, a gap that means "these belong together" and a gap
        // that means "a new section starts" cannot come out two points apart.
        for step in [space::TIGHT, space::STEP, space::BREAK] {
            assert_eq!(step % space::TIGHT, 0.0, "{step} is off the grid");
        }
    }

    // That the three ascend is a relation between constants, so it is a const
    // assertion rather than a test — clippy rightly calls out an `assert!` over
    // two `const`s inside a `#[test]`, and this way a bad edit fails the build.
    const _: () = assert!(space::TIGHT < space::STEP && space::STEP < space::BREAK);

    #[test]
    fn the_type_scale_descends() {
        use type_scale::*;
        let steps = [MEGA, HUGE, H1, H2, BODY, SMALL, TINY, MICRO];
        for pair in steps.windows(2) {
            assert!(pair[0] >= pair[1], "{:?} then {:?}", pair[0], pair[1]);
        }
    }
}
