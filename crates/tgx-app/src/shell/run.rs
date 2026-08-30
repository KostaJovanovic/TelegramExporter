//! What a run is doing: the queue table, the progress bar and the log.

use super::*;
use eframe::egui::{Align, Layout, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use tgx_ui::components::{
    action, block, caps, eyebrow, progress_bar, row, rule, soft_rule, thousands, EmptyState,
};
use tgx_ui::tokens::type_scale;

/// Widths for the queue's count columns.
///
/// **The Chat column is the stretch section and the only one that says which row
/// is which**, so it must not be the first thing a narrow panel throws away. The
/// counts are fixed and Chat takes the rest, with a floor under it — a **72px
/// minimum**. Without one it squeezes to 11px while Messages and Topics keep
/// their full width throughout, which was measured rather than guessed at.
///
/// **The floor is now enforced rather than described.** The first pass computed
/// the Chat width itself, as `available - 68 - 58 - 38 - 62 - 16 - 4*8`, written
/// out twice — once for the header and once per row — under a comment asking the
/// next reader to re-check the sum by hand after any edit. `Column::remainder()`
/// with an `at_least` is the same statement made to the layout instead of to the
/// reader, and it cannot fall out of step with the header above it.
const STATUS_W: f32 = 68.0;
const COUNT_W: f32 = 58.0;
const TOPICS_W: f32 = 38.0;
const MEDIA_W: f32 = 62.0;
/// The floor under the Chat column. Below every natural width at the default
/// geometry, so it costs nothing until the panel is genuinely too narrow — at
/// which point the table scrolls sideways rather than losing the only cell that
/// names the chat.
const CHAT_MIN_W: f32 = 72.0;
const COLUMN_GAP: f32 = 8.0;
/// One queue row. Tight, because the panel holds a table, a bar and a log.
const QUEUE_ROW_H: f32 = 20.0;

impl Shell {
    /// **Bar and log off the bottom, queue in what is left — as panels.**
    ///
    /// The first pass did this arithmetically: `46.0` reserved for the bar, 42%
    /// of the height for the log, and the queue given the remainder with a
    /// `.max(60.0)` in case the sums went negative. Three numbers to keep in
    /// step with three panels' actual contents. The bar measures itself now, and
    /// the log's share is a *default* the user can drag, which is the right
    /// answer to a split whose better position depends on whether an export is
    /// running.
    pub(super) fn run_panel(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let bare = egui::Frame::NONE.fill(p.bg);

        egui::TopBottomPanel::bottom("log")
            .frame(bare)
            .default_height(160.0)
            .height_range(80.0..=420.0)
            .resizable(true)
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                soft_rule(ui, &p);
                self.log_panel(ui);
            });

        egui::TopBottomPanel::bottom("progress")
            .frame(bare)
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                rule(ui, &p);
                self.progress_row(ui);
            });

        egui::CentralPanel::default()
            .frame(bare)
            .show_inside(ui, |ui| self.queue_panel(ui));
    }

    fn queue_panel(&mut self, ui: &mut Ui) {
        let p = self.palette;
        ui.add_space(10.0);
        row(ui, |ui| {
            ui.label(eyebrow("Queue", &p));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !self.queue.is_empty() {
                    ui.label(eyebrow(&format!("{} chats", self.queue.len()), &p));
                }
            });
        });
        ui.add_space(10.0);
        rule(ui, &p);

        if self.queue.is_empty() {
            EmptyState::new(
                "Nothing queued",
                Some("Tick the chats you want on the left, then choose Start export.".into()),
            )
            // A short panel drops the hint and keeps the headline.
            .show(ui, &p, false);
            return;
        }

        // Cloned once, because a click below needs `&mut self` and the borrow
        // cannot be held across it.
        let jobs: Vec<crate::queue::Job> = self.queue.jobs().to_vec();
        let mut opened = None;
        ui.add_space(6.0);
        block(ui, |ui| {
            // The table takes its column gap from the ui's own spacing.
            ui.spacing_mut().item_spacing.x = COLUMN_GAP;
            TableBuilder::new(ui)
                .id_salt("queue-rows")
                .vscroll(true)
                .auto_shrink([false, false])
                // A finished row opens what it wrote. The export's own folder is
                // the thing the user came for, and the alternative is retyping a
                // path they watched scroll past in the log.
                .sense(Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::remainder().at_least(CHAT_MIN_W).clip(true))
                .column(Column::exact(STATUS_W).clip(true))
                .column(Column::exact(COUNT_W))
                .column(Column::exact(TOPICS_W))
                .column(Column::exact(MEDIA_W))
                .header(18.0, |mut header| {
                    // Uppercase headers, letterspaced, because the design says
                    // so and because a column of counts needs its label to read
                    // as a label.
                    for label in ["Chat", "Status", "Messages", "Topics", "Media"] {
                        header.col(|ui| {
                            ui.label(caps(label, type_scale::MICRO, p.muted));
                        });
                    }
                })
                .body(|body| {
                    body.rows(QUEUE_ROW_H, jobs.len(), |mut r| {
                        let job = &jobs[r.index()];
                        let running = job.state == crate::queue::JobState::Exporting;
                        r.col(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&job.title)
                                        .font(tgx_ui::fonts::sans(type_scale::TINY))
                                        .color(p.fg),
                                )
                                .truncate(),
                            );
                        });
                        r.col(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(job.state.label())
                                        .font(tgx_ui::fonts::sans(type_scale::TINY))
                                        .color(if running { p.accent } else { p.muted }),
                                )
                                .truncate(),
                            );
                        });
                        // **Mono for every number**, as the stylesheet has it.
                        // These are counts that tick upward while the run goes,
                        // and Geist's proportional figures move the column
                        // sideways on every update — `1` is 384 units against
                        // `0`'s 663.
                        for text in [
                            thousands(job.messages as i64),
                            job.topics_text(),
                            job.media_text(),
                        ] {
                            r.col(|ui| {
                                ui.label(
                                    egui::RichText::new(text)
                                        .font(tgx_ui::fonts::mono(type_scale::TINY))
                                        .color(p.muted),
                                );
                            });
                        }
                        if job.root.is_some() && r.response().clicked() {
                            opened = Some(job.chat_id);
                        }
                    });
                });
        });

        if let Some(chat_id) = opened {
            if let Some(root) = self.queue.root_of(chat_id).cloned() {
                if let Err(e) = crate::actions::open_folder(&root) {
                    self.apply(crate::bridge::Event::Warn(e));
                }
            }
        }
    }

    /// **One bar, two claimants.** The export claims it; while it is claimed
    /// every counting handler keeps its hands off, or a count finishing
    /// mid-export paints "Counted 12 of 12 chats" over "6,000 of 6,643".
    fn progress_row(&mut self, ui: &mut Ui) {
        let p = self.palette;
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
        // A block, not a row inside an `allocate_ui` sized by hand: the bar and
        // its caption are stacked, and inside the gutter the available width
        // already is the bar's width.
        ui.add_space(10.0);
        block(ui, |ui| {
            progress_bar(ui, fraction, &p);
            ui.add_space(6.0);
            ui.label(caps(&caption, type_scale::MICRO, p.muted));
        });
        ui.add_space(10.0);
    }

    fn log_panel(&mut self, ui: &mut Ui) {
        let p = self.palette;
        ui.add_space(8.0);
        row(ui, |ui| {
            ui.label(eyebrow("Log", &p));
            // Two things the header has to say. A warning is marked so it
            // cannot be lost among the ordinary lines, and a thousand-line
            // transcript with one red line in it is exactly where it gets lost
            // — so the count is up here. And what the cap has dropped is stated
            // rather than silently presenting a truncated run as the whole run.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                // **The one way out of this panel.** An export's own account of
                // what it did used to stop at the screen: it could be read,
                // photographed, and handed on no other way. egui can select the
                // lines themselves now, but a transcript is wanted all at once.
                if !self.journal.is_empty() {
                    let copied = self.log_copied;
                    let text = caps(
                        if copied { "Copied" } else { "Copy" },
                        type_scale::MICRO,
                        // Ink while it is an offer, muted once it has been
                        // taken — the same weight as the metadata beside it.
                        if copied { p.muted } else { p.fg },
                    );
                    // Enabled either way: "Copied" is a report, not a control
                    // that has been used up, and the transcript can be taken
                    // again after it has grown.
                    if action(ui, text, true).clicked() {
                        ui.ctx().copy_text(self.journal.to_text());
                        self.log_copied = true;
                    }
                }
                if self.journal.dropped() > 0 {
                    ui.label(eyebrow(
                        &format!("{} earlier lines dropped", self.journal.dropped()),
                        &p,
                    ));
                }
                if self.journal.warnings() > 0 {
                    let n = self.journal.warnings();
                    ui.label(caps(
                        &format!("{n} warning{}", if n == 1 { "" } else { "s" }),
                        type_scale::MICRO,
                        p.accent,
                    ));
                }
            });
        });
        ui.add_space(8.0);

        if self.journal.is_empty() {
            EmptyState::new("Nothing logged yet", None).show(ui, &p, false);
            return;
        }

        egui::ScrollArea::vertical()
            .id_salt("log-lines")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.add_space(4.0);
                // One gutter around the whole transcript rather than one
                // `add_space` per line: the lines wrap, and a space allocated
                // inside a wrapping row is a space the second line does not get,
                // so long lines used to start flush against the panel edge.
                block(ui, |ui| {
                    for line in self.journal.lines() {
                        ui.label(
                            egui::RichText::new(&line.text)
                                .font(tgx_ui::fonts::sans(type_scale::TINY))
                                // A warning is painted in the accent because it
                                // was marked as one by whoever wrote it, never
                                // sniffed out of its text.
                                .color(if line.warning { p.accent } else { p.muted }),
                        );
                    }
                });
                ui.add_space(8.0);
            });
    }
}
