//! Making `gpui-component`'s borrowed machinery paint in our language.
//!
//! We borrow `Input` (and `Scrollbar`) from `gpui-component` for their state
//! machinery — cursor movement, selection, IME, drag — and hand-build every
//! other piece of chrome so the design stays ours. But a borrowed component
//! does not read [`tgx_ui::tokens::Palette`]: it paints from
//! `gpui_component::theme::Theme`, a gpui `Global`. Left alone, that global
//! holds the library's own colours, and **the window then shows two design
//! languages at once** — a rounded, blue-ringed, off-white text field sitting
//! inside a square, hairline-ruled, one-red layout.
//!
//! [`apply`] is the one place that reconciles them. It is a translation, not a
//! theme: nothing here invents a colour the palette does not have, except where
//! the library asks for a state (hover, press) that our nine tokens do not name,
//! and then the derivation is spelled out on the line it happens.
//!
//! **Ordering matters twice.**
//!
//! - Call this *after* `gpui_component::init(cx)`. That is what installs the
//!   global; `Theme::global_mut` panics without it. A panic here is reported by
//!   the startup hook in `main.rs` and names its cause, whereas a silent no-op
//!   would ship the library's default chrome and look like a design mistake
//!   rather than a wiring one.
//! - Call it *after* anything that calls `Theme::change` or
//!   `Theme::sync_system_appearance`. Both reapply the library's `ThemeConfig`
//!   over the whole colour set, which would undo every line below. That is also
//!   why the appearance is set by assigning `mode` directly instead of going
//!   through `Theme::change`, which would helpfully repaint us back to default.
//!
//! Re-running is free: every write is an assignment, so the settings screen can
//! flip appearance by rebuilding the palette and calling this again.

use gpui::{black, transparent_black, App};
use gpui_component::theme::{Theme, ThemeMode};
use tgx_ui::tokens::{metrics, type_scale, Palette};

