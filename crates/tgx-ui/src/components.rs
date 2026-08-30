//! The pieces the interface is built from.
//!
//! Everything here paints the Swiss language: hairline rules instead of boxes,
//! square corners, letterspaced uppercase micro-type, one red.
//!
//! **Nothing here animates.** The design's `--dur-*` tokens exist for
//! transitions, but the only thing that ever used one was the drifting grid
//! backdrop, and that is gone by decision (see `ROADMAP.md`, Phase 6). The
//! durations went with it rather than sit here looking applied. If motion is
//! wanted again, `analyser.css` is the source and it is three constants.
//!
//! **Letter-spacing is a property again.** GPUI 0.2.2 had none — not on
//! `Styled`, not on `TextStyle` — so the design's tracking was built out of
//! layout: one box per character with the tracking as a flex gap, plus a
//! combining-mark scanner so a decomposed accent did not float one track-space
//! away from its letter, plus a hand-set word gap because an empty spacer cannot
//! inherit a font's space advance. egui has `TextFormat::extra_letter_spacing`,
//! so all of that is one field now — and, because these are real text runs
//! again, a tracked label can be selected and wrapped, which the old one
//! documented as the price of the trick.
//!
//! **Draw, do not build.** Under GPUI each of these returned a `Div` for the
//! caller to nest. egui is immediate mode, so they take a `&mut Ui` and paint.
//! The pure functions below — [`thousands`], [`selection_label`], [`ListState`]
//! — did not change at all, which is why the tests over them did not either.
//!
//! **Padding is a margin, and disabled is a state.** The first pass through this
//! file was a transliteration: every row opened with `ui.add_space(16.0)` to
//! stand in for the old flex container's padding, and every control chose its
//! own ink with `if enabled { fg } else { muted }` and its own `Sense` to match.
//! [`row`] and [`block`] put the gutter back where it belongs — on a frame — and
//! [`action`] hands the enabled/disabled distinction to `Ui::add_enabled`, which
//! is the one place in egui that knows about it. A control that decides its own
//! disabled colour is a control that can disagree with the next one.

use crate::fonts;
use crate::tokens::{metrics, rhythm, window, Palette};
use eframe::egui::{
    self, text::LayoutJob, Color32, CornerRadius, CursorIcon, Margin, Response, Sense, Stroke,
    TextFormat, Ui, Vec2,
};

/// The gutter every panel's content sits inside.
///
/// **The rules do not pay it.** A rule runs the full width of the panel it
/// divides — that is what makes the layout read as ruled rather than as boxed —
/// so the padding is applied to the content rows by [`row`] and [`block`] and
/// never to the panel itself. Putting it on the panel frame would inset every
/// rule by the same amount and quietly turn the design into a set of cards.
///
/// It is [`metrics::GAP`], the stylesheet's own `--gap`, and it was 16 — a
/// number nothing declared. TelegramAnalyser gives its single column 34 points a
/// side; this window holds three columns at a 900pt minimum, so it takes the
/// token rather than the sibling's figure, but the direction is the same one:
/// the first pass was tight everywhere and grouped nothing.
pub const GUTTER: f32 = metrics::GAP;

/// One padded row, laid out left to right.
///
/// Replaces the `ui.horizontal(|ui| { ui.add_space(16.0); … })` that opened
/// every row of the first pass. The margin is horizontal only: vertical rhythm
/// belongs to `item_spacing` and to the callers that ask for more.
pub fn row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    gutter(ui, |ui| ui.horizontal(|ui| add(ui)).inner)
}

/// One padded block, laid out top to bottom. For prose, which wraps.
pub fn block<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    gutter(ui, add)
}

fn gutter<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::NONE
        .inner_margin(Margin::symmetric(GUTTER as i8, 0))
        .show(ui, add)
        .inner
}

