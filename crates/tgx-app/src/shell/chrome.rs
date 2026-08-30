//! The frame around the panels: the nav bar and the status bar.
//!
//! The panels themselves are the sibling modules -- chats, run, settings,
//! signin.

use super::*;
use eframe::egui::{Layout, Ui};
use tgx_ui::components::{action, caps, edge_rule, eyebrow, row, NavCell};
use tgx_ui::tokens::{space, window};

impl Shell {
    pub(super) fn nav_bar(&mut self, ui: &mut Ui) {
        let p = self.palette;
        // The bar numbers the steps and only the steps. Stop and Open output
        // folder are tools: unnumbered, and pushed right where the sequence
        // has ended.
        let busy = self.exporting || self.counting;
        let steps = [
            // Disabled while the dialog is up: a second click would spawn a
            // second `Session::connect` behind the one already running, and
            // `open_login` would raise the existing dialog while the extra
            // connection carried on in the background.
            NavCell::step(1, "Sign in").enabled(!busy && self.login.is_none()),
            NavCell::step(2, "Refresh chats").enabled(self.signed_in && !busy),
            // **The one red in the window.** This is the button the application
            // exists for; everything else on the bar leads to it or cleans up
            // after it. See `components::button`.
            NavCell::step(3, "Start export")
                .enabled(self.signed_in && !self.selected.is_empty() && !busy)
                .primary(true),
        ];
        let tools = [
            NavCell::tool("Stop").enabled(busy),
            NavCell::tool("Open output folder"),
        ];

        // **The row is given the bar's height, not `interact_size`'s.** The
        // panel is `NAV_HEIGHT` tall, but `Ui::horizontal` allocates a strip one
        // `interact_size.y` high and centres within *that* — so the buttons sat
        // in the top 30 points with their edges against the window, the rule
        // below them landed at 33, and the remaining 27 points of the bar were
        // empty. Laying out inside a rect of the full height puts them in the
        // middle of it and the rule at the bottom, where a rule under a bar goes.
        //
        // **A row of buttons, not a strip of text with rules through it.** The
        // cells were flat labels separated by vertical hairlines — five runs of
        // type along the top of a bar whose whole message is *press these three,
        // in order*. They are boxes now, and the separators went with them: a
        // hairline between two hairline boxes is a third line saying nothing.
        let body = egui::vec2(ui.available_width(), ui.available_height() - 1.0);
        ui.allocate_ui_with_layout(body, Layout::left_to_right(egui::Align::Center), |ui| {
            row(ui, |ui| {
                for (i, cell) in steps.iter().enumerate() {
                    if cell.show(ui, &p).clicked() {
                        match i {
                            0 => self.start_sign_in(),
                            1 => self.start_refresh(),
                            2 => self.start_export(),
                            _ => {}
                        }
                    }
                }

                // The tools and the appearance chip sit at the far end, so they
                // are laid out right-to-left from it and the sequence keeps the
                // left.
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    // **The chip names the theme it switches *to***, like the
                    // site's, so it reads as an action rather than as a label
                    // for where you already are. Left flat: it is the one thing
                    // up here that nobody arrives looking for.
                    let chip = caps(other_theme(&self.settings.theme), p.muted);
                    if action(ui, chip, true).clicked() {
                        self.toggle_theme();
                    }
                    ui.add_space(6.0);

                    for (i, cell) in tools.iter().enumerate() {
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
                });
            });
        });
        edge_rule(ui, &p);
    }

    /// Chats · Settings · Run, and which one the body is showing.
    ///
    /// **The selected one is ink over a hairline; the others are muted with
    /// nothing under them.** No boxes, no pills, no filled tab: an underline is
    /// the design's own primitive doing the one job a tab strip has. Each cell
    /// carries its own count on the right — the number of chats, the number
    /// queued — because the reason to look at a view you are not in is usually
    /// to find out whether it has anything in it.
    pub(super) fn view_bar(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let current = self.body;
        let mut chosen = None;

        ui.add_space(space::TIGHT);
        row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = space::BREAK;
            for view in View::ALL {
                let selected = view == current;
                let ink = if selected { p.fg } else { p.muted };
                let count = match view {
                    View::Chats if !self.chats.is_empty() => Some(self.chats.len()),
                    View::Run if !self.queue.is_empty() => Some(self.queue.len()),
                    _ => None,
                };
                let mut label = view.label().to_string();
                if let Some(n) = count {
                    label.push_str(&format!("  {n}"));
                }
                let hit = action(ui, caps(&label, ink), true);
                if selected {
                    // Two points below the text, so the rule sits under the
                    // word rather than against its descenders.
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
        });
        ui.add_space(space::TIGHT);
        edge_rule(ui, &p);

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
    pub(super) fn toggle_theme(&mut self) {
        self.settings.theme = other_theme(&self.settings.theme).into();
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
