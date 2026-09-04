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
//!
//! # The system
//!
//! Two passes at this window failed the same way: there were rules for colour
//! and type written in the docs, and thirty call sites each free to ignore them.
//! What is below is those rules made into functions, so that following them is
//! the path of least effort and departing from one is visible in a diff.
//!
//! **Type is three roles** — see `tokens::window`. Anything read is
//! [`text`]; anything that names rather than is, is [`caps`], [`eyebrow`],
//! [`meta`] or [`figure`]. Nothing takes a size, because a call site free to
//! answer "how big?" answers it differently from the one beside it.
//!
//! **Colour is one sentence per token, and this is the sentence:**
//!
//! - `fg` — the subject. A chat's title, a setting, a button's label, a panel's
//!   own [`title`], a log line that is a warning.
//! - `muted` — everything that is *about* the subject: captions, eyebrows,
//!   counts, hints, ordinary log lines, the status bar.
//! - `rule` — every divider. [`rule`].
//! - `hairline` — control borders only: [`button`], [`tick_box`], a field.
//!   Never a divider. [`edge_rule`] is the one exception and there are two of
//!   them, where the window's frame meets its contents.
//! - `surface` — a row that is selected or under the pointer. Nothing else.
//! - `accent` — **this run**: Start export, the progress fill, the queue row
//!   that is exporting, and the count of warnings it produced. A log warning
//!   takes `fg` instead, because in a transcript set in `muted` the ink *is* the
//!   loud one, and an accent spent in four places marks nothing.
//!
//! **Rules divide kinds, space divides groups.** A hairline goes where two
//! different sorts of thing meet — the nav bar and the body, a panel's title and
//! its contents. Between one settings section and the next there is
//! `space::BREAK` and no line. The window had a dozen rules per screen and
//! grouped nothing, which is a wireframe rather than a layout.

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
        .font(fonts::sans(window::READING))
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
    // **A fill as well as a border**, and this is what finally made these read
    // as controls. A hairline is `#333` against a `#0a0a0a` page: correct by the
    // token table, and at arm's length invisible — the nav bar looked like five
    // words floating along the top of the window rather than five buttons. One
    // step off the page is enough to give a button an edge without giving it
    // depth, which is as far as this design goes.
    //
    // A disabled button keeps the fill and loses the border, so it is still
    // plainly a control and plainly not available.
    let (fill, edge) = match (enabled, primary) {
        (true, true) => (palette.accent, palette.accent),
        (true, false) => (palette.surface, palette.hairline),
        (false, _) => (palette.surface, palette.rule),
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
        .font(fonts::mono(window::LABEL))
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

/// A letterspaced uppercase label — the design's one kind of heading.
///
/// **The size is not a parameter.** It was, and that is how three sizes ended up
/// in one window: every call site could answer "how big?" for itself, and they
/// answered differently. A label is `window::LABEL`; if something needs to
/// outrank a label, it does it with ink, space or caps, not with points.
///
/// The colour stays a parameter, because a label is sometimes ink (a panel's own
/// title), sometimes muted (a section, a column header), and sometimes the
/// accent (the warning count). Those are three different meanings, not three
/// sizes. See the module docs for which is which.
pub fn caps(text: &str, colour: Color32) -> LayoutJob {
    tracked(&uppercase(text), window::LABEL, rhythm::TRACK_CAPS, colour)
}

/// A muted letterspaced label — a section heading, a column header, a caption.
pub fn eyebrow(text: &str, palette: &Palette) -> LayoutJob {
    tracked(
        &uppercase(text),
        window::LABEL,
        rhythm::TRACK_MICRO,
        palette.muted,
    )
}

/// A panel's own title: `CHATS`, `SETTINGS`, `QUEUE`, `LOG`.
///
/// **Ink, where a section heading is muted.** There are four of these in the
/// window and they name its four regions, so they outrank everything inside one
/// — which they do by being the only labels set in the text colour, not by being
/// larger.
pub fn title(text: &str, palette: &Palette) -> LayoutJob {
    tracked(
        &uppercase(text),
        window::LABEL,
        rhythm::TRACK_MICRO,
        palette.fg,
    )
}

/// **Text a person reads**: a chat title, a setting, a log line, a sentence.
///
/// These three helpers exist so that the colour rule is applied rather than
/// merely written down. Thirty call sites each assembling
/// `RichText::new(..).font(..).color(..)` is thirty chances to pick the wrong
/// one of two greys, and no way to check.
pub fn text(s: impl Into<String>, palette: &Palette) -> egui::RichText {
    egui::RichText::new(s)
        .font(fonts::sans(window::READING))
        .color(palette.fg)
}

/// **Small print a person reads**: the sentence under a control saying what it
/// costs. Sans, because a wrapped paragraph in the mono is hard work.
pub fn meta(s: impl Into<String>, palette: &Palette) -> egui::RichText {
    egui::RichText::new(s)
        .font(fonts::sans(window::LABEL))
        .color(palette.muted)
}

/// **A number.** Always the mono, always muted, always [`window::LABEL`].
///
/// Every figure in this window is metadata beside something else — a row's
/// message count, a queue cell, a percentage. A monospaced face is tabular by
/// construction, so a column of these stays a column while it ticks upward.
pub fn figure(s: impl Into<String>, palette: &Palette) -> egui::RichText {
    egui::RichText::new(s)
        .font(fonts::mono(window::LABEL))
        .color(palette.muted)
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
/// **`#[must_use]` because throwing this away is a control that does nothing.**
///
/// It senses clicks, so inside a row that is *itself* click-sensing the box
/// sits on top and swallows the click — egui gives one to the topmost widget
/// that wants it. Discarding the response then means the pointer lands on the
/// box, the row never hears it, and the one part of the row that looks most
/// like the control is the one part that does not work. That shipped in the
/// chat list. Use [`tick_mark`] inside such a row.
#[must_use = "a tick box whose response is dropped is a control that does nothing; \
              inside a click-sensing row use tick_mark"]
pub fn tick_box(ui: &mut Ui, ticked: bool, enabled: bool, palette: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::splat(TICK_SIZE),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    // Hover brightens the border to the ink colour. The box is 12px and carries
    // no label of its own, so without this the only way to find out whether the
    // pointer is on it is to click.
    paint_tick(ui, rect, ticked, enabled, response.hovered(), palette);
    response
}

/// The same box, painted with **no interaction of its own**.
///
/// For a row that is the control — a chat row is 46 points of hit target and
/// the box is a readout of what it says, not a second place to click. Sensing
/// nothing is what lets the click reach the row underneath.
pub fn tick_mark(ui: &mut Ui, ticked: bool, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(TICK_SIZE), Sense::hover());
    // `hovered: false` — the row paints its own hover, and a box that lit up
    // independently would suggest it was separately clickable, which is the
    // impression this whole change exists to remove.
    paint_tick(ui, rect, ticked, true, false, palette);
}

/// Shared, so an interactive tick and an inert one cannot drift apart.
fn paint_tick(ui: &Ui, rect: egui::Rect, ticked: bool, enabled: bool, hovered: bool, p: &Palette) {
    let ink = if enabled { p.fg } else { p.muted };
    let border = match (enabled, hovered) {
        (true, true) => p.fg,
        (true, false) => p.hairline,
        (false, _) => p.muted,
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
}

/// An exclusive choice, drawn as a strip of joined hairline cells.
///
/// **This is what "either / or" looks like here.** Two tick boxes that untick
/// each other say the same thing, and that is what the Format section used to
/// do — but a tick box promises independence, so a pair that does not behave
/// independently is a control lying about its own kind. There is no radio in
/// this design and adding one for two options would be a component built for a
/// single call site; a strip where exactly one cell is filled is the same
/// statement in primitives the design already has.
///
/// **Never `primary`.** The one red belongs to the run — see [`button`] — and a
/// mode switch is not one.
///
/// Returns the index pressed, and **only when it is not already selected**:
/// pressing the mode you are in is not a change, and reporting it as one writes
/// `settings.json` on every stray click.
pub fn segmented(
    ui: &mut Ui,
    options: &[&str],
    selected: usize,
    palette: &Palette,
) -> Option<usize> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        // Joined, not spaced. A gap between two cells makes them two controls.
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, label) in options.iter().enumerate() {
            let on = i == selected;
            let caption = egui::RichText::new(*label)
                .font(fonts::sans(window::READING))
                .color(if on { palette.fg } else { palette.muted });
            // `surface` for the chosen cell, which is the one thing that colour
            // means: a row that is selected. See the module docs.
            let (fill, edge) = if on {
                (palette.surface, palette.hairline)
            } else {
                (palette.bg, palette.rule)
            };
            let cell = egui::Button::new(caption)
                .corner_radius(radius())
                .fill(fill)
                .stroke(Stroke::new(1.0_f32, edge));
            let hit = ui.add(cell).on_hover_cursor(CursorIcon::PointingHand);
            if hit.clicked() && !on {
                chosen = Some(i);
            }
        }
    });
    chosen
}