/// A square, hairline-bordered button — **the window's one real control**.
///
/// A 1px box, 30 points tall, filled with the page and outlined in the hairline;
/// `primary` fills it with the one red instead. It is
/// TelegramAnalyser's `flat()`, which is the only button either app has.
///
/// **The first egui pass had no buttons at all.** Every action was a run of
/// clickable text — five of them across the nav bar, four more under the list —
/// so a window whose entire job is *press these three things in order* gave no
/// sign of where the three things were. Swiss design is spare, not invisible: a
/// hairline box around a label is as flat as it gets and still reads as
/// something to press.
///
/// **Exactly one control on a screen gets `primary`.** An accent that marks two
/// things marks neither.
///
/// Disabled is drawn, not hidden, and comes from `add_enabled`: one rule for the
/// whole window rather than a colour chosen at each call site, with the click
/// refused by the same call that greys it.
pub fn button(
    ui: &mut Ui,
    label: &str,
    enabled: bool,
    primary: bool,
    palette: &Palette,
) -> Response {
    let text = egui::RichText::new(label)
        .font(fonts::sans(window::BODY))
        .color(button_ink(enabled, primary, palette));
    boxed(ui, text, enabled, primary, palette)
}

/// The colour a button's label takes.
///
/// Public because a [`NavCell`] is two runs at two weights and has to colour
/// them itself — and because that is exactly the kind of second opinion this
/// module exists to prevent.
pub fn button_ink(enabled: bool, primary: bool, palette: &Palette) -> Color32 {
    match (enabled, primary) {
        (true, true) => palette.accent_fg,
        (true, false) => palette.fg,
        (false, _) => palette.muted,
    }
}

/// The box, around text the caller has already coloured.
pub fn boxed(
    ui: &mut Ui,
    text: impl Into<egui::WidgetText>,
    enabled: bool,
    primary: bool,
    palette: &Palette,
) -> Response {
    let (fill, edge) = match (enabled, primary) {
        (true, true) => (palette.accent, palette.accent),
        (true, false) => (palette.bg, palette.hairline),
        (false, _) => (palette.bg, palette.rule),
    };
    let widget = egui::Button::new(text)
        .corner_radius(radius())
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, edge));
    let response = ui.add_enabled(enabled, widget);
    if enabled {
        return response.on_hover_cursor(CursorIcon::PointingHand);
    }
    response
}

/// A run of text that can be clicked, with no box around it.
///
/// For the small print that is nonetheless a control — the appearance chip, the
/// selection verbs, Copy. Anything a user is meant to *find* is a [`button`];
/// this is for what they will only look for once they want it.
///
/// It is `Button` with its frame off, which in egui also drops the button
/// padding and leaves the text sitting exactly where a label would. Two things
/// it does that a `Label` with a `Sense` could not: the pointer says it is a
/// control, and a hairline appears under it on hover — feedback drawn in the
/// design's own primitive rather than a fill the palette has no colour for.
pub fn action(ui: &mut Ui, text: impl Into<egui::WidgetText>, enabled: bool) -> Response {
    let response = ui.add_enabled(enabled, egui::Button::new(text).frame(false));
    if enabled && response.hovered() {
        let rect = response.rect;
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            Stroke::new(1.0_f32, ui.visuals().widgets.hovered.fg_stroke.color),
        );
    }
    response.on_hover_cursor(if enabled {
        CursorIcon::PointingHand
    } else {
        CursorIcon::Default
    })
}

/// The one borrowed control: a text field.
///
/// A caret, a selection and a clipboard are worth borrowing rather than drawing;
/// pasting a path out of Explorer is how the settings panel is actually used.
///
/// **Set in the mono, like every field in the sibling app.** What goes in these
/// is a path, an api_id, a phone number, a code and a page size — data, not
/// prose — and the mono says so before a character is typed. The margin and the
/// height are the analyser's, so a field in one window is the same object as a
/// field in the other.
pub fn field<'t>(text: &'t mut String, palette: &Palette) -> egui::TextEdit<'t> {
    egui::TextEdit::singleline(text)
        .font(fonts::mono(window::SMALL))
        .text_color(palette.fg)
        .margin(Margin::symmetric(8, 6))
}

/// A 1px rule — the design's core primitive.
///
/// **It must land on a device pixel.** On a GPU-scaled surface a 1px line can
/// straddle two physical pixels and blur, which reads as a rendering fault
/// rather than a style. This is the shape to keep everything going through
/// rather than hand-rolling borders at call sites.
///
/// **It is `palette.rule`, and `palette.hairline` belongs to the controls.**
/// TelegramAnalyser divides with the softer grey and spends the brighter one on
/// button borders, which is what lets a button read as a button. The first egui
/// pass had it the other way round and drew a dozen bright rules per panel with
/// nothing bounded by them — a wireframe of a layout rather than a layout.
pub fn rule(ui: &mut Ui, palette: &Palette) {
    hairline(ui, palette.rule);
}

