//! The chat list: the filter, the sort, the rows and the selection controls.
//!
//! **The rows are a cache**, rebuilt by `Shell::rebuild_rows` when something
//! changes what they would be. `show_rows` is virtualised, so it asks only for
//! the range on screen — re-filtering and re-sorting the whole account inside
//! that callback would be work done sixty times a second for an answer that
//! changed once.

use super::*;
use crate::list::SortMode;
use chrono::{DateTime, Local};
use eframe::egui::{Layout, Sense, Ui};
use tgx_ui::components::{
    caps, count_text, eyebrow, forum_dot, rule, selection_label, soft_rule, tick_box,
};
use tgx_ui::tokens::type_scale;

/// Every row is this tall, heading and chat alike.
///
/// The list is laid out from one row height, which is what makes several
/// thousand chats cost the same as nine. The price is that a category heading
/// cannot be shorter than a chat row; it is set in tracked micro-type instead,
/// which distinguishes it by weight rather than by size.
const ROW_HEIGHT: f32 = 46.0;

impl Shell {
    pub(super) fn chat_panel(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let live = self.selection_actions_enabled();
        let (total, any_uncounted) = self.selection_total();

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(eyebrow("Chats", &p));
        });
        ui.add_space(12.0);
        rule(ui, &p);

        self.search_row(ui);
        soft_rule(ui, &p);
        self.sort_row(ui);
        rule(ui, &p);

        // The footer is measured off the bottom first, so the list gets exactly
        // the slack that is left. Laying the list out first and the footer after
        // it is how a long list pushes its own controls off the panel.
        let footer = 62.0;
        let body = (ui.available_height() - footer).max(ROW_HEIGHT);
        ui.allocate_ui(egui::vec2(ui.available_width(), body), |ui| {
            self.list_body(ui);
        });

        rule(ui, &p);
        self.selection_row(ui, live);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(caps(
                &selection_label(self.selected.len(), total, any_uncounted),
                type_scale::MICRO,
                p.muted,
            ));
        });
    }

    fn search_row(&mut self, ui: &mut Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let width = ui.available_width() - 16.0;
            let before = self.search.clone();
            ui.add_sized(
                [width, 26.0],
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Filter chats…")
                    .desired_width(width),
            );
            // Read from the field, never typed into a mirror of it: a second
            // copy of the same string is how the empty state ends up quoting
            // something other than what is in the box.
            if self.search != before {
                let text = self.search.clone();
                self.set_filter(text);
            }
        });
        ui.add_space(10.0);
    }

    /// SORT and GROUP BY TYPE.
    ///
    /// The sort is a menu rather than seven chips: seven labels do not fit
    /// across a panel this width, and cycling through seven with one click
    /// means six clicks to undo a mistake.
    fn sort_row(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let current = self.view.sort;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(caps("Sort", type_scale::MICRO, p.muted));
            ui.add_space(12.0);

            // **egui closes this one.** The whole catcher apparatus the GPUI
            // version needed — an absolutely positioned 16000px hitbox with
            // `occlude`, deferred at a lower priority than the menu — existed
            // because choosing a mode was the only thing that closed the menu,
            // and gpui hands a click to every hitbox under the pointer unless
            // one in front blocks the rest. A `Popup` dismisses itself on a
            // click outside and takes that click with it.
            let button = ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{} \u{25be}", current.label()))
                        .font(tgx_ui::fonts::sans(type_scale::TINY))
                        .color(p.fg),
                )
                .sense(Sense::click()),
            );
            if button.clicked() {
                self.sort_open = !self.sort_open;
            }
            let popup = egui::Popup::from_response(&button)
                .open(self.sort_open)
                .layout(Layout::top_down_justified(egui::Align::LEFT));
            let closed = popup
                .show(|ui| {
                    ui.set_min_width(200.0);
                    for mode in SortMode::ALL {
                        let chosen = mode == current;
                        let label = egui::RichText::new(mode.label())
                            .font(tgx_ui::fonts::sans(type_scale::TINY))
                            .color(if chosen { p.accent } else { p.fg });
                        if ui
                            .add(egui::Label::new(label).sense(Sense::click()))
                            .clicked()
                        {
                            self.view.sort = mode;
                            self.sort_open = false;
                            self.rebuild_rows();
                            // Sorting by size with no counts fetched yet is not
                            // an error, but it is a list that will not move —
                            // say so rather than leaving it looking broken.
                            if matches!(mode, SortMode::Largest | SortMode::Smallest)
                                && self.chats.iter().all(|c| c.message_count.is_none())
                            {
                                self.status = "No message counts yet — press Count messages".into();
                            }
                            self.commit_settings();
                        }
                    }
                })
                .is_none();
            if closed {
                self.sort_open = false;
            }

            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(16.0);
                ui.label(caps("Group by type", type_scale::MICRO, p.muted));
                ui.add_space(8.0);
                if tick_box(ui, self.view.grouped, true, &p).clicked() {
                    self.view.grouped = !self.view.grouped;
                    self.rebuild_rows();
                    self.commit_settings();
                }
            });
        });
        ui.add_space(8.0);
    }

    fn list_body(&mut self, ui: &mut Ui) {
        let p = self.palette;
        if let Some(empty) = self.list_state().empty_state(&self.view.filter) {
            empty.show(ui, &p, true);
            return;
        }

        // **The clock is read once a frame, here, and carried down.** Every
        // caption says how long ago the chat last moved, and reading the clock
        // inside the row would be one syscall per visible row per frame — for
        // an answer that cannot change between two rows of the same list.
        let now = Local::now();
        let count = self.rows.len();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, count, |ui, range| {
                for i in range {
                    // Indexed rather than iterated: the row is read out of the
                    // cache and the body below needs `&mut self` to act on a
                    // click, so the borrow cannot be held across it.
                    let Some(row) = self.rows.get(i) else {
                        continue;
                    };
                    match row {
                        PaintedRow::Heading {
                            category,
                            total,
                            folded,
                        } => {
                            let (category, total, folded) = (*category, *total, *folded);
                            self.heading_row(ui, category, total, folded);
                        }
                        PaintedRow::Chat(chat) => {
                            let chat = chat.clone();
                            self.chat_row(ui, &chat, now);
                        }
                    }
                }
            });
    }

    /// A category heading.
    ///
    /// Folding is a way of looking at the list, not a second filter: a folded
    /// category's chats still count as visible, so tidying the view cannot
    /// silently change what All selects. See `crate::list`.
    fn heading_row(&mut self, ui: &mut Ui, category: Category, total: usize, folded: bool) {
        let p = self.palette;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::ZERO, p.surface);
        let mut row =
            ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(16.0, 0.0))));
        row.horizontal_centered(|ui| {
            // The disclosure marker is drawn from the text face rather than
            // assumed to exist as a glyph in some system font.
            ui.label(
                egui::RichText::new(if folded { "\u{25b8}" } else { "\u{25be}" })
                    .font(tgx_ui::fonts::sans(type_scale::TINY))
                    .color(p.muted),
            );
            ui.add_space(8.0);
            ui.label(caps(category.label(), type_scale::MICRO, p.fg));
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(total.to_string())
                        .font(tgx_ui::fonts::mono(type_scale::TINY))
                        .color(p.muted),
                );
            });
        });

        // **The click inverts the chevron it was painted beside**, rather than
        // re-deriving the state from `view.folded`. A heading whose painted
        // state and stored state ever part company — a search re-opening a
        // category is where that happened — would otherwise take one click to
        // undo a fold nobody could see and a second to do what was asked.
        if response.clicked() {
            if folded {
                self.view.folded.remove(&category);
            } else {
                self.view.folded.insert(category);
            }
            self.rebuild_rows();
            self.commit_settings();
        }
    }

    /// One chat. `now` is passed in rather than read here — see [`Self::list_body`].
    fn chat_row(&mut self, ui: &mut Ui, chat: &tgx_tg::client::ChatInfo, now: DateTime<Local>) {
        let p = self.palette;
        let ticked = self.selected.contains(&chat.id);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
        let indent = if self.view.grouped { 30.0 } else { 16.0 };
        let inner = egui::Rect::from_min_max(
            egui::pos2(rect.left() + indent, rect.top()),
            egui::pos2(rect.right() - 16.0, rect.bottom()),
        );
        let mut row = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        row.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            tick_box(ui, ticked, true, &p);
            ui.add_space(12.0);
            // A forum is marked by a painted dot, never by a suffix on the
            // stored title — presentation in the string is what the filter then
            // searches.
            if chat.is_forum {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), Sense::hover());
                ui.painter()
                    .rect_filled(dot, egui::CornerRadius::ZERO, forum_dot(&p));
                ui.add_space(8.0);
            }

            // The count is the row's one number, and the design sets every
            // number in the mono. Laid out from the right so a long title
            // cannot push it off the panel.
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(count_text(chat.message_count))
                        .font(tgx_ui::fonts::mono(type_scale::TINY))
                        .color(p.muted),
                );
                ui.add_space(12.0);
                ui.with_layout(Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&chat.title)
                                .font(tgx_ui::fonts::sans(type_scale::SMALL))
                                .color(p.fg),
                        )
                        .truncate(),
                    );
                    // The caption is the kind and the last activity, tracked
                    // uppercase micro-type in the mono. **The date is half its
                    // point**: the default sort is Recent activity, and without
                    // it the list is ordered on a unix second that appears
                    // nowhere on screen.
                    ui.add(
                        egui::Label::new(caps(
                            &list::caption(chat, now),
                            type_scale::MICRO,
                            p.muted,
                        ))
                        .truncate(),
                    );
                });
            });
        });

        // Clicking anywhere on a row ticks it. Ticks are held by chat id, so
        // they survive re-sorting, regrouping and filtering.
        if response.clicked() && !self.selected.remove(&chat.id) {
            self.selected.insert(chat.id);
        }
    }

    /// All / None / Invert / Only forums, and the Count button.
    ///
    /// Nothing offers to do what it cannot: these act on the *visible* rows, so
    /// over an empty list every one is a no-op. A button that is enabled and
    /// does nothing teaches that the interface is unreliable.
    fn selection_row(&mut self, ui: &mut Ui, live: bool) {
        let p = self.palette;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.spacing_mut().item_spacing.x = 14.0;
            for (i, label) in ["All", "None", "Invert", "Only forums"].iter().enumerate() {
                let text = caps(label, type_scale::MICRO, if live { p.fg } else { p.muted });
                let sense = if live { Sense::click() } else { Sense::hover() };
                if ui.add(egui::Label::new(text).sense(sense)).clicked() {
                    // Asked once: `visible()` sorts the whole account, and
                    // every one of these four needs the same answer.
                    let visible: Vec<(i64, bool)> =
                        self.visible().iter().map(|c| (c.id, c.is_forum)).collect();
                    match i {
                        0 => self.selected.extend(visible.iter().map(|(id, _)| *id)),
                        1 => {
                            for (id, _) in &visible {
                                self.selected.remove(id);
                            }
                        }
                        2 => {
                            for (id, _) in &visible {
                                if !self.selected.remove(id) {
                                    self.selected.insert(*id);
                                }
                            }
                        }
                        // **"Only forums" replaces, it does not add.** The
                        // label promises an exclusive selection, and adding to
                        // one meant clicking All and then this left every chat
                        // ticked with the footer still counting the lot.
                        // Assigning the set outright would do it, but the
                        // replacement is scoped to the visible rows like the
                        // other three, so a tick on a chat the filter is hiding
                        // is left where it is rather than silently dropped.
                        3 => {
                            for (id, is_forum) in &visible {
                                if *is_forum {
                                    self.selected.insert(*id);
                                } else {
                                    self.selected.remove(id);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            // **The label states the action; the price goes in the row under
            // it.** "Count messages (enables sorting by size)" named a side
            // effect rather than the action and said nothing about what it
            // costs — one request per chat, minutes on a large account, and a
            // rate-limit wait if Telegram decides so.
            let countable = !self.chats.is_empty() && !self.exporting;
            let label = if self.counting {
                "Stop counting"
            } else {
                "Count messages"
            };
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(16.0);
                let text = caps(
                    label,
                    type_scale::MICRO,
                    if countable { p.accent } else { p.muted },
                );
                let sense = if countable {
                    Sense::click()
                } else {
                    Sense::hover()
                };
                if ui.add(egui::Label::new(text).sense(sense)).clicked() {
                    self.start_count();
                }
            });
        });
        ui.add_space(8.0);
    }
}