/// Repaint the borrowed components in the palette's colours.
///
/// See the module comment for the ordering this depends on.
pub fn apply(palette: &Palette, cx: &mut App) {
    let dark = is_dark(palette);
    let theme = Theme::global_mut(cx);

    // **The library branches on `mode`**, not on how light the colours it was
    // given happen to be — icon polarity and a handful of `is_dark()` calls
    // inside the components. A dark palette under `ThemeMode::Light` gets those
    // decisions backwards while every colour looks right, which is the hardest
    // kind of mismatch to see.
    theme.mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };

    // `metrics::RADIUS` is `px(0.0)` and says why: square corners are the
    // design, not a default. The library ships 6px and 8px, and a rounded input
    // is the single most visible way the two languages disagree. `radius_lg`
    // takes the same value because a design with no radius has no large one
    // either. This also reaches the scrollbar, which squares its own thumb when
    // it finds `radius.is_zero()`, and the switch, which draws a pill only when
    // the radius is 4px or more.
    theme.radius = metrics::RADIUS;
    theme.radius_lg = metrics::RADIUS;

    // The borrowed input sizes its text from here. It happens to equal the
    // library's default today; binding it to the token means a change to the
    // type scale carries into the input instead of leaving it one step off.
    theme.font_size = type_scale::BODY;

    // **This field is the whole window's typeface, not just the borrowed
    // components'**: `Root::render` puts it on the div that wraps our view, so
    // everything inherits from here. It was left alone for as long as
    // `tgx-ui` registered no fonts — overriding it then would only have named a
    // family DirectWrite does not have, which resolves silently back to the
    // system face. `tgx_ui::fonts::register` now puts Geist in the text system
    // under exactly this name, so leaving the default `.SystemUIFont` here is
    // what would be wrong: the tokens, the type scale and the tracking would
    // all still be applied, and all of them to Segoe UI.
    //
    // Set the family, not a `Font`: the library assigns this to
    // `Styled::font_family`, which cannot carry OpenType features. `main.rs`
    // adds those a level down.
    theme.font_family = tgx_ui::fonts::SANS.into();

    // The page, and the ink on it. `background` is also the input's own fill,
    // so a field reads as part of the page rather than as a tray sunk into it.
    theme.background = palette.bg;
    theme.foreground = palette.fg;

    // **Borders: `hairline`, not `rule`.** These two are not interchangeable —
    // `hairline` is pure ink in light and a mid grey in dark, and it is what
    // `components::rule()` paints, the structural 1px line the whole layout is
    // divided by. `rule` is the softer divider used *inside* a panel to group
    // things. Every border the library names here is structural: the edge of a
    // popover, the box around a text field. Giving them the soft grey would
    // leave the borrowed pieces looking faded next to our own lines.
    theme.border = palette.hairline;
    theme.input = palette.hairline;
    theme.title_bar = palette.bg;
    theme.title_bar_border = palette.hairline;

    // **The one red says "here".** The focus ring and the caret are the same
    // statement — where you are — so they are the same colour, and it is the
    // only colour in the palette that has no other job. Selection is that red
    // held back to a wash: the library clamps its own selection alpha to 0.3
    // when loading a config, for the good reason that an opaque fill under
    // black text is a smear rather than a highlight.
    theme.ring = palette.accent;
    theme.caret = palette.accent;
    theme.selection = palette.accent.alpha(0.25);

    // Secondary text, and the greyed backgrounds that go under it. `muted` is a
    // *background* in this library — the skeleton, the switch track, and the
    // fill of a disabled input — so it takes `surface`, the raised fill, while
    // `muted_foreground` takes our actual secondary text grey. Swapping the two
    // paints placeholder text in a near-page grey and the disabled field in the
    // text colour, which is not subtle and is easy to write.
    theme.muted = palette.surface;
    theme.muted_foreground = palette.muted;

    // A popover is a raised sheet, which is exactly what `surface` names. It
    // needs the lift: `popover_style` in the library hard-codes `shadow_lg`, but
    // with square corners and a hairline border a sheet painted in `bg` would be
    // told from the page only by that one line.
    theme.popover = palette.surface;
    theme.popover_foreground = palette.fg;

    // **`accent` here is not our accent.** The library uses this field for the
    // hover *fill* on menu and list items, with `accent_foreground` as the text
    // on top. Handing it the red would spend the design's one emphasis colour on
    // pointer movement, and then the focus ring would stop meaning anything.
    theme.accent = palette.surface;
    theme.accent_foreground = palette.fg;

    // The design's emphasis is an inversion — ink fill, page-coloured text —
    // exactly as `NavCell` paints its active tab. The red is deliberately not
    // spent here for the reason above; instead the fill recedes toward the page
    // as the pointer engages, which is legible in both appearances because it
    // works by alpha rather than by lightness. (A press cannot be shown by
    // going darker: in light the fill is already at the ink end of the scale.)
    theme.primary = palette.fg;
    theme.primary_foreground = palette.bg;
    theme.primary_hover = palette.fg.opacity(0.85);
    theme.primary_active = palette.fg.opacity(0.72);

    // The quiet counterpart, on the ladder the palette already provides:
    // `bg` -> `surface` -> `rule` moves away from the page in *both*
    // appearances. There is no fourth rung — in light the next step, `hairline`,
    // is pure ink — so press and hover share a value rather than inventing a
    // colour the design does not own. No hand-built control uses these; they
    // only ever reach a borrowed one.
    theme.secondary = palette.surface;
    theme.secondary_foreground = palette.fg;
    theme.secondary_hover = palette.rule;
    theme.secondary_active = palette.rule;

    // **Our only red is the accent**, so danger is that red. There is no second
    // one to distinguish "destructive" from "focused", and inventing a warmer or
    // darker red would put two reds in a design whose whole claim is that it has
    // one. The states step by alpha, as `primary` does.
    theme.danger = palette.accent;
    theme.danger_foreground = palette.accent_fg;
    theme.danger_hover = palette.accent.opacity(0.85);
    theme.danger_active = palette.accent.opacity(0.72);

    // Lists are the page, not cards on it: no container fill, no zebra stripe —
    // striping is a table convention this design divides with rules instead, and
    // leaving `list_even` at the library's grey would band every long chat list.
    // Hover raises a row to `surface`; selection goes one rung further and adds
    // the accent border, because `ListItem` does **not** flip its text colour
    // when selected, so filling the row with ink would hide the label it is
    // meant to be highlighting.
    theme.list = palette.bg;
    theme.list_even = palette.bg;
    theme.list_head = palette.bg;
    theme.list_hover = palette.surface;
    theme.list_active = palette.rule;
    theme.list_active_border = palette.accent;

    // No track. A permanent gutter beside every scrollable column is a box, and
    // this design draws lines rather than boxes; the library's own default is
    // transparent for the same reason. The thumb is the secondary grey, held
    // back while idle so it sits under the text it scrolls past, and brought to
    // full strength under the pointer. The fade-out multiplies this alpha, so a
    // value below 1.0 stays well-behaved.
    theme.scrollbar = transparent_black();
    theme.scrollbar_thumb = palette.muted.alpha(0.6);
    theme.scrollbar_thumb_hover = palette.muted;

    // The export is the one thing the window is *doing*, and that is what the
    // accent is for.
    theme.progress_bar = palette.accent;

    // A modal scrim dims what is behind it, so it is ink in both appearances —
    // `fg` would paint a near-white veil over the dark theme. Dark needs the
    // heavier value because there is less light to take away.
    theme.overlay = black().alpha(if dark { 0.6 } else { 0.35 });

    // The switch reads its "on" fill from `primary` (ink) and its thumb from
    // `switch_thumb`, which therefore has to contrast with both the ink track
    // and the off track. The page colour is the only value that does that in
    // both appearances.
    theme.switch = palette.rule;
    theme.switch_thumb = palette.bg;
}

/// Which appearance this palette is.
///
/// [`Palette`] carries no field naming its own appearance, and `tgx-ui` is not
/// ours to change from here, so the honest test is equality against the two
/// palettes that exist — the struct derives `PartialEq` — rather than a
/// lightness threshold, which would guess wrong the moment a token moves.
///
/// Light is the case we can positively identify. `Palette::dark()`, and any
/// palette matching neither (one assembled by hand, or a third appearance added
/// later), fall to dark: the same fallback `Palette::named` takes, and for the
/// same reason. A wrong guess costs a mismatched icon polarity; the alternative
/// risks a window nobody can read.
fn is_dark(palette: &Palette) -> bool {
    *palette != Palette::light()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_appearance_is_recognised_as_itself() {
        assert!(!is_dark(&Palette::light()));
        assert!(is_dark(&Palette::dark()));
    }

    #[test]
    fn a_palette_matching_neither_appearance_falls_back_to_dark() {
        // A light palette with one token changed is no longer `Palette::light()`
        // and must not be mistaken for it by a lightness heuristic either — the
        // fallback is dark, because an unreadable window is the worse failure.
        let mut odd = Palette::light();
        odd.accent = Palette::dark().accent;
        assert!(is_dark(&odd));
    }
}