/// The `[?]` a setting hangs its explanation on.
///
/// **Literally three characters** — bracket, question mark, bracket — in the
/// mono, muted. Not a glyph and not an icon: `default_fonts` is off and this
/// window registers two Latin faces with nothing behind them, so a character
/// outside them draws as the replacement box. That is what [`disclosure`] is
/// painted for. `[`, `?` and `]` are ASCII and cannot go missing.
///
/// **A popup rather than a sentence under the control.** The settings panel
/// carried its explanations inline, and eight paragraphs interleaved with
/// twenty-five switches is what made it read as cluttered — the prose competed
/// with the controls and neither could be scanned. Not a tooltip either: a
/// hover is not discoverable, and on a touchpad it means holding still. A mark
/// that says *press me for the reason* is both findable and quiet.
///
/// `key` is the setting's own name and **is** the popup's identity. It must not
/// be left to the auto-id: the panel shows a different set of rows in each
/// mode, so an id derived from position would hand a popup to a different
/// setting the moment the mode changed.
pub fn help(ui: &mut Ui, key: &str, sentence: &str, palette: &Palette) {
    let mark = egui::RichText::new("[?]")
        .font(fonts::mono(window::LABEL))
        .color(palette.muted);
    let hit = action(ui, mark, true);
    let _ = egui::Popup::from_toggle_button_response(&hit)
        .id(egui::Id::new(("tgx-help", key)))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(HELP_WIDTH)
        .gap(4.0)
        // The window's own frame, not egui's: `Frame::popup` is a rounded panel
        // with a shadow, which is two of the three things `theme::install`
        // exists to take off everything else.
        .frame(
            egui::Frame::NONE
                .fill(palette.bg)
                .stroke(Stroke::new(1.0_f32, palette.hairline))
                .inner_margin(Margin::same(12)),
        )
        .show(|ui| {
            ui.set_max_width(HELP_WIDTH);
            ui.label(meta(sentence, palette));
        });
}

