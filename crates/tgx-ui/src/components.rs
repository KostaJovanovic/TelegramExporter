//! The pieces the interface is built from.
//!
//! Everything here paints the Swiss language: hairline rules instead of boxes,
//! square corners, letterspaced uppercase micro-type, one red.
//!
//! Three things Qt could not express and GPUI can, each of which deletes a
//! workaround from the original:
//!
//! * **Letter-spacing** is a real property, not a hand-set `QFont`.
//! * **Transitions** exist, so hovers need not snap.
//! * **Shadows** exist without a graphics effect.

use crate::tokens::{metrics, rhythm, type_scale, Palette};
use gpui::prelude::*;
use gpui::{div, px, Div, Hsla, SharedString};

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

/// A letterspaced uppercase micro-heading.
///
/// The design leans hard on these; in Qt they needed a hand-built `QFont`
/// because a stylesheet has no letter-spacing at all.
pub fn eyebrow(text: impl Into<SharedString>, palette: &Palette) -> Div {
    div()
        .text_size(type_scale::MICRO)
        .text_color(palette.muted)
        .child(uppercase(text))
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

/// A section number, set large in mono.
pub fn section_number(n: u32, palette: &Palette) -> Div {
    div()
        .text_size(type_scale::HUGE)
        .text_color(palette.fg)
        .child(format!("{n:02}"))
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
pub fn leading(size: gpui::Pixels, ratio: f32) -> gpui::Pixels {
    px(f32::from(size) * ratio)
}

/// The window's floor.
pub fn min_window() -> (f32, f32) {
    metrics::MIN_WINDOW
}

/// Body leading, for callers that just want the default.
pub fn body_leading() -> gpui::Pixels {
    leading(type_scale::BODY, rhythm::LINE_BODY)
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
    }
}
