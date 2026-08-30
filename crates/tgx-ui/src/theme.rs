//! Installing the design into an egui context.
//!
//! Swiss/International: hairline rules, **square corners**, one red, and no
//! shadows. egui's defaults are none of those — its stock widget is a rounded,
//! raised button with a shadow — so every one of them is set here rather than
//! inherited. A token that is declared and not applied is a token that looks
//! applied and is not.
//!
//! **This replaced a 242-line translation layer.** Under GPUI the window
//! borrowed `Input` and `Scrollbar` from `gpui-component`, and a borrowed
//! component does not read [`Palette`] — it painted from that library's own
//! theme global. Left alone the window showed two design languages at once: a
//! rounded, blue-ringed, off-white text field inside a square, hairline-ruled,
//! one-red layout. Reconciling them meant assigning roughly forty `ThemeColor`
//! fields by name, in an order that two other library calls could silently
//! undo, against a dependency pinned exactly because a routine `cargo update`
//! could rename any of them. None of that survives the swap: egui has one
//! `Style`, we own all of it, and there is no second vocabulary to translate
//! into.
//!
//! Re-running is free — every write is an assignment — so the settings screen
//! flips appearance by rebuilding the palette and calling this again.

use crate::fonts;
use crate::tokens::{metrics, window, Palette};
use eframe::egui::{self, Context, CornerRadius, Stroke, TextStyle};

/// Everything about the window that is a decision rather than a default.
///
/// Call once at startup, and again whenever the appearance changes.
pub fn install(ctx: &Context, palette: &Palette) {
    ctx.set_fonts(fonts::definitions());

    let mut style = (*ctx.style()).clone();

    // The design's scale, mapped onto the five roles egui lays text out with.
    // `Heading` is the medium weight rather than a larger size: this design
    // separates headings by face and tracking, not by scale.
    //
    // **`tokens::window`, not `tokens::type_scale`.** The second is the
    // stylesheet's, which describes a web page; these five roles are what a
    // widget falls back to when a call site does not name a font, and they have
    // to be the same three sizes the call sites use.
    style.text_styles = [
        (TextStyle::Body, fonts::sans(window::BODY)),
        (TextStyle::Button, fonts::sans(window::BODY)),
        (TextStyle::Small, fonts::sans(window::MICRO)),
        (TextStyle::Monospace, fonts::mono(window::SMALL)),
        (TextStyle::Heading, fonts::medium(window::BODY)),
    ]
    .into();

    let v = &mut style.visuals;
    // **The library branches on this flag**, not on how light the colours it was
    // given happen to be. A dark palette under `dark_mode: false` gets a handful
    // of contrast decisions backwards while every colour looks right, which is
    // the hardest kind of mismatch to see. The same trap the GPUI version had,
    // one field instead of an enum.
    v.dark_mode = !palette.is_light();
    v.panel_fill = palette.bg;
    v.window_fill = palette.bg;
    v.faint_bg_color = palette.surface;
    v.extreme_bg_color = palette.surface;
    v.override_text_color = Some(palette.fg);
    v.hyperlink_color = palette.accent;
    v.selection.bg_fill = palette.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, palette.fg);

    // **Square, flat, hairline.** Each line below takes one default off.
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius();
        w.bg_fill = palette.bg;
        w.weak_bg_fill = palette.bg;
        w.bg_stroke = Stroke::new(1.0_f32, palette.rule);
        w.fg_stroke = Stroke::new(1.0_f32, palette.fg);
        // egui grows a hovered widget by a couple of points by default. In a
        // layout ruled by hairlines that reads as the rule moving.
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, palette.muted);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, palette.hairline);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, palette.accent);
    v.widgets.active.bg_fill = palette.surface;
    v.widgets.active.weak_bg_fill = palette.surface;
    // **The design has no raised fill, so hover has to be the surface tint.**
    // Every control in this window is flat text or a hairline box; without one
    // state that differs, a framed widget — the sort menu is the only one —
    // gives no sign at all that the pointer is on it. `surface` is one step off
    // the page in both appearances, which is as far as this design goes.
    v.widgets.hovered.bg_fill = palette.surface;
    v.widgets.hovered.weak_bg_fill = palette.surface;
    v.widgets.open.bg_fill = palette.surface;
    v.widgets.open.weak_bg_fill = palette.surface;

    v.window_corner_radius = radius();
    v.menu_corner_radius = radius();
    v.window_stroke = Stroke::new(1.0_f32, palette.hairline);
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;

    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size.y = 30.0;

    ctx.set_style(style);
}