/// The × that takes a row out of the queue.
///
/// **Painted, not typed**, for [`disclosure`]'s reason: `\u{00d7}` is one
/// character outside the two registered faces away from drawing as nothing at
/// all, and two line segments cannot go missing.
///
/// Disabled paints **nothing**. A greyed × on the row that is currently
/// exporting would be one more mark in a table of numbers, offering something
/// it will not do; an empty cell says the same thing and says it silently.
/// Stop is the control for the chat that is running.
pub fn dismiss(ui: &mut Ui, enabled: bool, palette: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::splat(window::READING),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if enabled {
        let ink = if response.hovered() {
            palette.fg
        } else {
            palette.muted
        };
        let arms = rect.shrink(3.0);
        let stroke = Stroke::new(1.0_f32, ink);
        let painter = ui.painter();
        painter.line_segment([arms.left_top(), arms.right_bottom()], stroke);
        painter.line_segment([arms.right_top(), arms.left_bottom()], stroke);
        return response.on_hover_cursor(CursorIcon::PointingHand);
    }
    response
}

/// The disclosure marker on a category heading: ▸ folded, ▾ open.
///
/// **Painted, not typed, and that is a fix rather than a preference.** It was
/// `"\u{25b8}"` and `"\u{25be}"` set as text, and `default_fonts` is off — so
/// with no fallback face behind Geist, which carries no Geometric Shapes,
/// egui drew the replacement glyph and every category heading in the chat list
/// read `? CHANNELS`. A missing glyph in a window with no fallback is silent at
/// compile time, silent in the tests, and obvious only on screen.
///
/// A triangle is three points. There is nothing here that can be missing.
pub fn disclosure(ui: &mut Ui, folded: bool, palette: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(window::LABEL), Sense::hover());
    let c = rect.center();
    let r = window::LABEL * 0.3;
    let points = if folded {
        // Pointing right: the category is closed.
        vec![
            egui::pos2(c.x - r * 0.6, c.y - r),
            egui::pos2(c.x - r * 0.6, c.y + r),
            egui::pos2(c.x + r * 0.8, c.y),
        ]
    } else {
        // Pointing down: it is open.
        vec![
            egui::pos2(c.x - r, c.y - r * 0.6),
            egui::pos2(c.x + r, c.y - r * 0.6),
            egui::pos2(c.x, c.y + r * 0.8),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        palette.muted,
        Stroke::NONE,
    ));
    response
}