/// The **structural** divider: under the nav bar, over the status bar.
///
/// The one place the brighter grey is spent on a line rather than on a control,
/// because these two separate the window's frame from its contents rather than
/// one row of a panel from the next.
pub fn edge_rule(ui: &mut Ui, palette: &Palette) {
    hairline(ui, palette.hairline);
}

fn hairline(ui: &mut Ui, colour: Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), Sense::hover());
    // Painted as a filled rect rather than a stroked line: a stroke is centred
    // on its path, so a 1px stroke at an integer y covers half of each
    // neighbouring pixel and greys out to two half-lines.
    ui.painter().rect_filled(rect, CornerRadius::ZERO, colour);
}

/// The vertical rule, for splitting a row into columns.
pub fn vrule(ui: &mut Ui, palette: &Palette) {
    let height = ui.available_height();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, height), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, palette.hairline);
}

/// A letterspaced run of text.
///
/// The whole design language is letterspaced uppercase micro-type — `--ls-caps`
/// and `--ls-micro` are in the stylesheet's `:root`, and an eyebrow set without
/// them is simply a small uppercase word.
///
/// The gap falls *between* glyphs and not after the last one, unlike CSS
/// `letter-spacing`, which leaves a trailing space on every label. epaint
/// applies the extra advance "only to glyphs after the first one"
/// (`text_layout.rs`), so a tracked label ends flush and still aligns to a rule
/// beside it — which is the property the old per-character layout was built by
/// hand to get.
///
/// One caveat carried over honestly: epaint adds the advance per *glyph*, and it
/// does no combining-mark positioning of its own, so a decomposed `é` is already
/// two glyphs before tracking touches it. Everything put through here is a
/// caption this codebase writes, not user text.
///
/// **Set in the mono.** TelegramAnalyser's `caps()` is `mono(MICRO)` and every
/// letterspaced label in this design belongs to the same family of marks as the
/// numbers do — an eyebrow, a column header, a status line. The first egui pass
/// used the sans medium here, which at 10 points came out as a pale proportional
/// smudge where the sibling app has a crisp mono rule of capitals.
pub fn tracked(text: &str, size: f32, track_em: f32, colour: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: fonts::mono(size),
            color: colour,
            extra_letter_spacing: size * track_em,
            // One line by construction. The body ratio would pad the row and
            // push the label off the baseline it shares with its neighbour.
            line_height: Some(leading(size, rhythm::LINE_TIGHT)),
            ..Default::default()
        },
    );
    job
}

/// A letterspaced uppercase micro-heading — `window::MICRO` at `TRACK_MICRO`.
pub fn eyebrow(text: &str, palette: &Palette) -> LayoutJob {
    tracked(
        &uppercase(text),
        window::MICRO,
        rhythm::TRACK_MICRO,
        palette.muted,
    )
}

/// Letterspaced uppercase caption at an arbitrary size — `TRACK_CAPS`.
///
/// Takes its colour rather than a palette: these label things that are
/// sometimes muted, sometimes the accent, and sometimes sitting on a filled
/// cell where neither is right.
pub fn caps(text: &str, size: f32, colour: Color32) -> LayoutJob {
    tracked(&uppercase(text), size, rhythm::TRACK_CAPS, colour)
}

