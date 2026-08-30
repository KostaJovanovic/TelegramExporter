//! What a run is doing: the queue table, the progress bar and the log.

use super::*;
use eframe::egui::{Layout, Sense, Ui};
use tgx_ui::components::{caps, eyebrow, progress_bar, rule, soft_rule, thousands, EmptyState};
use tgx_ui::tokens::type_scale;

/// Widths for the queue's count columns.
///
/// **The Chat column is the stretch section and the only one that says which
/// row is which**, so it must not be the first thing a narrow panel throws
/// away. The counts are fixed and Chat takes the rest, with a floor under it —
/// a **72px minimum**. Without one it squeezes to 11px while Messages and
/// Topics keep their full width throughout, which was measured rather than
/// guessed at.
///
/// These four plus the gaps and the panel's padding have to leave more than
/// [`CHAT_MIN_W`] inside [`super::RIGHT_COLUMN_W`], or the row overflows the
/// panel it is in. 68+58+38+62 = 226, four 8px gaps and 32px of padding = 290,
/// which leaves 110 at 400px wide. Change one of these and check that sum.
const STATUS_W: f32 = 68.0;
const COUNT_W: f32 = 58.0;
const TOPICS_W: f32 = 38.0;
const MEDIA_W: f32 = 62.0;
/// The floor under the Chat column. Below every natural width at the default
/// geometry, so it costs nothing until the panel is genuinely too narrow — at
/// which point the row overflows rather than losing the only cell that names
/// the chat.
const CHAT_MIN_W: f32 = 72.0;
const COLUMN_GAP: f32 = 8.0;
const PANEL_PAD: f32 = 16.0;

impl Shell {
    pub(super) fn run_panel(&mut self, ui: &mut Ui) {
        let p = self.palette;
        // The bar and the log are measured off the bottom, so the queue takes
        // the slack. The reverse — laying the queue out first — is what let a
        // long queue push the bar off the panel.
        let bar_h = 46.0;
        let log_h = (ui.available_height() * 0.42).max(80.0);
        let queue_h = (ui.available_height() - bar_h - log_h).max(60.0);

        ui.allocate_ui(egui::vec2(ui.available_width(), queue_h), |ui| {
            self.queue_panel(ui);
        });
        rule(ui, &p);
        self.progress_row(ui);
        soft_rule(ui, &p);
        self.log_panel(ui);
    }

    fn queue_panel(&mut self, ui: &mut Ui) {
        let p = self.palette;
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(PANEL_PAD);
            ui.label(eyebrow("Queue", &p));
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(PANEL_PAD);
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

        // Uppercase headers, letterspaced, because the design says so and
        // because a column of counts needs its label to read as a label.
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(PANEL_PAD);
            ui.spacing_mut().item_spacing.x = COLUMN_GAP;
            let chat_w = (ui.available_width()
                - STATUS_W
                - COUNT_W
                - TOPICS_W
                - MEDIA_W
                - PANEL_PAD
                - 4.0 * COLUMN_GAP)
                .max(CHAT_MIN_W);
            for (label, w) in [
                ("Chat", chat_w),
                ("Status", STATUS_W),
                ("Messages", COUNT_W),
                ("Topics", TOPICS_W),
                ("Media", MEDIA_W),
            ] {
                ui.allocate_ui(egui::vec2(w, 14.0), |ui| {
                    ui.label(caps(label, type_scale::MICRO, p.muted));
                });
            }
        });
        ui.add_space(6.0);

        // Cloned once, because a click below needs `&mut self` and the borrow
        // cannot be held across it.
        let jobs: Vec<crate::queue::Job> = self.queue.jobs().to_vec();
        egui::ScrollArea::vertical()
            .id_salt("queue-rows")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for job in &jobs {
                    let running = job.state == crate::queue::JobState::Exporting;
                    // A finished row opens what it wrote. The export's own
                    // folder is the thing the user came for, and the
                    // alternative is retyping a path they watched scroll past
                    // in the log.
                    let done = job.root.is_some();
                    let response = ui
                        .horizontal(|ui| {
                            ui.add_space(PANEL_PAD);
                            ui.spacing_mut().item_spacing.x = COLUMN_GAP;
                            let chat_w = (ui.available_width()
                                - STATUS_W
                                - COUNT_W
                                - TOPICS_W
                                - MEDIA_W
                                - PANEL_PAD
                                - 4.0 * COLUMN_GAP)
                                .max(CHAT_MIN_W);
                            ui.allocate_ui(egui::vec2(chat_w, 16.0), |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&job.title)
                                            .font(tgx_ui::fonts::sans(type_scale::TINY))
                                            .color(p.fg),
                                    )
                                    .truncate(),
                                );
                            });
                            ui.allocate_ui(egui::vec2(STATUS_W, 16.0), |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(job.state.label())
                                            .font(tgx_ui::fonts::sans(type_scale::TINY))
                                            .color(if running { p.accent } else { p.muted }),
                                    )
                                    .truncate(),
                                );
                            });
                            // **Mono for every number**, as the stylesheet has
                            // it. These are counts that tick upward while the
                            // run goes, and Geist's proportional figures move
                            // the column sideways on every update — `1` is 384
                            // units against `0`'s 663.
                            for (text, w) in [
                                (thousands(job.messages as i64), COUNT_W),
                                (job.topics_text(), TOPICS_W),
                                (job.media_text(), MEDIA_W),
                            ] {
                                ui.allocate_ui(egui::vec2(w, 16.0), |ui| {
                                    ui.label(
                                        egui::RichText::new(text)
                                            .font(tgx_ui::fonts::mono(type_scale::TINY))
                                            .color(p.muted),
                                    );
                                });
                            }
                        })
                        .response;
                    if done && response.interact(Sense::click()).clicked() {
                        if let Some(root) = self.queue.root_of(job.chat_id).cloned() {
                            if let Err(e) = crate::actions::open_folder(&root) {
                                self.apply(crate::bridge::Event::Warn(e));
                            }
                        }
                    }
                }
            });
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
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(PANEL_PAD);
            ui.allocate_ui(egui::vec2(ui.available_width() - PANEL_PAD, 24.0), |ui| {
                progress_bar(ui, fraction, &p);
                ui.add_space(6.0);
                ui.label(caps(&caption, type_scale::MICRO, p.muted));
            });
        });
        ui.add_space(10.0);
    }

    fn log_panel(&mut self, ui: &mut Ui) {
        let p = self.palette;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(PANEL_PAD);
            ui.label(eyebrow("Log", &p));
            // Two things the header has to say. A warning is marked so it
            // cannot be lost among the ordinary lines, and a thousand-line
            // transcript with one red line in it is exactly where it gets lost
            // — so the count is up here. And what the cap has dropped is stated
            // rather than silently presenting a truncated run as the whole run.
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(PANEL_PAD);
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
                    if ui
                        .add(egui::Label::new(text).sense(Sense::click()))
                        .clicked()
                    {
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
                for line in self.journal.lines() {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(PANEL_PAD);
                        ui.label(
                            egui::RichText::new(&line.text)
                                .font(tgx_ui::fonts::sans(type_scale::TINY))
                                // A warning is painted in the accent because it
                                // was marked as one by whoever wrote it, never
                                // sniffed out of its text.
                                .color(if line.warning { p.accent } else { p.muted }),
                        );
                    });
                }
                ui.add_space(8.0);
            });
    }
}