/// The tick's edge, in points.
///
/// **Matched to the type it sits beside**, which is `window::READING`. At 12
/// against 14pt text it read as a slightly-too-small square rather than as a
/// box aligned with the label — the kind of half-point mismatch that makes a
/// row look assembled rather than laid out.
const TICK_SIZE: f32 = window::READING;

/// How wide a [`help`] popup is.
///
/// Wide enough for three lines of the small print, narrow enough that a
/// sentence stays a sentence: run across a maximised window a paragraph becomes
/// one 1,400-point line that the eye cannot get back to the start of.
const HELP_WIDTH: f32 = 320.0;

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
                    font_id: fonts::mono(window::LABEL),
                    color: figure,
                    ..Default::default()
                },
            );
        }
        job.append(
            &self.label,
            if self.number.is_some() { 10.0 } else { 0.0 },
            TextFormat {
                font_id: fonts::sans(window::READING),
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
                    .font(fonts::medium(window::READING))
                    .color(palette.fg),
            );
            if tall_enough {
                if let Some(hint) = &self.hint {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(hint)
                            .font(fonts::sans(window::LABEL))
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

    /// Press the `[?]` and see whether anything opens.
    ///
    /// **Driven through a real `Context`, because there is nothing else that
    /// can check it.** The popup is the whole reason twenty-five explanations
    /// could come off the settings panel, and a mark that painted correctly and
    /// opened nothing would look exactly like one that worked — the same shape
    /// of defect as the tick box that was drawn on top of the row and swallowed
    /// its click, which shipped because nobody could see it either. The window
    /// cannot be photographed from outside (see `shell::shot`), so this is the
    /// check that exists.
    ///
    /// `theme::install` first: `default_fonts` is off in this design and the
    /// mark is set in a family only that call registers.
    #[test]
    fn pressing_the_help_mark_opens_a_popup_and_pressing_it_again_closes_it() {
        let palette = Palette::dark();
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &palette);
        let id = egui::Id::new(("tgx-help", "size_limit"));
        let at = Cell::new(egui::Pos2::ZERO);

        let frame = |press: bool| {
            let mut input = egui::RawInput::default();
            if press {
                let pos = at.get();
                input.events.push(egui::Event::PointerMoved(pos));
                for pressed in [true, false] {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let before = ui.next_widget_position();
                    help(
                        ui,
                        "size_limit",
                        "Files over this are not downloaded.",
                        &palette,
                    );
                    at.set(before + egui::vec2(4.0, 4.0));
                });
            });
        };

        // One frame to lay it out and learn where the mark landed.
        frame(false);
        assert!(!egui::Popup::is_id_open(&ctx, id), "open before any click");
        frame(true);
        assert!(
            egui::Popup::is_id_open(&ctx, id),
            "the [?] painted and opened nothing"
        );
        // And it is a toggle, not a one-way door: the same press closes it,
        // which is the only way to dismiss one with the pointer still on it.
        frame(true);
        assert!(!egui::Popup::is_id_open(&ctx, id));
    }

    /// A press somewhere in a rect, or nothing.
    ///
    /// Shared by the three controls below, all of which have the same problem:
    /// they paint and they are new, so the only way to know they *do* anything
    /// is to press them.
    fn press(at: Option<egui::Pos2>) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        if let Some(pos) = at {
            input.events.push(egui::Event::PointerMoved(pos));
            for pressed in [true, false] {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
        }
        input
    }

    #[test]
    fn a_segmented_strip_reports_the_cell_pressed_and_not_the_one_already_chosen() {
        // Pressing the mode you are in is not a change. Reported as one it
        // would rewrite settings.json on every stray click — and, because the
        // Settings panel replaces its rows on a mode change, redraw the whole
        // section under the pointer for no reason.
        let palette = Palette::dark();
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &palette);
        let (left, right, got) = (Cell::new(None), Cell::new(None), Cell::new(None));

        let frame = |at: Option<egui::Pos2>| {
            let _ = ctx.run(press(at), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // Wrapped so there is a response whose rect is the strip's
                    // own: the panel's `min_rect` is the panel's width, which
                    // put "the right-hand cell" a long way to the right of both.
                    let laid =
                        ui.horizontal(|ui| segmented(ui, &["Classic", "Database"], 0, &palette));
                    got.set(laid.inner);
                    let strip = laid.response.rect;
                    let y = strip.center().y;
                    left.set(Some(egui::pos2(strip.left() + strip.width() * 0.25, y)));
                    right.set(Some(egui::pos2(strip.left() + strip.width() * 0.75, y)));
                });
            });
        };

        frame(None);
        assert_eq!(got.get(), None, "nothing was pressed");
        frame(right.get());
        assert_eq!(got.get(), Some(1), "the second cell did nothing");
        frame(left.get());
        assert_eq!(got.get(), None, "the cell already chosen reported a change");
    }

    #[test]
    fn a_dismiss_takes_a_click_when_it_is_live_and_none_when_it_is_not() {
        // The × on a queue row. Disabled it paints nothing at all, which is the
        // right look for the row that is exporting — and would be an
        // indistinguishable look for a × that had quietly stopped working.
        let palette = Palette::dark();
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &palette);
        let (at, hit) = (Cell::new(None), Cell::new(false));

        let frame = |live: bool, press_at: Option<egui::Pos2>| {
            let _ = ctx.run(press(press_at), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mark = dismiss(ui, live, &palette);
                    at.set(Some(mark.rect.center()));
                    hit.set(mark.clicked());
                });
            });
        };

        frame(true, None);
        frame(true, at.get());
        assert!(hit.get(), "a live × did not take the click");
        frame(false, at.get());
        assert!(!hit.get(), "a dead × took one");
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
            window::LABEL,
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
        assert_eq!(job.sections[0].format.font_id, fonts::mono(window::LABEL));
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
    fn every_label_is_the_one_label_size() {
        // The failure this whole scale exists to prevent, checked at the only
        // place it could recur: a helper that takes its size from somewhere
        // other than the token. Two passes ended with 14, 13 and 11 in play
        // because `caps` took a size and every call site answered for itself.
        let p = Palette::dark();
        for job in [caps("Sort", p.fg), eyebrow("Queue", &p), title("Chats", &p)] {
            assert_eq!(job.sections[0].format.font_id.size, window::LABEL);
        }
        // [`text`], [`meta`] and [`figure`] are deliberately not checked here.
        // `RichText` keeps its `FontId` behind a method rather than a field, and
        // measuring what they draw needs a context with this crate's fonts
        // installed, which `__run_test_ui` does not provide — a probe through it
        // returns the same height for all three and passes whatever they say.
        //
        // They return `RichText` rather than a `LayoutJob` on purpose: a job
        // carries its own wrapping, and these three set the log lines and the
        // settings hints, which have to wrap to the panel.
        //
        // What guards them is the signature. None of the three takes a size, so
        // there is no call site left that can answer the question differently
        // from the one beside it — and that the roles are far enough apart to
        // tell apart is `tokens`'s own test.
    }

    #[test]
    fn a_panel_title_outranks_a_section_heading_by_ink_and_not_by_size() {
        // Four panel titles name the window's regions and have to win against
        // the headings inside them. They do it with the text colour, because
        // there is no size above `LABEL` for a label to reach for.
        let p = Palette::dark();
        let (t, e) = (title("Chats", &p), eyebrow("Destination", &p));
        assert_eq!(t.sections[0].format.color, p.fg);
        assert_eq!(e.sections[0].format.color, p.muted);
        assert_eq!(
            t.sections[0].format.font_id.size,
            e.sections[0].format.font_id.size
        );
    }

    #[test]
    fn the_tick_box_is_the_size_of_the_text_beside_it() {
        // A 12pt square against 14pt type reads as a slightly-wrong square
        // rather than as a box aligned to its label.
        assert_eq!(TICK_SIZE, window::READING);
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