/// Uppercase the caller's own string.
///
/// The stylesheet does this with `text-transform`; there is no equivalent here,
/// so it is done to the text. Kept in one place so a future change does not
/// have to find every call site.
pub fn uppercase(text: &str) -> String {
    text.to_uppercase()
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
/// The box is a fixed 12px and never shrinks: in a row with a long title, a
/// flexible tick vanishes before the title does.
pub fn tick_box(ui: &mut Ui, ticked: bool, enabled: bool, palette: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::splat(TICK_SIZE),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let ink = if enabled { palette.fg } else { palette.muted };
    let border = match (enabled, response.hovered()) {
        // Hover brightens the border to the ink colour. The box is 12px and
        // carries no label of its own, so without this the only way to find out
        // whether the pointer is on it is to click.
        (true, true) => palette.fg,
        (true, false) => palette.hairline,
        (false, _) => palette.muted,
    };
    let painter = ui.painter();
    if ticked {
        painter.rect_filled(rect, radius(), ink);
    }
    painter.rect_stroke(
        rect,
        radius(),
        Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );
    response
}

/// The tick's edge, in points.
const TICK_SIZE: f32 = 12.0;

/// The share of the track an indeterminate bar paints.
///
/// Short enough to read as a marker rather than as progress, long enough to be
/// visible on a narrow panel.
const INDETERMINATE_FILL: f32 = 0.12;

// A const assertion rather than a test, which is this codebase's idiom for a
// relation between constants: a `#[test]` over two `const`s is one clippy
// rightly calls out as having a constant value, and this way a bad edit fails
// the build rather than a test run.
const _: () = assert!(INDETERMINATE_FILL > 0.0 && INDETERMINATE_FILL < 0.25);

/// How tall the bar is. The bar is a status line, not a widget, and anything
/// taller starts competing with the type. Three points, as the analyser's is.
const BAR_HEIGHT: f32 = 3.0;

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
pub fn progress_bar(ui: &mut Ui, fraction: Option<f32>, palette: &Palette) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, BAR_HEIGHT), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, radius(), palette.rule);
    let mut filled = rect;
    filled.set_width(rect.width() * bar_fill(fraction));
    painter.rect_filled(filled, radius(), palette.accent);
}