/// `metrics::RADIUS` in the type egui wants.
///
/// It is `0.0` and says why: square corners are the design, not a default.
fn radius() -> CornerRadius {
    CornerRadius::same(metrics::RADIUS as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The style a fresh context ends up with under a palette.
    fn styled(palette: &Palette) -> egui::Style {
        let ctx = Context::default();
        install(&ctx, palette);
        (*ctx.style()).clone()
    }

    #[test]
    fn nothing_is_left_rounded() {
        // egui's stock widget is a rounded raised button, and a single missed
        // assignment is visible as one soft corner in a square layout.
        let style = styled(&Palette::dark());
        let v = &style.visuals;
        for w in [
            &v.widgets.noninteractive,
            &v.widgets.inactive,
            &v.widgets.hovered,
            &v.widgets.active,
            &v.widgets.open,
        ] {
            assert_eq!(w.corner_radius, CornerRadius::ZERO);
        }
        assert_eq!(v.window_corner_radius, CornerRadius::ZERO);
        assert_eq!(v.menu_corner_radius, CornerRadius::ZERO);
    }

    #[test]
    fn nothing_casts_a_shadow() {
        let v = styled(&Palette::light()).visuals;
        assert_eq!(v.window_shadow, egui::epaint::Shadow::NONE);
        assert_eq!(v.popup_shadow, egui::epaint::Shadow::NONE);
    }

    #[test]
    fn the_appearance_flag_follows_the_palette_not_the_default() {
        // Getting this backwards leaves every colour correct and a handful of
        // contrast decisions inverted.
        assert!(styled(&Palette::dark()).visuals.dark_mode);
        assert!(!styled(&Palette::light()).visuals.dark_mode);
    }

    #[test]
    fn the_page_is_the_palettes_page_in_both_appearances() {
        for palette in [Palette::light(), Palette::dark()] {
            let v = styled(&palette).visuals;
            assert_eq!(v.panel_fill, palette.bg);
            assert_eq!(v.window_fill, palette.bg);
            assert_eq!(v.override_text_color, Some(palette.fg));
            assert_eq!(v.hyperlink_color, palette.accent);
        }
    }

    #[test]
    fn a_hovered_widget_does_not_grow() {
        // egui expands on hover by default. Under hairline rules that reads as
        // the rule itself moving, which is worse than no hover state at all.
        let v = styled(&Palette::dark()).visuals;
        assert_eq!(v.widgets.hovered.expansion, 0.0);
        assert_eq!(v.widgets.active.expansion, 0.0);
    }

    #[test]
    fn hover_is_visible_without_the_widget_moving() {
        // The two halves of the same rule. A flat design gives no depth cue, so
        // if the hovered fill equals the resting one there is no feedback at all
        // — and if the fix is expansion instead, the hairline beside it appears
        // to shift, which reads as a rendering fault.
        for palette in [Palette::light(), Palette::dark()] {
            let v = styled(&palette).visuals;
            assert_ne!(
                v.widgets.hovered.weak_bg_fill,
                v.widgets.inactive.weak_bg_fill
            );
            assert_eq!(v.widgets.hovered.expansion, v.widgets.inactive.expansion);
        }
    }

    #[test]
    fn every_text_role_is_set_in_geist() {
        // `default_fonts` is off, so a role left pointing at a family we did not
        // register draws nothing at all rather than falling back.
        let style = styled(&Palette::dark());
        let fonts = fonts::definitions();
        for (role, id) in &style.text_styles {
            assert!(
                fonts.families.contains_key(&id.family),
                "{role:?} asks for {:?}, which is not registered",
                id.family
            );
        }
    }

    #[test]
    fn reinstalling_a_different_palette_replaces_the_first() {
        // The settings screen flips appearance by calling `install` again, so
        // nothing here may accumulate.
        let ctx = Context::default();
        install(&ctx, &Palette::dark());
        install(&ctx, &Palette::light());
        assert_eq!(ctx.style().visuals.panel_fill, Palette::light().bg);
        assert!(!ctx.style().visuals.dark_mode);
    }
}
