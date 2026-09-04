//! The frame around the panels: the top bar and the status bar.
//!
//! The panels themselves are the sibling modules -- chats, queue, settings,
//! signin.

use super::*;
use eframe::egui::{Layout, Ui};
use tgx_ui::components::{action, caps, edge_rule, eyebrow, row, NavCell};
use tgx_ui::tokens::{space, window};

impl Shell {
    /// **One bar: where you are on the left, what to do on the right.**
    ///
    /// It was two stacked strips — five buttons in the first, three tab labels
    /// in the second — and the complaint they earned was that the top of the
    /// window read as two menus with no way to tell which was which. They are
    /// not the same kind of thing at all: the tabs choose what the body shows
    /// and change nothing, the buttons act on the account. Putting them on one
    /// line at opposite ends says that with the layout, and the two kinds stay
    /// apart by their own primitives — tabs are tracked capitals under a rule,
    /// actions are hairline boxes. A hairline box next to a run of small
    /// capitals is unmistakable; a hairline box above one was not.
    ///
    /// **The sequence keeps its numbers.** `01`–`03` across Sign in, Refresh
    /// and Start export is the one thing on this bar that tells a first-time
    /// user there is an order. Stop and Open folder are tools: unnumbered, and
    /// last.
    ///
    /// **The appearance chip moved into Settings.** It was a sixth kind of mark
    /// on a bar that was already carrying too many, and it is a stored setting
    /// with a home — see `settings::appearance_section`.
    pub(super) fn top_bar(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let busy = self.exporting || self.counting;

        // **Start export becomes Add to queue while a run is going**, because
        // that is what pressing it does now: the ticked chats join the run in
        // flight rather than replacing it. Same slot, same number, so the
        // control does not move under the pointer — only its promise changes,
        // and it changes to the true one.
        let start = if self.exporting {
            "Add to queue"
        } else {
            "Start export"
        };
        let steps = [
            // Disabled while the dialog is up: a second click would spawn a
            // second `Session::connect` behind the one already running, and
            // `open_login` would raise the existing dialog while the extra
            // connection carried on in the background.
            NavCell::step(1, "Sign in").enabled(!busy && self.login.is_none()),
            NavCell::step(2, "Refresh").enabled(self.signed_in && !busy),
            // **The one red in the window.** This is the button the application
            // exists for; everything else on the bar leads to it or cleans up
            // after it. See `components::button`.
            NavCell::step(3, start)
                .enabled(self.signed_in && !self.selected.is_empty() && !self.counting)
                .primary(true),
        ];
        let tools = [
            NavCell::tool("Stop").enabled(busy),
            NavCell::tool("Open folder"),
        ];

        // **The row is given the bar's height, not `interact_size`'s.** The
        // panel is `NAV_HEIGHT` tall, but `Ui::horizontal` allocates a strip one
        // `interact_size.y` high and centres within *that* — so the buttons sat
        // in the top 30 points with their edges against the window, the rule
        // below them landed at 33, and the remaining 27 points of the bar were
        // empty. Laying out inside a rect of the full height puts them in the
        // middle of it and the rule at the bottom, where a rule under a bar goes.
        let body = egui::vec2(ui.available_width(), ui.available_height() - 1.0);
        ui.allocate_ui_with_layout(body, Layout::left_to_right(egui::Align::Center), |ui| {
            row(ui, |ui| {
                self.view_tabs(ui);

                // Laid out right-to-left from the far end, so the tools finish
                // the line and the sequence keeps the middle. Pushed in reverse
                // for that reason: what is listed first lands furthest right.
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    for (i, cell) in tools.iter().enumerate().rev() {
                        if cell.show(ui, &p).clicked() {
                            match i {
                                0 => self.stop(),
                                // Reported, not discarded. A silent spawn
                                // failure is what let `explorer.exe` be looked
                                // for in System32 — where it is not — for as
                                // long as nobody happened to watch the button do
                                // nothing.
                                1 => {
                                    if let Err(e) = crate::actions::open_output_folder(
                                        &self.settings.output_dir,
                                    ) {
                                        self.journal.warn(e);
                                        self.log_copied = false;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    ui.add_space(space::STEP);
                    for (i, cell) in steps.iter().enumerate().rev() {
                        if cell.show(ui, &p).clicked() {
                            match i {
                                0 => self.start_sign_in(),
                                1 => self.start_refresh(),
                                2 => self.start_export(),
                                _ => {}
                            }
                        }
                    }
                });
            });
        });
        edge_rule(ui, &p);
    }

    /// Chats · Settings · Queue, and which one the body is showing.
    ///
    /// **The selected one is ink over a hairline; the others are muted with
    /// nothing under them.** No boxes, no pills, no filled tab: an underline is
    /// the design's own primitive doing the one job a tab strip has — and now
    /// that the tabs share a bar with the action buttons, it is also what keeps
    /// the two kinds of control from being mistaken for each other. Each cell
    /// carries its own count on the right — the number of chats, the number
    /// queued — because the reason to look at a view you are not in is usually
    /// to find out whether it has anything in it.
    fn view_tabs(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let current = self.body;
        let mut chosen = None;

        ui.spacing_mut().item_spacing.x = space::BREAK;
        for view in View::ALL {
            let selected = view == current;
            let ink = if selected { p.fg } else { p.muted };
            let count = match view {
                View::Chats if !self.chats.is_empty() => Some(self.chats.len()),
                View::Queue if !self.queue.is_empty() => Some(self.queue.len()),
                _ => None,
            };
            let mut label = view.label().to_string();
            if let Some(n) = count {
                label.push_str(&format!("  {n}"));
            }
            let hit = action(ui, caps(&label, ink), true);
            if selected {
                // Two points below the text, so the rule sits under the word
                // rather than against its descenders.
                ui.painter().hline(
                    hit.rect.x_range(),
                    hit.rect.bottom() + 2.0,
                    egui::Stroke::new(1.0_f32, p.fg),
                );
            }
            if hit.clicked() {
                chosen = Some(view);
            }
        }
        // Restored before the caller lays anything else out in this row: the
        // tabs want a `BREAK` between them and the buttons beside them do not.
        ui.spacing_mut().item_spacing.x = space::TIGHT;

        if let Some(view) = chosen {
            self.show(view);
        }
    }

    /// Swap the appearance.
    ///
    /// **A token swap, not a rebuild.** Every colour in this window comes from
    /// the palette, so re-reading it repaints the lot. Under GPUI there was a
    /// second call here for the borrowed components, which painted from
    /// `gpui-component`'s own theme global rather than from ours; nothing is
    /// borrowed now, so the style is reinstalled on the next frame from the one
    /// palette and there is no second vocabulary to keep in step.
    pub(super) fn set_theme(&mut self, name: &str) {
        if self.settings.theme == name {
            return;
        }
        self.settings.theme = name.into();
        self.palette = Palette::named(&self.settings.theme);
        self.theme_stale = true;
        self.commit_settings();
    }

    pub(super) fn status_bar(&mut self, ui: &mut Ui) {
        let p = self.palette;
        // Counting reports in chats and an export in messages, and the export
        // owns the line whenever it is running.
        let right = if self.exporting {
            self.queue
                .fraction()
                .map(|f| format!("{:.0}%", f * 100.0))
                .unwrap_or_default()
        } else {
            match self.count_progress {
                Some((done, total)) => format!("{done} of {total}"),
                None => String::new(),
            }
        };

        edge_rule(ui, &p);
        ui.add_space(space::TIGHT);
        // **The nested left-to-right is load-bearing, and taking it out was a
        // regression.** The figure is the fixed part, so it is allocated first
        // from the right; the status has to be laid out *forwards* in what
        // remains, or a truncating label in a right-to-left layout fills the
        // panel and draws its text against the far edge — which is what put the
        // status line on the right-hand side of the window with nothing at all
        // on the left.
        row(ui, |ui| {
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                // The right-hand figure is ours and short, so it can be tracked.
                ui.label(eyebrow(&right, &p));
                ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                    // **In the mono, not letterspaced.** `eyebrow` goes through
                    // `components::tracked`, whose own doc says not to put a
                    // string of unknown length through it, and this line carries
                    // chat titles and raw RPC errors. The mono also keeps a line
                    // that changes several times a second from changing width
                    // under every character.
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(tgx_ui::components::uppercase(&self.status))
                                .font(tgx_ui::fonts::mono(window::LABEL))
                                .color(p.muted),
                        )
                        .truncate(),
                    );
                });
            });
        });
        ui.add_space(space::TIGHT);
    }
}