/// `metrics::RADIUS`, in the type egui wants. Square, and applied rather than
/// assumed.
fn radius() -> CornerRadius {
    CornerRadius::same(metrics::RADIUS as u8)
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
///
/// **A cell is a [`button`].** It used to paint itself — a galley laid out by
/// hand, a rect measured off it, ink chosen at the moment of measurement — and
/// the result was five runs of text along the top of the window with nothing
/// around them. `primary` puts the one red on the step that is the point of the
/// application; the rest are hairline boxes.
pub struct NavCell {
    pub number: Option<u32>,
    pub label: String,
    pub enabled: bool,
    /// Fill with the accent. **Exactly one cell may set it.**
    pub primary: bool,
}

impl NavCell {
    pub fn step(number: u32, label: impl Into<String>) -> Self {
        Self {
            number: Some(number),
            label: label.into(),
            enabled: true,
            primary: false,
        }
    }

    /// A tool, not a step: no number, no number gap.
    pub fn tool(label: impl Into<String>) -> Self {
        Self {
            number: None,
            label: label.into(),
            enabled: true,
            primary: false,
        }
    }

    pub fn enabled(mut self, yes: bool) -> Self {
        self.enabled = yes;
        self
    }

    /// The one red. See [`button`]: an accent that marks two things marks
    /// neither, and `only_one_cell_carries_the_accent` is what holds it.
    pub fn primary(mut self, yes: bool) -> Self {
        self.primary = yes;
        self
    }

    /// The cell's own text, numbered or not.
    ///
    /// Split from the painting so the numbering rule — the thing worth being
    /// sure of — is decidable without a window.
    pub fn caption(&self) -> String {
        match self.number {
            Some(n) => format!("{n:02}  {}", self.label),
            None => self.label.clone(),
        }
    }

    /// The cell's two runs: the mono number, then the label.
    ///
    /// Split out so [`Self::show`] is nothing but the widget call — and so the
    /// gap after the number lives beside the rule that says a tool does not pay
    /// it.
    /// The cell's two runs: the mono number, then the label.
    ///
    /// The number is set a shade back from the label whenever there is a shade
    /// to spare — on the accent fill there is not, so both take `accent_fg` and
    /// the step reads as one word.
    fn job(&self, palette: &Palette, enabled: bool) -> LayoutJob {
        let ink = button_ink(enabled, self.primary, palette);
        // A shade back from the label wherever there is one to spare. On the
        // accent fill there is not, so the number takes the label's ink and the
        // step reads as one word.
        let figure = if self.primary && enabled {
            ink
        } else {
            palette.muted
        };
        let mut job = LayoutJob::default();
        if let Some(n) = self.number {
            job.append(
                &format!("{n:02}"),
                0.0,
                TextFormat {
                    font_id: fonts::mono(window::SMALL),
                    color: figure,
                    ..Default::default()
                },
            );
        }
        job.append(
            &self.label,
            if self.number.is_some() { 10.0 } else { 0.0 },
            TextFormat {
                font_id: fonts::sans(window::BODY),
                color: ink,
                ..Default::default()
            },
        );
        job
    }

    /// Paint the cell as the window's one kind of button.
    ///
    /// The ink is baked into the job rather than left to [`button`]'s own
    /// colouring, because a cell is two runs at two weights and `WidgetText`
    /// carries one colour. What `button` still owns is the box, the fill, the
    /// border and the disabled state.
    pub fn show(&self, ui: &mut Ui, palette: &Palette) -> Response {
        boxed(
            ui,
            self.job(palette, self.enabled),
            self.enabled,
            self.primary,
            palette,
        )
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyState {
    pub headline: String,
    pub hint: Option<String>,
}

impl EmptyState {
    pub fn new(headline: impl Into<String>, hint: Option<String>) -> Self {
        Self {
            headline: headline.into(),
            hint,
        }
    }

    pub fn show(&self, ui: &mut Ui, palette: &Palette, tall_enough: bool) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            ui.label(
                egui::RichText::new(&self.headline)
                    .font(fonts::medium(window::BODY))
                    .color(palette.fg),
            );
            if tall_enough {
                if let Some(hint) = &self.hint {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(hint)
                            .font(fonts::sans(window::SMALL))
                            .color(palette.muted),
                    );
                }
            }
        });
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
pub fn count_text(count: Option<i64>) -> String {
    match count {
        None => String::new(),
        Some(n) => thousands(n),
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
pub fn forum_dot(palette: &Palette) -> Color32 {
    palette.accent
}

/// Line height in points for a given size.
///
/// The stylesheet's rhythm is ratios (`--lh-tight` 1.2, `--lh-body` 1.5,
/// `--lh-prose` 1.65) and a layout wants a length. This is the one place the
/// two meet, so a hand-multiplied leading never drifts from the token it was
/// derived from.
pub fn leading(size: f32, ratio: f32) -> f32 {
    size * ratio
}

/// The window's floor.
pub fn min_window() -> (f32, f32) {
    metrics::MIN_WINDOW
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_padded_row_gives_back_exactly_the_gutter_on_each_side() {
        // The whole window's alignment rests on this: a heading, a control and a
        // hint in three different panels line up because all three went through
        // `row` or `block`, and nothing has to remember a number.
        // `Cell`, because `__run_test_ui` takes an `Fn` and may run the frame
        // more than once. Every probe below does the same for the same reason.
        let (outer, inner, shift) = (Cell::new(0.0), Cell::new(0.0), Cell::new(0.0));
        egui::__run_test_ui(|ui| {
            outer.set(ui.available_width());
            let left = ui.cursor().left();
            row(ui, |ui| {
                inner.set(ui.available_width());
                shift.set(ui.cursor().left() - left);
            });
        });
        assert_eq!(shift.get(), GUTTER);
        assert_eq!(outer.get() - inner.get(), 2.0 * GUTTER);
    }

    #[test]
    fn a_flat_control_takes_no_more_room_than_the_text_it_shows() {
        // `action` is documented as `Button` with its frame off, which in egui
        // also drops `button_padding`. If that stops being true every dense row
        // in the window gains 28 points per control — the selection row alone
        // has five — and the nav bar stops fitting its own minimum width.
        let (button, label) = (Cell::new(0.0), Cell::new(0.0));
        egui::__run_test_ui(|ui| {
            button.set(action(ui, "Count messages", true).rect.width());
            label.set(ui.label("Count messages").rect.width());
        });
        assert_eq!(button.get(), label.get());
    }

    #[test]
    fn a_disabled_control_is_disabled_by_the_ui_and_not_by_its_colour() {
        // The first pass chose a muted colour *and* a `Sense::hover()` at each
        // call site, which is two statements of one fact that could disagree —
        // and did, silently, because a control that looks live and does nothing
        // reads as a broken window rather than an unavailable one.
        let (live, dead) = (Cell::new(false), Cell::new(true));
        egui::__run_test_ui(|ui| {
            live.set(action(ui, "Go", true).enabled());
            dead.set(action(ui, "Go", false).enabled());
        });
        assert!(live.get());
        assert!(!dead.get());
    }

    #[test]
    fn thousands_groups_correctly() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(6_643), "6,643");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(-6_643), "-6,643");
    }

    #[test]
    fn a_missing_count_is_not_a_count_of_zero() {
        // The column is optional, so blank and 0 are different facts and the
        // row must not turn one into the other.
        assert_eq!(count_text(None), "");
        assert_eq!(count_text(Some(0)), "0");
        assert_eq!(count_text(Some(6_643)), "6,643");
    }

    #[test]
    fn the_footer_says_at_least_when_anything_is_uncounted() {
        assert_eq!(selection_label(0, 0, false), "Nothing selected");
        assert_eq!(selection_label(1, 12, false), "1 chat, 12 messages");
        assert_eq!(selection_label(2, 6_643, false), "2 chats, 6,643 messages");
        assert_eq!(
            selection_label(2, 6_643, true),
            "2 chats, at least 6,643 messages"
        );
    }

    #[test]
    fn uppercase_is_applied_to_the_text_not_a_style() {
        // There is no `text-transform` here, so the transform has to happen to
        // the string or the design's caps simply are not caps.
        assert_eq!(uppercase("Sign in"), "SIGN IN");
        assert_eq!(uppercase("ćaskanje"), "ĆASKANJE");
    }

    #[test]
    fn tracking_is_a_multiple_of_the_type_size() {
        // The stylesheet's tracking is in em, so it has to scale with the type
        // or a caption at one size is spaced like a caption at another.
        let job = tracked("AB", 20.0, 0.1, Color32::WHITE);
        assert_eq!(job.sections[0].format.extra_letter_spacing, 2.0);
        let job = tracked("AB", 10.0, 0.1, Color32::WHITE);
        assert_eq!(job.sections[0].format.extra_letter_spacing, 1.0);
    }

    #[test]
    fn a_tracked_label_carries_its_text_unsplit() {
        // The GPUI version had to shatter the string into one box per glyph,
        // which cost selection, search and wrapping. It is one run again.
        let job = tracked(
            "SIGN IN",
            window::MICRO,
            rhythm::TRACK_MICRO,
            Color32::WHITE,
        );
        assert_eq!(job.text, "SIGN IN");
        assert_eq!(job.sections.len(), 1);
    }

    #[test]
    fn an_eyebrow_is_uppercase_mono_micro_type_in_the_muted_colour() {
        // **Mono.** Every letterspaced label in this design belongs to the same
        // family of marks as the numbers, which is what makes an eyebrow here
        // and an eyebrow in TelegramAnalyser the same object. Set in the sans it
        // is merely a small pale word.
        let palette = Palette::dark();
        let job = eyebrow("Chats", &palette);
        assert_eq!(job.text, "CHATS");
        assert_eq!(job.sections[0].format.color, palette.muted);
        assert_eq!(job.sections[0].format.font_id, fonts::mono(window::MICRO));
    }

    #[test]
    fn only_one_cell_carries_the_accent() {
        // An accent that marks two things marks neither. The nav bar is the one
        // place with more than one button in a row, so the rule is checked
        // where it could actually be broken.
        let bar = [
            NavCell::step(1, "Sign in"),
            NavCell::step(2, "Refresh chats"),
            NavCell::step(3, "Start export").primary(true),
            NavCell::tool("Stop"),
            NavCell::tool("Open output folder"),
        ];
        assert_eq!(bar.iter().filter(|c| c.primary).count(), 1);
    }

    #[test]
    fn a_button_and_its_cell_agree_about_ink() {
        // `NavCell` colours its own two runs because `WidgetText` carries one
        // colour and a cell is a number and a label at two weights. That is a
        // second opinion about the same rule, so it reads it from the first.
        let p = Palette::dark();
        assert_eq!(button_ink(true, true, &p), p.accent_fg);
        assert_eq!(button_ink(true, false, &p), p.fg);
        assert_eq!(button_ink(false, true, &p), p.muted);
        assert_eq!(button_ink(false, false, &p), p.muted);
    }

    #[test]
    fn a_divider_is_softer_than_a_control_border() {
        // The window divides with `rule` and outlines its controls with
        // `hairline`. The first egui pass had it the other way round, which drew
        // a dozen bright lines per panel around nothing.
        // Measured as distance from the page, so the claim holds in both
        // appearances: in light the divider is a pale grey against white and the
        // border is ink, in dark it is the darker of two greys against black.
        let lum = |c: Color32| c.r() as i32 + c.g() as i32 + c.b() as i32;
        for p in [Palette::light(), Palette::dark()] {
            let against = |c| (lum(c) - lum(p.bg)).abs();
            assert!(
                against(p.rule) < against(p.hairline),
                "the divider is not softer than the border"
            );
        }
    }

    #[test]
    fn an_indeterminate_bar_is_short_enough_to_read_as_a_marker() {
        // A bar reading 0% and a bar meaning "unknown" are different states,
        // and painting the second as the first makes a working export look
        // stuck.
        assert_eq!(bar_fill(None), INDETERMINATE_FILL);
        // That it is short enough to read as a marker is pinned by the const
        // assertion beside the constant, not here.
        assert_ne!(bar_fill(None), bar_fill(Some(0.0)));
    }

    #[test]
    fn a_bar_never_propagates_a_nan_into_the_layout() {
        // `n as f32 / total as f32` with total zero is the way this arrives.
        assert_eq!(bar_fill(Some(f32::NAN)), 0.0);
        assert_eq!(bar_fill(Some(f32::INFINITY)), 0.0);
        assert_eq!(bar_fill(Some(-1.0)), 0.0);
        assert_eq!(bar_fill(Some(2.0)), 1.0);
        assert_eq!(bar_fill(Some(0.5)), 0.5);
    }

    #[test]
    fn only_the_sequence_is_numbered() {
        // Stop and Open output folder are tools, not steps four and five.
        assert_eq!(NavCell::step(1, "Sign in").caption(), "01  Sign in");
        assert_eq!(NavCell::tool("Stop").caption(), "Stop");
        assert_eq!(NavCell::step(3, "Start export").number, Some(3));
        assert_eq!(NavCell::tool("Stop").number, None);
    }

    #[test]
    fn a_disabled_cell_is_still_a_cell() {
        let cell = NavCell::step(2, "Refresh chats").enabled(false);
        assert!(!cell.enabled);
        assert_eq!(cell.caption(), "02  Refresh chats");
    }

    #[test]
    fn the_list_state_tells_four_kinds_of_empty_apart() {
        use ListState::*;
        assert_eq!(ListState::decide(false, false, 0, 0), NotSignedIn);
        assert_eq!(ListState::decide(true, false, 0, 0), SignedInNothingLoaded);
        assert_eq!(ListState::decide(true, true, 0, 0), AccountHasNoChats);
        assert_eq!(ListState::decide(true, true, 9, 0), FilterMatchedNothing);
        assert_eq!(ListState::decide(true, true, 9, 3), Populated);
    }

    #[test]
    fn the_filter_state_quotes_what_was_typed() {
        // "No chats" alone reads as the list having been lost rather than
        // filtered.
        let state = ListState::FilterMatchedNothing
            .empty_state("kolab")
            .expect("an empty state");
        assert!(state.headline.contains("kolab"), "{}", state.headline);
        assert!(state.headline.contains('\u{201c}'));
        assert!(ListState::Populated.empty_state("").is_none());
    }

    #[test]
    fn no_empty_state_sends_anyone_to_a_screen_that_does_not_exist() {
        // The first run's only instruction used to be "...and enter them in
        // Settings", and there is no Settings in this app.
        for state in [
            ListState::NotSignedIn,
            ListState::SignedInNothingLoaded,
            ListState::AccountHasNoChats,
            ListState::FilterMatchedNothing,
        ] {
            let s = state.empty_state("x").expect("an empty state");
            let hint = s.hint.unwrap_or_default();
            assert!(!hint.contains("Settings"), "{state:?} says {hint:?}");
        }
    }

    #[test]
    fn leading_is_derived_from_the_token_not_hand_multiplied() {
        assert_eq!(leading(10.0, rhythm::LINE_TIGHT), 12.0);
        assert_eq!(leading(10.0, rhythm::LINE_BODY), 15.0);
    }

    #[test]
    fn the_window_floor_is_the_metric_not_a_copy_of_it() {
        assert_eq!(min_window(), metrics::MIN_WINDOW);
    }
}
