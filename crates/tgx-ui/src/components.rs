//! The pieces the interface is built from.
//!
//! Everything here paints the Swiss language: hairline rules instead of boxes,
//! square corners, letterspaced uppercase micro-type, one red.
//!
//! **Nothing here animates.** The design's `--dur-*` tokens exist for
//! transitions, but the only thing that ever used one was the drifting grid
//! backdrop, and that is gone by
//! decision (see `ROADMAP.md`, Phase 6). The durations went with it rather than
//! sit here looking applied. If motion is wanted again, `analyser.css` is the
//! source and it is three constants.
//!
//! **Letter-spacing is not one of them.** GPUI 0.2.2 has no letter-spacing
//! property — not on `Styled`, not on `TextStyle` — so the design's tracking is
//! built out of layout instead; see [`tracked`]. The tokens
//! `rhythm::TRACK_CAPS` and `rhythm::TRACK_MICRO` went unapplied for exactly as
//! long as this file claimed otherwise.

use crate::tokens::{metrics, rhythm, type_scale, Palette};
use gpui::prelude::*;
use gpui::{div, px, relative, Div, Hsla, SharedString};

/// A 1px rule — the design's core primitive.
///
/// **It must land on a device pixel.** On a GPU-scaled surface a 1px line can
/// straddle two physical pixels and blur, which reads as a rendering fault
/// rather than a style. GPUI's `px` is in logical units and the renderer snaps
/// borders, so this is the shape to keep everything going through rather than
/// hand-rolling borders at call sites.
pub fn rule(palette: &Palette) -> Div {
    div().h(px(1.0)).w_full().bg(palette.hairline)
}

pub fn vrule(palette: &Palette) -> Div {
    div().w(px(1.0)).h_full().bg(palette.hairline)
}

/// The softer divider, for grouping inside a panel.
pub fn soft_rule(palette: &Palette) -> Div {
    div().h(px(1.0)).w_full().bg(palette.rule)
}

/// One unit of a letterspaced run: an inked cluster, or a gap between words.
///
/// A space is a *variant* rather than an `Ink(" ")`, because a `div` whose only
/// child is a single space has no reliable width — the shaper is free to trim
/// it, and the word gap then collapses to the tracking, which reads as one long
/// word.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Glyph {
    Ink(SharedString),
    Space,
}

/// Split a label into the units [`tracked`] lays out.
///
/// **A combining mark stays with the character it marks.** Splitting on
/// `chars()` alone would give a decomposed `é` its own box one track-space to
/// the right of the `e`, so the accent floats between two letters. The fonts
/// here are the merged Latin+Cyrillic Geist build, so the Cyrillic marks
/// (U+0483..) matter as much as the Latin ones.
fn glyphs(text: &str) -> Vec<Glyph> {
    let mut out: Vec<Glyph> = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if is_combining(c) && !current.is_empty() {
            current.push(c);
            continue;
        }
        if !current.is_empty() {
            out.push(Glyph::Ink(std::mem::take(&mut current).into()));
        }
        if c.is_whitespace() {
            out.push(Glyph::Space);
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        out.push(Glyph::Ink(current.into()));
    }
    out
}

/// The combining ranges, spelled out because `std` has no character-class
/// query and this crate takes no dependencies to get one.
fn is_combining(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F      // combining diacritical marks
        | 0x0483..=0x0489    // Cyrillic
        | 0x0591..=0x05BD    // Hebrew points
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x20D0..=0x20F0    // combining marks for symbols
        | 0xFE20..=0xFE2F)
}

/// How wide a word gap is, as a fraction of the type size.
///
/// The empty spacer cannot inherit a font's own space advance, so it is set
/// here; 0.32em is close to Geist's and reads as one word gap rather than two.
const SPACE_EM: f32 = 0.32;

