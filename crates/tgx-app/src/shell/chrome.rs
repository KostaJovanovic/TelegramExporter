//! The frame around the panels: the nav bar and the status bar.
//!
//! The panels themselves are the sibling modules -- chats, run, settings,
//! signin.

use super::*;
use eframe::egui::{Layout, Ui};
use tgx_ui::components::{eyebrow, vrule, NavCell};
use tgx_ui::tokens::{metrics, type_scale};

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
            NavCell::step(3, "Start export")
                .enabled(self.signed_in && !self.selected.is_empty() && !busy),
        ];
        let tools = [
            NavCell::tool("Stop").enabled(busy),
            NavCell::tool("Open output folder"),
        ];

        ui.set_height(metrics::NAV_HEIGHT);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (i, cell) in steps.iter().enumerate() {
                ui.add_space(16.0);
                if cell.show(ui, &p).clicked() {
                    match i {
                        0 => self.start_sign_in(),
                        1 => self.start_refresh(),
                        2 => self.start_export(),
                        _ => {}
                    }
                }
                ui.add_space(16.0);
                vrule(ui, &p);
            }

            // The tools and the theme chip sit at the far end, so they are laid
            // out right-to-left from it and the sequence keeps the left.
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                // **The chip names the theme it switches *to***, like the
                // site's, so it reads as an action rather than as a label for
                // where you already are.
                ui.add_space(16.0);
                let chip = tgx_ui::components::caps(
                    other_theme(&self.settings.theme),
                    type_scale::MICRO,
                    p.muted,
                );
                if ui
                    .add(egui::Label::new(chip).sense(egui::Sense::click()))
                    .clicked()
                {
                    self.toggle_theme();
                }
                ui.add_space(16.0);

                for (i, cell) in tools.iter().enumerate() {
                    vrule(ui, &p);
                    ui.add_space(16.0);
                    if cell.show(ui, &p).clicked() {
                        match i {
                            0 => self.stop(),
                            // Reported, not discarded. A silent spawn failure
                            // is what let `explorer.exe` be looked for in
                            // System32 — where it is not — for as long as
                            // nobody happened to watch the button do nothing.
                            1 => {
                                if let Err(e) =
                                    crate::actions::open_output_folder(&self.settings.output_dir)
                                {
                                    self.journal.warn(e);
                                    self.log_copied = false;
                                }
                            }
                            _ => {}
                        }
                    }
                    ui.add_space(16.0);
                }
            });
        });
        tgx_ui::components::rule(ui, &p);
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

        tgx_ui::components::rule(ui, &p);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(16.0);
                // The right-hand figure is ours and short, so it can be tracked.
                ui.label(eyebrow(&right, &p));
                ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                    // **Not letterspaced, deliberately.** `eyebrow` goes
                    // through `components::tracked`, whose own doc says not to
                    // put a string of unknown length through it, and this line
                    // carries chat titles and raw RPC errors.
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(tgx_ui::components::uppercase(&self.status))
                                .font(tgx_ui::fonts::sans(type_scale::MICRO))
                                .color(p.muted),
                        )
                        .truncate(),
                    );
                });
            });
        });
        ui.add_space(6.0);
    }
}
