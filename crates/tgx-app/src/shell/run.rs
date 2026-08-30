//! What a run is doing: the queue table, the progress bar and the log.

use super::*;
use eframe::egui::{Align, Layout, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use tgx_ui::components::{
    action, block, caps, eyebrow, figure, progress_bar, row, rule, text, thousands, title,
    EmptyState,
};
use tgx_ui::tokens::{space, window};

/// Widths for the queue's count columns.
///
/// **The Chat column is the stretch section and the only one that says which row
/// is which**, so it must not be the first thing a narrow window throws away.
/// The counts are fixed and Chat takes the rest, with a floor under it.
///
/// **The floor is enforced rather than described.** An earlier pass computed the
/// Chat width itself, as `available - 68 - 58 - 38 - 62 - 16 - 4*8`, written out
/// twice — once for the header and once per row — under a comment asking the
/// next reader to re-check the sum by hand after any edit.
/// `Column::remainder().at_least(..)` is the same statement made to the layout
/// instead of to the reader, and it cannot fall out of step with the header
/// above it.
///
/// The counts are wider than they were, because there is now room: this table
/// had a whole window's width taken off it by a side panel, and `MESSAGES` at 58
/// points cannot hold `6,643` under its own header.
const STATUS_W: f32 = 96.0;
const COUNT_W: f32 = 88.0;
const TOPICS_W: f32 = 64.0;
const MEDIA_W: f32 = 88.0;
/// The floor under the Chat column.
///
/// **240, not 72.** Seventy-two was the width at which a name is still
/// technically present in a 400pt side column; at a window's width the column is
/// six hundred points and the floor only matters on a very narrow window, where
/// what it should protect is a title long enough to recognise rather than the
/// first two characters of one.
const CHAT_MIN_W: f32 = 240.0;
const COLUMN_GAP: f32 = space::TIGHT;

/// One queue row, and the header above it.
///
/// **26, not 20.** A row was 20 points tall carrying 13-point text, which is
/// tighter than the text's own leading — the type filled the row edge to edge
/// with nothing around it, and the row was also the click target that opens the
/// folder an export wrote. `READING` at the design's body leading is 21, so 26
/// gives it the same breathing room every other row in the window has.
const QUEUE_ROW_H: f32 = 26.0;
const HEADER_H: f32 = 20.0;

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

        // **The log gets a third of the window rather than 160 points.** It is
        // the only account of what a run actually did — every rate-limit wait,
        // every file that did not arrive, the read total against what Telegram
        // promised — and in a side column it showed six lines of a two-thousand
        // line ring.
        egui::TopBottomPanel::bottom("log")
            .frame(bare)
            .default_height(ui.available_height() * 0.38)
            .height_range(120.0..=640.0)
            .resizable(true)
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                rule(ui, &p);
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
        ui.add_space(space::STEP);
        row(ui, |ui| {
            ui.label(title("Queue", &p));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !self.queue.is_empty() {
                    ui.label(eyebrow(&format!("{} chats", self.queue.len()), &p));
                }
            });
        });
        ui.add_space(space::TIGHT);
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
        ui.add_space(space::TIGHT);
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
                .header(HEADER_H, |mut header| {
                    // Uppercase headers, letterspaced, because a column of
                    // counts needs its label to read as a label.
                    for label in ["Chat", "Status", "Messages", "Topics", "Media"] {
                        header.col(|ui| {
                            ui.label(eyebrow(label, &p));
                        });
                    }
                })
                .body(|body| {
                    body.rows(QUEUE_ROW_H, jobs.len(), |mut r| {
                        let job = &jobs[r.index()];
                        let running = job.state == crate::queue::JobState::Exporting;
                        r.col(|ui| {
                            ui.add(egui::Label::new(text(&job.title, &p)).truncate());
                        });
                        // **The one place the accent appears in this table**,
                        // and it means the same thing it means on the nav bar:
                        // *this run*. The state of a row that is not running is
                        // metadata like the counts beside it.
                        r.col(|ui| {
                            let state = if running {
                                caps(job.state.label(), p.accent)
                            } else {
                                eyebrow(job.state.label(), &p)
                            };
                            ui.add(egui::Label::new(state).truncate());
                        });
                        // Counts that tick upward while the run goes, so Geist's
                        // proportional figures would move the column sideways on
                        // every update — `1` is 384 units against `0`'s 663.
                        // `figure` is the mono, which is tabular by
                        // construction.
                        for n in [
                            thousands(job.messages as i64),
                            job.topics_text(),
                            job.media_text(),
                        ] {
                            r.col(|ui| {
                                ui.label(figure(n, &p));
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
        ui.add_space(space::STEP);
        block(ui, |ui| {
            progress_bar(ui, fraction, &p);
            ui.add_space(space::TIGHT);
            ui.label(eyebrow(&caption, &p));
        });
        ui.add_space(space::STEP);
    }

    fn log_panel(&mut self, ui: &mut Ui) {
        let p = self.palette;
        ui.add_space(space::STEP);
        row(ui, |ui| {
            ui.label(title("Log", &p));
            // Two things the header has to say. A warning is marked so it
            // cannot be lost among the ordinary lines, and a thousand-line
            // transcript with one red line in it is exactly where it gets lost
            // — so the count is up here. And what the cap has dropped is stated
            // rather than silently presenting a truncated run as the whole run.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = space::STEP;
                // **The one way out of this panel.** An export's own account of
                // what it did used to stop at the screen: it could be read,
                // photographed, and handed on no other way. egui can select the
                // lines themselves now, but a transcript is wanted all at once.
                if !self.journal.is_empty() {
                    let copied = self.log_copied;
                    // Ink while it is an offer, muted once it has been taken —
                    // the same weight as the metadata beside it. Enabled either
                    // way: "Copied" is a report, not a control that has been
                    // used up, and the transcript can be taken again once it has
                    // grown.
                    let label = if copied { "Copied" } else { "Copy" };
                    let ink = if copied { p.muted } else { p.fg };
                    if action(ui, caps(label, ink), true).clicked() {
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
                // **The accent, and it is still "this run".** A warning is
                // something the run produced and has not been read yet, which is
                // the same category as the progress bar beside it.
                if self.journal.warnings() > 0 {
                    let n = self.journal.warnings();
                    let word = if n == 1 { "warning" } else { "warnings" };
                    ui.label(caps(&format!("{n} {word}"), p.accent));
                }
            });
        });
        ui.add_space(space::TIGHT);

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
                        // **A warning is ink, not the accent.** In a transcript
                        // set entirely in `muted`, the text colour already *is*
                        // the loud one — and the accent is spoken for: it means
                        // *this run*, on the Start button, the progress fill and
                        // the row that is exporting. Spending it on a fourth
                        // thing is how it stopped meaning anything. Marked as a
                        // warning by whoever wrote the line, never sniffed out
                        // of its text.
                        let ink = if line.warning { p.fg } else { p.muted };
                        ui.label(
                            egui::RichText::new(&line.text)
                                .font(tgx_ui::fonts::sans(window::LABEL))
                                .color(ink),
                        );
                    }
                });
                ui.add_space(space::TIGHT);
            });
    }
}