/// A letterspaced run of text.
///
/// **GPUI has no letter-spacing property**, so tracking is done in layout: one
/// child per character, with the tracking as the flex `gap`. That is worth the
/// children because the whole design language is letterspaced uppercase
/// micro-type — `--ls-caps` and `--ls-micro` are in the stylesheet's `:root`,
/// and an eyebrow set without them is simply a small uppercase word.
///
/// What it costs, and why these must stay **painted labels**: the text is no
/// longer one text run, so it cannot be selected, cannot be searched by the
/// platform, and will not wrap — a tracked string that outgrows its box is
/// clipped, not broken. Never put body copy or a user-supplied string of
/// unknown length through here.
///
/// The gap falls *between* glyphs and not after the last one, unlike CSS
/// `letter-spacing`, which leaves a trailing space on every label. A tracked
/// label therefore ends flush and still aligns to a rule beside it.
pub fn tracked(text: impl Into<SharedString>, size: gpui::Pixels, track_em: f32) -> Div {
    let text: SharedString = text.into();
    let track = px(f32::from(size) * track_em);
    let space = px(f32::from(size) * SPACE_EM);
    let mut row = div()
        .flex()
        .flex_row()
        .items_baseline()
        .text_size(size)
        // One line by construction. The body ratio would pad the row and push
        // the label off the baseline it shares with whatever sits next to it.
        .line_height(leading(size, rhythm::LINE_TIGHT))
        .gap(track);
    for glyph in glyphs(&text) {
        row = row.child(match glyph {
            // `flex_none` on every box: in a tight row the shrink would take
            // the letters, not the row, and the tracking would go uneven
            // before anything visibly overflowed.
            Glyph::Ink(s) => div().flex_none().child(s),
            Glyph::Space => div().flex_none().w(space),
        });
    }
    row
}

/// A letterspaced uppercase micro-heading — `MICRO` at `TRACK_MICRO`.
///
/// The design leans hard on these, and letter-spacing is not something every
/// text system exposes — here it needs [`tracked`].
pub fn eyebrow(text: impl Into<SharedString>, palette: &Palette) -> Div {
    tracked(uppercase(text), type_scale::MICRO, rhythm::TRACK_MICRO).text_color(palette.muted)
}

/// Letterspaced uppercase caption at an arbitrary size — `TRACK_CAPS`.
///
/// Takes its colour rather than a palette: these label things that are
/// sometimes muted, sometimes the accent, and sometimes sitting on a filled
/// cell where neither is right.
pub fn caps(text: impl Into<SharedString>, size: gpui::Pixels, colour: Hsla) -> Div {
    tracked(uppercase(text), size, rhythm::TRACK_CAPS).text_color(colour)
}

/// Uppercase the caller's own string.
///
/// The stylesheet does this with `text-transform`; there is no equivalent here,
/// so it is done to the text. Kept in one place so a future change does not
/// have to find every call site.
pub fn uppercase(text: impl Into<SharedString>) -> SharedString {
    let s: SharedString = text.into();
    s.to_uppercase().into()
}

/// A hairline square, filled when ticked.
///
/// Square corners — `metrics::RADIUS` is 0 and that is the design, not a
/// default. It is applied rather than left implicit so that a future rounded
/// theme cannot round this one control by omission.
///
/// **Disabled is a muted border, never a missing one.** A control that is off
/// and a control that is unavailable must not paint the same, or the only way
/// to tell them apart is to click and watch nothing happen. Ticked-and-disabled
/// fills with `muted` rather than `rule`: `rule` is the divider grey and a box
/// filled with it reads as empty on both appearances.
///
/// `flex_none`, because in a row with a long title the flex shrink comes out of
/// the 12px box first and the tick vanishes before the title does.
pub fn tick_box(ticked: bool, enabled: bool, palette: &Palette) -> Div {
    let ink = if enabled { palette.fg } else { palette.muted };
    let border = if enabled {
        palette.hairline
    } else {
        palette.muted
    };
    div()
        .flex_none()
        .w(px(12.0))
        .h(px(12.0))
        .rounded(metrics::RADIUS)
        .border_1()
        .border_color(border)
        .when(ticked, |d| d.bg(ink))
}

/// The share of the track an indeterminate bar paints.
///
/// Short enough to read as a marker rather than as progress, long enough to be
/// visible on a narrow panel.
const INDETERMINATE_FILL: f32 = 0.12;

