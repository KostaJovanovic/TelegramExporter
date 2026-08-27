//! The run panel: the queue, one progress bar, and the log.
//!
//! The panel titled QUEUE used to render the log, so the queue `start_export`
//! built was never on screen and the empty state said "Nothing queued" when
//! what was empty was the log. They are separate here, and both scroll.
//!
//! The log is the more consequential fix. It was an unbounded `Vec` of which
//! only the last six lines were ever painted, in a panel with no scrolling —
//! so the INCOMPLETE-export warning, the one line the code goes out of its way
//! to distinguish, became unreachable after six more chats finished.

use super::Shell;
use gpui::prelude::*;
use gpui::{div, px, Context, Div, SharedString};
use gpui_component::scroll::Scrollbar;
use tgx_ui::components::{caps, eyebrow, progress_bar, rule, soft_rule, thousands, EmptyState};
use tgx_ui::tokens::{type_scale, Palette};

/// Widths for the queue's count columns.
///
/// **The Chat column is the stretch section and the only one that says which
/// row is which**, so it must not be the first thing a narrow panel throws
/// away. The counts are fixed and Chat takes the rest, with a floor under it —
/// the same arrangement the original reached by giving its stretch section a
/// 72px minimum, after measuring it squeezed to 11px while Messages and Topics
/// kept their full width throughout.
///
/// These four plus the gaps and the panel's padding have to leave more than
/// [`CHAT_MIN_W`] inside [`super::RIGHT_COLUMN_W`], or the row overflows the
/// panel it is in. 68+58+38+62 = 226, four 8px gaps and 32px of padding = 290,
/// which leaves 110 at 400px wide. Change one of these and check that sum.
const STATUS_W: gpui::Pixels = px(68.0);
const COUNT_W: gpui::Pixels = px(58.0);
const TOPICS_W: gpui::Pixels = px(38.0);
const MEDIA_W: gpui::Pixels = px(62.0);
/// The floor under the Chat column. Below every natural width at the default
/// geometry, so it costs nothing until the panel is genuinely too narrow — at
/// which point the row overflows rather than losing the only cell that names
/// the chat.
const CHAT_MIN_W: gpui::Pixels = px(72.0);

impl Shell {
    pub(super) fn run_panel(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        div()
            .flex()
            .flex_col()
            .flex_none()
            // Measured, not guessed: the queue header and four rows, the bar
            // and its caption, and eight log lines. Below this the log stops
            // being a transcript and becomes a ticker.
            //
            // **Fixed, and the settings panel takes the slack** — the reverse
            // of the arrangement that put 181px of children under a 100%
            // height and let flex squeeze both. The trade is visible at the
            // 620px window minimum, where this is nearly half the column and
            // the settings scroll into a short viewport: the run panel is what
            // is *happening*, and a queue you cannot see is worse than a
            // settings list you have to scroll.
            .h(px(300.0))
            .child(self.queue_panel(p, cx))
            .child(rule(p))
            .child(self.progress_row(p))
            .child(soft_rule(p))
            .child(self.log_panel(p, cx))
    }

    fn queue_panel(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let mut panel = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px(px(16.0))
                    .py(px(10.0))
                    .child(eyebrow("Queue", p))
                    .child(eyebrow(
                        if self.queue.is_empty() {
                            String::new()
                        } else {
                            format!("{} chats", self.queue.len())
                        },
                        p,
                    )),
            )
            .child(rule(p));

        if self.queue.is_empty() {
            return panel.child(
                div().flex_1().child(
                    EmptyState::new(
                        "Nothing queued",
                        Some(
                            "Tick the chats you want on the left, then choose \
                             Start export."
                                .into(),
                        ),
                    )
                    // A short panel drops the hint and keeps the headline.
                    .render(p, false),
                ),
            );
        }