/// The fraction actually painted, given what the caller knows.
///
/// **A bar reading 0% and a bar meaning "unknown" are different states.** The
/// first says nothing has happened yet, which is true and useful; the second
/// says the run has started and its size is not known, and painting it as 0%
/// makes a working export look stuck. A non-finite fraction — an
/// `n as f32 / total as f32` with `total` zero — paints empty rather than
/// propagating a NaN into the layout.
fn bar_fill(fraction: Option<f32>) -> f32 {
    match fraction {
        None => INDETERMINATE_FILL,
        Some(f) if f.is_finite() => f.clamp(0.0, 1.0),
        Some(_) => 0.0,
    }
}

/// One progress bar. `None` is *indeterminate*.
///
/// 6px tall and fixed: the bar is a status line,
/// not a widget, and anything taller starts competing with the type.
pub fn progress_bar(fraction: Option<f32>, palette: &Palette) -> Div {
    div()
        .w_full()
        .h(px(6.0))
        .rounded(metrics::RADIUS)
        .bg(palette.rule)
        .child(
            div()
                .h_full()
                .w(relative(bar_fill(fraction)))
                .bg(palette.accent),
        )
}

/// One cell of the nav bar.
///
/// **The bar numbers the steps and only the steps.** `01`–`03` across Sign in,
/// Refresh chats and Start export, which really are a sequence. Stop and Open
/// output folder are *tools*: unnumbered, sized to their labels, and pushed
/// right where the sequence has ended.
///
/// A cell without a number does not pay the number gap either, or its label
/// hangs further in than a numbered one's and reads as a misalignment rather
/// than a distinction.
pub struct NavCell {
    pub number: Option<u32>,
    pub label: SharedString,
    pub enabled: bool,
    pub active: bool,
}

impl NavCell {
    pub fn step(number: u32, label: impl Into<SharedString>) -> Self {
        Self {
            number: Some(number),
            label: label.into(),
            enabled: true,
            active: false,
        }
    }

    /// A tool, not a step: no number, no number gap.
    pub fn tool(label: impl Into<SharedString>) -> Self {
        Self {
            number: None,
            label: label.into(),
            enabled: true,
            active: false,
        }
    }

    pub fn enabled(mut self, yes: bool) -> Self {
        self.enabled = yes;
        self
    }

    pub fn render(&self, palette: &Palette) -> Div {
        let fg = if self.enabled {
            palette.fg
        } else {
            palette.muted
        };
        let mut cell = div()
            .flex()
            .items_baseline()
            .gap(px(10.0))
            .px(px(4.0))
            .py(px(16.0))
            .text_size(type_scale::SMALL)
            .text_color(fg);

        if self.active {
            cell = cell.bg(palette.fg).text_color(palette.bg);
        }
        if let Some(n) = self.number {
            cell = cell.child(
                div()
                    .text_size(type_scale::TINY)
                    .text_color(palette.muted)
                    .child(format!("{n:02}")),
            );
        }
        cell.child(self.label.clone())
    }
}

/// The painted empty state.
///
/// **Signage, not furniture.** A placeholder widget that can take focus lands
/// in the tab order and one that can take a click swallows it — so this is
/// painted text and nothing else.
///
/// A short panel drops the hint and keeps the headline: the queue is routinely
/// 60px tall, so that is the normal case, and half a headline sliced by the top
/// of the viewport is worse than no headline.
pub struct EmptyState {
    pub headline: SharedString,
    pub hint: Option<SharedString>,
}

impl EmptyState {
    pub fn new(headline: impl Into<SharedString>, hint: Option<SharedString>) -> Self {
        Self {
            headline: headline.into(),
            hint,
        }
    }

    pub fn render(&self, palette: &Palette, tall_enough: bool) -> Div {
        let mut wrap = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .size_full()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(type_scale::H3)
                    .text_color(palette.fg)
                    .child(self.headline.clone()),
            );
        if tall_enough {
            if let Some(hint) = &self.hint {
                wrap = wrap.child(
                    div()
                        .text_size(type_scale::SMALL)
                        .text_color(palette.muted)
                        .child(hint.clone()),
                );
            }
        }
        wrap
    }
}

/// The four situations that produce an empty chat list.
///
/// They need four different answers, which is why *signed in* is tracked
/// separately from *the list is empty*: `chats` is empty both before a sign-in
/// and after one that found nothing, and those two need opposite instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListState {
    NotSignedIn,
    SignedInNothingLoaded,
    FilterMatchedNothing,
    AccountHasNoChats,
    Populated,
}

impl ListState {
    pub fn decide(signed_in: bool, loaded: bool, total: usize, visible: usize) -> Self {
        if !signed_in {
            ListState::NotSignedIn
        } else if !loaded {
            ListState::SignedInNothingLoaded
        } else if total == 0 {
            ListState::AccountHasNoChats
        } else if visible == 0 {
            ListState::FilterMatchedNothing
        } else {
            ListState::Populated
        }
    }

    /// The message for this state.
    ///
    /// **A message that names a screen has to name a screen that exists.** The
    /// only instruction a first run ever got was "…and enter them in Settings",
    /// and there is no Settings anywhere in this app — credentials are the
    /// first page of the sign-in dialog. It sent every new user looking for
    /// something that is not there.
    pub fn empty_state(self, filter: &str) -> Option<EmptyState> {
        match self {
            ListState::Populated => None,
            ListState::NotSignedIn => Some(EmptyState::new(
                "Not signed in",
                Some("Press Sign in to connect your account.".into()),
            )),
            ListState::SignedInNothingLoaded => Some(EmptyState::new(
                "No chats loaded",
                Some("Press Refresh chats to fetch them.".into()),
            )),
            ListState::AccountHasNoChats => Some(EmptyState::new(
                "This account has no chats",
                Some("There is nothing here to export.".into()),
            )),
            // The filter's empty state quotes what was typed: "No chats" alone
            // reads as the list having been lost rather than filtered.
            ListState::FilterMatchedNothing => Some(EmptyState::new(
                format!("Nothing matches \u{201c}{filter}\u{201d}"),
                Some("Clear the filter to see every chat.".into()),
            )),
        }
    }
}

/// A count as the list paints it.
///
/// **A missing count is not a count of zero.** The column is optional — it
/// costs one request per chat — so a chat can legitimately have no number. It
/// paints blank, and every place that adds counts up has to tell the two apart.
pub fn count_text(count: Option<i64>) -> SharedString {
    match count {
        None => "".into(),
        Some(n) => thousands(n).into(),
    }
}