        // Uppercase headers, letterspaced, because the design says so and
        // because a column of counts needs its label to read as a label.
        panel = panel.child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(COLUMN_GAP))
                .px(px(PANEL_PAD))
                .py(px(6.0))
                .child(div().flex_1().min_w(CHAT_MIN_W).child(caps(
                    "Chat",
                    type_scale::MICRO,
                    p.muted,
                )))
                .child(cell(STATUS_W).child(caps("Status", type_scale::MICRO, p.muted)))
                .child(cell(COUNT_W).child(caps("Messages", type_scale::MICRO, p.muted)))
                .child(cell(TOPICS_W).child(caps("Topics", type_scale::MICRO, p.muted)))
                .child(cell(MEDIA_W).child(caps("Media", type_scale::MICRO, p.muted))),
        );

        let mut rows = div()
            .id("queue-rows")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.queue_scroll);
        for (i, job) in self.queue.jobs().iter().enumerate() {
            let running = job.state == crate::queue::JobState::Exporting;
            // A finished row opens what it wrote. The export's own folder is
            // the thing the user came for, and the alternative is retyping a
            // path they watched scroll past in the log.
            let done = job.root.is_some();
            let chat_id = job.chat_id;
            rows = rows.child(
                div()
                    .id(("queue-row", i))
                    .flex()
                    .items_center()
                    .gap(px(COLUMN_GAP))
                    .px(px(PANEL_PAD))
                    .py(px(6.0))
                    .text_size(type_scale::TINY)
                    .when(done, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let Some(root) = this.queue.root_of(chat_id).cloned() else {
                                    return;
                                };
                                if let Err(e) = crate::actions::open_folder(&root) {
                                    this.apply(crate::bridge::Event::Warn(e));
                                    cx.notify();
                                }
                            }))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(CHAT_MIN_W)
                            .truncate()
                            .text_color(p.fg)
                            .child(SharedString::from(job.title.clone())),
                    )
                    .child(
                        cell(STATUS_W)
                            .truncate()
                            .text_color(if running { p.accent } else { p.muted })
                            .child(SharedString::from(job.state.label().to_string())),
                    )
                    // **Mono for every number**, as the stylesheet has it. These
                    // are counts that tick upward while the run goes, and Geist's
                    // proportional figures move the column sideways on every
                    // update — `1` is 384 units against `0`'s 663.
                    .child(
                        cell(COUNT_W)
                            .font(tgx_ui::fonts::mono())
                            .text_color(p.muted)
                            .child(SharedString::from(thousands(job.messages as i64))),
                    )
                    .child(
                        cell(TOPICS_W)
                            .font(tgx_ui::fonts::mono())
                            .text_color(p.muted)
                            .child(SharedString::from(job.topics_text())),
                    )
                    .child(
                        cell(MEDIA_W)
                            .font(tgx_ui::fonts::mono())
                            .text_color(p.muted)
                            .child(SharedString::from(job.media_text())),
                    ),
            );
        }

        panel.child(rows).child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .child(Scrollbar::vertical(&self.queue_scroll)),
        )
    }

    /// **One bar, two claimants.** The export claims it; while it is claimed
    /// every counting handler keeps its hands off, or a count finishing
    /// mid-export paints "Counted 12 of 12 chats" over "6,000 of 6,643".
    fn progress_row(&self, p: &Palette) -> Div {
        let (fraction, caption) = if self.exporting {
            (self.queue.fraction(), self.queue.running_caption())
        } else if let Some((done, total)) = self.count_progress {
            (
                Some(done as f32 / total.max(1) as f32),
                format!("Counting {done} of {total} chats"),
            )
        } else if !self.queue.is_empty() {
            // A finished run keeps its bar and its summary. Resetting to Idle
            // the moment the last chat lands throws away the one place that
            // says how the run ended, on the frame the user looks at it.
            (self.queue.fraction(), self.queue.summary())
        } else {
            // Not indeterminate: nothing is running, and a marker sitting at
            // the left of an idle bar reads as a job that has stalled.
            (Some(0.0), "Idle".to_string())
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(6.0))
            .px(px(16.0))
            .py(px(10.0))
            .child(progress_bar(fraction, p))
            .child(caps(caption, type_scale::MICRO, p.muted))
    }

    fn log_panel(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let mut panel = div().relative().flex().flex_col().flex_1().min_h_0().child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_between()
                .px(px(16.0))
                .py(px(8.0))
                .child(eyebrow("Log", p))
                // Two things the header has to say. A warning is marked so
                // it cannot be lost among the ordinary lines, and a
                // thousand-line transcript with one red line in it is
                // exactly where it gets lost — so the count is up here. And
                // what the cap has dropped is stated rather than silently
                // presenting a truncated run as the whole run.
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .when(self.journal.warnings() > 0, |d| {
                            let n = self.journal.warnings();
                            d.child(caps(
                                format!("{n} warning{}", if n == 1 { "" } else { "s" }),
                                type_scale::MICRO,
                                p.accent,
                            ))
                        })
                        .when(self.journal.dropped() > 0, |d| {
                            d.child(eyebrow(
                                format!("{} earlier lines dropped", self.journal.dropped()),
                                p,
                            ))
                        })
                        // **The one way out of this panel.** GPUI paints text
                        // and does not let you select it, so an export's own
                        // account of what it did stopped at the screen: it
                        // could be read, photographed, and handed on no other
                        // way. The sign-in error already solved this a line at
                        // a time; a transcript needs it all at once.
                        .when(!self.journal.is_empty(), |d| {
                            let copied = self.log_copied;
                            d.child(
                                div()
                                    .id("log-copy")
                                    .cursor_pointer()
                                    .child(caps(
                                        if copied { "Copied" } else { "Copy" },
                                        type_scale::MICRO,
                                        // Ink while it is an offer, muted once
                                        // it has been taken — the same weight
                                        // as the metadata beside it.
                                        if copied { p.muted } else { p.fg },
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            this.journal.to_text(),
                                        ));
                                        this.log_copied = true;
                                        cx.notify();
                                    })),
                            )
                        }),
                ),
        );

        if self.journal.is_empty() {
            return panel.child(
                div()
                    .flex_1()
                    .child(EmptyState::new("Nothing logged yet", None).render(p, false)),
            );
        }

        let mut lines = div()
            .id("log-lines")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.log_scroll)
            .px(px(16.0))
            .pb(px(8.0));
        for line in self.journal.lines() {
            lines = lines.child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(type_scale::TINY)
                    // A warning is painted in the accent because it was marked
                    // as one by whoever wrote it, never sniffed out of its text.
                    .text_color(if line.warning { p.accent } else { p.muted })
                    .child(SharedString::from(line.text.clone())),
            );
        }

        panel = panel.child(lines);
        panel.child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .child(Scrollbar::vertical(&self.log_scroll)),
        )
    }
}

/// A fixed-width column cell.
fn cell(width: gpui::Pixels) -> Div {
    div().flex_none().w(width)
}

/// The gap between queue columns, and the panel's horizontal padding.
const COLUMN_GAP: f32 = 8.0;
const PANEL_PAD: f32 = 16.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_columns_leave_room_for_the_one_that_names_the_chat() {
        // This is arithmetic nobody does by eye, and getting it wrong does not
        // look like a bug: the row simply overflows and the Chat cell is the
        // part that goes, which is the one column that says which row is which.
        let fixed = f32::from(STATUS_W + COUNT_W + TOPICS_W + MEDIA_W);
        let chrome = COLUMN_GAP * 4.0 + PANEL_PAD * 2.0;
        let left = f32::from(super::super::RIGHT_COLUMN_W) - fixed - chrome;
        assert!(
            left >= f32::from(CHAT_MIN_W),
            "the Chat column gets {left}px, below its {CHAT_MIN_W:?} floor"
        );
    }
}