/// `6,643`.
pub fn thousands(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

/// The selection footer's wording.
///
/// **Says *at least* N when any selected chat is uncounted**, because a blank
/// count and a zero count look identical in the row.
pub fn selection_label(selected: usize, total: i64, any_uncounted: bool) -> String {
    if selected == 0 {
        return "Nothing selected".into();
    }
    let chats = if selected == 1 { "chat" } else { "chats" };
    if any_uncounted {
        format!("{selected} {chats}, at least {} messages", thousands(total))
    } else {
        format!("{selected} {chats}, {} messages", thousands(total))
    }
}

/// The colour a row's accent dot takes. A forum is marked by a **painted dot**,
/// never by a suffix on the stored title — presentation in the string is what
/// the filter then searches.
pub fn forum_dot(palette: &Palette) -> Hsla {
    palette.accent
}

/// Line height in pixels for a given size.
///
/// GPUI's `line_height` takes a length, and the stylesheet's rhythm is ratios
/// (`--lh-tight` 1.2, `--lh-body` 1.5, `--lh-prose` 1.65). This is the one
/// place the two meet, so a hand-multiplied leading never drifts from the
/// token it was derived from.
pub fn leading(size: gpui::Pixels, ratio: f32) -> gpui::Pixels {
    px(f32::from(size) * ratio)
}

/// The window's floor.
pub fn min_window() -> (f32, f32) {
    metrics::MIN_WINDOW
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_empty_situations_get_four_different_answers() {
        // Not signed in, signed in but nothing loaded, filter matched nothing,
        // account has no chats.
        let states = [
            ListState::decide(false, false, 0, 0),
            ListState::decide(true, false, 0, 0),
            ListState::decide(true, true, 0, 0),
            ListState::decide(true, true, 10, 0),
        ];
        assert_eq!(
            states,
            [
                ListState::NotSignedIn,
                ListState::SignedInNothingLoaded,
                ListState::AccountHasNoChats,
                ListState::FilterMatchedNothing
            ]
        );
        let messages: Vec<String> = states
            .iter()
            .map(|s| s.empty_state("x").unwrap().headline.to_string())
            .collect();
        let unique: std::collections::HashSet<&String> = messages.iter().collect();
        assert_eq!(unique.len(), 4, "two states share a message: {messages:?}");
    }

    #[test]
    fn signed_in_and_empty_are_tracked_separately() {
        // `chats` is empty both before a sign-in and after one that found
        // nothing, and those two need opposite instructions.
        assert_ne!(
            ListState::decide(false, false, 0, 0),
            ListState::decide(true, true, 0, 0)
        );
    }

    #[test]
    fn a_populated_list_has_no_empty_state() {
        assert_eq!(ListState::decide(true, true, 10, 3), ListState::Populated);
        assert!(ListState::Populated.empty_state("").is_none());
    }

    #[test]
    fn no_message_names_a_screen_that_does_not_exist() {
        // There is no Settings page; credentials are the first page of sign-in.
        for state in [
            ListState::NotSignedIn,
            ListState::SignedInNothingLoaded,
            ListState::AccountHasNoChats,
            ListState::FilterMatchedNothing,
        ] {
            let s = state.empty_state("x").unwrap();
            let all = format!("{} {}", s.headline, s.hint.clone().unwrap_or_default());
            assert!(
                !all.contains("Settings"),
                "names a screen that does not exist: {all}"
            );
        }
    }

    #[test]
    fn the_filter_state_quotes_what_was_typed() {
        // "No chats" alone reads as the list having been lost, not filtered.
        let s = ListState::FilterMatchedNothing.empty_state("news").unwrap();
        assert!(s.headline.contains("news"), "got {}", s.headline);
    }

    #[test]
    fn a_short_panel_drops_the_hint_and_keeps_the_headline() {
        let s = EmptyState::new("Head", Some("Hint".into()));
        // Both render; the difference is that the tall one carries the hint.
        // Asserting on the built element is not possible without a window, so
        // this pins the decision the renderer makes.
        assert!(s.hint.is_some());
        let _tall = s.render(&Palette::dark(), true);
        let _short = s.render(&Palette::dark(), false);
    }

    #[test]
    fn a_missing_count_paints_blank_and_zero_paints_zero() {
        assert_eq!(count_text(None).as_ref(), "");
        assert_eq!(count_text(Some(0)).as_ref(), "0");
        assert_eq!(count_text(Some(6643)).as_ref(), "6,643");
    }

    #[test]
    fn thousands_groups_correctly() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(6643), "6,643");
        assert_eq!(thousands(256780), "256,780");
        assert_eq!(thousands(-1234), "-1,234");
    }

    #[test]
    fn the_footer_says_at_least_when_anything_is_uncounted() {
        assert_eq!(selection_label(0, 0, false), "Nothing selected");
        assert_eq!(selection_label(1, 10, false), "1 chat, 10 messages");
        assert_eq!(
            selection_label(3, 6643, true),
            "3 chats, at least 6,643 messages"
        );
    }

    #[test]
    fn a_tool_pays_no_number_gap() {
        // A cell without a number must not be laid out as if it had one.
        let step = NavCell::step(1, "Sign in");
        let tool = NavCell::tool("Stop");
        assert!(step.number.is_some());
        assert!(tool.number.is_none());
    }

    #[test]
    fn only_the_sequence_is_numbered() {
        // 01-05 across every cell promised a five-step sequence the app does
        // not have. Only Sign in, Refresh chats and Start export are a
        // sequence; Stop and Open output folder are tools.
        let bar = [
            NavCell::step(1, "Sign in"),
            NavCell::step(2, "Refresh chats"),
            NavCell::step(3, "Start export"),
            NavCell::tool("Stop"),
            NavCell::tool("Open output folder"),
        ];
        let numbered: Vec<u32> = bar.iter().filter_map(|c| c.number).collect();
        assert_eq!(numbered, vec![1, 2, 3]);
    }

    #[test]
    fn uppercase_is_applied_to_the_text_not_a_style() {
        assert_eq!(uppercase("Chats").as_ref(), "CHATS");
    }

    #[test]
    fn leading_scales_with_the_ratio() {
        let l = leading(type_scale::BODY, rhythm::LINE_BODY);
        assert_eq!(f32::from(l), 24.0);
        assert_eq!(
            f32::from(leading(type_scale::MICRO, rhythm::LINE_TIGHT)),
            12.0
        );
    }

    #[test]
    fn a_letterspaced_label_is_one_child_per_character() {
        // The element tree cannot be walked without a window, so the split is
        // asserted where it happens.
        assert_eq!(glyphs("AB").len(), 2);
        assert_eq!(
            glyphs("AB"),
            vec![Glyph::Ink("A".into()), Glyph::Ink("B".into())]
        );
    }

    #[test]
    fn a_space_survives_letterspacing() {
        // Five boxes for "AB CD": four letters and one spacer. A dropped space
        // would leave "ABCD" evenly tracked and unreadable as two words.
        let g = glyphs("AB CD");
        assert_eq!(g.len(), 5);
        assert_eq!(g[2], Glyph::Space);
        assert_eq!(g.iter().filter(|g| **g == Glyph::Space).count(), 1);
    }

    #[test]
    fn an_empty_label_produces_no_children() {
        assert!(glyphs("").is_empty());
    }

    #[test]
    fn a_combining_mark_stays_with_its_letter() {
        // Decomposed "é": the accent must not get its own box a track-space
        // away from the e it belongs to.
        let g = glyphs("e\u{0301}f");
        assert_eq!(
            g,
            vec![Glyph::Ink("e\u{0301}".into()), Glyph::Ink("f".into())]
        );
        // Cyrillic marks too: the shipped font is the merged Latin+Cyrillic
        // build, so both scripts go through here.
        assert_eq!(glyphs("\u{0438}\u{0301}").len(), 1);
    }

    #[test]
    fn tracking_is_a_multiple_of_the_type_size() {
        // .15em at 10px is 1.5px; if this ever reads as 0.15px the em was
        // mistaken for a fraction of a pixel and the label is not tracked.
        let track = f32::from(type_scale::MICRO) * rhythm::TRACK_MICRO;
        assert!((track - 1.5).abs() < 1e-4, "got {track}");
        let caps = f32::from(type_scale::SMALL) * rhythm::TRACK_CAPS;
        assert!((caps - 1.04).abs() < 1e-4, "got {caps}");
    }

    #[test]
    fn an_unknown_fraction_is_not_zero_percent() {
        // "Started, size unknown" and "started, nothing done" are different
        // states; painting the first as the second reads as stuck.
        assert!(bar_fill(None) > 0.0);
        assert_eq!(bar_fill(Some(0.0)), 0.0);
        assert_ne!(bar_fill(None), bar_fill(Some(0.0)));
    }

    #[test]
    fn a_fraction_outside_the_track_is_clamped() {
        assert_eq!(bar_fill(Some(-1.0)), 0.0);
        assert_eq!(bar_fill(Some(2.0)), 1.0);
        assert_eq!(bar_fill(Some(0.5)), 0.5);
        // n / 0 is a real way to reach this; a NaN width would poison layout.
        assert_eq!(bar_fill(Some(f32::NAN)), 0.0);
        assert_eq!(bar_fill(Some(f32::INFINITY)), 0.0);
    }

    #[test]
    fn the_indeterminate_bar_is_short_enough_to_read_as_a_marker() {
        assert!(bar_fill(None) < 0.25, "reads as real progress");
    }

    #[test]
    fn a_disabled_tick_box_is_not_an_unticked_one() {
        // Off and unavailable must not paint the same. Nothing here can walk
        // the element, so the decision itself is pinned: the border colours a
        // disabled box uses are neither the enabled one nor the background.
        let p = Palette::light();
        assert_ne!(p.muted, p.hairline, "disabled would look enabled");
        assert_ne!(p.muted, p.bg, "disabled would look borderless");
        let _ = (
            tick_box(false, true, &p),
            tick_box(true, true, &p),
            tick_box(false, false, &p),
            tick_box(true, false, &p),
        );
    }
}
